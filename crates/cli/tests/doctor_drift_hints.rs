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
use agentstack::scope::Scope;
use agentstack::state::{manifest_identity, target_key, State};

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

/// PROJECT-scope managed entries drift too: a project-scope apply records its
/// bookkeeping under `<id>@project:<root>`, and doctor used to check only the
/// global key — so removing the server from the manifest produced no warning
/// and the next `apply --scope project --write` deleted the `.mcp.json` entry
/// silently. The warning must fire for the project key, labeled with the
/// scope, and its hint must reach the project config.
#[test]
fn project_scope_pending_prune_names_victims() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup(&home);

    // A live project-scope Claude Code config carrying one server…
    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    let content = "{\n  \"mcpServers\": {\n    \"docs\": { \"type\": \"http\", \"url\": \"https://docs/mcp\" }\n  }\n}\n";
    fs::write(proj.join(".mcp.json"), content).unwrap();

    // …that the current manifest no longer selects…
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n",
    )
    .unwrap();

    // …recorded as managed by a previous `apply --scope project` of this same
    // manifest (hash matches disk, so only the prune warning fires).
    let mut state = State::default();
    state.record(
        &target_key("claude-code", Scope::Project, &proj),
        vec!["docs".into()],
        content,
        &manifest_identity(&proj),
    );
    state.save().unwrap();

    let report = doctor::collect(Some(&proj)).unwrap();
    let drift = section_msgs(&report, "Drift").join("\n");
    // The core fix: doctor checks the PROJECT-scope key (only project state was
    // recorded), so the pending prune is named rather than silently deleted on
    // the next apply. Under the default-scope model this project is a repo, so
    // project IS the default write scope — the bare `agentstack apply --write`
    // reaches the project config, and the hint carries no scope flag precisely
    // because none is needed (a stray `--scope global` here would be the bug).
    assert!(
        drift.contains("would REMOVE docs"),
        "project-scope prune victim must be named, got: {drift}"
    );
    assert!(
        drift.contains("prune them: agentstack apply --write") && !drift.contains("--scope global"),
        "the prune hint must reach the project config via the default-scope apply, got: {drift}"
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

/// A config whose MANAGED region was edited by hand since our last write gets
/// a next step, not just a bare observation. The edit must actually reach the
/// region we own — the on-disk server URL differs from what the manifest
/// renders — so the warning agrees with `agentstack diff` (see
/// `unmanaged_churn_is_not_edited_on_disk` for the converse).
#[test]
fn hand_edit_hints_diff_and_adopt() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup(&home);

    // On disk the managed server points at a hand-edited URL…
    let cfg = home.join(".claude.json");
    fs::write(
        &cfg,
        "{\n  \"mcpServers\": {\n    \"kibana_mcp\": { \"type\": \"http\", \"url\": \"https://HAND-EDITED/mcp\" }\n  }\n}\n",
    )
    .unwrap();
    // …while the manifest still renders the original URL, so the managed region
    // on disk differs from the render (plan.changed() is true).
    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [servers.kibana_mcp]\ntype = \"http\"\nurl = \"https://kibana/mcp\"\n",
    )
    .unwrap();

    // Recorded hash differs from what's on disk now → the file was touched.
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
        "hand-edit to the managed region must be detected, got: {drift}"
    );
    assert!(
        drift.contains("review: agentstack diff") && drift.contains("agentstack adopt"),
        "hand-edit warning must carry a next step, got: {drift}"
    );
}

/// Regression (P10): a config that doubles as a live state store — Claude
/// Code's ~/.claude.json is rewritten continuously by running sessions —
/// changes its UNMANAGED keys constantly. Doctor's "edited on disk" signal used
/// to compare the whole rendered file against the last-apply record, so any
/// such churn tripped it: doctor flapped forever while `agentstack diff`, which
/// compares only the managed content, said "in sync". Now doctor uses the same
/// managed-content comparison, so unmanaged churn is silent and only a change
/// to the managed region warns.
#[test]
fn unmanaged_churn_is_not_edited_on_disk() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup(&home);

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [servers.kibana_mcp]\ntype = \"http\"\nurl = \"https://kibana/mcp\"\n",
    )
    .unwrap();

    // Apply once (global scope) so the on-disk managed region and the recorded
    // hash both reflect the real render — no exact-match guessing.
    agentstack::commands::apply::run(
        &agentstack::cli::ApplyArgs {
            targets: vec![],
            profile: None,
            dry_run: false,
            write: true,
            scope: Some(Scope::Global),
            allow_unresolved: false,
            prune_foreign: false,
            no_gitignore: true,
        },
        Some(&proj),
    )
    .unwrap();

    let cfg = home.join(".claude.json");
    let applied = fs::read_to_string(&cfg).unwrap();

    // A running session rewrites an UNMANAGED top-level key; the managed
    // `mcpServers` region is untouched. This is exactly the live-state churn
    // that used to flap.
    let churned = applied.replace("\"mcpServers\"", "\"numStartups\": 99,\n  \"mcpServers\"");
    assert_ne!(churned, applied, "test setup: churn must change the file");
    fs::write(&cfg, &churned).unwrap();

    let report = doctor::collect(Some(&proj)).unwrap();
    let drift = section_msgs(&report, "Drift").join("\n");
    assert!(
        !drift.contains("edited on disk"),
        "unmanaged churn must NOT read as an on-disk edit, got: {drift}"
    );
    assert!(
        drift.contains("all targets in sync"),
        "managed region unchanged → doctor agrees with diff, got: {drift}"
    );

    // Now hand-edit the MANAGED region (the server URL): that must warn.
    let managed_edit = churned.replace("https://kibana/mcp", "https://HAND-EDITED/mcp");
    assert_ne!(
        managed_edit, churned,
        "test setup: managed edit must change the file"
    );
    fs::write(&cfg, &managed_edit).unwrap();

    let report = doctor::collect(Some(&proj)).unwrap();
    let drift = section_msgs(&report, "Drift").join("\n");
    assert!(
        drift.contains("edited on disk since last apply"),
        "a change to the managed region must warn, got: {drift}"
    );
}
