// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! `agentstack yes` leaves an undoable row — and promises only what that row
//! contains.
//!
//! Found by replaying the v2 journey against the v0.18.0-rc.1 binary, not by
//! review: `yes` printed "Undo any of it with `agentstack restore --last
//! --write`" before writing, and then recorded nothing. On a skills-only
//! project the ledger stayed empty, so both `undo` and `restore --list`
//! answered "nothing recorded" and the promised undo did not exist. The
//! promise predated the ledger row.
//!
//! Three properties, matching the three ways this can go wrong again:
//!
//! a. **The row exists.** One entry, listed by both undo surfaces.
//! b. **The row is true.** Undoing reverts what it claims — and the state it
//!    leaves reads honestly: not ready, not silent, one next action.
//! c. **The promise matches the row.** The messages name what `restore` will
//!    actually put back, checked against the entry's own contents rather than
//!    against a remembered wording. This is the assertion that keeps the two
//!    from drifting apart again, which is how the bug happened in the first
//!    place.

use std::fs;
use std::path::{Path, PathBuf};

use agentstack::cli::YesArgs;

/// A project with one dropped skill that qualifies for the short path:
/// untracked inside a git work tree, i.e. the user's own work.
fn project(tmp: &Path) -> PathBuf {
    let home = tmp.join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    let proj = tmp.join("proj");
    fs::create_dir_all(proj.join(".agentstack")).unwrap();
    fs::write(proj.join(".agentstack/agentstack.toml"), "version = 1\n").unwrap();

    // A git work tree with the manifest committed; the skill below stays
    // untracked, which is what makes it "your own work".
    run_git(&proj, &["init", "-q", "."]);
    run_git(&proj, &["add", "-A"]);
    run_git(
        &proj,
        &[
            "-c",
            "user.email=t@e",
            "-c",
            "user.name=t",
            "commit",
            "-qm",
            "init",
        ],
    );

    let skill = proj.join(".agentstack/skills/summarize");
    fs::create_dir_all(&skill).unwrap();
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: summarize\ndescription: Summarize a document.\n---\n\nSummarize it.\n",
    )
    .unwrap();
    proj
}

fn run_git(dir: &Path, args: &[&str]) {
    std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git must be available");
}

/// Drive the funnel with the confirmation already answered — the seam
/// `run_answered` exists for.
fn say_yes(proj: &Path) -> anyhow::Result<()> {
    agentstack::commands::yes::run_answered(&YesArgs { yes: true }, Some(proj), true, Some(true))
}

// ─────────────────────────────────────────────────────────────── (a) the row

/// One undoable action, and both undo surfaces can see it. Before this, the
/// list was empty and the printed undo pointed at nothing.
///
/// "One action" is a claim about what `restore --last` reverses, not about how
/// many rows the ledger keeps. The funnel writes in phases — its own manifest
/// and lock declaration, and whatever `use --write` renders (the managed
/// `.gitignore` block, each CLI's own files) — and each phase records its own
/// row. They share a batch, which `restore --last` reverses whole, newest phase
/// first. Asserting a bare row count instead would pass only for a project
/// where activation happens to render nothing, and would fail the moment the
/// funnel actually delivered something.
#[test]
fn accepting_records_exactly_one_revertable_entry() {
    let _g = agentstack::util::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project(tmp.path());

    say_yes(&proj).expect("the funnel must complete");

    let entries = agentstack::history::list();
    let entry = entries.first().expect("the funnel must record a row");
    assert_eq!(entry.operation, "yes", "the row must name the command");
    assert!(
        !entry.id.is_empty(),
        "the row needs an id or `restore <id>` cannot address it"
    );
    let batch = entry.batch.as_deref().expect(
        "the yes row must belong to a batch — the screen promises `restore --last`, \
         and without a batch that reverses only the newest phase",
    );
    assert!(
        entries.iter().all(|e| e.batch.as_deref() == Some(batch)),
        "every row this one yes produced belongs to that one batch, not a \
         scattering of separately-undoable ones: {entries:#?}"
    );
}

// ────────────────────────────────────────────────────────────── (b) it is true

