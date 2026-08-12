// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The two in-process undo units: replaying the change LEDGER, and the exact
//! filesystem restore `session end` performs.
//!
//! They are separate mechanisms and stay separate tests — the ledger replays
//! captured bytes by id prefix, `--last` or batch, while `session end` decides
//! whether to remove a skills directory from its own `dir_preexisted` flag. But
//! they are five short tests that all call into `agentstack` in-process and all
//! mutate the same process globals (`HOME`, `AGENTSTACK_HOME`), so they share
//! one binary and one [`ENV_LOCK`].
//!
//! Directory PRUNING after a write (`FileChange::created_dirs`) is a third
//! mechanism again, and keeps its own binary in
//! `restore_prunes_what_the_write_created.rs`.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use agentstack::cli::RestoreArgs;
use agentstack::commands::restore;
use agentstack::history;
use agentstack::scope::Scope;
use agentstack::session;

/// Every test here mutates the process-global `HOME`/`AGENTSTACK_HOME`;
/// serialize them.
static ENV_LOCK: Mutex<()> = Mutex::new(());

// ══ The change ledger ═════════════════════════════════════════════════════
//
// `restore` is the one undo verb: recorded history entries (which cover every
// category apply writes — servers, settings, hooks, instructions) are
// CLI-undoable by id prefix or `--last`, not only from t3code.

fn restore_args(target: Option<&str>, last: bool, write: bool) -> RestoreArgs {
    RestoreArgs {
        adapter: target.map(str::to_string),
        last,
        list: false,
        scope: None,
        write,
        json: false,
    }
}

#[test]
fn restore_undoes_a_history_entry_by_prefix_and_last() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = assert_fs::TempDir::new().unwrap();
    std::env::set_var("HOME", home.path());
    std::env::set_var("AGENTSTACK_HOME", home.path().join(".agentstack"));

    let work = assert_fs::TempDir::new().unwrap();
    let file = work.path().join("settings.json");
    fs::write(&file, "before").unwrap();

    // Simulate what apply does: capture, overwrite, record.
    let cap = history::capture(&file, "Claude Code · settings");
    fs::write(&file, "after").unwrap();
    let id = history::record("global", "apply", vec!["Claude Code".into()], vec![cap])
        .unwrap()
        .unwrap();

    // Dry-run reverts nothing.
    restore::run(&restore_args(Some(&id[..8]), false, false), None).unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), "after");

    // Undo by unique id prefix actually reverts.
    restore::run(&restore_args(Some(&id[..8]), false, true), None).unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), "before");

    // A second event, undone via --last.
    let cap = history::capture(&file, "Claude Code · settings");
    fs::write(&file, "after-2").unwrap();
    history::record("global", "apply", vec!["Claude Code".into()], vec![cap]).unwrap();
    restore::run(&restore_args(None, true, true), None).unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), "before");

    std::env::remove_var("AGENTSTACK_HOME");
}

#[test]
fn restore_last_undoes_every_phase_in_one_batch() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = assert_fs::TempDir::new().unwrap();
    std::env::set_var("HOME", home.path());
    std::env::set_var("AGENTSTACK_HOME", home.path().join(".agentstack"));

    let work = assert_fs::TempDir::new().unwrap();
    let manifest = work.path().join("agentstack.toml");
    let rendered = work.path().join(".mcp.json");
    {
        let _batch = history::begin_batch("setup");
        let manifest_cap = history::capture(&manifest, "manifest · import");
        fs::write(&manifest, "version = 1\n").unwrap();
        history::record("project", "init", Vec::new(), vec![manifest_cap]).unwrap();

        let rendered_cap = history::capture(&rendered, "Claude Code · servers");
        fs::write(&rendered, "{}\n").unwrap();
        history::record(
            "project",
            "apply",
            vec!["Claude Code".into()],
            vec![rendered_cap],
        )
        .unwrap();
    }

    restore::run(&restore_args(None, true, true), None).unwrap();
    assert!(!manifest.exists(), "the import phase belongs to the batch");
    assert!(!rendered.exists(), "the apply phase belongs to the batch");
    assert!(
        history::list().iter().all(|entry| entry.undone),
        "every entry in the setup batch is marked undone"
    );
    std::env::remove_var("AGENTSTACK_HOME");
}

// ══ `session end`'s exact filesystem restore ══════════════════════════════
//
// `session end` must restore the filesystem exactly: it removes the skills dir
// it emptied only when the session itself created that dir — a dir the user
// pre-created (even empty) survives.

/// A project with one inline skill wired into `[profiles.p]`.
fn setup_project(tmp: &Path) -> PathBuf {
    let proj = tmp.join("proj");
    fs::create_dir_all(proj.join("skills/local-notes")).unwrap();
    fs::write(proj.join("skills/local-notes/SKILL.md"), "# local\n").unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[delivery]\nrender_locally = true\n[targets]\ndefault = [\"claude-code\"]\n\
         [skills.local-notes]\npath = \"./skills/local-notes\"\n\
         [profiles.p]\nskills = [\"local-notes\"]\n",
    )
    .unwrap();
    // `session start` is intentionally headless and fail-closed. These tests
    // exercise restore behavior, so establish its real pin + trust
    // preconditions instead of bypassing the readiness gate.
    agentstack::commands::lock::run(&Default::default(), Some(&proj)).unwrap();
    agentstack::trust::trust_unreviewed(&proj).unwrap();
    proj
}

fn set_home(home: &Path) {
    fs::create_dir_all(home).unwrap();
    std::env::set_var("HOME", home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));
}

fn unset_home() {
    std::env::remove_var("AGENTSTACK_HOME");
    std::env::remove_var("HOME");
}

#[test]
fn end_keeps_a_preexisting_empty_skills_dir() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    set_home(&tmp.path().join("home"));
    let proj = setup_project(tmp.path());

    // The user created the (empty) skills dir before any session existed.
    let skills_dir = proj.join(".claude/skills");
    fs::create_dir_all(&skills_dir).unwrap();

    session::start(Some(&proj), "p", Scope::Project).unwrap();
    assert!(
        skills_dir.join("local-notes").exists(),
        "session materialized the skill"
    );
    session::end(Some(&proj)).unwrap();

    assert!(
        !skills_dir.join("local-notes").exists(),
        "session skill reverted"
    );
    assert!(
        skills_dir.exists(),
        "a dir the user pre-created must survive session end — exact restore"
    );

    unset_home();
}

#[test]
fn end_removes_the_skills_dir_it_created() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    set_home(&tmp.path().join("home"));
    let proj = setup_project(tmp.path());

    let skills_dir = proj.join(".claude/skills");
    assert!(!skills_dir.exists(), "precondition: no skills dir at rest");

    session::start(Some(&proj), "p", Scope::Project).unwrap();
    assert!(skills_dir.join("local-notes").exists());
    session::end(Some(&proj)).unwrap();

    assert!(
        !skills_dir.exists(),
        "a dir the session created is cleaned up on end"
    );

    unset_home();
}

#[test]
fn old_session_records_default_to_preexisting() {
    // sessions.json written by an older binary has no `dir_preexisted` field —
    // it must load as true so `end` conservatively never removes the dir.
    let sa: session::SkillAdd =
        serde_json::from_str(r#"{ "dir": "/x/.claude/skills", "names": ["a"] }"#).unwrap();
    assert!(sa.dir_preexisted, "missing field defaults to pre-existing");
}
