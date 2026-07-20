//! Content store: `~/.agentstack/store/` — where capability sources are fetched
//! and cached (PLAN §9d). Git sources are cloned/checked-out via the `git` CLI;
//! path sources pass through. A content digest gives the lockfile its integrity
//! field.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::manifest::{Skill, SkillSource};
use crate::util::paths;

pub struct Store {
    root: PathBuf,
}

/// The resolved local location of a skill's content.
#[derive(Debug)]
pub struct Resolved {
    pub path: PathBuf,
    /// Resolved git commit (git sources only).
    pub rev: Option<String>,
    pub checksum: String,
    /// Whether a network fetch happened this call.
    pub fetched: bool,
    pub source_kind: &'static str,
}

impl Store {
    pub fn default_store() -> Self {
        Store {
            root: paths::agentstack_home().join("store"),
        }
    }

    pub fn with_root(root: PathBuf) -> Self {
        Store { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolve a skill to a local directory, fetching git sources as needed.
    /// `pinned_rev` (from the lockfile) wins over the manifest's rev for
    /// reproducibility.
    pub fn resolve(
        &self,
        skill: &Skill,
        manifest_dir: &Path,
        pinned_rev: Option<&str>,
    ) -> Result<Resolved> {
        self.resolve_inner(skill, manifest_dir, pinned_rev, false)
    }

    /// The update/relock posture: ignore the lock pin, honor the manifest
    /// rev, and REQUIRE the fetch — a rev-less git skill re-tracks the
    /// remote's default-branch head, and an unreachable upstream errors
    /// instead of silently reusing the cached clone (see `ensure_git`).
    pub fn resolve_refresh(&self, skill: &Skill, manifest_dir: &Path) -> Result<Resolved> {
        self.resolve_inner(skill, manifest_dir, None, true)
    }

    fn resolve_inner(
        &self,
        skill: &Skill,
        manifest_dir: &Path,
        pinned_rev: Option<&str>,
        refresh: bool,
    ) -> Result<Resolved> {
        match skill.source()? {
            SkillSource::Path(p) => {
                let path = resolve_path(manifest_dir, &p);
                let checksum = if path.exists() {
                    dir_digest(&path)?.hex().to_string()
                } else {
                    String::new()
                };
                Ok(Resolved {
                    path,
                    rev: None,
                    checksum,
                    fetched: false,
                    source_kind: "path",
                })
            }
            SkillSource::Git { url, rev, subpath } => {
                let want = pinned_rev.map(str::to_string).or(rev);
                let clone = self.git_dir(&url);
                let fetched = ensure_git(&url, want.as_deref(), &clone, refresh)?;
                // HEAD is read from the clone root (`.git` lives there); the
                // skill body — and thus the checksum — is the subpath dir.
                let resolved_rev = git_head(&clone)?;
                let content = git_content_dir(&clone, subpath.as_deref())?;
                if let Some(sub) = subpath.as_deref() {
                    if !content.exists() {
                        bail!(
                            "subpath '{sub}' does not exist in {url} at {} — \
                             check the path within the repo",
                            &resolved_rev[..resolved_rev.len().min(12)]
                        );
                    }
                }
                let checksum = dir_digest(&content)?.hex().to_string();
                Ok(Resolved {
                    path: content,
                    rev: Some(resolved_rev),
                    checksum,
                    fetched,
                    source_kind: "git",
                })
            }
        }
    }

    /// Resolve a skill to a local directory **without any network access**.
    /// Path sources resolve as usual. Git sources resolve to the store clone
    /// *only if it already exists* (reporting its current HEAD); an
    /// un-cached git source yields `Ok(None)` so callers can report it as
    /// unavailable offline rather than fetching.
    pub fn resolve_local(&self, skill: &Skill, manifest_dir: &Path) -> Result<Option<Resolved>> {
        match skill.source()? {
            SkillSource::Path(p) => {
                let path = resolve_path(manifest_dir, &p);
                let checksum = if path.exists() {
                    dir_digest(&path)?.hex().to_string()
                } else {
                    String::new()
                };
                Ok(Some(Resolved {
                    path,
                    rev: None,
                    checksum,
                    fetched: false,
                    source_kind: "path",
                }))
            }
            SkillSource::Git { url, subpath, .. } => {
                let clone = self.git_dir(&url);
                if !clone.exists() {
                    return Ok(None);
                }
                let content = git_content_dir(&clone, subpath.as_deref())?;
                Ok(Some(Resolved {
                    rev: git_head(&clone).ok(),
                    checksum: dir_digest(&content)?.hex().to_string(),
                    path: content,
                    fetched: false,
                    source_kind: "git",
                }))
            }
        }
    }

    /// Locate a skill's directory without network access **or content
    /// digesting** — for read-only callers that only need the path (reading
    /// `SKILL.md`, listing). `checksum` is left empty, so the result must never
    /// feed lock recording; digesting is what makes small ops pay a whole-
    /// library read+hash. Un-cached git sources yield `Ok(None)`, like
    /// [`Store::resolve_local`].
    pub fn resolve_path_only(
        &self,
        skill: &Skill,
        manifest_dir: &Path,
    ) -> Result<Option<Resolved>> {
        match skill.source()? {
            SkillSource::Path(p) => Ok(Some(Resolved {
                path: resolve_path(manifest_dir, &p),
                rev: None,
                checksum: String::new(),
                fetched: false,
                source_kind: "path",
            })),
            SkillSource::Git { url, subpath, .. } => {
                let clone = self.git_dir(&url);
                if !clone.exists() {
                    return Ok(None);
                }
                Ok(Some(Resolved {
                    path: git_content_dir(&clone, subpath.as_deref())?,
                    rev: None,
                    checksum: String::new(),
                    fetched: false,
                    source_kind: "git",
                }))
            }
        }
    }

    fn git_dir(&self, url: &str) -> PathBuf {
        self.root.join("git").join(sanitize(url))
    }

    /// Adopt a staged clone into this store's slot for `url` — only if the
    /// slot is empty, and **rename-only**: staging (see [`Stage`]) lives on
    /// this filesystem by construction, so the scanned bytes land verbatim
    /// (`.git` and symlinks included). There is deliberately no copy
    /// fallback — the shipped copy helpers strip `.git` and dereference
    /// symlinks, either of which corrupts a promoted clone (design §3 of
    /// add-skill-source-grammar.md). `Ok(None)` = slot taken or rename
    /// refused; the caller falls back to a commit-pinned re-resolve.
    pub fn adopt_clone(&self, url: &str, staged_clone: &Path) -> Result<Option<PathBuf>> {
        let dest = self.git_dir(url);
        if dest.exists() {
            return Ok(None);
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        Ok(fs::rename(staged_clone, &dest).ok().map(|()| dest))
    }

    /// The cached clone **root** for `url` with no network access, plus its
    /// current HEAD — `None` when the clone does not exist yet (report it as
    /// unavailable offline). Unlike [`Store::resolve_local`], this returns the
    /// clone root (where `.git` lives), not the subpath content dir: git
    /// extensions digest the checkout root anchored at a `subpath` with the
    /// strict integrity-root digest, so they need the root, not the body dir.
    pub fn local_git_clone(&self, url: &str) -> Option<(PathBuf, Option<String>)> {
        let clone = self.git_dir(url);
        if !clone.exists() {
            return None;
        }
        Some((clone.clone(), git_head(&clone).ok()))
    }
}

/// Transient staging for previewing remote sources without touching the
/// persistent store (design §3 of add-skill-source-grammar.md). Lives under
/// the agentstack home — the store's own filesystem by construction, so
/// promotion is a rename, never a copy. Random id (never reused: a crashed
/// run's leftovers must not skip re-fetch/re-scan), 0700, best-effort
/// removal on drop — the `SandboxGateway` RAII pattern.
pub struct Stage {
    root: PathBuf,
}

impl Stage {
    pub fn create() -> Result<Self> {
        let root = paths::agentstack_home()
            .join("stage")
            .join(crate::runs::gen_id());
        if root.exists() {
            bail!(
                "staging path {} already exists — retry the command",
                root.display()
            );
        }
        fs::create_dir_all(&root).with_context(|| format!("creating {}", root.display()))?;
        crate::util::restrict(&root, true);
        Ok(Self { root })
    }

    /// A `Store` rooted at this staging area: clones land under
    /// `<stage>/git/<sanitized-url>` with zero writes anywhere persistent.
    pub fn store(&self) -> Store {
        Store::with_root(self.root.clone())
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Resolve a skill's local source dir for materialization, *without* fetching
/// (path → local; git → store dir if already installed, else `None`).
pub fn local_source_dir(store: &Store, skill: &Skill, manifest_dir: &Path) -> Option<PathBuf> {
    match skill.source().ok()? {
        SkillSource::Path(p) => {
            let path = resolve_path(manifest_dir, &p);
            path.exists().then_some(path)
        }
        SkillSource::Git { url, subpath, .. } => {
            let clone = store.git_dir(&url);
            if !clone.exists() {
                return None;
            }
            let content = git_content_dir(&clone, subpath.as_deref()).ok()?;
            content.exists().then_some(content)
        }
    }
}

/// Resolve a git skill's content directory: the clone root, or a validated
/// subdirectory within it. The subpath must be a plain relative path — no
/// absolute prefix and no `..` component — so a crafted library entry can never
/// point the skill body outside its own clone.
fn git_content_dir(clone: &Path, subpath: Option<&str>) -> Result<PathBuf> {
    let Some(sub) = subpath.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(clone.to_path_buf());
    };
    let rel = Path::new(sub);
    let safe = rel
        .components()
        .all(|c| matches!(c, std::path::Component::Normal(_)));
    if !safe {
        bail!("git subpath '{sub}' must be a relative path inside the repo (no '..' or absolute path)");
    }
    let content = clone.join(rel);
    // A `Normal`-only subpath still escapes if a component is a *symlink* the
    // repo shipped (e.g. `skills/x` → `~/.ssh`; git checks out symlinks). When
    // the target exists, resolve it and require it stay inside the clone —
    // otherwise dir_digest/scan/copy would read and vendor files outside the repo.
    if let (Ok(real_content), Ok(real_clone)) =
        (fs::canonicalize(&content), fs::canonicalize(clone))
    {
        if !real_content.starts_with(&real_clone) {
            bail!("git subpath '{sub}' resolves outside the repo (symlinked escape) — refusing");
        }
    }
    Ok(content)
}

fn resolve_path(dir: &Path, p: &str) -> PathBuf {
    let pb = PathBuf::from(p);
    if pb.is_absolute() {
        pb
    } else {
        dir.join(pb)
    }
}

/// Clone (or refresh) `url` into the store and check out `rev` when given.
/// Returns the checkout dir and the resolved HEAD commit. The public seam the
/// git-pack provider uses; skills keep going through [`Store::resolve`].
pub fn checkout(store: &Store, url: &str, rev: Option<&str>) -> Result<(PathBuf, String)> {
    let dest = store.git_dir(url);
    ensure_git(url, rev, &dest, false)?;
    let head = git_head(&dest)?;
    Ok((dest, head))
}

/// List `url`'s tags via `git ls-remote --tags`, peeled entries preferred,
/// without cloning. Network; callers gate on policy first.
pub fn ls_remote_tags(url: &str) -> Result<Vec<String>> {
    crate::gitx::deny_weird_transport(url)?;
    let out = run_git(&["ls-remote", "--tags", url], None)?;
    let mut tags: Vec<String> = out
        .lines()
        .filter_map(|l| l.split_once("refs/tags/").map(|(_, t)| t))
        .map(|t| t.trim_end_matches("^{}").to_string())
        .collect();
    tags.sort();
    tags.dedup();
    Ok(tags)
}

/// Ensure a git clone exists at `dest` and is checked out at `want_rev` (or
/// its default branch). `refresh` is the update/relock posture: fetching is
/// REQUIRED — an unreachable or deleted upstream must surface, detecting
/// that is what update exists for — and a rev-less skill re-tracks the
/// remote's current default-branch head. Without that, `lock --update` on a
/// rev-less git skill with a cached clone made no network call at all: a
/// silent no-op that could neither update nor notice a vanished upstream.
fn ensure_git(url: &str, want_rev: Option<&str>, dest: &Path, refresh: bool) -> Result<bool> {
    crate::gitx::deny_weird_transport(url)?;
    let fresh = !dest.exists();
    if fresh {
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        run_git(&["clone", url, &dest.to_string_lossy()], None)
            .with_context(|| format!("cloning {url}"))?;
    }
    match want_rev {
        Some(rev) => {
            if refresh {
                run_git(&["fetch", "--all", "--tags"], Some(dest))
                    .with_context(|| format!("fetching {url}"))?;
                // A branch pin must adopt the FETCHED head: `checkout
                // <branch>` lands on the stale LOCAL branch, which fetch
                // never fast-forwards (only remote-tracking refs moved).
                // Try the remote-tracking ref first; tags and commit shas
                // have no origin/ form and fall back to the plain checkout.
                let remote_ref = format!("origin/{rev}");
                if run_git(&["checkout", "--detach", &remote_ref], Some(dest)).is_err() {
                    run_git(&["checkout", rev], Some(dest))
                        .with_context(|| format!("checking out {rev}"))?;
                }
            } else {
                // Best-effort fetch so a pinned rev that arrived later is
                // available; a resolve against a cached clone stays offline-
                // tolerant.
                let _ = run_git(&["fetch", "--all", "--tags"], Some(dest));
                run_git(&["checkout", rev], Some(dest))
                    .with_context(|| format!("checking out {rev}"))?;
            }
        }
        None if refresh && !fresh => {
            run_git(&["fetch", "origin", "HEAD", "--tags"], Some(dest))
                .with_context(|| format!("fetching {url}"))?;
            run_git(&["checkout", "--detach", "FETCH_HEAD"], Some(dest))
                .with_context(|| format!("checking out the latest revision of {url}"))?;
        }
        None => {}
    }
    Ok(fresh)
}

/// The clone-containment guard, exposed for callers that hold a checkout
/// root directly (the add/lib source-grammar paths): resolves `subpath`
/// inside `clone_root` and refuses a checked-out symlink that escapes it —
/// the same refusal `Store::resolve` applies, so a hostile repo can't get a
/// preview's digest or scan to read files outside the repo.
pub fn contained_content_dir(clone_root: &Path, subpath: Option<&str>) -> Result<PathBuf> {
    git_content_dir(clone_root, subpath)
}

fn git_head(dest: &Path) -> Result<String> {
    run_git(&["rev-parse", "HEAD"], Some(dest))
}

/// All store git runs under the `Ingest` profile — this is remote content on
/// its way to the trust gate (design §B).
fn run_git(args: &[&str], cwd: Option<&Path>) -> Result<String> {
    crate::gitx::run(crate::gitx::Profile::Ingest, args, cwd)
}

fn sanitize(url: &str) -> String {
    url.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Total size in bytes of a directory's files (`.git` excluded, like
/// [`dir_digest`]). Best-effort: unreadable entries count as zero.
pub fn dir_size(root: &Path) -> u64 {
    let mut files: Vec<PathBuf> = Vec::new();
    if collect_files(root, root, &mut files).is_err() {
        return 0;
    }
    files
        .iter()
        .filter_map(|rel| fs::metadata(root.join(rel)).ok())
        .map(|m| m.len())
        .sum()
}

// The digest itself (paths + bytes → sha256) lives in core with the lockfile
// types it feeds. Authoritative skill checksums call it directly — no
// stat-fingerprint cache sits on the verification path (see ARCHITECTURE.md).
// TODO(phase-1): shim — migrate callers to agentstack_core::digest and drop.
pub use agentstack_core::digest::{collect_files, dir_digest};

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::prelude::*;

    #[test]
    fn resolves_path_source() {
        let tmp = assert_fs::TempDir::new().unwrap();
        tmp.child("skills/x/SKILL.md").write_str("# x\n").unwrap();
        let store = Store::with_root(tmp.child("store").path().to_path_buf());
        let skill: Skill = toml::from_str("path = \"./skills/x\"").unwrap();
        let r = store.resolve(&skill, tmp.path(), None).unwrap();
        assert_eq!(r.source_kind, "path");
        assert!(r.path.join("SKILL.md").exists());
        assert!(!r.checksum.is_empty());
    }

    /// Sandbox `AGENTSTACK_HOME` under `TEST_ENV_LOCK` so this regression is
    /// safe when run against the previously cached implementation or if a cache
    /// is reintroduced.
    fn with_home<T>(f: impl FnOnce(&assert_fs::TempDir) -> T) -> T {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", home.path());
        let out = f(&home);
        std::env::remove_var("AGENTSTACK_HOME");
        out
    }

    /// Pin a file's mtime to an exact time so two directory states can be
    /// made stat-identical in the aggregate.
    fn set_mtime(path: &Path, t: std::time::SystemTime) {
        fs::OpenOptions::new()
            .append(true)
            .open(path)
            .unwrap()
            .set_modified(t)
            .unwrap();
    }

    /// Regression — contract §3 step 4 / ruling 3 and ARCHITECTURE.md:119.
    /// Drives the real authoritative verification seam (`skill_lock_status` —
    /// the chain trust-grant, `use --write`, and the MCP loader all share): a
    /// same-size, mtime-restored in-place edit — the "same-stat" change a
    /// reintroduced stat-fingerprint cache would miss — is still caught as lock
    /// drift. AGENTSTACK_HOME is sandboxed under TEST_ENV_LOCK so running this
    /// against the old (cached) code cannot touch the developer's real cache.
    #[test]
    fn same_stat_skill_edit_is_detected_by_skill_lock_status() {
        with_home(|_home| {
            let tmp = assert_fs::TempDir::new().unwrap();
            let skill_md = tmp.child("skills/x/SKILL.md");
            skill_md.write_str("# x\ncontent-AAAA\n").unwrap(); // 17 bytes
            let skill_dir = tmp.path().join("skills/x");

            // Pin an old mtime so a hypothetical settle-window cache would deem
            // the dir cache-eligible; the fix must not depend on mtime at all.
            let t = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
            set_mtime(skill_md.path(), t);

            // Ground-truth pin from the raw, cache-free digest.
            let pin = dir_digest(&skill_dir).unwrap();
            let lock = crate::lock::Lock {
                version: crate::lock::SUPPORTED_LOCK_VERSION,
                extensions: Vec::new(),
                skills: vec![crate::lock::LockedSkill {
                    name: "x".into(),
                    source: crate::lock::SkillLockSource::Path,
                    path: Some("./skills/x".into()),
                    git: None,
                    rev: None,
                    checksum: pin,
                }],
                servers: Vec::new(),
                instructions: Vec::new(),
                executables: Vec::new(),
            };

            let manifest: crate::manifest::Manifest =
                toml::from_str("version = 1\n[skills.x]\npath = \"./skills/x\"\n").unwrap();
            let library = crate::library::Library::default();
            let lib_home = tmp.child("lib").path().to_path_buf();
            let store = Store::with_root(tmp.child("store").path().to_path_buf());

            // Same `store` instance across both calls, so any in-memory cache on
            // the seam is primed by the first call and would be hit by the second.
            let status_of = || {
                crate::resolve::skill_lock_status(
                    "x",
                    &manifest,
                    tmp.path(),
                    &library,
                    &lib_home,
                    &store,
                    &lock,
                    crate::resolve::ResolveMode::NoFetch,
                )
                .status
            };

            // First pass: clean, and primes any cache that sits on the seam.
            assert_eq!(
                status_of(),
                crate::resolve::SkillLockStatus::Matches,
                "freshly pinned content verifies"
            );

            // Same-size, same-mtime in-place edit — only the bytes differ.
            skill_md.write_str("# x\ncontent-BBBB\n").unwrap(); // also 17 bytes
            set_mtime(skill_md.path(), t);

            // Second pass: drift despite an identical stat fingerprint.
            let status = status_of();
            assert!(
                matches!(
                    status,
                    crate::resolve::SkillLockStatus::ChecksumDrift { .. }
                ),
                "same-stat content change must be lock drift, got {status:?}"
            );
            assert!(
                matches!(
                    crate::verify::skill_verdict(&status),
                    crate::verify::Verdict::Block(_)
                ),
                "drift must fail closed (Block)"
            );
        });
    }

    #[test]
    fn resolves_git_source_from_local_repo() {
        // Build a local git repo and resolve it via a file:// URL — exercises the
        // real git path without network.
        let tmp = assert_fs::TempDir::new().unwrap();
        let repo = tmp.child("repo");
        repo.create_dir_all().unwrap();
        let git = |args: &[&str]| {
            super::run_git(args, Some(repo.path())).unwrap();
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@e.st"]);
        git(&["config", "user.name", "t"]);
        repo.child("SKILL.md").write_str("# from git\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "init"]);

        let store = Store::with_root(tmp.child("store").path().to_path_buf());
        let url = format!("file://{}", repo.path().display());
        let skill: Skill = toml::from_str(&format!("git = \"{url}\"")).unwrap();
        let r = store.resolve(&skill, tmp.path(), None).unwrap();
        assert_eq!(r.source_kind, "git");
        assert!(r.rev.is_some());
        assert!(r.path.join("SKILL.md").exists());
    }

    /// A git source with a subpath resolves to the subdir; a `..`/symlink escape
    /// is refused (the supply-chain boundary the subpath feature must hold).
    #[test]
    fn git_subpath_resolves_subdir_and_rejects_escapes() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let repo = tmp.child("repo");
        repo.create_dir_all().unwrap();
        let git = |args: &[&str]| {
            super::run_git(args, Some(repo.path())).unwrap();
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@e.st"]);
        git(&["config", "user.name", "t"]);
        repo.child("skills/improve/SKILL.md")
            .write_str("# improve\n")
            .unwrap();
        // A symlink that points outside the repo.
        std::os::unix::fs::symlink("/etc", repo.path().join("evil")).unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", "init"]);

        let store = Store::with_root(tmp.child("store").path().to_path_buf());
        let url = format!("file://{}", repo.path().display());

        // Good subpath → the subdir's SKILL.md.
        let ok: Skill =
            toml::from_str(&format!("git = \"{url}\"\nsubpath = \"skills/improve\"")).unwrap();
        let r = store.resolve(&ok, tmp.path(), None).unwrap();
        assert!(r.path.ends_with("skills/improve"));
        assert!(r.path.join("SKILL.md").exists());

        // `..` component → rejected before any read.
        let dots: Skill = toml::from_str(&format!("git = \"{url}\"\nsubpath = \"../x\"")).unwrap();
        assert!(store.resolve(&dots, tmp.path(), None).is_err());

        // Symlink escape → rejected.
        let evil: Skill = toml::from_str(&format!("git = \"{url}\"\nsubpath = \"evil\"")).unwrap();
        let err = store.resolve(&evil, tmp.path(), None).unwrap_err();
        assert!(
            err.to_string().contains("outside the repo"),
            "symlink escape must be refused: {err}"
        );
    }
}
