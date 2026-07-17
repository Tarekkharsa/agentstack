// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Drift-safety finding: doctor's drift section used to hint a bare
//! `agentstack apply --write` even when the pending change would DELETE
//! entries from a live config (hand-added or foreign-manifest servers), and
//! the "edited on disk" warning carried no next step at all. These tests pin
//! the fix: a pending prune names its victims and offers the keep path
//! (`adopt`) next to the prune path; a hand-edit points at `diff`/`adopt`.

use std::fs;
use std::sync::Mutex;

use agentstack::commands::doctor;
use agentstack::state::{manifest_identity, State};

// doctor mutates the process-global HOME; serialize these tests.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn setup(home: &std::path::Path) {
    fs::create_dir_all(home).unwrap();
    std::env::set_var("HOME", home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));
}

/// The msgs of one titled section from `doctor::collect`'s JSON report.
fn section_msgs(report: &serde_json::Value, title: &str) -> Vec<String> {
    report["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["title"] == title)
        .unwrap_or_else(|| panic!("no '{title}' section in {report}"))["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["msg"].as_str().unwrap().to_string())
        .collect()
}

/// A pending prune (state manages servers the manifest no longer selects)
/// must name the entries it would delete and offer `adopt` as the keep path —
/// not just the one-way `apply --write` hint.
#[test]
fn pending_prune_names_victims_and_offers_adopt() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup(&home);

    // A live Claude Code global config carrying two servers…
    let cfg = home.join(".claude.json");
    let content = r#"{
  "mcpServers": {
    "kibana_mcp": { "type": "http", "url": "https://kibana/mcp" },
    "figma": { "type": "http", "url": "https://figma/mcp" }
  }
}
"#;
    fs::write(&cfg, content).unwrap();

    // The current manifest selects neither server.
    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n",
    )
    .unwrap();

    // …that a previous apply of this same manifest recorded as managed (hash
    // matches disk, so the hand-edit branch stays quiet and only the prune
    // warning fires).
    let mut state = State::default();
    state.record(
        "claude-code",
        vec!["kibana_mcp".into(), "figma".into()],
        content,
        &manifest_identity(&proj),
    );
    state.save().unwrap();

    let report = doctor::collect(Some(&proj)).unwrap();
    let drift = section_msgs(&report, "Drift").join("\n");
    assert!(
        drift.contains("would REMOVE kibana_mcp, figma"),
        "prune victims must be named, got: {drift}"
    );
    assert!(
        drift.contains("keep them: agentstack adopt"),
        "keep path must be offered, got: {drift}"
    );
    assert!(
        drift.contains("prune them: agentstack apply --write"),
        "prune path must stay available, got: {drift}"
    );
}

/// When the managed set was recorded by a *different* manifest, a bare
/// `apply --write` no longer prunes it (cross-manifest guard) — the hint must
/// point at the explicit `--prune-foreign` escape hatch instead.
#[test]
fn foreign_pending_prune_hints_prune_foreign() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup(&home);

    let cfg = home.join(".claude.json");
    let content = "{\n  \"mcpServers\": {\n    \"kibana_mcp\": { \"type\": \"http\", \"url\": \"https://kibana/mcp\" }\n  }\n}\n";
    fs::write(&cfg, content).unwrap();

    // Recorded by manifest A…
    let proj_a = tmp.path().join("proj-a");
    fs::create_dir_all(&proj_a).unwrap();
    fs::write(proj_a.join("agentstack.toml"), "version = 1\n").unwrap();
    let mut state = State::default();
    state.record(
        "claude-code",
        vec!["kibana_mcp".into()],
        content,
        &manifest_identity(&proj_a),
    );
    state.save().unwrap();

    // …while doctor runs from manifest B, which doesn't select it.
    let proj_b = tmp.path().join("proj-b");
    fs::create_dir_all(&proj_b).unwrap();
    fs::write(
        proj_b.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n",
    )
    .unwrap();

    let report = doctor::collect(Some(&proj_b)).unwrap();
    let drift = section_msgs(&report, "Drift").join("\n");
    assert!(
        drift.contains("would REMOVE kibana_mcp"),
        "victim named, got: {drift}"
    );
    assert!(
        drift.contains("prune them: agentstack apply --prune-foreign"),
        "foreign prune must point at --prune-foreign, got: {drift}"
    );
}

