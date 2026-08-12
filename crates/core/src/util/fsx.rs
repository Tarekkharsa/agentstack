//! Small filesystem helpers shared across commands.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// The ancestor directories a write to `path` would have to create, deepest
/// first.
///
/// Called immediately BEFORE the write (the same moment a change ledger
/// snapshots the pre-write bytes), this is the only honest record of what a
/// write brought into existence: it walks up from the file's parent for as
/// long as nothing is there, and stops at the first ancestor that already
/// exists. Everything it returns is therefore ours; everything it omits
/// pre-dated us, which is what makes the reverse operation
/// ([`prune_empty_dirs`]) safe to run without a second ownership test.
///
/// `symlink_metadata` rather than `exists`, so a dangling symlink counts as
/// "something is already here" and never enters the list.
pub fn dirs_a_write_will_create(path: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut current = path.parent();
    while let Some(dir) = current {
        if dir.as_os_str().is_empty() || fs::symlink_metadata(dir).is_ok() {
            break;
        }
        out.push(dir.to_path_buf());
        current = dir.parent();
    }
    out
}

/// True when `path` is a real (non-symlink) directory holding nothing.
pub fn is_empty_real_dir(path: &Path) -> bool {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return false;
    }
    fs::read_dir(path)
        .map(|mut entries| entries.next().is_none())
        .unwrap_or(false)
}

/// Remove each candidate directory that is empty by the time we reach it,
/// deepest first; returns the ones actually removed, in removal order.
///
/// Three guards, in order of who enforces them:
///
/// * **Deepest first** — sorted here rather than trusted from the caller, so
///   `.agentstack/mcp` comes off before `.agentstack` can become empty.
/// * **Never a symlink** — checked explicitly, so a candidate path can never
///   redirect the removal somewhere else.
/// * **Never a directory holding anything** — enforced by `remove_dir` itself,
///   which is the guard that matters: a directory that also holds a user's file
///   simply refuses, and `DirectoryNotEmpty` is a normal outcome here, not an
///   error.
pub fn prune_empty_dirs(candidates: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut ordered: Vec<PathBuf> = candidates.to_vec();
    ordered.sort_by(|a, b| {
        b.components()
            .count()
            .cmp(&a.components().count())
            .then_with(|| a.cmp(b))
    });
    ordered.dedup();

    let mut removed = Vec::new();
    for path in ordered {
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("could not remove {}", path.display()))
            }
        };
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            continue;
        }
        match fs::remove_dir(&path) {
            Ok(()) => removed.push(path),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                ) => {}
            Err(error) => {
                return Err(error).with_context(|| format!("could not remove {}", path.display()))
            }
        }
    }
    Ok(removed)
}

/// Recursively copy `src` into `dst` (created if missing), skipping `.git`.
///
/// A source's `.git` is never carried into the copy: it bloats the
/// destination and, once the destination is itself a git repo (e.g. `lib
/// sync`), a nested `.git` is recorded as a gitlink whose body vanishes on
/// clone. A symlink is handed to `fs::copy` as-is (its target's bytes are
/// copied if it points at a file; a directory symlink errors) — use
/// [`copy_dir_all_following_symlinks`] to recurse into directory symlinks
/// instead.
pub fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    copy_dir_impl(src, dst, false)
}

/// Like [`copy_dir_all`], but a symlink whose target resolves to a directory
/// is recursed into (copying the target's contents, not the link), and a
/// broken/unreadable link is skipped silently.
pub fn copy_dir_all_following_symlinks(src: &Path, dst: &Path) -> Result<()> {
    copy_dir_impl(src, dst, true)
}

fn copy_dir_impl(src: &Path, dst: &Path, follow_symlinks: bool) -> Result<()> {
    let mut active_directories = HashSet::new();
    copy_dir_impl_inner(src, dst, follow_symlinks, &mut active_directories)
}

