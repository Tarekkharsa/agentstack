// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! `agentstack setup` is the interactive newcomer wizard. In a non-interactive
//! shell (CI, pipes — which is what `cargo test` is) it must preview and stop
//! at the confirm having written nothing to any live CLI config. With no
//! manifest, it must not run `init` in a non-interactive shell.

use std::fs;
use std::sync::Mutex;

use agentstack::cli::{InitArgs, RestoreArgs, SecretStore, SetupArgs};
use agentstack::commands::{init, restore, setup};
use agentstack::history;
use agentstack::scope::Scope;

// setup + init read/write process-global HOME; serialize with sibling tests.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn args() -> SetupArgs {
    SetupArgs {
        targets: vec!["claude-code".into()],
        profile: None,
        scope: Some(Scope::Global),
    }
}

fn set_home(home: &std::path::Path) {
    fs::create_dir_all(home).unwrap();
    std::env::set_var("HOME", home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));
}

#[test]
fn setup_previews_but_writes_nothing_without_a_terminal() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    set_home(&tmp.path().join("home"));

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [servers.demo]\ntype = \"http\"\nurl = \"https://demo/mcp\"\n",
    )
    .unwrap();

    // Non-interactive: previews, then stops at the confirm without writing.
    setup::run(&args(), Some(&proj)).unwrap();

    assert!(
        !tmp.path().join("home/.claude.json").exists(),
        "setup in a non-interactive shell must not touch live CLI config"
    );
}

#[test]
fn materialize_profile_writes_skills_and_pins_the_lock() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    set_home(&home);

    let proj = tmp.path().join("proj");
    fs::create_dir_all(proj.join("skills/helper")).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [skills.helper]\npath = \"./skills/helper\"\n\
         [profiles.p]\nskills = [\"helper\"]\n",
    )
    .unwrap();
    fs::write(proj.join("skills/helper/SKILL.md"), "# helper\n").unwrap();

    // Drive the setup phase directly: `setup::run` stops at its interactive
    // confirm in a test shell, so the phase function is the testable seam.
    let ctx = agentstack::commands::load(Some(&proj)).unwrap();
    setup::materialize_profile(&ctx, &args(), Scope::Global, Some("p")).unwrap();

    assert!(
        home.join(".claude/skills/helper").exists(),
        "the profile's skill must be materialized into the target's skills dir"
    );
    let lock = fs::read_to_string(proj.join("agentstack.lock")).unwrap();
    assert!(
        lock.contains("helper"),
        "activation must pin the skill in agentstack.lock: {lock}"
    );
}

#[test]
fn setup_does_not_run_init_without_a_terminal() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    set_home(&tmp.path().join("home"));

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();

    // No manifest yet, and no terminal: setup is interactive-only and should not
    // run `init` or otherwise write files.
    setup::run(&args(), Some(&proj)).unwrap();

    let created =
        proj.join(".agentstack/agentstack.toml").exists() || proj.join("agentstack.toml").exists();
    assert!(
        !created,
        "setup must not run init or create a manifest without a terminal"
    );
    assert!(
        !tmp.path().join("home/.claude.json").exists(),
        "setup must not write live config without a terminal"
    );
}

#[test]
fn init_records_its_manifest_and_restore_last_removes_it() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    set_home(&tmp.path().join("home"));

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    init::run(
        &InitArgs {
            global: false,
            force: false,
            dry_run: false,
            plan: false,
            secrets: Some(SecretStore::Skip),
            no_keychain: false,
            yes: false,
        },
        Some(&proj),
    )
    .unwrap();

    let manifest = proj.join(".agentstack/agentstack.toml");
    assert!(manifest.exists(), "init wrote the project manifest");
    assert_eq!(history::list().len(), 1, "init wrote one undo entry");

    restore::run(
        &RestoreArgs {
            adapter: None,
            last: true,
            scope: None,
            write: true,
        },
        Some(&proj),
    )
    .unwrap();
    assert!(
        !manifest.exists(),
        "restore --last removed the imported manifest"
    );
}

#[test]
fn init_rolls_back_when_history_cannot_be_recorded() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    // A regular file cannot contain the history/ directory, forcing the final
    // history commit to fail after the manifest write succeeds.
    let blocked_history_home = tmp.path().join("history-blocked");
    fs::write(&blocked_history_home, "not a directory").unwrap();
    std::env::set_var("AGENTSTACK_HOME", &blocked_history_home);

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    let err = init::run(
        &InitArgs {
            global: false,
            force: false,
            dry_run: false,
            plan: false,
            secrets: Some(SecretStore::Skip),
            no_keychain: false,
            yes: false,
        },
        Some(&proj),
    )
    .expect_err("history commit must fail when its home is a regular file");

    let message = err.to_string();
    assert!(
        message.contains("history") || message.contains("recording"),
        "error names the failed recovery contract: {err:#}"
    );
    assert!(
        !proj.join(".agentstack/agentstack.toml").exists(),
        "manifest write was rolled back when its undo record could not commit"
    );
}
