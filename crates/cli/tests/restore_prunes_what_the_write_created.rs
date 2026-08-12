// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! **An undo must leave the project in the state it was in before the write.**
//!
//! `restore --last --write` and `undo --to <n> --write` deleted the files a
//! recorded write had created and stopped there, so undoing an `init` left an
//! empty `.agentstack/` behind — a directory the project never had, in a
//! command whose entire promise is that the write is reversible. Reversing a
//! write means reversing the directories it made to hold its files.
//!
//! The dangerous version of this fix is a cleanup that infers ownership from
//! the disk: an empty directory cannot tell you who made it, and a tool that
//! guesses will one day take a user's. So creation is a FACT read from the
//! ledger — `FileChange::created_dirs`, recorded by `history::capture` at the
//! one instant the answer exists (was the path there yet?) — and everything
//! else is out of reach by construction. `x uninstall` reached the same shape
//! first (`undo_promises.rs`, group 1); this shares its guards
//! through `util::fsx::prune_empty_dirs` rather than keeping a second copy.
//!
//! The three negative controls are the point of the file. A prune that fires
//! whenever a directory happens to be empty would pass the first test and fail
//! every one of these:
//!
//! * [`a_created_parent_holding_a_user_file_survives_the_undo`] — content the
//!   undo did not put there makes the directory untouchable.
//! * [`a_directory_the_write_did_not_create_survives_even_left_empty`] — a
//!   pre-existing directory stays, empty or not. This is also what keeps the
//!   cleanup out of a linked library checkout, a `.git`, and the project root:
//!   none of them were ever recorded as created.
//! * [`undoing_a_modification_records_and_prunes_no_directories`] — a write
//!   that changed an existing file created nothing, so its undo removes
//!   nothing.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

struct Run {
    text: String,
    code: i32,
}

/// Strip SGR escapes so an assertion reads the sentence, not its styling.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        if chars.next() != Some('[') {
            continue;
        }
        for f in chars.by_ref() {
            if ('@'..='~').contains(&f) {
                break;
            }
        }
    }
    out
}

/// Run the real binary in its own fenced HOME: the claim is about what a person
/// gets back after running two commands, so the evidence is the process's own
/// output and the directory it leaves behind.
fn run(args: &[&str], home: &Path, proj: &Path) -> Run {
    let out = Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(args)
        .current_dir(proj)
        .env_clear()
        .env("HOME", home)
        .env("AGENTSTACK_HOME", home.join(".agentstack"))
        .env("PATH", "/usr/bin:/bin")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .output()
        .expect("the binary must run");
    Run {
        text: strip_ansi(&format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )),
        code: out.status.code().expect("the process must exit normally"),
    }
}

/// An empty project and its own HOME. Nothing is created under the project —
/// `.agentstack/` must be `init`'s doing, or the tests below prove nothing.
fn project(tmp: &Path, name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let home = tmp.join(format!("{name}-home"));
    let proj = tmp.join(name);
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&proj).unwrap();
    (home, proj)
}

/// `init --yes` on a project with no detected CLIs writes the starter manifest
/// at `.agentstack/agentstack.toml`, creating `.agentstack/` on the way.
fn init(home: &Path, proj: &Path) {
    let run = run(&["init", "--yes"], home, proj);
    assert_eq!(run.code, 0, "fixture: init failed:\n{}", run.text);
    assert!(
        proj.join(".agentstack/agentstack.toml").exists(),
        "fixture: init must have written the manifest:\n{}",
        run.text
    );
}

/// The wording every conditional cleanup shares (`commands::IF_EMPTY_AFTER_CLEANUP`).
const IF_EMPTY: &str = "(if empty after cleanup)";

// ------------------------------------------------------------------ the defect

