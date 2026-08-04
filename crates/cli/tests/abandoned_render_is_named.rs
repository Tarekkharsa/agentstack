// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]
#![cfg(unix)]

//! Invariant 8 over the delivery flip: a file AgentStack wrote does not stop
//! existing because the routing changed.
//!
//! The sequence is the reviewer's: `render-locally --write` → `apply --write`
//! (a real `.mcp.json` appears) → `render-locally --off --write` → `apply
//! --write`. From that second apply on, the harness still reads a config
//! AgentStack wrote and no longer maintains, and the state ledger still
//! records that we wrote it. Three witnesses:
//!
//! 1. `apply`, `doctor` and `status` all NAME the file — none of them reports
//!    the empty-and-clean state that only the routing plan can see.
//! 2. `doctor` does not put a green tick over it: the finding is a warning and
//!    it counts in the totals, so `all targets in sync` is not printed.
//! 3. The command those surfaces name is runnable and actually removes the
//!    file — and once it has, every surface goes quiet again.
//!
//! Witness 3 is the one that makes the other two honest: naming a leftover
//! without a way to remove it would trade one dishonesty for a dead end.

use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_agentstack");

fn run(args: &[&str], cwd: &Path, home: &Path) -> String {
    let out = Command::new(BIN)
        .args(args)
        .current_dir(cwd)
        .env("HOME", home)
        .env("AGENTSTACK_HOME", home.join(".agentstack"))
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// A project whose only target is the MCP-capable harness, so the flip moves
/// its one server from files to the live lane and nothing else changes.
fn project(root: &Path) -> PathBuf {
    let dir = root.join("proj");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("agentstack.toml"),
        r#"version = 1

[targets]
default = ["claude-code"]

[servers.demo]
type = "http"
url = "https://example.invalid/mcp"
"#,
    )
    .unwrap();
    dir
}

#[test]
fn a_config_left_behind_by_the_flip_is_named_by_every_surface_and_removable() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let proj = project(tmp.path());
    let mcp = proj.join(".mcp.json");

    // Rendered lane first: a real file, and a state ledger that records it.
    run(
        &["x", "delivery", "render-locally", "--write"],
        &proj,
        &home,
    );
    run(&["apply", "--write"], &proj, &home);
    assert!(mcp.exists(), "the rendered lane wrote .mcp.json");

    // The flip. `apply` stops writing the file; nothing removes it.
    run(
        &["x", "delivery", "render-locally", "--off", "--write"],
        &proj,
        &home,
    );
    assert!(
        mcp.exists(),
        "the flip does not delete what it stops writing"
    );

    // 1. Every surface names it, with the same removal command.
    for (label, args) in [
        ("apply", &["apply", "--write"][..]),
        ("doctor", &["doctor", "--all"][..]),
        ("status", &["status"][..]),
    ] {
        let out = run(args, &proj, &home);
        assert!(
            out.contains(".mcp.json") && out.contains("still on disk"),
            "`{label}` must name the file it left behind:\n{out}"
        );
        assert!(
            out.contains("agentstack x unrender --write"),
            "`{label}` must name the command that removes it:\n{out}"
        );
    }

    // 2. No green tick over a live unmanaged file.
    let doctor = run(&["doctor", "--all"], &proj, &home);
    assert!(
        !doctor.contains("all targets in sync"),
        "doctor claimed sync over an abandoned render:\n{doctor}"
    );
    assert!(
        doctor.contains("warning"),
        "the finding must count in doctor's totals:\n{doctor}"
    );

    // 3. The named command is runnable and does the thing.
    let removal = run(&["x", "unrender", "--write"], &proj, &home);
    assert!(
        removal.contains("removed"),
        "`x unrender --write` reports what it removed:\n{removal}"
    );
    assert!(!mcp.exists(), "the file is gone:\n{removal}");

    // And the surfaces go quiet — the warning was about a fact, not a mode.
    for (label, args) in [
        ("apply", &["apply", "--write"][..]),
        ("doctor", &["doctor", "--all"][..]),
        ("status", &["status"][..]),
    ] {
        let out = run(args, &proj, &home);
        assert!(
            !out.contains("still on disk"),
            "`{label}` still reports a file that is gone:\n{out}"
        );
    }
}

/// The good part of the routing change must survive the fix: an apply whose
/// every capability is routed live with no bridge registered still refuses,
/// names both ways forward, and exits nonzero.
#[test]
fn the_live_lane_refusal_and_its_two_ways_forward_are_unchanged() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let proj = project(tmp.path());

    let out = Command::new(BIN)
        .args(["apply", "--write"])
        .current_dir(&proj)
        .env("HOME", &home)
        .env("AGENTSTACK_HOME", home.join(".agentstack"))
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    assert!(
        !out.status.success(),
        "a delivery of nothing is a refused write:\n{text}"
    );
    assert!(
        text.contains("gateway connect --all --write"),
        "the bridge is one way forward:\n{text}"
    );
    assert!(
        text.contains("delivery render-locally --write"),
        "writing files anyway is the other:\n{text}"
    );
    // The binding copy rules: never a bare "0 files", and no instruction
    // described as going live via the gateway.
    assert!(!text.contains("0 files"), "bare `0 files`:\n{text}");
}
