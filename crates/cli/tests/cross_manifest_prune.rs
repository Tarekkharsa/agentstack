// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Root-cause drift-safety finding: global-scope state keys are bare adapter
//! ids, so servers applied from manifest A became prune targets whenever
//! manifest B applied globally — `apply --write` from B silently deleted A's
//! servers from the live config. These tests pin the guard: a prune recorded
//! by a different manifest is kept (and reported) unless `--prune-foreign`,
//! while the recording manifest itself still prunes freely.

use std::fs;
use std::sync::Mutex;

use agentstack::cli::{ApplyArgs, DiffArgs, UseArgs};
use agentstack::commands::{apply, diff, use_profile};
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

/// The escape hatch must survive the guarded write that precedes it. A
/// guarded `apply --write` from manifest B keeps A's server on disk but
/// re-records the state key with only B's managed set (and B as source) — so
/// a follow-up `apply --prune-foreign` used to be a silent no-op ("up to
/// date") and the foreign server stayed in the live config forever,
/// untracked. The kept names must stay reachable (state bookkeeping) so the
/// advertised follow-up command actually prunes them.
#[test]
fn prune_foreign_still_works_after_guarded_apply() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup(&home);
    seed_state_from_manifest_a(tmp.path(), &home);
    let proj_b = write_manifest_b(tmp.path());

    // Step 1: guarded apply from B — A's server is kept on disk.
    apply::run(&apply_args(false), Some(&proj_b)).unwrap();
    let config = fs::read_to_string(home.join(".claude.json")).unwrap();
    assert!(
        config.contains("kibana_mcp"),
        "guarded apply keeps the foreign server, got: {config}"
    );

    // The kept-foreign name stays reachable in state even though B's record
    // overwrote the managed set.
    let state = State::load().unwrap();
    assert_eq!(
        state.kept_foreign("claude-code"),
        vec!["kibana_mcp".to_string()],
        "guarded write must track what it kept"
    );

    // Step 2: the advertised follow-up — `apply --prune-foreign` — must
    // still prune the kept entry, not report "up to date".
    apply::run(&apply_args(true), Some(&proj_b)).unwrap();
    let config = fs::read_to_string(home.join(".claude.json")).unwrap();
    assert!(
        !config.contains("kibana_mcp"),
        "--prune-foreign must prune the previously-kept server, got: {config}"
    );
    assert!(config.contains("beta"), "B's own server intact: {config}");

    // And the bookkeeping clears once they're gone.
    let state = State::load().unwrap();
    assert!(
        state.kept_foreign("claude-code").is_empty(),
        "kept-foreign list cleared after the prune"
    );
}

/// Re-running the guarded apply (no --prune-foreign) must keep reporting and
/// tracking the kept entries — the choice stays open run over run.
#[test]
fn guarded_apply_reruns_keep_tracking_kept_foreign() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup(&home);
    seed_state_from_manifest_a(tmp.path(), &home);
    let proj_b = write_manifest_b(tmp.path());

    apply::run(&apply_args(false), Some(&proj_b)).unwrap();
    // Second run: state now names B as source, so the cross-manifest guard
    // itself no longer fires — the kept list must carry the name forward.
    apply::run(&apply_args(false), Some(&proj_b)).unwrap();

    let config = fs::read_to_string(home.join(".claude.json")).unwrap();
    assert!(config.contains("kibana_mcp"), "still kept: {config}");
    let state = State::load().unwrap();
    assert_eq!(
        state.kept_foreign("claude-code"),
        vec!["kibana_mcp".to_string()],
        "kept-foreign tracking must survive re-runs"
    );
}

/// `use --prune-foreign` shares apply's guard — and must share the fix: the
/// escape hatch keeps working after a guarded `use --write` re-recorded the
/// key with only the profile's managed set.
#[test]
fn use_prune_foreign_still_works_after_guarded_use() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup(&home);
    seed_state_from_manifest_a(tmp.path(), &home);

    // Manifest B activates a profile that doesn't include A's server.
    let proj_b = tmp.path().join("proj-b");
    fs::create_dir_all(&proj_b).unwrap();
    fs::write(
        proj_b.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [servers.beta]\ntype = \"http\"\nurl = \"https://beta/mcp\"\n\
         [profiles.p]\nservers = [\"beta\"]\n",
    )
    .unwrap();
    let use_args = |prune_foreign: bool| UseArgs {
        profile: "p".into(),
        targets: vec![],
        scope: None,
        write: true,
        allow_unresolved: false,
        prune_foreign,
        no_gitignore: true,
    };

    use_profile::run(&use_args(false), Some(&proj_b)).unwrap();
    let config = fs::read_to_string(home.join(".claude.json")).unwrap();
    assert!(config.contains("kibana_mcp"), "guarded use keeps: {config}");
    assert_eq!(
        State::load().unwrap().kept_foreign("claude-code"),
        vec!["kibana_mcp".to_string()]
    );

    use_profile::run(&use_args(true), Some(&proj_b)).unwrap();
    let config = fs::read_to_string(home.join(".claude.json")).unwrap();
    assert!(
        !config.contains("kibana_mcp"),
        "use --prune-foreign must still prune the kept server, got: {config}"
    );
    assert!(config.contains("beta"), "profile server intact: {config}");
    assert!(State::load()
        .unwrap()
        .kept_foreign("claude-code")
        .is_empty());
}

fn diff_args() -> DiffArgs {
    DiffArgs {
        targets: vec![],
        profile: None,
        scope: None,
    }
}

/// `diff` must apply the same cross-manifest guard as `apply`: a foreign
/// entry is not a pending deletion (a bare `apply --write` would keep it),
/// so it must not be previewed as one — it's surfaced as kept instead.
#[test]
fn diff_does_not_preview_foreign_prunes() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup(&home);

    // Manifest A applies kibana_mcp for real (normalized formatting + state).
    let proj_a = tmp.path().join("proj-a");
    fs::create_dir_all(&proj_a).unwrap();
    fs::write(
        proj_a.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [servers.kibana_mcp]\ntype = \"http\"\nurl = \"https://kibana/mcp\"\n",
    )
    .unwrap();
    apply::run(&apply_args(false), Some(&proj_a)).unwrap();

    // Manifest B selects nothing at all — the only "change" the old diff saw
    // was deleting A's server.
    let proj_b = tmp.path().join("proj-b");
    fs::create_dir_all(&proj_b).unwrap();
    fs::write(
        proj_b.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n",
    )
    .unwrap();

    let outcome = diff::report(&diff_args(), Some(&proj_b)).unwrap();
    assert_eq!(
        outcome.drifted, 0,
        "a foreign entry is not drift — apply's guard keeps it"
    );
    assert_eq!(outcome.kept.len(), 1, "kept names surfaced");
    assert_eq!(outcome.kept[0].1, vec!["kibana_mcp".to_string()]);
}

/// After a guarded apply, the kept-foreign bookkeeping (not the managed set)
/// carries the name — diff must keep surfacing it.
#[test]
fn diff_surfaces_kept_foreign_after_guarded_apply() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup(&home);
    seed_state_from_manifest_a(tmp.path(), &home);
    let proj_b = write_manifest_b(tmp.path());

    apply::run(&apply_args(false), Some(&proj_b)).unwrap();

    let outcome = diff::report(&diff_args(), Some(&proj_b)).unwrap();
    assert_eq!(outcome.drifted, 0, "config in sync after the guarded write");
    assert_eq!(outcome.kept.len(), 1);
    assert_eq!(outcome.kept[0].1, vec!["kibana_mcp".to_string()]);
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
