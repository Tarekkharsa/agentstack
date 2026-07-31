// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Strategy v2 Phase 3, item 3 — the undo timeline.
//!
//! Two properties the design rests on, plus the one it adds:
//!
//! 1. **Revert-to-point equals the corresponding restore sequence,
//!    byte-for-byte.** `undo` is a second door onto `restore`'s room, not a
//!    second implementation of reverting. If the two can ever disagree about
//!    the resulting bytes, the friendlier door is the one people will use and
//!    the tested one is the other.
//! 2. **The timeline never lists an unrecorded write.** Trust decisions,
//!    secret writes, and library changes are not in the history ledger.
//!    Offering to revert something we cannot revert is worse than not
//!    offering — the user acts on the offer and discovers the gap afterwards,
//!    while recovering.
//! 3. **The revert is itself recorded**, so one step too far is not a
//!    one-way door.

use std::fs;
use std::path::Path;
use std::sync::Mutex;

use agentstack::history;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn setup_home(home: &Path) {
    fs::create_dir_all(home).unwrap();
    std::env::set_var("HOME", home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));
}

/// Write `content` to `path`, recording the pre-write bytes the way every
/// real call site does.
fn recorded_write(path: &Path, content: &str, operation: &str) -> String {
    let change = history::capture(path, "test file");
    fs::write(path, content).unwrap();
    history::record(
        "project",
        operation.to_string(),
        vec!["test".to_string()],
        vec![change],
    )
    .unwrap()
    .expect("a captured file must produce an entry")
}

/// Property 1, the important one: reverting to a point through the timeline
/// leaves exactly the bytes that undoing the same entries through the ledger
/// leaves. Run twice over identical starting states and compared.
#[test]
fn revert_to_point_equals_the_restore_sequence_byte_for_byte() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    // Two identical worlds, driven two different ways.
    let mut results = Vec::new();
    for door in ["undo", "restore"] {
        let tmp = assert_fs::TempDir::new().unwrap();
        let home = tmp.path().join("home");
        setup_home(&home);
        let proj = tmp.path().join("proj");
        fs::create_dir_all(&proj).unwrap();

        let a = proj.join("a.txt");
        let b = proj.join("b.txt");
        fs::write(&a, "a0").unwrap();
        fs::write(&b, "b0").unwrap();

        let _first = recorded_write(&a, "a1", "apply");
        let second = recorded_write(&b, "b1", "use");
        let third = recorded_write(&a, "a2", "session start");

        // Revert to before the SECOND change: third and second come off, the
        // first stays. Newest-first is the order both doors use.
        let ids = vec![third.clone(), second.clone()];

        if door == "undo" {
            history::undo_recorded(&ids).unwrap();
        } else {
            // The sequence `restore` performs: one entry at a time, newest
            // first. This is the reference behaviour.
            for id in &ids {
                history::undo(id).unwrap();
            }
        }

        results.push((
            fs::read_to_string(&a).unwrap(),
            fs::read_to_string(&b).unwrap(),
        ));
    }

    assert_eq!(
        results[0], results[1],
        "the timeline's revert and the restore sequence disagreed about the resulting bytes"
    );
    // And it is the right answer, not merely a consistent one.
    assert_eq!(
        results[0],
        ("a1".to_string(), "b0".to_string()),
        "reverting to before the second change must undo the second and third only"
    );
}

/// Property 3: the revert leaves its own row, and undoing that row is a redo.
#[test]
fn the_revert_is_recorded_so_undo_is_undoable() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup_home(&home);
    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();

    let f = proj.join("f.txt");
    fs::write(&f, "v0").unwrap();
    let change = recorded_write(&f, "v1", "apply");
    assert_eq!(fs::read_to_string(&f).unwrap(), "v1");

    // Revert.
    let revert_id = history::undo_recorded(std::slice::from_ref(&change))
        .unwrap()
        .expect("the revert must record an entry of its own");
    assert_eq!(fs::read_to_string(&f).unwrap(), "v0", "the revert happened");

    // The revert is a row like any other.
    let rows = history::list();
    assert!(
        rows.iter().any(|e| e.id == revert_id && !e.undone),
        "the revert must appear in the ledger as an undoable entry: {:?}",
        rows.iter()
            .map(|e| (&e.id, &e.operation))
            .collect::<Vec<_>>()
    );

    // And undoing it is a redo — back to v1.
    history::undo(&revert_id).unwrap();
    assert_eq!(
        fs::read_to_string(&f).unwrap(),
        "v1",
        "undoing the revert must return the state the revert removed"
    );
}

/// Property 2: only recorded writes are offered.
///
/// The ledger is the single source the timeline reads, so this asserts the
/// thing that makes that safe — a write performed *without* recording leaves
/// nothing for the timeline to offer, rather than an entry that cannot be
/// honoured.
#[test]
fn an_unrecorded_write_is_never_offered() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup_home(&home);
    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();

    let recorded = proj.join("recorded.txt");
    fs::write(&recorded, "r0").unwrap();
    recorded_write(&recorded, "r1", "apply");

    // The kinds of write that genuinely are not in this ledger today: trust
    // decisions, secret files, library changes, and lockfile writes from
    // add/remove/install/upgrade/lock.
    let unrecorded = proj.join("trust.json");
    fs::write(&unrecorded, "granted").unwrap();

    let offered: Vec<String> = history::list()
        .into_iter()
        .flat_map(|e| e.files)
        .map(|f| f.path)
        .collect();

    assert!(
        offered.iter().any(|p| p.ends_with("recorded.txt")),
        "a recorded write must be offered: {offered:?}"
    );
    assert!(
        !offered.iter().any(|p| p.ends_with("trust.json")),
        "an unrecorded write must never be offered — we cannot honour it: {offered:?}"
    );
}

/// `undo` adds no way to change a file. Every byte it moves goes through the
/// same `history::rollback` → `atomic::write` path `restore` uses, so the
/// reversible-write guarantees hold for both doors without being restated.
#[test]
fn undo_composes_restore_and_adds_no_destructive_machinery() {
    let src = include_str!("../src/commands/undo.rs");

    for forbidden in ["fs::remove_file", "fs::write(", "fs::remove_dir"] {
        assert!(
            !src.contains(forbidden),
            "`undo` must not touch the filesystem directly — it composes \
             history::undo_recorded. Found: {forbidden}"
        );
    }
    assert!(
        src.contains("history::undo_recorded"),
        "the revert must go through the recorded-undo seam"
    );
}
