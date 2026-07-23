// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! `restore` is the one undo verb: recorded history entries (which cover every
//! category apply writes — servers, settings, hooks, instructions) are
//! CLI-undoable by id prefix or `--last`, not only from t3code.

use std::fs;
use std::sync::Mutex;

use agentstack::cli::RestoreArgs;
use agentstack::commands::restore;
use agentstack::history;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn restore_args(target: Option<&str>, last: bool, write: bool) -> RestoreArgs {
    RestoreArgs {
        adapter: target.map(str::to_string),
        last,
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
    let id = history::record("global", vec!["Claude Code".into()], vec![cap])
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
    history::record("global", vec!["Claude Code".into()], vec![cap]).unwrap();
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
        history::record("project", Vec::new(), vec![manifest_cap]).unwrap();

        let rendered_cap = history::capture(&rendered, "Claude Code · servers");
        fs::write(&rendered, "{}\n").unwrap();
        history::record("project", vec!["Claude Code".into()], vec![rendered_cap]).unwrap();
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
