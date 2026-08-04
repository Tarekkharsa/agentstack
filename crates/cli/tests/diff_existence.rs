// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! `diff-existence-v1`: each `diff --json` target carries `existed_before` —
//! whether the config file was on disk when the diff was computed. The t3code
//! panel needs "never rendered here / file absent" vs "the manifest moved
//! ahead of a rendered file" as data; its stopgap was parsing the unified-diff
//! hunk header (`@@ -0,0`), which an empty-but-present file misclassifies.
//! These tests pin both sides of that line.

use std::fs;
use std::sync::Mutex;

use agentstack::cli::DiffArgs;
use agentstack::commands::diff;
use agentstack::scope::Scope;

// diff reads the process-global HOME; serialize these tests.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn setup(home: &std::path::Path) {
    fs::create_dir_all(home).unwrap();
    std::env::set_var("HOME", home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));
}

fn diff_args() -> DiffArgs {
    DiffArgs {
        targets: vec![],
        profile: None,
        scope: Some(Scope::Global),
        json: false,
    }
}

fn write_manifest(proj: &std::path::Path) {
    fs::create_dir_all(proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        // `render_locally` is what puts this project's servers in the
        // rendered lane. Without it the planner routes an MCP-capable
        // harness's servers live, `diff` compares no file for it, and
        // `existed_before` — a fact ABOUT a rendered file — would have nothing
        // to describe. The field is still the thing under test; this line only
        // makes sure there is a rendered lane for it to report on.
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [delivery]\nrender_locally = true\n\
         [servers.docs]\ntype = \"http\"\nurl = \"https://docs/mcp\"\n",
    )
    .unwrap();
}

/// An absent config file is a first render, and says so: `changed` with
/// `existed_before: false`.
#[test]
fn absent_config_reports_never_rendered() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup(&home);

    let proj = tmp.path().join("proj");
    write_manifest(&proj);

    let outcome = diff::report(&diff_args(), Some(&proj)).unwrap();
    let json = serde_json::to_value(&outcome).unwrap();
    assert_eq!(json["targets"][0]["changed"], true, "in: {json}");
    assert_eq!(
        json["targets"][0]["existed_before"], false,
        "no file on disk → never rendered here, in: {json}"
    );
}

/// The heuristic-breaker: an EMPTY config file also diffs as `@@ -0,0` (no
/// prior lines), but it exists — so `existed_before` must be true, which is
/// exactly the bit the hunk header cannot carry.
#[test]
fn empty_but_present_config_still_existed_before() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup(&home);
    fs::write(home.join(".claude.json"), "").unwrap();

    let proj = tmp.path().join("proj");
    write_manifest(&proj);

    let outcome = diff::report(&diff_args(), Some(&proj)).unwrap();
    let json = serde_json::to_value(&outcome).unwrap();
    assert_eq!(json["targets"][0]["changed"], true, "in: {json}");
    assert_eq!(
        json["targets"][0]["existed_before"], true,
        "an empty file is still a file — the manifest moved ahead of it, in: {json}"
    );
}
