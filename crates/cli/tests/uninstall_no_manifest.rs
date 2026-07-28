// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Review finding F07: "reset all AgentStack data on this machine" must be a
//! reachable action.
//!
//! `uninstall` used to load a manifest before doing anything, so the one
//! command that exists to remove AgentStack refused to start in any directory
//! without the one file it deliberately never removes. Someone who deleted a
//! project — or who simply ran it from `~` — was told to run `agentstack init`.
//!
//! Spawns the real binary, because the claim is about what the command does
//! when given no manifest, which is a decision made before any library call.

use std::fs;
use std::process::{Command, Stdio};

fn run(args: &[&str], home: &std::path::Path, cwd: &std::path::Path) -> (String, bool) {
    let out = Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("HOME", home)
        .env("AGENTSTACK_HOME", home.join(".agentstack"))
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null())
        .output()
        .expect("spawn agentstack");
    (
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
        out.status.success(),
    )
}

#[test]
fn uninstall_without_a_manifest_offers_the_machine_state_reset() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let elsewhere = tmp.path().join("elsewhere");
    let store = home.join(".agentstack");
    fs::create_dir_all(&elsewhere).unwrap();
    fs::create_dir_all(store.join("history")).unwrap();
    fs::write(store.join("state.json"), "{}").unwrap();

    // Dry run: names the machine state, changes nothing, and does not pretend
    // it can revert rendered output it has no manifest to plan against.
    let (out, ok) = run(&["uninstall"], &home, &elsewhere);
    assert!(ok, "a missing manifest is not an error here: {out}");
    assert!(
        out.contains("no rendered output to revert"),
        "says why the project half is absent: {out}"
    );
    assert!(
        out.contains("Re-run with --write"),
        "offers the action: {out}"
    );
    assert!(
        out.contains("undo ledger"),
        "warns that machine-wide undo goes with it: {out}"
    );
    assert!(store.exists(), "a dry run removes nothing");

    // `--keep-home` here asks to remove the only thing there is to remove, so
    // it says so rather than succeeding at nothing.
    let (out, ok) = run(&["uninstall", "--keep-home"], &home, &elsewhere);
    assert!(ok, "{out}");
    assert!(out.contains("Nothing to do"), "{out}");
    assert!(store.exists());

    // The write removes the machine store and nothing else.
    let bystander = elsewhere.join(".mcp.json");
    fs::write(&bystander, "{}").unwrap();
    let (out, ok) = run(&["uninstall", "--write"], &home, &elsewhere);
    assert!(ok, "{out}");
    assert!(!store.exists(), "the machine store is gone: {out}");
    assert!(
        bystander.exists(),
        "no manifest means no plan, so no project file is touched: {out}"
    );
    assert!(
        out.contains("still on PATH"),
        "the binary is named as the remaining step: {out}"
    );
}

#[test]
fn uninstall_with_neither_manifest_nor_machine_state_still_explains_itself() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let elsewhere = tmp.path().join("elsewhere");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&elsewhere).unwrap();

    // Nothing on either side. The original "no manifest here" error is still
    // the most useful answer — inventing a success would tell someone their
    // machine was cleaned when nothing was ever on it.
    let (out, ok) = run(&["uninstall"], &home, &elsewhere);
    assert!(!ok, "still an error when there is nothing at all: {out}");
    assert!(out.contains("no agentstack manifest"), "{out}");
}