fn copy_dir_impl_inner(
    src: &Path,
    dst: &Path,
    follow_symlinks: bool,
    active_directories: &mut HashSet<std::path::PathBuf>,
) -> Result<()> {
    let canonical_src =
        fs::canonicalize(src).with_context(|| format!("canonicalizing {}", src.display()))?;
    if !active_directories.insert(canonical_src.clone()) {
        anyhow::bail!("symlink cycle while copying {}", src.display());
    }

    let result = copy_dir_contents(src, dst, follow_symlinks, active_directories);
    active_directories.remove(&canonical_src);
    result
}

fn copy_dir_contents(
    src: &Path,
    dst: &Path,
    follow_symlinks: bool,
    active_directories: &mut HashSet<std::path::PathBuf>,
) -> Result<()> {
    fs::create_dir_all(dst).with_context(|| format!("creating {}", dst.display()))?;
    for entry in fs::read_dir(src).with_context(|| format!("reading {}", src.display()))? {
        let entry = entry?;
        if entry.file_name() == ".git" {
            continue;
        }
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let ft = entry.file_type()?;
        if follow_symlinks && ft.is_symlink() {
            // Copy the link's target contents (skills rarely nest links; keep it simple).
            if let Ok(real) = fs::canonicalize(&from) {
                if real.is_dir() {
                    copy_dir_impl_inner(&real, &to, follow_symlinks, active_directories)?;
                } else {
                    fs::copy(&real, &to).with_context(|| format!("copying {}", from.display()))?;
                }
            }
        } else if ft.is_dir() {
            copy_dir_impl_inner(&from, &to, follow_symlinks, active_directories)?;
        } else {
            fs::copy(&from, &to).with_context(|| format!("copying {}", from.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::prelude::*;

    #[test]
    fn skips_git_dir() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let src = tmp.child("src");
        src.child("SKILL.md").write_str("# skill\n").unwrap();
        src.child(".git/HEAD")
            .write_str("ref: refs/heads/main\n")
            .unwrap();
        let dst = tmp.child("dst").path().to_path_buf();

        copy_dir_all(src.path(), &dst).unwrap();

        assert!(dst.join("SKILL.md").exists());
        assert!(!dst.join(".git").exists());
    }

    #[cfg(unix)]
    #[test]
    fn following_symlinks_recurses_into_a_linked_dir_and_still_skips_git() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let real = tmp.child("real");
        real.child("SKILL.md").write_str("# skill\n").unwrap();
        real.child(".git/HEAD")
            .write_str("ref: refs/heads/main\n")
            .unwrap();

        let src = tmp.child("src");
        src.create_dir_all().unwrap();
        std::os::unix::fs::symlink(real.path(), src.child("linked").path()).unwrap();
        let dst = tmp.child("dst").path().to_path_buf();

        copy_dir_all_following_symlinks(src.path(), &dst).unwrap();

        assert!(dst.join("linked/SKILL.md").exists());
        assert!(!dst.join("linked/.git").exists());
    }

    #[cfg(unix)]
    #[test]
    fn following_symlinks_skips_a_broken_link_silently() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let src = tmp.child("src");
        src.create_dir_all().unwrap();
        std::os::unix::fs::symlink(tmp.child("missing").path(), src.child("dangling").path())
            .unwrap();
        let dst = tmp.child("dst").path().to_path_buf();

        copy_dir_all_following_symlinks(src.path(), &dst).unwrap();

        assert!(!dst.join("dangling").exists());
    }

    #[cfg(unix)]
    #[test]
    fn following_symlinks_rejects_an_ancestor_cycle() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let src = tmp.child("src");
        src.child("nested").create_dir_all().unwrap();
        std::os::unix::fs::symlink(src.path(), src.child("nested/back").path()).unwrap();
        let dst = tmp.child("dst").path().to_path_buf();

        assert!(copy_dir_all_following_symlinks(src.path(), &dst).is_err());
    }
}
