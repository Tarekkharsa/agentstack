// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! v0.17.1 — the project-scope-only fixture, and the false-ready it used to
//! produce.
//!
//! The shape: a repo whose entire agent setup lives in project-scope native
//! configs (`.mcp.json`, `.codex/config.toml`) and nothing in the user's home.
//! The activation-study pilot (docs/design/activation-study.md §8.1, Run B)
//! found that this shape dead-ended silently — `status` reported "none detected
//! on this machine", `init` wrote an empty starter manifest, and `doctor` then
//! reported `0 error(s), 0 warning(s)` over it. `adopt` could read the files the
//! whole time; no surface ever said they existed.
//!
//! Each test below is one of that finding's witnesses.

use std::fs;
use std::path::Path;

use agentstack::cli::InitArgs;
use agentstack::commands::{doctor, init};

// These tests set HOME/AGENTSTACK_HOME and the process cwd, all of which are
// process-global; serialize the binary against itself.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn init_args() -> InitArgs {
    InitArgs {
        global: false,
        force: false,
        dry_run: false,
        plan: false,
        secrets: None,
        no_keychain: true,
        yes: true,
        consented_plan: None,
    }
}

/// A repo with servers ONLY in project-scope native configs, and an isolated,
/// empty HOME so nothing from the real machine can be mistaken for discovery.
fn project_scope_only_fixture(tmp: &Path) -> std::path::PathBuf {
    let home = tmp.join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    let proj = tmp.join("proj");
    fs::create_dir_all(proj.join(".codex")).unwrap();
    fs::write(
        proj.join(".mcp.json"),
        r#"{"mcpServers":{"filesystem":{"command":"npx","args":["-y","@modelcontextprotocol/server-filesystem","./"]}}}"#,
    )
    .unwrap();
    fs::write(
        proj.join(".codex/config.toml"),
        "[mcp_servers.sqlite]\ncommand = \"uvx\"\nargs = [\"mcp-server-sqlite\"]\n",
    )
    .unwrap();
    proj
}

/// Witness (a): the orientation reading NAMES what is configured here.
/// Before: `clis_detected` was empty — a machine-scope answer to a question
/// about this directory.
#[test]
fn status_sees_project_scope_configs() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project_scope_only_fixture(tmp.path());

    let registry = agentstack::adapter::Registry::load().unwrap();
    let detected: Vec<&str> = registry
        .iter()
        .filter(|d| d.detected_in(&proj))
        .map(|d| d.id.as_str())
        .collect();
    assert!(
        detected.contains(&"claude-code") && detected.contains(&"codex"),
        "both project-scope configs are detected for this directory: {detected:?}"
    );

    // And the servers behind them are named, with the manifest-coverage
    // question answered — this is what `status` prints and what routes it to
    // `adopt` instead of a dead end.
    let native = agentstack::discover::native_configs(&registry, &proj, &Default::default(), false);
    let found: Vec<&str> = native
        .iter()
        .flat_map(|n| n.unimported.iter().map(String::as_str))
        .collect();
    assert!(
        found.contains(&"filesystem") && found.contains(&"sqlite"),
        "both servers are reported as not-yet-covered: {found:?}"
    );
}

/// Witness (b): `init` does not silently write an empty manifest over a setup
/// that is sitting right there. It imports it.
#[test]
fn init_imports_project_scope_configs_instead_of_an_empty_manifest() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project_scope_only_fixture(tmp.path());

    init::run(&init_args(), Some(&proj)).unwrap();

    let loaded = agentstack::manifest::load_from_dir(&proj.join(".agentstack")).unwrap();
    assert!(
        loaded.manifest.servers.contains_key("filesystem"),
        "the .mcp.json server was imported: {:?}",
        loaded.manifest.servers.keys().collect::<Vec<_>>()
    );
    assert!(
        loaded.manifest.servers.contains_key("sqlite"),
        "the .codex/config.toml server was imported: {:?}",
        loaded.manifest.servers.keys().collect::<Vec<_>>()
    );
}

/// Witness (c): a manifest that does not cover what is configured here is NOT
/// reported as healthy. The finding names the file, the servers, and `adopt`.
///
/// This is the Status pillar: a clean doctor has to MEAN ready.
#[test]
fn doctor_refuses_to_call_an_uncovered_setup_clean() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project_scope_only_fixture(tmp.path());
    // A manifest that knows nothing about either native config — exactly the
    // empty starter the old `init` wrote here.
    fs::create_dir_all(proj.join(".agentstack")).unwrap();
    fs::write(proj.join(".agentstack/agentstack.toml"), "version = 1\n").unwrap();

    let report = doctor::collect(Some(&proj)).unwrap();

    assert!(
        report["warnings"].as_u64().unwrap() > 0,
        "an uncovered setup is not 0 warnings: {report}"
    );
    let section = report["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["title"] == "Unmanaged setup")
        .expect("the Unmanaged setup section is reported");
    assert_eq!(section["relevant"], true);
    let text = section["lines"].to_string();
    assert!(text.contains("filesystem"), "names the server: {text}");
    assert!(text.contains("sqlite"), "names the server: {text}");
    assert!(
        text.contains("agentstack adopt"),
        "names the one next action: {text}"
    );

    // One next action, and it is the one that fixes this.
    assert_eq!(report["next_action"], "agentstack adopt");
}

/// The complement, so the check cannot become permanent noise: once the
/// manifest covers what is on disk, the section is quiet and tagged
/// irrelevant (hidden from the default report).
#[test]
fn doctor_is_quiet_once_the_manifest_covers_the_configs() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project_scope_only_fixture(tmp.path());

    init::run(&init_args(), Some(&proj)).unwrap();
    let report = doctor::collect(Some(&proj)).unwrap();

    let section = report["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["title"] == "Unmanaged setup")
        .expect("the section still exists — checks run, nothing is skipped");
    assert_eq!(
        section["relevant"], false,
        "nothing uncovered → hidden from the default report: {section}"
    );
}
