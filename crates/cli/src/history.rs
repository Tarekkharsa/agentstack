//! Apply history: a per-apply snapshot of every target file we were about to
//! overwrite, so a bad apply is reversible via `restore` (and t3code's
//! read-only Activity tab lists each entry). Each `apply`
//! that writes records one entry under `~/.agentstack/history/<id>.json` holding
//! the *pre-write* content of each touched file. Undo restores those bytes (or
//! deletes a file that didn't exist before). The manifest is left untouched —
//! undo reverts your tools, not your declared stack — so reverted changes simply
//! show up as pending again.
//!
//! # What this ledger deliberately does not hold
//!
//! A recorded change is **a file and its pre-write bytes** ([`FileChange`]),
//! and [`rollback`] either writes those bytes back or deletes a file that did
//! not exist before. A skill materialized by `use --write` is neither: it is a
//! SYMLINK to a directory (the default strategy) or a copied directory TREE.
//! [`capture`] reads no content from either — `before` comes out `None` — and
//! `rollback`'s `remove_file` cannot act on a directory at all. `x unrender`
//! reached the same conclusion first and marks its skills leg `capture: false`
//! for exactly this reason.
//!
//! Teaching the ledger to carry them would take more than a second entry kind.
//! [`rollback`] acts on a path with **no ownership test**; the only proof that a
//! skill directory is ours lives in [`crate::render::skills`] (it is a symlink,
//! or it carries the `.agentstack-managed` marker). And even that marker proves
//! only that WE CREATED the directory — never that its contents are still ours.
//! An undo built on it would take a `SKILL.md` the user edited after delivery.
//! Since an undo that cannot tell our bytes from the user's must not run, the
//! ledger stays narrow and every Undo surface says so: see
//! [`SKILLS_ARE_NOT_RECORDED`].

/// Why a materialized skill never appears in an undo listing, in the words the
/// Undo surfaces print. One sentence, one place, so `undo`, `x restore` and
/// `use --write` cannot drift into three different promises.
pub const SKILLS_ARE_NOT_RECORDED: &str =
    "materialized skills are not recorded here — this ledger holds files, and a delivered skill \
     is a linked directory";

/// The way back for a materialized skill, since this ledger has none. Both are
/// real, verified paths: `x uninstall` plans the skills leg through
/// `unrender::plan`, and activating a toolset without a skill prunes it.
pub const SKILLS_COME_OFF_WITH: &str =
    "take them off with `agentstack x uninstall --write`, or activate a toolset that omits them";

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::util::paths;

/// Keep the most recent N apply events; older ones are pruned.
const MAX_ENTRIES: usize = 40;

/// Last id value handed out this process. Two `record` calls can land in the
/// same nanosecond (coarse clock / fast machine), which would give them the
/// same `<id>.json` filename and silently clobber the first. Clamping each new
/// id to at least `last + 1` keeps every entry distinct while preserving
/// lexicographic-== time order. Truncating nanos to `u64` matches the existing
/// 16-hex-digit id width (good past year 2500).
static LAST_ID_NANOS: AtomicU64 = AtomicU64::new(0);

/// Turn an observed nanosecond timestamp into a strictly-increasing,
/// process-unique id value.
fn monotonic_id_nanos(observed: u128) -> u64 {
    let observed = observed as u64;
    let mut last = LAST_ID_NANOS.load(Ordering::Relaxed);
    loop {
        let next = observed.max(last.wrapping_add(1));
        match LAST_ID_NANOS.compare_exchange_weak(last, next, Ordering::SeqCst, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(cur) => last = cur,
        }
    }
}

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
    /// Entries written as part of one user-facing operation share a batch id.
    /// `restore --last` reverses the whole batch newest-to-oldest, so a setup
    /// that imports a manifest and then applies native configs is genuinely
    /// one undo even though each phase keeps its own history entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch: Option<String>,
    /// The command that produced this entry, e.g. `session start 'backend'` or
    /// `apply`. Without this, `restore`'s bare listing showed rows that were
    /// textually identical apart from age — nothing said whether a row was an
    /// `init`, an `apply`, or a `session start` (review finding H7). Every
    /// [`record`] call site now names its own operation.
    ///
    /// `#[serde(default = "legacy_operation")]` because entries written before
    /// this field existed have no `operation` key in their JSON — an old
    /// ledger must keep loading (and rendering something other than a blank
    /// or a panic), not just newly recorded ones.
    #[serde(default = "legacy_operation")]
    pub operation: String,
    #[serde(default)]
    pub undone: bool,
}

