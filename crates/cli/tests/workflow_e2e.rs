//! Stage C end-to-end witnesses driven through the REAL binary (design doc
//! §12.4): the adopted `examples/workflow-acceptance` fixture admits and runs
//! under `agentstack workflow run`, and the out-of-thread watchdog force-exits
//! a stalled run at the PROCESS level.
//!
//! Children are a fake `claude` on PATH (prompt-driven), so these tests prove
//! the admission + drive + spawn composition, not model behavior — the real-
//! harness acceptance run stays `examples/workflow-acceptance/README.md`'s
//! manual procedure with `check-evidence.sh`.

#![cfg(unix)]
// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use assert_fs::prelude::*;

/// A fake `claude` whose reply is chosen by the prompt (the last argv
/// element under the claude-code headless spec).
const FAKE_CLAUDE: &str = r#"#!/bin/sh
last=""
for a in "$@"; do last="$a"; done
case "$last" in
  sleep-forever*) sleep 300 ;;
  Reply\ CONFIRMED*) echo "CONFIRMED captures all three rules" ;;
  Combine*) echo "AgentStack fails closed: policy only narrows, untrusted content stays inert, secrets never serialize." ;;
  In\ 6\ words*) echo "Nothing runs until it is trusted" ;;
  *) echo ok ;;
esac
"#;

/// Temp home + fake-claude PATH dir; returns (home, bin_dir, PATH value).
fn fixture() -> (assert_fs::TempDir, PathBuf, std::ffi::OsString) {
    let home = assert_fs::TempDir::new().unwrap();
    let bins = home.child("fakebin");
    bins.create_dir_all().unwrap();
    let fake = bins.child("claude");
    fake.write_str(FAKE_CLAUDE).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(fake.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::join_paths(
        std::iter::once(bins.path().to_path_buf()).chain(
            std::env::var_os("PATH")
                .iter()
                .flat_map(std::env::split_paths),
        ),
    )
    .unwrap();
    let bins_path = bins.path().to_path_buf();
    (home, bins_path, path)
}

fn agentstack(
    home: &Path,
    path: &std::ffi::OsString,
    cwd: &Path,
    args: &[&str],
) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(args)
        .current_dir(cwd)
        .env("AGENTSTACK_HOME", home)
        .env("PATH", path)
        .output()
        .expect("agentstack binary runs")
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// The Stage C acceptance witness: the tracked `examples/workflow-acceptance`
/// bundle — pinned, trusted, then run end-to-end through `workflow run` —
/// returns `pass: true` with three map outputs, a reduce sentence, and a
/// well-formed verdict, leaving the project `.mcp.json` untouched (absent).
/// (The performance bookends and real-model semantics are the manual
/// procedure in the fixture's README; `budget` is unused there — Stage D.)
#[test]
fn acceptance_bundle_admits_and_runs_end_to_end() {
    let (home, _bins, path) = fixture();
    let proj = assert_fs::TempDir::new().unwrap();
    let bundle =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/workflow-acceptance/bundle");
    copy_tree(&bundle, proj.path());

    let lock = agentstack(home.path(), &path, proj.path(), &["lock"]);
    assert!(
        lock.status.success(),
        "lock failed: {}",
        String::from_utf8_lossy(&lock.stderr)
    );
    let trust = agentstack(home.path(), &path, proj.path(), &["trust", ".", "--yes"]);
    assert!(
        trust.status.success(),
        "trust failed: {}",
        String::from_utf8_lossy(&trust.stderr)
    );

    let run = agentstack(
        home.path(),
        &path,
        proj.path(),
        &["workflow", "run", "mapreduce-acceptance"],
    );
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "workflow run failed: {stderr}");

    // Stdout is the deliverable: the workflow's final value as JSON.
    let value: serde_json::Value =
        serde_json::from_slice(&run.stdout).expect("stdout is the final value as JSON");
    assert_eq!(
        value["pass"],
        serde_json::json!(true),
        "{value:#} / {stderr}"
    );
    assert_eq!(value["mapOutputs"].as_array().map(Vec::len), Some(3));
    assert!(value["verdict"]
        .as_str()
        .unwrap_or("")
        .starts_with("CONFIRMED"));

    // The rig's project MCP config stayed untouched (absent) throughout.
    assert!(!proj.path().join(".mcp.json").exists());

    // The phase()/log() progress surfaced as sanitized stderr lines.
    assert!(stderr.contains("Map"), "{stderr}");
    assert!(stderr.contains("Verify"), "{stderr}");
}

