//! Witnesses for the workflows-promotion item "per-role `model` and `effort`,
//! plumbed through to adapters" — driven through the REAL binary, because the
//! claim is about the argv a child process is actually launched with.
//!
//! Two things are witnessed here, and they are the two halves of the same
//! honesty rule:
//!
//! 1. **Delivery.** A role's toolset declares `model` / `effort`; the bound
//!    adapter's descriptor says how to carry them on a headless launch; the
//!    child's own argv carries them. The assertion is on argv recorded by the
//!    fake harness itself, not on an internal function's return value — an
//!    internal seam can be right while the spawn is wrong.
//! 2. **Honest reporting.** When the bound adapter CANNOT carry a declared
//!    value — it has no notion of the dimension, or has the setting but no
//!    confirmed per-launch flag — the value is named as undeliverable in the
//!    output (role, harness, dimension, why) and the run still proceeds.
//!    Silence would be the failure mode: an author would only discover the
//!    drop from the model's behavior.
//!
//! Children are fake harnesses on PATH, so these prove the resolve → argv →
//! spawn composition, never model behavior. The rig is the one
//! `workflow_e2e.rs` established (temp `AGENTSTACK_HOME`, a fake binary on a
//! synthesized PATH, `lock` + `trust` + `workflow run` through
//! `CARGO_BIN_EXE_agentstack`).

#![cfg(unix)]
// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

use assert_fs::prelude::*;

// ────────────────────────────────── the rig ──────────────────────────────────

/// A fake harness that appends its own argv (one element per line, framed by a
/// marker) to `record`, then answers with a fixed line. Recording argv from
/// INSIDE the child is the point: it is the argv the OS actually delivered.
fn fake_harness(record: &Path) -> String {
    format!(
        "#!/bin/sh\n\
         {{ echo '--- launch ---'; for a in \"$@\"; do echo \"$a\"; done; }} >> {}\n\
         echo ok\n",
        record.display()
    )
}