/// The reported bug: the file goes, the directory that held it stays.
#[test]
fn init_then_restore_leaves_no_empty_agentstack_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = project(tmp.path(), "undo-init");
    init(&home, &proj);

    let undo = run(&["x", "restore", "--last", "--write"], &home, &proj);
    assert_eq!(undo.code, 0, "the undo runs:\n{}", undo.text);

    assert!(
        !proj.join(".agentstack/agentstack.toml").exists(),
        "premise: the undo deleted the file the write created:\n{}",
        undo.text
    );
    assert!(
        !proj.join(".agentstack").exists(),
        "the undo must also take back the directory the write made to hold it — \
         an empty .agentstack/ is not a state this project was ever in:\n{}",
        undo.text
    );
    assert!(
        undo.text.contains("removed empty"),
        "and it must say so: a directory removed without a word is the same \
         surprise in the other direction:\n{}",
        undo.text
    );
    // The project itself is never a candidate: it was not created by the write.
    assert!(proj.exists(), "the project root is out of reach, always");
}

/// The same reversal through the other Undo door, which has its own preview and
/// its own write leg. `undo --to 1 --write` and `x restore --last --write` are
/// two doors onto one room, and a fix applied to one of them is a bug in the
/// other.
#[test]
fn the_undo_timeline_reverses_the_directory_too() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = project(tmp.path(), "undo-timeline");
    init(&home, &proj);

    let preview = run(&["undo", "--to", "1"], &home, &proj);
    assert_eq!(preview.code, 0, "the preview runs:\n{}", preview.text);
    assert!(
        preview.text.contains(".agentstack") && preview.text.contains(IF_EMPTY),
        "the timeline's preview must name the conditional cleanup too:\n{}",
        preview.text
    );

    let undo = run(&["undo", "--to", "1", "--write"], &home, &proj);
    assert_eq!(undo.code, 0, "the revert runs:\n{}", undo.text);
    assert!(
        !proj.join(".agentstack").exists(),
        "both Undo doors must leave the same project behind:\n{}",
        undo.text
    );
}

// -------------------------------------------------------------- the dry run

/// A preview may only promise what the write does — and must promise this,
/// because a directory disappearing unannounced is exactly the surprise the
/// preview exists to prevent. Measured against the write on the SAME state.
#[test]
fn the_dry_run_predicts_the_prune_the_write_performs() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = project(tmp.path(), "predicted");
    init(&home, &proj);

    let dry = run(&["x", "restore", "--last"], &home, &proj);
    assert_eq!(dry.code, 0, "a preview exits 0:\n{}", dry.text);
    assert!(
        dry.text.contains(".agentstack"),
        "the preview must name the directory at all:\n{}",
        dry.text
    );
    assert!(
        dry.text.contains(IF_EMPTY),
        "and qualify it in the words `x uninstall` uses for the same guard — \
         the removal is conditional, and a preview that stated it flatly would \
         be wrong whenever the directory holds something:\n{}",
        dry.text
    );
    assert!(
        proj.join(".agentstack/agentstack.toml").exists(),
        "a dry run changed nothing:\n{}",
        dry.text
    );

    // The promise is kept.
    let write = run(&["x", "restore", "--last", "--write"], &home, &proj);
    assert_eq!(write.code, 0, "the predicted write runs:\n{}", write.text);
    assert!(
        !proj.join(".agentstack").exists(),
        "the preview said the directory would come off, so it must:\n{}",
        write.text
    );
}

// -------------------------------------------------------- negative controls