/// Witness 5, the Stage C half: a drive stalled on a long-running child is
/// force-exited by the OUT-OF-THREAD watchdog at the PROCESS level — exit
/// code 124, an honest stderr line, well before the child's own duration.
/// Stage D arms the watchdog at the EFFECTIVE ceiling plus the fixed grace
/// (1s + 30s here), so the force-exit lands at ~31s — the cooperative
/// in-band path cannot fire in this scenario (the drive thread is blocked
/// joining the batch while the child sleeps), which is precisely the stall
/// class the watchdog backstop exists for. The "outcome is recorded" clause
/// of witness 5 completes in Stage E, when the workflow gains recorder
/// events; this test states that honestly and asserts only the
/// process-level backstop.
#[test]
fn watchdog_force_exits_a_stalled_run_at_the_process_level() {
    let (home, _bins, path) = fixture();
    let proj = assert_fs::TempDir::new().unwrap();
    proj.child("workflows/main.js")
        .write_str(
            "export const meta = { roles: ['w'] };\n\
             return await agent('sleep-forever', { role: 'w' });",
        )
        .unwrap();
    proj.child("agentstack.toml")
        .write_str(
            "version = 1\n\
             [profiles.w]\n\
             [workflows.slow]\n\
             path = \"./workflows/main.js\"\n\
             roles = [\"w\"]\n\
             max_wall_seconds = 1\n",
        )
        .unwrap();
    let lock = agentstack(home.path(), &path, proj.path(), &["lock"]);
    assert!(
        lock.status.success(),
        "lock failed: {}",
        String::from_utf8_lossy(&lock.stderr)
    );
    let trust = agentstack(home.path(), &path, proj.path(), &["trust", ".", "--yes"]);
    assert!(
        trust.status.success(),
        "trust failed: {}",
        String::from_utf8_lossy(&trust.stderr)
    );

    let started = Instant::now();
    let run = agentstack(
        home.path(),
        &path,
        proj.path(),
        &["workflow", "run", "slow"],
    );
    let elapsed = started.elapsed();

    assert_eq!(
        run.status.code(),
        Some(124),
        "the watchdog exits 124 (timeout convention); stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(stderr.contains("wall-clock ceiling"), "{stderr}");
    // Far below the child's 300 s sleep: the exit came from the watchdog
    // (1 s effective ceiling + 30 s grace), not from the child finishing.
    assert!(
        elapsed.as_secs() < 60,
        "watchdog should fire at ~31s (ceiling+grace), took {elapsed:?}"
    );

    // Stage E, witness 5's recorded half: the dying watchdog appended its
    // terminal event BEFORE exit(124) — the outcome was recorded by a
    // process that then died. The workflow run id comes from the admission
    // banner (printed unstyled for exactly this parse).
    let run_id = stderr
        .split("admitted: run ")
        .nth(1)
        .and_then(|rest| rest.split(',').next())
        .expect("admission banner names the workflow run id")
        .trim()
        .to_string();
    assert!(run_id.starts_with("w-"), "{run_id}");
    let events =
        std::fs::read_to_string(home.path().join("runs").join(&run_id).join("events.jsonl"))
            .expect("workflow events.jsonl exists after the force-exit");
    assert!(
        events.contains("\"outcome\":\"watchdog_kill\""),
        "the terminal event must be recorded by the dying process: {events}"
    );
}
