//! Apply history: a per-apply snapshot of every target file we were about to
//! overwrite, so a bad apply is reversible via `restore` (and the dashboard's
//! read-only Activity tab lists each entry). Each `apply`
//! that writes records one entry under `~/.agentstack/history/<id>.json` holding
//! the *pre-write* content of each touched file. Undo restores those bytes (or
//! deletes a file that didn't exist before). The manifest is left untouched —
//! undo reverts your tools, not your declared stack — so reverted changes simply
//! show up as pending again.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::util::paths;

/// Keep the most recent N apply events; older ones are pruned.
const MAX_ENTRIES: usize = 40;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    /// File content before this apply; `None` if the file did not exist.
    pub before: Option<String>,
    /// Short label, e.g. "Claude Code · servers".
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub time_unix: u64,
    pub scope: String,
    pub summary: String,
    pub targets: Vec<String>,
    pub files: Vec<FileChange>,
    #[serde(default)]
    pub undone: bool,
}

pub fn dir() -> PathBuf {
    paths::agentstack_home().join("history")
}

/// Snapshot a file's current content for later undo. Call immediately before
/// the write that will replace it.
pub fn capture(path: &Path, label: impl Into<String>) -> FileChange {
    let before = fs::read_to_string(path).ok();
    FileChange {
        path: path.to_string_lossy().into_owned(),
        before,
        label: label.into(),
    }
}

/// Persist one apply event. Returns the new entry id (or `None` if nothing was
/// captured). Best-effort: history must never break an otherwise-good apply.
pub fn record(scope: &str, targets: Vec<String>, files: Vec<FileChange>) -> Result<Option<String>> {
    if files.is_empty() {
        return Ok(None);
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    // 16 hex digits, zero-padded so ids stay fixed-width (lexicographic order
    // == time order). Not 32: nanoseconds-since-epoch only fills ~16 digits,
    // and the extra leading zeros made every displayed 8-char prefix "00000000".
    let id = format!("{:016x}", now.as_nanos());
    let summary = format!(
        "{} file{} · {}",
        files.len(),
        if files.len() == 1 { "" } else { "s" },
        if targets.is_empty() {
            "—".to_string()
        } else {
            targets.join(", ")
        }
    );
    let entry = Entry {
        id: id.clone(),
        time_unix: now.as_secs(),
        scope: scope.to_string(),
        summary,
        targets,
        files,
        undone: false,
    };
    let d = dir();
    fs::create_dir_all(&d).with_context(|| format!("creating {}", d.display()))?;
    let path = d.join(format!("{id}.json"));
    let mut text = serde_json::to_string_pretty(&entry)?;
    text.push('\n');
    fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    prune(MAX_ENTRIES);
    Ok(Some(id))
}

/// All recorded apply events, newest first.
pub fn list() -> Vec<Entry> {
    let mut out: Vec<Entry> = fs::read_dir(dir())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
        .filter_map(|p| fs::read_to_string(p).ok())
        .filter_map(|s| serde_json::from_str::<Entry>(&s).ok())
        .collect();
    out.sort_by(|a, b| b.time_unix.cmp(&a.time_unix).then_with(|| b.id.cmp(&a.id)));
    out
}

/// Restore the files captured by entry `id` to their pre-apply content.
pub fn undo(id: &str) -> Result<()> {
    let path = dir().join(format!("{id}.json"));
    let text = fs::read_to_string(&path).with_context(|| format!("reading history entry {id}"))?;
    let mut entry: Entry = serde_json::from_str(&text)?;
    if entry.undone {
        anyhow::bail!("this change was already undone");
    }
    for f in &entry.files {
        let p = Path::new(&f.path);
        match &f.before {
            Some(content) => crate::util::atomic::write(p, content)?,
            None => {
                if p.exists() {
                    let _ = fs::remove_file(p);
                }
            }
        }
    }
    entry.undone = true;
    let mut out = serde_json::to_string_pretty(&entry)?;
    out.push('\n');
    fs::write(&path, out).with_context(|| format!("updating history entry {id}"))?;
    Ok(())
}

fn prune(max: usize) {
    let entries = list();
    if entries.len() <= max {
        return;
    }
    for e in entries.into_iter().skip(max) {
        let _ = fs::remove_file(dir().join(format!("{}.json", e.id)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::TEST_ENV_LOCK;

    #[test]
    fn capture_record_and_undo_roundtrip() {
        let _guard = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", home.path());
        let work = assert_fs::TempDir::new().unwrap();
        let file = work.path().join("c.json");
        fs::write(&file, "before").unwrap();

        let cap = capture(&file, "Test · servers");
        // Simulate the apply overwriting the file.
        fs::write(&file, "after").unwrap();
        let id = record("global", vec!["Test".into()], vec![cap])
            .unwrap()
            .unwrap();

        assert_eq!(list().len(), 1);
        undo(&id).unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "before");
        // A second undo is refused.
        assert!(undo(&id).is_err());
        std::env::remove_var("AGENTSTACK_HOME");
    }

    #[test]
    fn undo_deletes_a_file_that_did_not_exist() {
        let _guard = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", home.path());
        let work = assert_fs::TempDir::new().unwrap();
        let file = work.path().join("new.json");

        let cap = capture(&file, "Test · servers"); // file absent → before = None
        fs::write(&file, "created by apply").unwrap();
        let id = record("global", vec!["Test".into()], vec![cap])
            .unwrap()
            .unwrap();

        undo(&id).unwrap();
        assert!(!file.exists());
        std::env::remove_var("AGENTSTACK_HOME");
    }
}
