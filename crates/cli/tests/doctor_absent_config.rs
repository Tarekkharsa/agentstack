// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! **A CLI that is installed but has never been configured.**
//!
//! The mirror of `doctor_absent_adapter.rs`: there the config outlived its
//! binary, here the binary has arrived and no config exists yet. It is the
//! ordinary state of a freshly installed machine, and `doctor` described it
//! with a claim about a file that is not there:
//!
//! ```text
//!   ✓ Claude Code    installed · ~/.claude.json parses
//! ```
//!
//! `read_config_value` is a READ, not an assertion of existence — a missing or
//! empty file is `Ok(None)`, and the branch matched `Ok(_)`. So every detected
//! CLI on a fresh machine got a tick for parsing a file that has never
//! existed, in the terminal report AND in `sections[].lines[].msg`, which is
//! what a panel renders. It also contradicted `init` in the same run, which
//! reports the same tools honestly as "binary on PATH — no config files
//! found": one binary, two surfaces, opposite claims about one file.
//!
//! Three states now have three sentences — a file that parsed, no file yet,
//! and an adapter with no config concept at all (Pi) — and none of them is a
//! fault, so the level is unchanged in every case.

use std::fs;
use std::sync::Mutex;

use agentstack::commands::doctor;

// doctor mutates the process-global HOME and PATH; serialize these tests.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// A home with NO CLI config files at all, and a stub `claude` on PATH so
/// `is_installed()` is true for exactly one adapter.
///
/// The stub is what makes this deterministic. `is_installed()` asks whether
/// the binary is on PATH, so inheriting the developer's PATH would make the
/// set of detected adapters differ between machines and CI.
fn setup(tmp: &std::path::Path) -> std::path::PathBuf {
    let home = tmp.join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    let stub_bin = tmp.join("bin");
    fs::create_dir_all(&stub_bin).unwrap();
    let claude = stub_bin.join("claude");
    fs::write(&claude, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&claude, fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::env::set_var("PATH", &stub_bin);

    let proj = tmp.join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n",
    )
    .unwrap();
    proj
}

fn adapter_lines(report: &serde_json::Value) -> Vec<String> {
    report["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["title"] == "Adapters & CLIs")
        .expect("Adapters & CLIs section missing")["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["msg"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The defect, stated directly: no surface may say a file parses when no such
/// file exists.
#[test]
fn a_config_that_does_not_exist_is_never_reported_as_parsing() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = setup(tmp.path());

    // The premise the assertion rests on: there really is no config here.
    assert!(
        !tmp.path().join("home/.claude.json").exists(),
        "fixture wrote a config; the test would pass for the wrong reason"
    );

    let report = doctor::collect(Some(&proj)).unwrap();
    let lines = adapter_lines(&report);
    let claude = lines
        .iter()
        .find(|l| l.contains("Claude Code"))
        .unwrap_or_else(|| panic!("no Claude Code adapter line in {lines:?}"));

    assert!(
        !claude.contains("parses"),
        "claimed a nonexistent file parses: {claude}"
    );
    assert!(
        claude.contains("no config yet"),
        "the honest state is not stated: {claude}"
    );
    // Still installed, and still not a fault — an unconfigured CLI is the
    // ordinary first-run state, not something to repair.
    assert!(claude.contains("installed"), "{claude}");
    assert_eq!(report["errors"], 0, "{report}");
}

/// And the positive case, so the fix cannot be "never say parses again": a
/// config that IS on disk and IS valid keeps its claim.
#[test]
fn a_config_that_exists_still_reports_that_it_parses() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = setup(tmp.path());
    fs::write(tmp.path().join("home/.claude.json"), r#"{"mcpServers":{}}"#).unwrap();

    let report = doctor::collect(Some(&proj)).unwrap();
    let lines = adapter_lines(&report);
    let claude = lines
        .iter()
        .find(|l| l.contains("Claude Code"))
        .unwrap_or_else(|| panic!("no Claude Code adapter line in {lines:?}"));

    assert!(
        claude.contains("parses"),
        "a real, valid config lost its reading: {claude}"
    );
    assert!(!claude.contains("no config yet"), "{claude}");
}
