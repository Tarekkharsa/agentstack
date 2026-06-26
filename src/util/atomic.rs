//! Crash-safe file writes for the configs we touch. agentstack edits *live*
//! files (`~/.claude.json`, `CLAUDE.md`, the manifest), so a partial write on a
//! crash must never corrupt them. We write to a temp file in the same directory
//! and atomically `rename` it over the target, and we keep a pre-write backup so
//! a bad apply is recoverable.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::util::paths;

/// Atomically write `contents` to `path`: back up the current file (best
/// effort), write a sibling temp file, then `rename` it into place.
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
    fs::write(&tmp, contents).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| {
        let _ = fs::remove_file(&tmp);
        format!("replacing {}", path.display())
    })?;
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

fn tmp_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(".agentstack-tmp");
    PathBuf::from(s)
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
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
        assert!(!tmp.child("config.json.agentstack-tmp").path().exists());
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
