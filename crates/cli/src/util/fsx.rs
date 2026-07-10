//! Small filesystem helpers shared across commands.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

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
                    copy_dir_impl(&real, &to, follow_symlinks)?;
                } else {
                    fs::copy(&real, &to).with_context(|| format!("copying {}", from.display()))?;
                }
            }
        } else if ft.is_dir() {
            copy_dir_impl(&from, &to, follow_symlinks)?;
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
}
