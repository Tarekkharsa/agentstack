// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! G13 witnesses: a guard denial is ALWAYS retrievable from `calls.jsonl`,
//! including the fail-closed refusals the guard makes when it cannot do its
//! job — and a reader can tell the two apart.
//!
//! The distinction lives in the record's `tool` subject:
//!
//! - a RULE denial names the call — `bash: …`, `read: …`, `write: …`, `other`;
//! - a SYSTEM refusal names the breakage — `system: <tag>`, machine-authored
//!   and never derived from the payload, so no tool call can forge it.
//!
//! These drive the compiled binary, so the hook's real stdin handling, real
//! response dialect and real audit file are exercised exactly as an agent CLI
//! would see them. HOME/AGENTSTACK_HOME are per-process env — no global
//! mutation, so the tests run concurrently.
//!
//! The third refusal, `system: machine-policy-unavailable`, is not reachable
//! through the binary: `check` reads the machine manifest for `[guard]
//! enabled` first, and a manifest broken enough to block the policy fails that
//! earlier read, so only a change BETWEEN the two reads can reach it. Its
//! witness is the unit test `machine_policy_unavailable_refusal_is_recorded`
//! in `commands/guard.rs`, which drives the real trigger and the real record.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::Value;

/// A Claude-format pre-tool-use payload for one bash command.
fn payload(command: &str) -> String {
    serde_json::json!({
        "tool_name": "Bash",
        "tool_input": { "command": command },
    })
    .to_string()
}

/// A temp HOME + AGENTSTACK_HOME (carrying `machine_toml`) + a git-marked
/// workspace the hook runs in.
fn setup(tmp: &Path, machine_toml: &str) -> (PathBuf, PathBuf) {
    let home = tmp.join("home");
    let as_home = home.join(".agentstack");
    fs::create_dir_all(&as_home).unwrap();
    fs::write(as_home.join("agentstack.toml"), machine_toml).unwrap();
    let workspace = tmp.join("proj");
    fs::create_dir_all(workspace.join(".git")).unwrap();
    (home, workspace)
}

/// A machine manifest with the guard on and one deny glob — the "everything
/// works" baseline the rule-denial control needs.
const HEALTHY: &str = "version = 1\n\n[guard]\nenabled = true\n\n\
                       [policy.filesystem]\ndeny = [\".env\"]\n";