fn install_bin(dir: &Path, name: &str, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Temp home + a bin dir prepended to PATH. Returns (home, bin dir, PATH).
fn fixture() -> (assert_fs::TempDir, PathBuf, std::ffi::OsString) {
    let home = assert_fs::TempDir::new().unwrap();
    let bins = home.child("fakebin");
    bins.create_dir_all().unwrap();
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

/// `lock` then `trust` — the admission prerequisites every workflow run has.
fn lock_and_trust(home: &Path, path: &std::ffi::OsString, proj: &Path) {
    let lock = agentstack(home, path, proj, &["lock", "--write"]);
    assert!(
        lock.status.success(),
        "lock failed: {}",
        String::from_utf8_lossy(&lock.stderr)
    );
    // §7.2: a non-interactive grant presents the previewed surface digest.
    let consent = agentstack::trust::digest_for(proj).unwrap();
    let trust = agentstack(
        home,
        path,
        proj,
        &["trust", ".", "--yes", "--consented-digest", &consent],
    );
    assert!(
        trust.status.success(),
        "trust failed: {}",
        String::from_utf8_lossy(&trust.stderr)
    );
}

// ───────────────────────────────── witness 1 ─────────────────────────────────

/// A role's declared model and effort reach the CHILD'S OWN ARGV, in the exact
/// shape the bound adapter's descriptor declares.
///
/// codex is the harness under test because it declares both dimensions and
/// carries them in the `-c key=value` shape — i.e. the substituted value lands
/// INSIDE an argv token, the one case whose safety rests entirely on the
/// descriptor validating the value against its own settings catalog first.
/// Asserting on the recorded argv proves the whole chain: manifest toolset →
/// `RoleBinding` → child `RunArgs` → `resolve_selection` → the options-region
/// splice → `execve`.
#[test]
fn a_role_carries_its_model_and_effort_to_the_adapter() {
    let (home, bins, path) = fixture();
    let proj = assert_fs::TempDir::new().unwrap();
    let record = home.path().join("codex-argv.txt");
    install_bin(&bins, "codex", &fake_harness(&record));

    proj.child("workflows/main.js")
        .write_str(
            "export const meta = { roles: ['builder'] };\n\
             return await agent('do the thing', { role: 'builder' });",
        )
        .unwrap();
    proj.child("agentstack.toml")
        .write_str(
            "version = 1\n\
             [profiles.builder]\n\
             harness = \"codex\"\n\
             model = \"gpt-5.5\"\n\
             effort = \"high\"\n\
             [workflows.build]\n\
             path = \"./workflows/main.js\"\n\
             roles = [\"builder\"]\n",
        )
        .unwrap();
    lock_and_trust(home.path(), &path, proj.path());

    let run = agentstack(
        home.path(),
        &path,
        proj.path(),
        &["workflow", "run", "build"],
    );
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "workflow run failed: {stderr}");

    let argv: Vec<String> = std::fs::read_to_string(&record)
        .expect("the fake harness recorded its argv")
        .lines()
        .map(str::to_string)
        .collect();

    // The descriptor's fragments, verbatim, with the declared values.
    assert!(
        argv.windows(2)
            .any(|w| w == ["-c".to_string(), "model=gpt-5.5".to_string()]),
        "the child argv must carry codex's model override: {argv:?}"
    );
    assert!(
        argv.windows(2)
            .any(|w| w == ["-c".to_string(), "model_reasoning_effort=high".to_string()]),
        "the child argv must carry codex's effort override: {argv:?}"
    );

    // Both fragments live in the OPTIONS region — before the `--` guard that
    // makes the prompt a positional. After it they would be prompt text.
    let terminator = argv
        .iter()
        .position(|a| a == "--")
        .expect("the headless argv keeps its `--` guard");
    let model_at = argv.iter().position(|a| a == "model=gpt-5.5").unwrap();
    let effort_at = argv
        .iter()
        .position(|a| a == "model_reasoning_effort=high")
        .unwrap();
    assert!(
        model_at < terminator && effort_at < terminator,
        "selection flags must precede the `--` guard: {argv:?}"
    );
    assert_eq!(
        argv.last().map(String::as_str),
        Some("do the thing"),
        "the prompt stays the one trailing positional: {argv:?}"
    );

    // And the run says what it delivered, in the same breath.
    //
    // The surface asserted here is the DRIVE LOOP's per-child line, not the
    // locked-run launch banner: a workflow child runs under
    // `LockedDelivery::WorkflowChild`, whose `quiet()` is `true`, so every
    // banner inside the locked launch is suppressed for exactly the path this
    // feature exists to serve. The drive loop is the surface that speaks per
    // child, and it names the role — which the launch banner could not, since
    // it only knows the toolset it was handed.
    for expected in [
        "role 'builder' model 'gpt-5.5' delivered to Codex CLI",
        "role 'builder' effort 'high' delivered to Codex CLI",
    ] {
        assert!(
            stderr.contains(expected),
            "the run must state the delivered selection per child ({expected}): {stderr}"
        );
    }
}

// ───────────────────────────────── witness 2 ─────────────────────────────────

/// An adapter that cannot carry a declared value SAYS SO — and the run
/// proceeds anyway.
///
/// Both undeliverable sub-cases are covered on one run, because they are
/// genuinely different facts and a surface that conflates them is not honest:
///
/// * `writer` binds claude-code, which HAS an effort setting (`effortLevel`)
///   but declares no confirmed per-launch flag for it. Its model IS delivered,
///   so this also proves the two dimensions are decided independently.
/// * `reader` binds a drop-in adapter whose descriptor declares no settings
///   catalog at all — no notion of a model to begin with.
///
/// Asserted in both places the design requires: the static
/// `workflow explain --json`, and the live stderr at child launch.
#[test]
fn an_adapter_without_model_or_effort_reports_it_honestly() {
    let (home, bins, path) = fixture();
    let proj = assert_fs::TempDir::new().unwrap();
    install_bin(
        &bins,
        "claude",
        &fake_harness(&home.path().join("claude-argv.txt")),
    );
    install_bin(
        &bins,
        "plaincli",
        &fake_harness(&home.path().join("plain-argv.txt")),
    );

    // A drop-in adapter with a headless spec and NO settings block: it can run
    // a governed child, and it has no notion of a model or an effort level.
    // (Adding a harness stays a YAML edit — nothing on the delivery path names
    // a CLI, so this file is all it takes to exercise the third case.)
    let adapters = home.child("adapters");
    adapters.create_dir_all().unwrap();
    adapters
        .child("plaincli.yaml")
        .write_str(
            "id: plaincli\n\
             display: Plain CLI\n\
             detect:\n  bin: plaincli\n\
             headless:\n  args: [\"run\", \"--\", \"{prompt}\"]\n",
        )
        .unwrap();

    proj.child("workflows/main.js")
        .write_str(
            "export const meta = { roles: ['writer', 'reader'] };\n\
             const a = await agent('draft it', { role: 'writer' });\n\
             const b = await agent('read it', { role: 'reader' });\n\
             return [a, b];",
        )
        .unwrap();
    proj.child("agentstack.toml")
        .write_str(
            "version = 1\n\
             [profiles.writer]\n\
             harness = \"claude-code\"\n\
             model = \"claude-opus-4-5\"\n\
             effort = \"high\"\n\
             [profiles.reader]\n\
             harness = \"plaincli\"\n\
             model = \"some-model\"\n\
             [workflows.write]\n\
             path = \"./workflows/main.js\"\n\
             roles = [\"writer\", \"reader\"]\n",
        )
        .unwrap();
    lock_and_trust(home.path(), &path, proj.path());

    // ---- the static surface: `workflow explain --json` ----
    let explain = agentstack(
        home.path(),
        &path,
        proj.path(),
        &["workflow", "explain", "write", "--json"],
    );
    assert!(
        explain.status.success(),
        "explain failed: {}",
        String::from_utf8_lossy(&explain.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&explain.stdout).unwrap();
    let roles = v["roles"].as_array().expect("explain reports roles");
    let row = |name: &str| {
        roles
            .iter()
            .find(|r| r["role"] == name)
            .unwrap_or_else(|| panic!("explain reports role {name}: {v:#}"))
    };
    // The pre-existing keys are untouched — this is additive.
    assert!(row("writer")["serial"].is_boolean());

    let writer = row("writer");
    assert_eq!(writer["harness"], "claude-code");
    assert_eq!(writer["model"], "claude-opus-4-5");
    assert_eq!(writer["effort"], "high");
    let writer_gaps = writer["undeliverable"].as_array().unwrap();
    assert_eq!(
        writer_gaps.len(),
        1,
        "only effort is undeliverable for claude-code: {writer:#}"
    );
    let gap = &writer_gaps[0];
    assert_eq!(gap["dimension"], "effort");
    assert_eq!(gap["harness"], "claude-code");
    let reason = gap["reason"].as_str().unwrap();
    for expected in ["writer", "effort", "high", "Claude Code", "effortLevel"] {
        assert!(
            reason.contains(expected),
            "the sentence must name {expected}: {reason}"
        );
    }

    let reader = row("reader");
    assert_eq!(reader["harness"], "plaincli");
    assert_eq!(reader["effort"], serde_json::Value::Null);
    let reader_gaps = reader["undeliverable"].as_array().unwrap();
    assert_eq!(reader_gaps.len(), 1, "{reader:#}");
    let reason = reader_gaps[0]["reason"].as_str().unwrap();
    for expected in ["reader", "model", "some-model", "Plain CLI", "no notion"] {
        assert!(
            reason.contains(expected),
            "the sentence must name {expected}: {reason}"
        );
    }

    // ---- the live surface: one stderr line per gap, and the run proceeds ----
    let run = agentstack(
        home.path(),
        &path,
        proj.path(),
        &["workflow", "run", "write"],
    );
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "the run must still proceed: {stderr}");

    assert!(
        stderr.contains("effortLevel") && stderr.contains("Claude Code"),
        "claude-code's effort gap must be named at launch: {stderr}"
    );
    assert!(
        stderr.contains("Plain CLI") && stderr.contains("no notion of model"),
        "plaincli's missing notion of a model must be named at launch: {stderr}"
    );
    // The deliverable half of the same role still went through — reported by
    // the drive loop's per-child line (see witness 1 for why that, and not the
    // suppressed locked-run banner, is the surface to assert on).
    assert!(
        stderr.contains("role 'writer' model 'claude-opus-4-5' delivered to Claude Code"),
        "claude-code's model IS deliverable and must be reported as delivered: {stderr}"
    );
    let claude_argv = std::fs::read_to_string(home.path().join("claude-argv.txt")).unwrap();
    assert!(
        claude_argv.contains("--model\nclaude-opus-4-5\n"),
        "claude-code's child argv carries the model flag: {claude_argv}"
    );
    assert!(
        !claude_argv.contains("effortLevel") && !claude_argv.contains("high"),
        "an undeliverable value must never be guessed into the argv: {claude_argv}"
    );
}