/// After a guarded `apply --write` keeps a foreign server, state re-records
/// the key with only the writing manifest's set — doctor used to go silent
/// (the entry was in nobody's managed list any more). The kept-foreign
/// bookkeeping must keep the adopt-or-prune choice on the report.
#[test]
fn kept_foreign_stays_reported_after_guarded_apply() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup(&home);

    let cfg = home.join(".claude.json");
    let content = "{\n  \"mcpServers\": {\n    \"kibana_mcp\": { \"type\": \"http\", \"url\": \"https://kibana/mcp\" }\n  }\n}\n";
    fs::write(&cfg, content).unwrap();

    // Recorded by manifest A…
    let proj_a = tmp.path().join("proj-a");
    fs::create_dir_all(&proj_a).unwrap();
    fs::write(proj_a.join("agentstack.toml"), "version = 1\n").unwrap();
    let mut state = State::default();
    state.record(
        "claude-code",
        vec!["kibana_mcp".into()],
        content,
        &manifest_identity(&proj_a),
    );
    state.save().unwrap();

    // …then manifest B runs a guarded `apply --write`: kibana_mcp is kept on
    // disk, B becomes the recorded source, and the kept name moves to the
    // kept-foreign list.
    let proj_b = tmp.path().join("proj-b");
    fs::create_dir_all(&proj_b).unwrap();
    fs::write(
        proj_b.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [servers.beta]\ntype = \"http\"\nurl = \"https://beta/mcp\"\n",
    )
    .unwrap();
    agentstack::commands::apply::run(
        &agentstack::cli::ApplyArgs {
            targets: vec![],
            profile: None,
            dry_run: false,
            write: true,
            // Guarded-keep is a global-scope behavior; pin it explicitly now
            // that a repo manifest defaults to project scope.
            scope: Some(agentstack::scope::Scope::Global),
            allow_unresolved: false,
            prune_foreign: false,
            no_gitignore: true,
        },
        Some(&proj_b),
    )
    .unwrap();

    // Doctor (from B) must still report the kept entry with both paths.
    let report = doctor::collect(Some(&proj_b)).unwrap();
    let drift = section_msgs(&report, "Drift").join("\n");
    assert!(
        drift.contains("kept kibana_mcp") && drift.contains("applied by another manifest"),
        "kept-foreign entry must stay on the report, got: {drift}"
    );
    assert!(
        drift.contains("keep them: agentstack adopt")
            && drift.contains("prune them: agentstack apply --prune-foreign"),
        "adopt-or-prune choice must stay offered, got: {drift}"
    );
}

/// A config edited by hand since our last write gets a next step, not just a
/// bare observation.
#[test]
fn hand_edit_hints_diff_and_adopt() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup(&home);

    let cfg = home.join(".claude.json");
    fs::write(
        &cfg,
        "{\n  \"mcpServers\": {\n    \"kibana_mcp\": { \"type\": \"http\", \"url\": \"https://kibana/mcp\" }\n  }\n}\n",
    )
    .unwrap();
    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [servers.kibana_mcp]\ntype = \"http\"\nurl = \"https://kibana/mcp\"\n",
    )
    .unwrap();

    // Recorded hash differs from what's on disk now → the hand-edit branch.
    let mut state = State::default();
    state.record(
        "claude-code",
        vec!["kibana_mcp".into()],
        "what we wrote",
        &manifest_identity(&proj),
    );
    state.save().unwrap();

    let report = doctor::collect(Some(&proj)).unwrap();
    let drift = section_msgs(&report, "Drift").join("\n");
    assert!(
        drift.contains("edited on disk since last apply"),
        "hand-edit must be detected, got: {drift}"
    );
    assert!(
        drift.contains("review: agentstack diff") && drift.contains("agentstack adopt"),
        "hand-edit warning must carry a next step, got: {drift}"
    );
}
