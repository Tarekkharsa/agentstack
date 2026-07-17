// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Project-review finding: `doctor --ci` is documented as a trust gate, but it
//! used to downgrade *every* manifest-validation issue to a warning and only
//! exit nonzero on `report.errors > 0` — so a structural error (e.g. a profile
//! referencing an undefined server) passed `--ci`. These tests pin the fix:
//! structural issues count as errors and fail `--ci`, while a clean manifest
//! passes. Non-CI `doctor` never fails on validation regardless.

use std::fs;
use std::sync::Mutex;

use agentstack::cli::DoctorArgs;
use agentstack::commands::doctor;

// doctor mutates the process-global HOME; serialize these tests.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn setup(home: &std::path::Path) {
    fs::create_dir_all(home).unwrap();
    std::env::set_var("HOME", home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));
}

fn ci_args() -> DoctorArgs {
    DoctorArgs {
        ci: true,
        live: false,
        fix: false,
        deep: false,
        all: false,
    }
}

/// A profile whose `servers` list names a server that is never defined — this
/// is an `UnknownServerRef`, a structural (is_error) validation issue.
fn write_manifest_with_structural_error(proj: &std::path::Path) {
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [profiles.work]\nservers = [\"ghost\"]\n",
    )
    .unwrap();
}

fn write_clean_manifest(proj: &std::path::Path) {
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n",
    )
    .unwrap();
}

#[test]
fn ci_fails_on_structural_validation_error() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    setup(&tmp.path().join("home"));

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    write_manifest_with_structural_error(&proj);

    let err = doctor::run(&ci_args(), Some(&proj)).unwrap_err();
    assert!(
        err.to_string().contains("error"),
        "doctor --ci must fail the trust gate on a structural validation error, got: {err}"
    );
}

#[test]
fn ci_passes_on_clean_manifest() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    setup(&tmp.path().join("home"));

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    write_clean_manifest(&proj);

    doctor::run(&ci_args(), Some(&proj)).expect("doctor --ci must pass on a clean manifest");
}

/// Without `--ci`, a structural validation error is reported but never fails
/// the command — the gate is opt-in.
#[test]
fn non_ci_never_fails_on_validation_error() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    setup(&tmp.path().join("home"));

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    write_manifest_with_structural_error(&proj);

    let dargs = DoctorArgs {
        ci: false,
        live: false,
        fix: false,
        deep: false,
        all: false,
    };
    doctor::run(&dargs, Some(&proj)).expect("non-CI doctor must not fail on a validation error");
}

/// Trust visibility: an untrusted project is Ok-level noise — until a harness
/// actually has the bridge registered AND the manifest declares servers. Then
/// every auto-mode session silently drops to control-plane tools, so `doctor`
/// must warn and name the trust command.
#[test]
fn untrusted_warns_only_when_bridge_is_registered_and_servers_declared() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup(&home);

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [servers.x]\ntype = \"http\"\nurl = \"https://x/mcp\"\n",
    )
    .unwrap();

    let bridge_line = |report: &serde_json::Value| -> (String, String) {
        let sections = report["sections"].as_array().unwrap();
        let s = sections
            .iter()
            .find(|s| s["title"] == "Zero-files bridge")
            .expect("bridge section present");
        let line = s["lines"]
            .as_array()
            .unwrap()
            .iter()
            .find(|l| l["msg"].as_str().unwrap().contains("trusted"))
            .expect("trust line present");
        (
            line["level"].as_str().unwrap().to_string(),
            line["msg"].as_str().unwrap().to_string(),
        )
    };

    // No harness connected → untrusted stays Ok-level.
    let (level, _) = bridge_line(&doctor::collect(Some(&proj)).unwrap());
    assert_eq!(level, "ok", "no bridge registered → no warning");

    // Register the bridge in claude-code's global config (config presence is
    // detection) → the same untrusted state must now warn, naming the command.
    fs::write(
        home.join(".claude.json"),
        r#"{ "mcpServers": { "agentstack": { "type": "stdio", "command": "agentstack", "args": ["mcp", "--auto-project"] } } }"#,
    )
    .unwrap();
    let (level, msg) = bridge_line(&doctor::collect(Some(&proj)).unwrap());
    assert_eq!(level, "warn", "bridge + declared servers → warn: {msg}");
    assert!(
        msg.contains("agentstack trust"),
        "hint names the command: {msg}"
    );
}
