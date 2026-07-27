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

/// The mode a secret-bearing file is created with: owner read/write only. A
/// `.env` holds real token values, so it must not be readable by other local
/// accounts — the same bar `crates/cli`'s key and machine-policy writers already
/// hold themselves to.
#[cfg(unix)]
pub const PRIVATE_MODE: u32 = 0o600;

/// Atomically write `contents` to `path`: back up the current file (best
/// effort), write a sibling temp file, fsync it, then `rename` it into place.
pub fn write(path: &Path, contents: &str) -> Result<()> {
    write_inner(path, contents, false)
}

/// Like [`write`], but the result is readable only by its owner ([`PRIVATE_MODE`]
/// on Unix; a no-op elsewhere). Use this for every file that holds a real secret
/// *value* rather than a `${REF}` placeholder.
///
/// The mode is applied to the temp file *before* any bytes are written, so the
/// secret is never briefly world-readable, and the rename carries that inode's
/// permissions to the target — replacing a too-permissive file left behind by an
/// older version. Any pre-write backup is tightened the same way.
pub fn write_private(path: &Path, contents: &str) -> Result<()> {
    write_inner(path, contents, true)
}

fn write_inner(path: &Path, contents: &str, private: bool) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
    }
    if path.exists() {
        // Best effort — never block a write on backup. A backup of a secret file
        // is itself a secret file: `fs::copy` carries the source mode, but the
        // source may predate `write_private`, so tighten it explicitly.
        if let Ok(dst) = backup(path) {
            if private {
                harden(&dst);
            }
        }
    }
    let tmp = tmp_path(path);
    // Write, then fsync the temp file so its bytes are durably on disk BEFORE
    // the rename. Without the fsync, a crash right after the rename can leave
    // the renamed file EMPTY on common filesystems: the rename's directory
    // metadata reaches disk before the file's data pages do. fsync-then-rename
    // is the standard durable-replace recipe.
    {
        let mut f = create(&tmp, private).with_context(|| format!("writing {}", tmp.display()))?;
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

/// Create the temp file, restricting its mode up front when the payload is a
/// secret. `OpenOptions::mode` intersects with the process umask, so the result
/// is never *more* permissive than [`PRIVATE_MODE`].
fn create(path: &Path, private: bool) -> std::io::Result<fs::File> {
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::OpenOptionsExt;
        return fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(PRIVATE_MODE)
            .open(path);
    }
    let _ = private; // no permission bits to set off Unix
    fs::File::create(path)
}

/// Best-effort tightening of an existing file to owner-only. Used for backups of
/// secret files, where failing to chmod must not fail the write it protects.
fn harden(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(PRIVATE_MODE));
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Whether `path` is readable by anyone other than its owner. `None` when the
/// mode cannot be read (missing file, or a platform without Unix permissions),
/// so callers can distinguish "not a problem" from "cannot tell".
#[cfg(unix)]
pub fn is_group_or_world_readable(path: &Path) -> Option<bool> {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(path).ok()?.permissions().mode();
    Some(mode & 0o077 != 0)
}

/// Off Unix there are no permission bits to inspect, so every caller gets
/// "cannot tell" rather than a false all-clear.
#[cfg(not(unix))]
pub fn is_group_or_world_readable(_path: &Path) -> Option<bool> {
    None
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
    // processes (or threads) replacing the same file at once — e.g. an external UI
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

    /// Rule 5's filesystem half: a file holding real secret *values* must not be
    /// readable by other local accounts. Before this, `.env` inherited the
    /// default umask (0644 on a normal machine) while the CLI's key and
    /// machine-policy writers already used 0600.
    #[cfg(unix)]
    #[test]
    fn write_private_leaves_the_file_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = assert_fs::TempDir::new().unwrap();
        let f = tmp.child("secrets.env");
        write_private(f.path(), "TOKEN=abc\n").unwrap();
        let mode = fs::metadata(f.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, PRIVATE_MODE, "expected 0600, got {mode:o}");
        assert_eq!(is_group_or_world_readable(f.path()), Some(false));
        // A plain `write` to the same path is the contrast the bug was about.
        let g = tmp.child("plain.json");
        write(g.path(), "{}").unwrap();
        assert_eq!(is_group_or_world_readable(g.path()), Some(true));
    }

    /// Replacing a file an older version left at 0644 must tighten it: the
    /// rename carries the temp inode's mode, so the permissive inode goes away
    /// rather than being written into again.
    #[cfg(unix)]
    #[test]
    fn write_private_tightens_a_previously_permissive_file() {
        use std::os::unix::fs::PermissionsExt;
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", home.path());
        let tmp = assert_fs::TempDir::new().unwrap();
        let f = tmp.child("secrets.env");
        f.write_str("TOKEN=old\n").unwrap();
        fs::set_permissions(f.path(), fs::Permissions::from_mode(0o644)).unwrap();

        write_private(f.path(), "TOKEN=new\n").unwrap();

        let mode = fs::metadata(f.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, PRIVATE_MODE, "expected 0600, got {mode:o}");
        assert_eq!(fs::read_to_string(f.path()).unwrap(), "TOKEN=new\n");
        // The pre-write backup holds the old secret, so it is private too.
        let b = backup_path(f.path());
        assert_eq!(fs::read_to_string(&b).unwrap(), "TOKEN=old\n");
        let bmode = fs::metadata(&b).unwrap().permissions().mode() & 0o777;
        assert_eq!(bmode, PRIVATE_MODE, "backup leaked at {bmode:o}");
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
