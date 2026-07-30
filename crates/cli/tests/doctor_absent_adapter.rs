// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! A CLI whose config file is on disk but whose binary is not installed.
//!
//! This used to be a `warn`, which made it a thing the user was told to fix.
//! Nothing can be fixed: uninstalling an editor leaves its config directory
//! behind, so the line fires on a machine where nothing is wrong, and as a
//! warning it never cleared — a healthy project sat permanently at "needs
//! attention" over somebody else's leftovers.
//!
//! It is an advisory now: still stated (we would render for a tool that cannot
//! launch), but counted in its own total, never the recommended next action,
//! and it does not move `state` off ready. The severity had no test at all
//! before, which is how it stayed wrong; these pin it.

use std::fs;
use std::sync::Mutex;

use agentstack::commands::doctor;

// doctor mutates the process-global HOME and PATH; serialize these tests.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// A home with `~/.claude.json` present and an empty PATH.
///
/// Empty PATH is what makes this deterministic: `is_installed()` asks whether
/// the binary is on PATH, so a machine that genuinely has Claude Code
/// installed would otherwise report `ok` and the test would pass for the wrong
/// reason — or fail only on developer machines. With nothing on PATH, every
/// adapter is uninstalled and only the one whose config we wrote is
/// config-present.
fn setup(tmp: &std::path::Path) -> std::path::PathBuf {
    let home = tmp.join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    let empty_bin = tmp.join("empty-bin");
    fs::create_dir_all(&empty_bin).unwrap();
    std::env::set_var("PATH", &empty_bin);

    // Claude Code's config, left behind by an uninstall.
    fs::write(home.join(".claude.json"), "{}\n").unwrap();

    let proj = tmp.join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n",
    )
    .unwrap();
    proj
}

fn adapter_line<'a>(report: &'a serde_json::Value, needle: &str) -> &'a serde_json::Value {
    report["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["title"] == "Adapters & CLIs")
        .expect("Adapters & CLIs section missing")["lines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["msg"].as_str().unwrap_or_default().contains(needle))
        .unwrap_or_else(|| panic!("no adapter line containing '{needle}'"))
}

#[test]
fn config_without_its_binary_is_an_advisory_not_a_warning() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = setup(tmp.path());

    let report = doctor::collect(Some(&proj)).unwrap();
    let line = adapter_line(&report, "config present but binary not on PATH");

    assert_eq!(
        line["level"], "advisory",
        "a leftover config is a fact, not a fault: {line}"
    );
}

#[test]
fn it_does_not_count_as_a_warning_or_become_the_next_action() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = setup(tmp.path());

    let report = doctor::collect(Some(&proj)).unwrap();

    // The counters are what the panel's chip and the CI gate read. Folding an
    // advisory into `warnings` is precisely the permanent-orange problem the
    // advisory tier exists to remove.
    assert!(
        report["advisories"].as_u64().unwrap() >= 1,
        "advisory not counted: {report}"
    );

    // And it must never be the one thing the user is told to start with —
    // there is no command that fixes it.
    let next = report["next_action"].as_str().unwrap_or_default();
    assert!(
        !next.contains("PATH"),
        "an unfixable line was recommended as the next action: {next}"
    );
}
