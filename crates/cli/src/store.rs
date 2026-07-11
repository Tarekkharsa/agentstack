//! Content store: `~/.agentstack/store/` — where capability sources are fetched
//! and cached (PLAN §9d). Git sources are cloned/checked-out via the `git` CLI;
//! path sources pass through. A content digest gives the lockfile its integrity
//! field.

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};

use crate::manifest::{Skill, SkillSource};
use crate::util::paths;

pub struct Store {
    root: PathBuf,
    digest_cache: RefCell<DigestCache>,
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
            digest_cache: RefCell::new(DigestCache::load()),
        }
    }

    pub fn with_root(root: PathBuf) -> Self {
        Store {
            root,
            digest_cache: RefCell::new(DigestCache::load()),
        }
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
        match skill.source()? {
            SkillSource::Path(p) => {
                let path = resolve_path(manifest_dir, &p);
                let checksum = if path.exists() {
                    dir_digest_cached_with(&path, &mut self.digest_cache.borrow_mut())?
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
                let fetched = ensure_git(&url, want.as_deref(), &clone)?;
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
                let checksum =
                    dir_digest_cached_with(&content, &mut self.digest_cache.borrow_mut())?;
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
                    dir_digest_cached_with(&path, &mut self.digest_cache.borrow_mut())?
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
                    checksum: dir_digest_cached_with(
                        &content,
                        &mut self.digest_cache.borrow_mut(),
                    )?,
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
}

impl Drop for Store {
    fn drop(&mut self) {
        let cache = self.digest_cache.get_mut();
        if cache.dirty {
            cache.save();
        }
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
    ensure_git(url, rev, &dest)?;
    let head = git_head(&dest)?;
    Ok((dest, head))
}

/// List `url`'s tags via `git ls-remote --tags`, peeled entries preferred,
/// without cloning. Network; callers gate on policy first.
pub fn ls_remote_tags(url: &str) -> Result<Vec<String>> {
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

/// Ensure a git clone exists at `dest` and is checked out at `want_rev` (or its
/// default branch). Returns whether a clone happened.
fn ensure_git(url: &str, want_rev: Option<&str>, dest: &Path) -> Result<bool> {
    let fresh = !dest.exists();
    if fresh {
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        run_git(&["clone", url, &dest.to_string_lossy()], None)
            .with_context(|| format!("cloning {url}"))?;
    }
    if let Some(rev) = want_rev {
        // Best-effort fetch so a pinned rev that arrived later is available.
        let _ = run_git(&["fetch", "--all", "--tags"], Some(dest));
        run_git(&["checkout", rev], Some(dest)).with_context(|| format!("checking out {rev}"))?;
    }
    Ok(fresh)
}

fn git_head(dest: &Path) -> Result<String> {
    run_git(&["rev-parse", "HEAD"], Some(dest))
}

fn run_git(args: &[&str], cwd: Option<&Path>) -> Result<String> {
    let mut cmd = Command::new("git");
    if let Some(dir) = cwd {
        cmd.arg("-C").arg(dir);
    }
    cmd.args(args);
    let out = cmd.output().context("running git (is it installed?)")?;
    if !out.status.success() {
        bail!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn sanitize(url: &str) -> String {
    url.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

// ---------- persistent digest cache ----------
//
// `dir_digest` reads and hashes every byte under a directory — fine for one
// skill, painful when doctor/use/install digest a multi-hundred-MB library on
// every run. The cache maps a canonical dir to the digest it had under a stat
// fingerprint (file count + total size + max mtime + a hash of the sorted
// relative paths, each with its file's size and mtime). Any mismatch falls
// back to the full read+hash; a matching
// fingerprint turns a multi-second pass into stat calls.

/// Where digest cache entries live: `~/.agentstack/digest-cache.json`.
fn digest_cache_path() -> PathBuf {
    paths::agentstack_home().join("digest-cache.json")
}

/// Fingerprints younger than this are never cached: within mtime granularity a
/// same-size edit right after a digest would be invisible to the fingerprint
/// (the same racy-clean window git's index handles).
const DIGEST_CACHE_SETTLE_NS: u64 = 2_000_000_000;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct DirFingerprint {
    files: u64,
    bytes: u64,
    max_mtime_ns: u64,
    /// SHA-256 over the sorted relative paths, each folded with its file's
    /// (size, mtime) — catches renames *and* per-file stat changes the
    /// directory aggregates miss (e.g. two files swapping sizes, or an
    /// mtime-preserving replacement whose mtime stays below the max).
    paths_sha: String,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct DigestCacheEntry {
    #[serde(flatten)]
    fingerprint: DirFingerprint,
    sha256: String,
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct DigestCache {
    #[serde(default)]
    entries: std::collections::BTreeMap<String, DigestCacheEntry>,
    #[serde(skip)]
    dirty: bool,
}

impl DigestCache {
    fn load() -> DigestCache {
        fs::read_to_string(digest_cache_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Best-effort persist — a failed cache write must never fail the caller.
    fn save(&self) {
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = crate::util::atomic::write(&digest_cache_path(), &text);
        }
    }
}

/// Stat-only fingerprint of a directory (`.git` excluded, like `dir_digest`).
fn dir_fingerprint(root: &Path) -> Result<DirFingerprint> {
    let mut files: Vec<PathBuf> = Vec::new();
    collect_files(root, root, &mut files)?;
    files.sort();
    let mut bytes = 0u64;
    let mut max_mtime_ns = 0u64;
    let mut hasher = Sha256::new();
    for rel in &files {
        let meta = fs::metadata(root.join(rel))
            .with_context(|| format!("stat {}", root.join(rel).display()))?;
        let mtime_ns = meta
            .modified()
            .ok()
            .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        // Per-file stats go into the hash, not just the aggregates: two files
        // swapping sizes (or a replacement carrying an old mtime below the
        // max) leave count/total/max unchanged but must break the fingerprint.
        hasher.update(rel.to_string_lossy().as_bytes());
        hasher.update([0]);
        hasher.update(meta.len().to_le_bytes());
        hasher.update(mtime_ns.to_le_bytes());
        bytes = bytes.saturating_add(meta.len());
        max_mtime_ns = max_mtime_ns.max(mtime_ns);
    }
    Ok(DirFingerprint {
        files: files.len() as u64,
        bytes,
        max_mtime_ns,
        paths_sha: format!("{:x}", hasher.finalize()),
    })
}

/// [`dir_digest`] behind the persistent stat-fingerprint cache: a matching
/// fingerprint returns the cached digest without reading file contents; any
/// mismatch (or a fingerprint too fresh to be trustworthy) recomputes the full
/// digest and updates the cache.
pub fn dir_digest_cached(root: &Path) -> Result<String> {
    let mut cache = DigestCache::load();
    let sha256 = dir_digest_cached_with(root, &mut cache)?;
    if cache.dirty {
        cache.save();
    }
    Ok(sha256)
}

fn dir_digest_cached_with(root: &Path, cache: &mut DigestCache) -> Result<String> {
    let canon = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let key = canon.display().to_string();
    let fingerprint = dir_fingerprint(&canon)?;

    if let Some(entry) = cache.entries.get(&key) {
        if entry.fingerprint == fingerprint {
            return Ok(entry.sha256.clone());
        }
    }

    let sha256 = dir_digest(&canon)?;
    let now_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    if now_ns.saturating_sub(fingerprint.max_mtime_ns) > DIGEST_CACHE_SETTLE_NS {
        cache.entries.insert(
            key,
            DigestCacheEntry {
                fingerprint,
                sha256: sha256.clone(),
            },
        );
        cache.dirty = true;
    }
    // TODO(perf): fingerprinting and content hashing still walk separately on
    // misses; combining them risks changing the digest algorithm owned by core.
    Ok(sha256)
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
// types it feeds; only the stat-fingerprint cache above is cli policy.
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

    /// Serialize AGENTSTACK_HOME mutation (the digest cache lives under it).
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

    /// Backdate every file's mtime past the settle window so the fingerprint
    /// is cache-eligible (fresh mtimes are deliberately never cached).
    fn backdate_all(dir: &Path) {
        let past = std::time::SystemTime::now() - std::time::Duration::from_secs(10);
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                backdate_all(&entry.path());
            } else {
                let f = fs::OpenOptions::new()
                    .append(true)
                    .open(entry.path())
                    .unwrap();
                f.set_modified(past).unwrap();
            }
        }
    }

    fn cache_key(dir: &Path) -> String {
        fs::canonicalize(dir).unwrap().display().to_string()
    }

    #[test]
    fn cached_digest_matches_full_digest_and_persists() {
        with_home(|home| {
            let tmp = assert_fs::TempDir::new().unwrap();
            tmp.child("a.txt").write_str("hello").unwrap();
            tmp.child("sub/b.txt").write_str("world").unwrap();
            backdate_all(tmp.path());

            let cached = dir_digest_cached(tmp.path()).unwrap();
            assert_eq!(cached, dir_digest(tmp.path()).unwrap());

            let text =
                fs::read_to_string(home.path().join("digest-cache.json")).expect("cache written");
            assert!(text.contains(&cache_key(tmp.path())), "entry keyed by dir");
        });
    }

    #[test]
    fn matching_fingerprint_short_circuits_the_hash() {
        with_home(|home| {
            let tmp = assert_fs::TempDir::new().unwrap();
            tmp.child("a.txt").write_str("hello").unwrap();
            backdate_all(tmp.path());
            dir_digest_cached(tmp.path()).unwrap();

            // Poison the cached sha; an unchanged fingerprint must return it —
            // proof the content was not re-read.
            let cache_path = home.path().join("digest-cache.json");
            let mut v: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&cache_path).unwrap()).unwrap();
            v["entries"][cache_key(tmp.path())]["sha256"] = "deadbeef".into();
            fs::write(&cache_path, serde_json::to_string(&v).unwrap()).unwrap();

            assert_eq!(dir_digest_cached(tmp.path()).unwrap(), "deadbeef");
        });
    }

    #[test]
    fn changed_content_and_renames_fall_back_to_full_hash() {
        with_home(|home| {
            let tmp = assert_fs::TempDir::new().unwrap();
            tmp.child("a.txt").write_str("hello").unwrap();
            backdate_all(tmp.path());
            dir_digest_cached(tmp.path()).unwrap();
            let cache_path = home.path().join("digest-cache.json");
            let poison = |key: &str| {
                let mut v: serde_json::Value =
                    serde_json::from_str(&fs::read_to_string(&cache_path).unwrap()).unwrap();
                v["entries"][key]["sha256"] = "deadbeef".into();
                fs::write(&cache_path, serde_json::to_string(&v).unwrap()).unwrap();
            };

            // A size-changing edit breaks the fingerprint → real digest again.
            poison(&cache_key(tmp.path()));
            tmp.child("a.txt").write_str("changed!").unwrap();
            backdate_all(tmp.path());
            let after_edit = dir_digest_cached(tmp.path()).unwrap();
            assert_ne!(after_edit, "deadbeef");
            assert_eq!(after_edit, dir_digest(tmp.path()).unwrap());

            // A rename keeps count/size/mtime but must still invalidate (the
            // digest covers relative paths).
            poison(&cache_key(tmp.path()));
            fs::rename(tmp.path().join("a.txt"), tmp.path().join("z.txt")).unwrap();
            backdate_all(tmp.path());
            let after_rename = dir_digest_cached(tmp.path()).unwrap();
            assert_ne!(after_rename, "deadbeef");
            assert_eq!(after_rename, dir_digest(tmp.path()).unwrap());
        });
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

    #[test]
    fn size_swap_with_identical_aggregates_changes_fingerprint() {
        with_home(|home| {
            // Two files swap sizes between states: file count, TOTAL bytes and
            // max mtime are all identical — only the per-path stats differ. An
            // aggregate-only fingerprint collides here and serves a stale
            // digest.
            let t = std::time::SystemTime::now() - std::time::Duration::from_secs(30);
            let tmp = assert_fs::TempDir::new().unwrap();
            tmp.child("a.txt").write_str("aa").unwrap();
            tmp.child("b.txt").write_str("bbbb").unwrap();
            set_mtime(&tmp.path().join("a.txt"), t);
            set_mtime(&tmp.path().join("b.txt"), t);
            let before = dir_fingerprint(tmp.path()).unwrap();
            dir_digest_cached(tmp.path()).unwrap();

            // Poison the cached sha to detect whether it gets served.
            let cache_path = home.path().join("digest-cache.json");
            let mut v: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&cache_path).unwrap()).unwrap();
            v["entries"][cache_key(tmp.path())]["sha256"] = "deadbeef".into();
            fs::write(&cache_path, serde_json::to_string(&v).unwrap()).unwrap();

            tmp.child("a.txt").write_str("bbbb").unwrap();
            tmp.child("b.txt").write_str("aa").unwrap();
            set_mtime(&tmp.path().join("a.txt"), t);
            set_mtime(&tmp.path().join("b.txt"), t);

            let after = dir_fingerprint(tmp.path()).unwrap();
            assert_eq!(before.files, after.files);
            assert_eq!(before.bytes, after.bytes);
            assert_eq!(before.max_mtime_ns, after.max_mtime_ns);
            assert_ne!(before, after, "per-path sizes must break the fingerprint");

            let digest = dir_digest_cached(tmp.path()).unwrap();
            assert_ne!(
                digest, "deadbeef",
                "stale digest served for swapped content"
            );
            assert_eq!(digest, dir_digest(tmp.path()).unwrap());
        });
    }

    #[test]
    fn mtime_preserving_replacement_below_max_changes_fingerprint() {
        with_home(|_home| {
            // Simulate `cp -p` / `rsync -t` dropping in a same-size file that
            // carries an old mtime still below the directory's max: aggregates
            // are unchanged, but the per-path mtime moved.
            let now = std::time::SystemTime::now();
            let t_old = now - std::time::Duration::from_secs(60);
            let t_max = now - std::time::Duration::from_secs(10);
            let t_carried = now - std::time::Duration::from_secs(40);
            let tmp = assert_fs::TempDir::new().unwrap();
            tmp.child("a.txt").write_str("hello").unwrap();
            tmp.child("b.txt").write_str("world").unwrap();
            set_mtime(&tmp.path().join("a.txt"), t_old);
            set_mtime(&tmp.path().join("b.txt"), t_max);
            let before = dir_fingerprint(tmp.path()).unwrap();

            // Same-size content replacement whose mtime stays under the max.
            tmp.child("a.txt").write_str("HELLO").unwrap();
            set_mtime(&tmp.path().join("a.txt"), t_carried);

            let after = dir_fingerprint(tmp.path()).unwrap();
            assert_eq!(before.files, after.files);
            assert_eq!(before.bytes, after.bytes);
            assert_eq!(before.max_mtime_ns, after.max_mtime_ns);
            assert_ne!(before, after, "per-path mtime must break the fingerprint");
        });
    }

    #[test]
    fn fresh_mtimes_are_not_cached() {
        with_home(|home| {
            // Files written just now sit inside mtime granularity — caching them
            // could hide a same-size edit made a moment later.
            let tmp = assert_fs::TempDir::new().unwrap();
            tmp.child("a.txt").write_str("hello").unwrap();
            let d = dir_digest_cached(tmp.path()).unwrap();
            assert_eq!(d, dir_digest(tmp.path()).unwrap());
            let text = fs::read_to_string(home.path().join("digest-cache.json"))
                .unwrap_or_else(|_| "{}".into());
            assert!(
                !text.contains(&cache_key(tmp.path())),
                "settle window keeps fresh dirs out of the cache"
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