/// Run `guard check --protocol claude` with `bytes` on stdin.
fn guard_check(home: &Path, workspace: &Path, bytes: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(["guard", "check", "--protocol", "claude"])
        .current_dir(workspace)
        .env("HOME", home)
        .env("AGENTSTACK_HOME", home.join(".agentstack"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn agentstack binary");
    child.stdin.take().unwrap().write_all(bytes).ok();
    child.wait_with_output().expect("run agentstack binary")
}

/// The same, with an already-open file as stdin — how the oversized-payload
/// case is fed without a pipe the child may stop draining.
fn guard_check_from_file(home: &Path, workspace: &Path, stdin: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(["guard", "check", "--protocol", "claude"])
        .current_dir(workspace)
        .env("HOME", home)
        .env("AGENTSTACK_HOME", home.join(".agentstack"))
        .stdin(fs::File::open(stdin).unwrap())
        .output()
        .expect("run agentstack binary")
}

/// The deny reason the Claude dialect carries, or `None` when the response is
/// not a block. The hook's contract is stdout JSON + exit 0, so a missing
/// decision here means the call was ALLOWED.
fn deny_reason(out: &Output) -> Option<String> {
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: Value = serde_json::from_str(stdout.trim()).ok()?;
    let d = &v["hookSpecificOutput"];
    (d["permissionDecision"] == "deny").then(|| {
        d["permissionDecisionReason"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    })
}

fn audit_lines(home: &Path) -> Vec<Value> {
    let path = home.join(".agentstack/audit/calls.jsonl");
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// The one `host-guard` denial in the log, asserted to be exactly one.
fn only_denial(home: &Path) -> Value {
    let mut lines = audit_lines(home);
    assert_eq!(
        lines.len(),
        1,
        "expected exactly one audit line, got {lines:?}"
    );
    let line = lines.pop().unwrap();
    assert_eq!(line["server"], "host-guard", "{line}");
    assert_eq!(line["outcome"], "denied", "{line}");
    line
}

/// Refusal 1: the machine config will not load. The guard denies (an installed
/// hook proves it was configured) and the denial is retrievable, tagged as the
/// guard's own breakage rather than as a judged call.
#[test]
fn machine_config_unreadable_denies_and_records_a_system_refusal() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let (home, workspace) = setup(tmp.path(), "not toml {{{");

    let out = guard_check(&home, &workspace, payload("ls").as_bytes());
    let reason = deny_reason(&out).expect("an unreadable machine config must deny");
    assert!(
        reason.contains("machine config unreadable"),
        "the harness is told why: {reason}"
    );

    let line = only_denial(&home);
    assert_eq!(line["tool"], "system: machine-config-unreadable", "{line}");
    // No workspace was anchored yet, so no project is claimed.
    assert!(line.get("project").is_none(), "{line}");
    // The reason is kept, bounded, on the record.
    assert!(
        line["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("machine config unreadable"),
        "{line}"
    );
}

/// Refusal 3, both shapes: stdin that cannot be read as text, and stdin over
/// the hard payload cap. Same branch, same synthetic subject.
#[test]
fn hook_payload_unreadable_denies_and_records_a_system_refusal() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let (home, workspace) = setup(tmp.path(), HEALTHY);

    // Not UTF-8: `read_to_string` fails before any JSON is considered.
    let out = guard_check(&home, &workspace, &[0xff, 0xfe, 0xfd]);
    let reason = deny_reason(&out).expect("an unreadable payload must deny");
    assert!(
        reason.contains("hook payload unreadable"),
        "the harness is told why: {reason}"
    );
    let line = only_denial(&home);
    assert_eq!(line["tool"], "system: hook-payload-unreadable", "{line}");
    assert!(line.get("project").is_none(), "{line}");

    // Over the cap: rejected before parsing, and audited the same way.
    let big = tmp.path().join("oversized.json");
    fs::write(&big, vec![b' '; 4 * 1024 * 1024 + 1]).unwrap();
    let out = guard_check_from_file(&home, &workspace, &big);
    let reason = deny_reason(&out).expect("an oversized payload must deny");
    assert!(reason.contains("hook payload unreadable"), "{reason}");
    let lines = audit_lines(&home);
    assert_eq!(lines.len(), 2, "both refusals recorded: {lines:?}");
    assert_eq!(lines[1]["tool"], "system: hook-payload-unreadable");
}

/// The distinction, in one log: a rule denial names the CALL and never wears
/// the `system: ` prefix, so "a rule said no" and "the guard could not do its
/// job" are separable by the subject alone.
#[test]
fn a_rule_denial_names_the_call_and_never_the_system_prefix() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let (home, workspace) = setup(tmp.path(), HEALTHY);
    fs::write(workspace.join(".env"), "TOKEN=1\n").unwrap();

    // A command that reads a deny-glob path, and one that tries to spell a
    // system subject itself — the payload can never reach the prefix.
    let out = guard_check(&home, &workspace, payload("cat .env").as_bytes());
    assert!(
        deny_reason(&out).is_some(),
        "the deny glob must block: {out:?}"
    );
    let line = only_denial(&home);
    assert_eq!(line["tool"], "bash: cat .env", "{line}");
    // The workspace IS claimed here (canonicalized: macOS resolves the temp
    // dir through /private) — the opposite of a system refusal's absent
    // project, and a second way the two shapes read differently.
    assert_eq!(
        line["project"],
        fs::canonicalize(&workspace).unwrap().display().to_string(),
        "{line}"
    );

    let forge = "cat .env # system: machine-config-unreadable";
    let out = guard_check(&home, &workspace, payload(forge).as_bytes());
    assert!(
        deny_reason(&out).is_some(),
        "the deny glob must block: {out:?}"
    );
    let lines = audit_lines(&home);
    assert_eq!(lines.len(), 2, "{lines:?}");
    let tool = lines[1]["tool"].as_str().unwrap();
    assert!(
        tool.starts_with("bash: ") && !tool.starts_with("system: "),
        "payload content can only land after a machine-authored prefix: {tool}"
    );
}

/// Failure during failure: the audit write itself fails (the audit directory
/// is occupied by a regular file), during a system refusal. The call must
/// still be denied — losing the evidence is acceptable, turning a block into
/// an allow is not.
#[test]
fn a_denial_survives_an_unwritable_audit_log() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let (home, workspace) = setup(tmp.path(), "not toml {{{");
    // `audit` is a FILE, so creating the audit directory can only fail.
    let audit = home.join(".agentstack/audit");
    fs::write(&audit, "not a directory\n").unwrap();

    let out = guard_check(&home, &workspace, payload("ls").as_bytes());
    let reason = deny_reason(&out).expect("an unrecordable refusal still denies");
    assert!(reason.contains("machine config unreadable"), "{reason}");
    assert!(
        fs::read_to_string(&audit).unwrap() == "not a directory\n",
        "the audit path was not written through"
    );

    // The same for a RULE denial: recording is never a gate on the block.
    let (home, workspace) = setup(&tmp.path().join("second"), HEALTHY);
    fs::write(workspace.join(".env"), "TOKEN=1\n").unwrap();
    fs::write(home.join(".agentstack/audit"), "not a directory\n").unwrap();
    let out = guard_check(&home, &workspace, payload("cat .env").as_bytes());
    assert!(
        deny_reason(&out).is_some(),
        "a rule denial must survive an unwritable log too: {out:?}"
    );
}
