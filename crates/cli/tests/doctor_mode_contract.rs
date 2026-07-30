// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! `doctor-mode-v1`: `doctor --json` carries `mode` and `activation`.
//!
//! `agentstack status` has printed both for a while; `doctor --json` did not,
//! so every JSON consumer was blind to them. That matters because a panel that
//! shows an on-disk path is making a claim only `static` makes true — and
//! because "never activated" is the difference between a project that is set
//! up and one that has never written anything, which nothing in the JSON said.

use std::fs;
use std::sync::Mutex;

use agentstack::commands::doctor;

// doctor mutates the process-global HOME; serialize these tests.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn project(tmp: &std::path::Path) -> std::path::PathBuf {
    let home = tmp.join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    let proj = tmp.join("proj");
    fs::create_dir_all(proj.join(".agentstack")).unwrap();
    fs::write(
        proj.join(".agentstack/agentstack.toml"),
        "version = 1\n[servers.demo]\ntype = \"stdio\"\ncommand = \"echo\"\n",
    )
    .unwrap();
    proj
}

#[test]
fn a_never_activated_project_says_so() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project(tmp.path());

    let report = doctor::collect(Some(&proj)).unwrap();

    // No lockfile and nothing rendered: the default mode, and the fact that
    // nothing has ever been written for this project.
    assert_eq!(report["mode"], "static", "{report}");
    assert_eq!(report["activation"], "never_activated", "{report}");
}

#[test]
fn a_lockfile_flips_activation() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project(tmp.path());
    fs::write(proj.join(".agentstack/agentstack.lock"), "version = 1\n").unwrap();

    let report = doctor::collect(Some(&proj)).unwrap();
    assert_eq!(report["activation"], "locked", "{report}");
}

#[test]
fn a_discovered_project_always_reports_both() {
    // `collect(None)` does NOT mean "no project" — it discovers one from the
    // working directory, so it always has a mode to report. The genuine
    // no-project case is answered earlier, by the `needs_setup` payload in
    // `run`, which carries both keys as explicit nulls.
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project(tmp.path());

    let report = doctor::collect(Some(&proj)).unwrap();
    assert!(report["mode"].is_string(), "{report}");
    assert!(report["activation"].is_string(), "{report}");
}

#[test]
fn the_contract_name_is_advertised() {
    // Without the name, a UI cannot distinguish an older binary's absent keys
    // from this binary's legitimate nulls, so it cannot use either.
    assert!(
        agentstack::ui_contract::FEATURES.contains(&"doctor-mode-v1"),
        "doctor-mode-v1 missing from FEATURES"
    );
}