/// Undoing reverts what the row claims, and the state left behind reports
/// itself honestly — no false ready, no silent half-state, one next action.
#[test]
fn undoing_reverts_what_the_row_claims_and_the_result_reads_honestly() {
    let _g = agentstack::util::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project(tmp.path());

    say_yes(&proj).expect("the funnel must complete");

    let manifest = proj.join(".agentstack/agentstack.toml");
    assert!(
        fs::read_to_string(&manifest).unwrap().contains("summarize"),
        "precondition: the declaration was written"
    );

    let ids: Vec<String> = agentstack::history::list()
        .into_iter()
        .map(|e| e.id)
        .collect();
    agentstack::history::undo_recorded(&ids).expect("undo must succeed");

    // What the row claimed: the manifest declaration and the lock pin.
    assert!(
        !fs::read_to_string(&manifest).unwrap().contains("summarize"),
        "the declaration must be gone after undo"
    );
    assert!(
        !proj.join(".agentstack/agentstack.lock").exists(),
        "the lock the yes created must be gone after undo"
    );

    // And the resulting state must not lie about itself.
    let report = agentstack::commands::doctor::collect(Some(&proj)).unwrap();
    assert_ne!(
        report["readiness"].as_str(),
        Some("ready"),
        "a project mid-undo is not ready — reporting it as ready is the \
         false-ready class this journey exists to have removed: {report}"
    );
    let next = report["next_action"].as_str().unwrap_or_default();
    assert!(
        !next.is_empty(),
        "and it must still name exactly one thing to do next: {report}"
    );
}

// ─────────────────────────────────────────────── (c) the promise matches the row

/// The messages name what `restore` actually puts back — checked against the
/// ledger entry's own contents, not against a remembered sentence.
///
/// This is the property whose absence caused the bug. A copy-only assertion
/// ("does the text say X") would have passed happily while the ledger was
/// empty; this one reads the row, derives what undo covers, and requires the
/// wording to match it.
#[test]
fn the_promise_names_only_what_the_row_can_put_back() {
    let _g = agentstack::util::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project(tmp.path());

    say_yes(&proj).expect("the funnel must complete");

    let entries = agentstack::history::list();
    let entry = entries.first().expect("a row");
    // What the row can actually put back, by its own account.
    let covered: Vec<String> = entry.files.iter().map(|f| f.label.to_lowercase()).collect();
    assert!(
        covered.iter().any(|l| l.contains("manifest")),
        "the row should cover the manifest: {covered:?}"
    );
    assert!(
        covered.iter().any(|l| l.contains("lock")),
        "the row should cover the lock: {covered:?}"
    );
    // It does NOT cover the files delivered into each CLI.
    assert!(
        !covered
            .iter()
            .any(|l| l.contains("skill") && l.contains("symlink")),
        "if delivered artifacts ever join the row, the wording below must widen \
         to match — that is the point of checking against the row: {covered:?}"
    );

    let src = include_str!("../src/commands/yes.rs");
    // The wording must name the two things the row covers…
    assert!(
        src.contains("Undo the declaration and pin"),
        "the messages must name what undo puts back — the declaration and the pin"
    );
    // …and must not make the wider claim it used to make.
    assert!(
        !src.contains("Undo any of it"),
        "the old promise covered everything the funnel wrote, including delivered \
         files the ledger row does not contain. A narrow true promise beats a \
         wide false one."
    );
}

/// F10 witness (FINDINGS.md): `undo` after a `yes` must not claim "nothing
/// else touched". A `yes` row covers only the manifest declaration and the
/// lock pin — the files `use --write` delivered into each CLI are NOT in it.
/// Part one, on disk: after undo the manifest/lock are reverted (the row's
/// files) — the honesty gap is what the message claims about everything else.
/// Part two, in source: the `undo` command routes a `yes` revert to a line
/// that names the delivered files it did NOT retract and the command that
/// reconciles them, instead of the unconditional "nothing else touched".
#[test]
fn undo_after_yes_is_honest_about_the_delivered_files() {
    let _g = agentstack::util::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project(tmp.path());

    say_yes(&proj).expect("the funnel must complete");
    let entry = agentstack::history::list();
    let row = entry.first().expect("a yes row");
    assert_eq!(row.operation, "yes");
    // The row genuinely covers only manifest + lock — the premise of the
    // honesty gap. (The delivered CLI files are not among its captures.)
    let labels: Vec<String> = row.files.iter().map(|f| f.label.to_lowercase()).collect();
    assert!(labels
        .iter()
        .all(|l| l.contains("manifest") || l.contains("lock")));

    // The `undo` command's success path names the reconcile step for a `yes`
    // revert, and does not reach the bare "nothing else touched" for it.
    let src = include_str!("../src/commands/undo.rs");
    assert!(
        src.contains("run `agentstack use --write` to reconcile"),
        "undo must name how to reconcile the still-delivered files after a yes"
    );
    assert!(
        src.contains("target.operation == \"yes\""),
        "undo must special-case the yes revert whose row omits delivered files"
    );
}
