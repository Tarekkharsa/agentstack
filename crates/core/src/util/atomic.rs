//! Crash-safe file writes for the configs we touch. agentstack edits *live*
//! files (`~/.claude.json`, `CLAUDE.md`, the manifest), so a partial write on a
//! crash must never corrupt them. We write to a temp file in the same directory
//! and atomically `rename` it over the target, and we keep a pre-write backup so
//! a bad apply is recoverable.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};

use crate::util::paths;

/// Atomically write `contents` to `path`: back up the current file (best
/// effort), write a sibling temp file, fsync it, then `rename` it into place.
pub fn write(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
    }
    if path.exists() {
        let _ = backup(path); // best effort — never block a write on backup
    }
    let tmp = tmp_path(path);
    // Write, then fsync the temp file so its bytes are durably on disk BEFORE
    // the rename. Without the fsync, a crash right after the rename can leave
    // the renamed file EMPTY on common filesystems: the rename's directory
    // metadata reaches disk before the file's data pages do. fsync-then-rename
    // is the standard durable-replace recipe.
    {
        let mut f = fs::File::create(&tmp).with_context(|| format!("writing {}", tmp.display()))?;
        f.write_all(contents.as_bytes())
            .with_context(|| format!("writing {}", tmp.display()))?;
        f.sync_all()
            .with_context(|| format!("flushing {}", tmp.display()))?;
    }
    fs::rename(&tmp, path).with_context(|| {
        let _ = fs::remove_file(&tmp);
        format!("replacing {}", path.display())
    })?;
    // Best-effort: fsync the containing directory so the rename itself is
    // durable across a crash. A failure here can't corrupt the file (it is
    // already in place), so it never fails the write. Opening a directory as a
    // File is a no-op on platforms that don't support it (the open errors and
    // we skip it).
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty());
    if let Ok(handle) = fs::File::open(dir.unwrap_or_else(|| Path::new("."))) {
        let _ = handle.sync_all();
    }
    Ok(())
}

/// Copy the current file to `~/.agentstack/backups/<sanitized-path>` (a single
/// rolling backup per target — the last content before our most recent write).
pub fn backup(path: &Path) -> Result<PathBuf> {
    let dir = paths::backups_dir();
    fs::create_dir_all(&dir)?;
    let dst = dir.join(sanitize(&path.to_string_lossy()));
    fs::copy(path, &dst)?;
    Ok(dst)
}

/// The backup path for a given target (whether or not it exists yet).
pub fn backup_path(path: &Path) -> PathBuf {
    paths::backups_dir().join(sanitize(&path.to_string_lossy()))
}

/// Monotonic per-process counter so concurrent writes get distinct temp names.
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

fn tmp_path(path: &Path) -> PathBuf {
    // The temp name must be unique per writer, not just per target: two
    // processes (or threads) replacing the same file at once — e.g. a dashboard
    // `kill` and the foreground run wrapper both updating runs.json — would
    // otherwise share one temp path, and the loser's rename fails with ENOENT
    // after the winner renames it away.
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let mut s = path.as_os_str().to_os_string();
    s.push(format!(".agentstack-tmp.{}.{seq}", std::process::id()));
    PathBuf::from(s)
}

/// Backup file name for a target path. Two different paths can map to the same
/// readable form (`/a/b` and `/a-b` both → `-a-b`), so a short digest of the
/// FULL original path is appended: same target → same name (a rolling backup),
/// different target → different name (no silent clobber). The readable part is
/// bounded so a deep path can't blow past filesystem name limits.
fn sanitize(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    // `cleaned` is pure ASCII, so byte-slicing the tail (the distinctive end of
    // a path) can't split a char. Keep the last 160 bytes at most.
    let tail = &cleaned[cleaned.len().saturating_sub(160)..];
    let digest = &crate::digest::sha256_hex(s.as_bytes())[..12];
    format!("{tail}.{digest}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::prelude::*;

    #[test]
    fn writes_then_atomically_replaces() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let f = tmp.child("config.json");
        write(f.path(), "{\"a\":1}").unwrap();
        assert_eq!(fs::read_to_string(f.path()).unwrap(), "{\"a\":1}");
        // Overwrite — content fully replaced, no temp file left behind.
        write(f.path(), "{\"a\":2}").unwrap();
        assert_eq!(fs::read_to_string(f.path()).unwrap(), "{\"a\":2}");
        let leftovers: Vec<_> = fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".agentstack-tmp"))
            .collect();
        assert!(leftovers.is_empty(), "stray temp files: {leftovers:?}");
    }

    /// Concurrent replaces of the SAME target must all succeed. With a shared
    /// temp name the loser's rename hit ENOENT after the winner renamed the
    /// temp file away — the race behind the flaky `runs.json` writes when a
    /// kill and the run wrapper's cleanup fired together.
    #[test]
    fn concurrent_writes_to_one_target_all_succeed() {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = assert_fs::TempDir::new().unwrap();
        // Keep the best-effort backups inside the tempdir, not the real home.
        std::env::set_var("AGENTSTACK_HOME", tmp.path());
        let target = tmp.child("shared.json").path().to_path_buf();
        std::thread::scope(|s| {
            for i in 0..8 {
                let target = target.clone();
                s.spawn(move || {
                    for j in 0..25 {
                        write(&target, &format!("writer-{i}-{j}")).unwrap();
                    }
                });
            }
        });
        // Whoever renamed last wins, but the file is intact and complete.
        let last = fs::read_to_string(&target).unwrap();
        assert!(last.starts_with("writer-"), "unexpected content: {last}");
        std::env::remove_var("AGENTSTACK_HOME");
    }

    #[test]
    fn backup_names_dont_collide_across_distinct_paths() {
        // The pre-2026-07 sanitizer mapped every non-word char to `-`, so these
        // two distinct targets shared one backup file. The digest suffix keeps
        // them apart; the same path still maps to a stable name.
        assert_ne!(sanitize("/a/b"), sanitize("/a-b"));
        assert_eq!(sanitize("/a/b"), sanitize("/a/b"));
    }

    #[test]
    fn backup_captures_previous_content() {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", home.path());
        let tmp = assert_fs::TempDir::new().unwrap();
        let f = tmp.child("c.toml");
        f.write_str("original").unwrap();
        let b = backup(f.path()).unwrap();
        assert_eq!(fs::read_to_string(&b).unwrap(), "original");
        std::env::remove_var("AGENTSTACK_HOME");
    }
}