/// What a pre-H7 entry renders as: it genuinely doesn't know which command
/// wrote it, so this says that plainly instead of guessing or leaving the
/// column blank (an empty string there would read as a rendering bug, not a
/// fact about old data).
fn legacy_operation() -> String {
    "unlabeled change (recorded before undo entries named their operation)".to_string()
}

thread_local! {
    /// Batch context is thread-local: command execution is synchronous, while
    /// tests may record history concurrently on independent worker threads.
    static ACTIVE_BATCH: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// RAII scope for grouping every [`record`] call made by one user-facing
/// operation. Dropping the guard restores any outer batch.
pub struct BatchGuard {
    previous: Option<String>,
}

impl Drop for BatchGuard {
    fn drop(&mut self) {
        ACTIVE_BATCH.with(|active| {
            active.replace(self.previous.take());
        });
    }
}

/// Start a history batch for the current thread.
pub fn begin_batch(label: &str) -> BatchGuard {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let id = format!("{label}-{:016x}", now.as_nanos());
    let previous = ACTIVE_BATCH.with(|active| active.replace(Some(id)));
    BatchGuard { previous }
}

fn active_batch() -> Option<String> {
    ACTIVE_BATCH.with(|active| active.borrow().clone())
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

/// Persist one apply event. `operation` names the command that produced it
/// (e.g. `"apply"`, `"session start 'backend'"`) — it is what `restore`'s
/// listing renders so undo history reads as an audit trail instead of rows
/// distinguishable only by timestamp. Returns the new entry id (or `None` if
/// nothing was captured). Best-effort: history must never break an
/// otherwise-good apply.
pub fn record(
    scope: &str,
    operation: impl Into<String>,
    targets: Vec<String>,
    files: Vec<FileChange>,
) -> Result<Option<String>> {
    if files.is_empty() {
        return Ok(None);
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    // 16 hex digits, zero-padded so ids stay fixed-width (lexicographic order
    // == time order). Not 32: nanoseconds-since-epoch only fills ~16 digits,
    // and the extra leading zeros made every displayed 8-char prefix "00000000".
    // `monotonic_id_nanos` bumps past any same-nanosecond predecessor so two
    // back-to-back records never share a filename.
    let id = format!("{:016x}", monotonic_id_nanos(now.as_nanos()));
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
        batch: active_batch(),
        operation: operation.into(),
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
    rollback(&entry.files)?;
    entry.undone = true;
    let mut out = serde_json::to_string_pretty(&entry)?;
    out.push('\n');
    fs::write(&path, out).with_context(|| format!("updating history entry {id}"))?;
    Ok(())
}

/// Revert every entry in `ids` (newest first), and record the revert itself
/// as a new entry — so an undo is undoable.
///
/// This is the whole difference between [`undo`] and the timeline's revert.
/// [`undo`] flips `undone = true` in place and leaves no row of its own: the
/// action that changed the user's files is the one action the ledger does not
/// contain, so "I undid one step too far" has no way back through any command
/// we expose. Capturing the *pre-undo* bytes first turns the revert into an
/// ordinary entry like any other, and undoing that entry is a redo.
///
/// No new destructive machinery: the reverting is still [`undo`] per entry,
/// which is still [`rollback`] per file. The only addition is the capture
/// before and the [`record`] after — and it deliberately happens in that
/// order, so a crash mid-revert leaves a durable way back rather than a
/// half-reverted project with nothing describing it.
///
/// `ids` must already be newest-first; the caller owns the selection, because
/// "which point to revert to" is a product decision and this is the ledger.
pub fn undo_recorded(ids: &[String]) -> Result<Option<String>> {
    if ids.is_empty() {
        return Ok(None);
    }
    // Capture what is on disk NOW, across every file the revert will touch.
    // Deduplicated by path, keeping the FIRST sighting: `ids` is newest-first,
    // and the newest entry's view of a path is the state the revert starts
    // from. Capturing the same path twice would make the redo replay an
    // intermediate state that never existed as a resting point.
    let mut seen = std::collections::HashSet::new();
    let mut before = Vec::new();
    // The revert inherits the scope of the newest entry it reverses — it acts
    // on the same files, so claiming a different scope would misdescribe it.
    let mut scope = String::from("project");
    for id in ids {
        let path = dir().join(format!("{id}.json"));
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(entry) = serde_json::from_str::<Entry>(&text) else {
            continue;
        };
        if before.is_empty() {
            scope = entry.scope.clone();
        }
        for f in &entry.files {
            if seen.insert(f.path.clone()) {
                before.push(capture(Path::new(&f.path), f.label.clone()));
            }
        }
    }

    let summary_targets = vec![format!("back to before {} change(s)", ids.len())];
    // Recorded BEFORE the reverting starts, for the same reason every other
    // record-before-mutation call site does it: an interrupted revert must
    // still leave a way back.
    let recorded = record(
        &scope,
        format!("undo of {} change(s)", ids.len()),
        summary_targets,
        before,
    )?;

    for id in ids {
        // A revert spanning several entries must not abort halfway on one that
        // was already undone concurrently — the remaining entries are still
        // the user's request. A genuinely broken entry still errors.
        match undo(id) {
            Ok(()) => {}
            Err(e) if e.to_string().contains("already undone") => {}
            Err(e) => return Err(e),
        }
    }
    Ok(recorded)
}

/// Drop an entry that turned out to describe nothing.
///
/// This exists for the record-before-mutation pattern: a command that must
/// survive a crash records its undo entry BEFORE the first write, so an
/// interrupted run still leaves a durable way back. When such a run instead
/// fails cleanly and rolls itself back completely, the entry it pre-recorded
/// now describes a project state that already holds — keeping it would put a
/// no-op at the head of the ledger and shadow the user's real last change from
/// `restore --last`. Deleting is safe precisely because the entry is
/// declarative (put these bytes back / delete this file), so applying it twice
/// is the same as applying it once; a caller that could NOT fully roll back
/// must keep its entry instead.
pub fn discard(id: &str) {
    let _ = fs::remove_file(dir().join(format!("{id}.json")));
}

/// Restore captured files in reverse write order. Public so a command that is
/// still assembling an entry can roll back partial writes if a later write or
/// the history record itself fails.
pub fn rollback(files: &[FileChange]) -> Result<()> {
    for f in files.iter().rev() {
        let p = Path::new(&f.path);
        match &f.before {
            Some(content) => crate::util::atomic::write(p, content)?,
            None => {
                if p.exists() {
                    fs::remove_file(p)
                        .with_context(|| format!("removing {} during rollback", p.display()))?;
                }
            }
        }
    }
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
        let id = record("global", "apply", vec!["Test".into()], vec![cap])
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
        let id = record("global", "apply", vec!["Test".into()], vec![cap])
            .unwrap()
            .unwrap();

        undo(&id).unwrap();
        assert!(!file.exists());
        std::env::remove_var("AGENTSTACK_HOME");
    }

    #[test]
    fn batch_guard_tags_every_record_and_then_restores_the_outer_context() {
        let _guard = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", home.path());
        let work = assert_fs::TempDir::new().unwrap();

        {
            let _batch = begin_batch("setup");
            for name in ["one", "two"] {
                let file = work.path().join(name);
                let cap = capture(&file, name);
                fs::write(&file, "after").unwrap();
                record("project", "setup", Vec::new(), vec![cap]).unwrap();
            }
        }

        let entries = list();
        assert_eq!(entries.len(), 2);
        let batch = entries[0].batch.as_deref().expect("batch id");
        assert!(batch.starts_with("setup-"));
        assert_eq!(entries[1].batch.as_deref(), Some(batch));

        let file = work.path().join("outside");
        let cap = capture(&file, "outside");
        fs::write(&file, "after").unwrap();
        record("project", "apply", Vec::new(), vec![cap]).unwrap();
        assert!(list()[0].batch.is_none(), "batch context ended with guard");
        std::env::remove_var("AGENTSTACK_HOME");
    }

    /// An entry written before `operation` existed (no key in its JSON at
    /// all) must still deserialize — and render something that says plainly
    /// it predates labeling, not an empty string.
    #[test]
    fn entry_without_operation_field_loads_with_a_legacy_label() {
        let json = r#"{
            "id": "18c61936",
            "time_unix": 1,
            "scope": "project",
            "summary": "2 files · Claude Code, Codex CLI",
            "targets": ["Claude Code", "Codex CLI"],
            "files": []
        }"#;
        let entry: Entry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.operation, legacy_operation());
        assert!(!entry.operation.is_empty());
    }
}
