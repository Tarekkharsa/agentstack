//! Content store: `~/.agentstack/store/` — where capability sources are fetched
//! and cached (PLAN §9d). Git sources are cloned/checked-out via the `git` CLI;
//! path sources pass through. A content digest gives the lockfile its integrity
//! field.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::manifest::{Skill, SkillSource};
use crate::util::paths;

pub struct Store {
    root: PathBuf,
}

/// The resolved local location of a skill's content.
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
        match skill.source()? {
            SkillSource::Path(p) => {
                let path = resolve_path(manifest_dir, &p);
                let checksum = if path.exists() {
                    dir_digest(&path)?
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
            SkillSource::Git { url, rev } => {
                let want = pinned_rev.map(str::to_string).or(rev);
                let dest = self.git_dir(&url);
                let fetched = ensure_git(&url, want.as_deref(), &dest)?;
                let resolved_rev = git_head(&dest)?;
                let checksum = dir_digest(&dest)?;
                Ok(Resolved {
                    path: dest,
                    rev: Some(resolved_rev),
                    checksum,
                    fetched,
                    source_kind: "git",
                })
            }
        }
    }

    fn git_dir(&self, url: &str) -> PathBuf {
        self.root.join("git").join(sanitize(url))
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
        SkillSource::Git { url, .. } => {
            let dir = store.git_dir(&url);
            dir.exists().then_some(dir)
        }
    }
}

fn resolve_path(dir: &Path, p: &str) -> PathBuf {
    let pb = PathBuf::from(p);
    if pb.is_absolute() {
        pb
    } else {
        dir.join(pb)
    }
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

/// FNV-1a digest of a directory's contents (relative paths + file bytes,
/// sorted; `.git` excluded). Integrity check, not security.
pub fn dir_digest(root: &Path) -> Result<String> {
    let mut files: Vec<PathBuf> = Vec::new();
    collect_files(root, root, &mut files)?;
    files.sort();
    let mut h: u64 = 0xcbf29ce484222325;
    let feed = |bytes: &[u8], h: &mut u64| {
        for b in bytes {
            *h ^= *b as u64;
            *h = h.wrapping_mul(0x100000001b3);
        }
    };
    for rel in &files {
        feed(rel.to_string_lossy().as_bytes(), &mut h);
        let bytes = fs::read(root.join(rel))
            .with_context(|| format!("reading {}", root.join(rel).display()))?;
        feed(&bytes, &mut h);
    }
    Ok(format!("{h:016x}"))
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_name() == ".git" {
            continue;
        }
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            collect_files(root, &path, out)?;
        } else if let Ok(rel) = path.strip_prefix(root) {
            out.push(rel.to_path_buf());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::prelude::*;

    #[test]
    fn dir_digest_stable_and_sensitive() {
        let tmp = assert_fs::TempDir::new().unwrap();
        tmp.child("a.txt").write_str("hello").unwrap();
        tmp.child("sub/b.txt").write_str("world").unwrap();
        let d1 = dir_digest(tmp.path()).unwrap();
        let d2 = dir_digest(tmp.path()).unwrap();
        assert_eq!(d1, d2);
        tmp.child("a.txt").write_str("changed").unwrap();
        assert_ne!(d1, dir_digest(tmp.path()).unwrap());
    }

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
}
