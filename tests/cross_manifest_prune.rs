//! Root-cause drift-safety finding: global-scope state keys are bare adapter
//! ids, so servers applied from manifest A became prune targets whenever
//! manifest B applied globally — `apply --write` from B silently deleted A's
//! servers from the live config. These tests pin the guard: a prune recorded
//! by a different manifest is kept (and reported) unless `--prune-foreign`,
//! while the recording manifest itself still prunes freely.

use std::fs;
use std::sync::Mutex;

use agentstack::cli::ApplyArgs;
use agentstack::commands::apply;
use agentstack::state::{manifest_identity, State};

// apply mutates the process-global HOME; serialize these tests.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn setup(home: &std::path::Path) {
    fs::create_dir_all(home).unwrap();
    std::env::set_var("HOME", home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));
}

fn apply_args(prune_foreign: bool) -> ApplyArgs {
    ApplyArgs {
        targets: vec![],
        profile: None,
        dry_run: false,
        write: true,
        scope: None,
        allow_unresolved: false,
        prune_foreign,
        no_gitignore: true,
    }
}

const CONFIG_FROM_A: &str = r#"{
  "mcpServers": {
    "kibana_mcp": { "type": "http", "url": "https://kibana/mcp" }
  }
}
"#;

/// Global config + state as manifest A left them: `kibana_mcp` applied and
/// recorded with A as the source manifest.
fn seed_state_from_manifest_a(tmp: &std::path::Path, home: &std::path::Path) {
    let proj_a = tmp.join("proj-a");
    fs::create_dir_all(&proj_a).unwrap();
    fs::write(
        proj_a.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [servers.kibana_mcp]\ntype = \"http\"\nurl = \"https://kibana/mcp\"\n",
    )
    .unwrap();
    fs::write(home.join(".claude.json"), CONFIG_FROM_A).unwrap();
    let mut state = State::default();
    state.record(
        "claude-code",
        vec!["kibana_mcp".into()],
        CONFIG_FROM_A,
        &manifest_identity(&proj_a),
    );
    state.save().unwrap();
}

/// A different manifest with its own server and no `kibana_mcp`.
fn write_manifest_b(tmp: &std::path::Path) -> std::path::PathBuf {
    let proj_b = tmp.join("proj-b");
    fs::create_dir_all(&proj_b).unwrap();
    fs::write(
        proj_b.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [servers.beta]\ntype = \"http\"\nurl = \"https://beta/mcp\"\n",
    )
    .unwrap();
    proj_b
}

#[test]
fn apply_from_another_manifest_keeps_foreign_servers() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup(&home);
    seed_state_from_manifest_a(tmp.path(), &home);
    let proj_b = write_manifest_b(tmp.path());

    apply::run(&apply_args(false), Some(&proj_b)).unwrap();

    let config = fs::read_to_string(home.join(".claude.json")).unwrap();
    assert!(
        config.contains("kibana_mcp"),
        "manifest A's server must survive manifest B's apply, got: {config}"
    );
    assert!(config.contains("beta"), "B's own server written: {config}");

    // B now owns only its own set; the foreign entry is untracked (adopt can
    // pull it into B's manifest later).
    let state = State::load().unwrap();
    assert_eq!(
        state.managed_servers("claude-code"),
        vec!["beta".to_string()]
    );
    assert_eq!(
        state.manifest_source("claude-code"),
        Some(manifest_identity(&proj_b).as_str())
    );
}

#[test]
fn prune_foreign_flag_restores_the_old_prune() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup(&home);
    seed_state_from_manifest_a(tmp.path(), &home);
    let proj_b = write_manifest_b(tmp.path());

    apply::run(&apply_args(true), Some(&proj_b)).unwrap();

    let config = fs::read_to_string(home.join(".claude.json")).unwrap();
    assert!(
        !config.contains("kibana_mcp"),
        "--prune-foreign explicitly opts into the prune, got: {config}"
    );
    assert!(config.contains("beta"), "B's own server written: {config}");
}

/// The recording manifest itself must keep pruning its own removals — the
/// guard only fires across manifests.
#[test]
fn same_manifest_still_prunes_its_own_removals() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup(&home);

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    // The manifest no longer defines kibana_mcp…
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n",
    )
    .unwrap();
    // …but the same manifest recorded it on a previous apply.
    fs::write(home.join(".claude.json"), CONFIG_FROM_A).unwrap();
    let mut state = State::default();
    state.record(
        "claude-code",
        vec!["kibana_mcp".into()],
        CONFIG_FROM_A,
        &manifest_identity(&proj),
    );
    state.save().unwrap();

    apply::run(&apply_args(false), Some(&proj)).unwrap();

    let config = fs::read_to_string(home.join(".claude.json")).unwrap();
    assert!(
        !config.contains("kibana_mcp"),
        "a server that left its own manifest still prunes, got: {config}"
    );
}