/// User content makes a created directory untouchable.
///
/// `.agentstack/` here IS ours — the ledger recorded creating it — and it still
/// must survive, because by the time the undo reaches it the directory holds a
/// file no undo of ours put there.
#[test]
fn a_created_parent_holding_a_user_file_survives_the_undo() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = project(tmp.path(), "user-content");
    init(&home, &proj);
    fs::write(proj.join(".agentstack/notes.txt"), "user-owned\n").unwrap();

    let undo = run(&["x", "restore", "--last", "--write"], &home, &proj);
    assert_eq!(undo.code, 0, "the undo runs:\n{}", undo.text);

    assert!(
        !proj.join(".agentstack/agentstack.toml").exists(),
        "premise: the undo still reverses its own file:\n{}",
        undo.text
    );
    assert!(
        proj.join(".agentstack").exists(),
        "a directory holding content the undo did not put there must survive:\n{}",
        undo.text
    );
    assert_eq!(
        fs::read_to_string(proj.join(".agentstack/notes.txt")).unwrap(),
        "user-owned\n",
        "and the user's file must be exactly as they left it"
    );
    assert!(
        !undo.text.contains("removed empty"),
        "nothing was removed, so nothing may be reported as removed:\n{}",
        undo.text
    );
}

/// A directory the write did not create is not the write's to remove — even
/// when the undo leaves it completely empty.
///
/// This is the guard that keeps the cleanup out of a linked library checkout,
/// a `.git`, a pre-existing `.claude/`, and the project root: none of them was
/// ever recorded as created, so none of them is ever a candidate. There is no
/// list of exceptions to keep in sync.
#[test]
fn a_directory_the_write_did_not_create_survives_even_left_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = project(tmp.path(), "pre-existing");
    // The user made this, before AgentStack ever ran here.
    fs::create_dir_all(proj.join(".agentstack")).unwrap();
    init(&home, &proj);

    let undo = run(&["x", "restore", "--last", "--write"], &home, &proj);
    assert_eq!(undo.code, 0, "the undo runs:\n{}", undo.text);

    assert!(
        !proj.join(".agentstack/agentstack.toml").exists(),
        "premise: the file the write created is gone:\n{}",
        undo.text
    );
    assert!(
        proj.join(".agentstack").exists(),
        "the directory pre-dated the write, so reversing the write cannot take \
         it — an empty directory on disk is not evidence of who made it:\n{}",
        undo.text
    );
    assert!(
        !undo.text.contains(IF_EMPTY),
        "and the preview line must not appear either — there is no conditional \
         cleanup to announce here:\n{}",
        undo.text
    );
}

/// A write that MODIFIED an existing file created no directory, so its undo
/// prunes none. Asserted at the ledger, because that is where the distinction
/// is made: `created_dirs` is populated from what was missing at capture time,
/// and for a modification nothing is.
#[test]
fn undoing_a_modification_records_and_prunes_no_directories() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = project(tmp.path(), "modification");
    init(&home, &proj);
    let manifest = proj.join(".agentstack/agentstack.toml");

    // A second recorded write over the SAME path — now an existing file.
    let second = run(
        &["x", "delivery", "render-locally", "--write"],
        &home,
        &proj,
    );
    assert_eq!(
        second.code, 0,
        "fixture: the second write failed:\n{}",
        second.text
    );

    let entries: serde_json::Value =
        serde_json::from_str(&run(&["x", "restore", "--json"], &home, &proj).text)
            .expect("`restore --json` must be JSON");
    let entries = entries
        .pointer("/data/entries")
        .or_else(|| entries.get("entries"))
        .and_then(|v| v.as_array())
        .expect("the inventory must carry its entries");
    assert!(
        entries.len() >= 2,
        "fixture: two recorded writes were expected, got {}",
        entries.len()
    );

    // Undo them all, newest first. The manifest existed before the last write,
    // so that undo puts bytes back rather than deleting — and touches no
    // directory at all.
    let undo = run(&["undo", "--to", "1", "--write"], &home, &proj);
    assert_eq!(undo.code, 0, "the undo runs:\n{}", undo.text);
    assert!(
        manifest.exists(),
        "premise: undoing a modification restores the file:\n{}",
        undo.text
    );
    assert!(
        proj.join(".agentstack").exists(),
        "and leaves the directory holding it alone:\n{}",
        undo.text
    );
    assert!(
        !undo.text.contains("removed empty"),
        "a modification created nothing, so its undo removes nothing:\n{}",
        undo.text
    );
}
