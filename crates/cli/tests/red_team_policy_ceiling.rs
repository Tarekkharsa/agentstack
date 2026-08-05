// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! RED TEAM — a repository writes itself a wider policy than the machine has.
//!
//! Invariant 2 is one sentence: effective project policy is always a subset of
//! the machine ceiling. The attack is the obvious one — a checked-in manifest
//! declares `[policy.secrets]` and `[policy.egress]` entries that name exactly
//! what the machine denied, plus a `"*"` catch-all for good measure — and the
//! test asserts the three things that make the invariant real:
//!
//! 1. the write is refused, and nothing lands on disk;
//! 2. the refusal names the **machine** layer and the file it came from, so a
//!    user cannot mistake a ceiling for a project mistake and "fix" it in the
//!    repo;
//! 3. `--allow-unresolved`, a convenience flag on the same code path, does not
//!    become a bypass. A flag that turns a policy denial into a warning would
//!    be the whole invariant, gone.
//!
//! The manifest's request is not hidden either: it must appear in the consent
//! preview as a *request*, because the human deserves to see that this repo
//! asked for more than it got.

use std::fs;
use std::path::{Path, PathBuf};

const MACHINE: &str = "version = 1\n\
[policy.secrets]\n\"*\" = [\"!RT_CEILING_TOKEN\"]\n\
[policy.egress]\n\"*\" = [\"!evil.example\"]\n";

fn run(args: &[&str], home: &Path, cwd: &Path) -> (String, bool) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("HOME", home)
        .env("AGENTSTACK_HOME", home.join(".agentstack"))
        .env("PATH", "/usr/bin:/bin")
        // The secret IS resolvable. "Denied" must never be reachable only
        // because the value happened to be missing.
        .env("RT_CEILING_TOKEN", "sk-ceiling-DEADBEEF")
        .stdin(std::process::Stdio::null())
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

/// `host` lets one test attack the egress ceiling and another the secret
/// ceiling in isolation — a single fixture that trips both would not show
/// which check did the work.
fn fixture(tmp: &Path, host: &str) -> (PathBuf, PathBuf) {
    let home = tmp.join("home");
    let proj = tmp.join("proj");
    fs::create_dir_all(home.join(".agentstack")).unwrap();
    fs::create_dir_all(&proj).unwrap();
    fs::write(home.join(".agentstack/agentstack.toml"), MACHINE).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        format!(
            "version = 1\n[delivery]\nrender_locally = true\n\
             [targets]\ndefault = [\"claude-code\"]\n\
             [policy.secrets]\n\"*\" = [\"RT_CEILING_TOKEN\", \"*\"]\n\
             [policy.egress]\n\"*\" = [\"evil.example\", \"*\"]\n\
             [servers.api]\ntype = \"http\"\nurl = \"https://{host}/mcp\"\n\
             headers = {{ Authorization = \"Bearer ${{RT_CEILING_TOKEN}}\" }}\n"
        ),
    )
    .unwrap();
    (home, proj)
}

#[test]
fn a_repo_cannot_grant_itself_a_secret_the_machine_denied() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = fixture(tmp.path(), "ok.example");

    let (text, ok) = run(&["apply", "--write", "--no-gitignore"], &home, &proj);
    assert!(!ok, "the repo widened the secret ceiling:\n{text}");
    assert!(
        text.contains("machine policy"),
        "the refusal must name the machine layer, not read as a repo mistake:\n{text}"
    );
    assert!(
        text.contains("!RT_CEILING_TOKEN"),
        "the refusal must quote the machine rule that denied it:\n{text}"
    );
    assert!(
        !proj.join(".mcp.json").exists(),
        "a policy-denied render still wrote a native config"
    );

    // The convenience flag is not a bypass.
    let (text, ok) = run(
        &["apply", "--write", "--allow-unresolved", "--no-gitignore"],
        &home,
        &proj,
    );
    assert!(!ok, "--allow-unresolved bypassed machine policy:\n{text}");
    assert!(
        !proj.join(".mcp.json").exists(),
        "--allow-unresolved wrote a config the machine ceiling denied:\n{text}"
    );
}

#[test]
fn a_repo_cannot_grant_itself_a_host_the_machine_denied() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = fixture(tmp.path(), "evil.example");

    let (text, _ok) = run(&["apply", "--write", "--no-gitignore"], &home, &proj);
    assert!(
        text.contains("evil.example") && text.contains("machine policy"),
        "the denied host must be named, and attributed to the machine layer:\n{text}"
    );
    assert!(
        !proj.join(".mcp.json").exists(),
        "a server bound for a machine-denied host was written to a native config"
    );
}

/// Narrowing still works — otherwise "only narrows" could be satisfied by
/// ignoring project policy entirely, which is a different bug wearing the
/// same green tick.
#[test]
fn a_repo_can_still_narrow_below_the_ceiling() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = fixture(tmp.path(), "ok.example");
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[delivery]\nrender_locally = true\n\
         [targets]\ndefault = [\"claude-code\"]\n\
         [policy.egress]\n\"*\" = [\"!ok.example\"]\n\
         [servers.api]\ntype = \"http\"\nurl = \"https://ok.example/mcp\"\n",
    )
    .unwrap();

    let (text, _ok) = run(&["apply", "--write", "--no-gitignore"], &home, &proj);
    assert!(
        text.contains("ok.example"),
        "a project-declared deny was ignored:\n{text}"
    );
    assert!(
        !proj.join(".mcp.json").exists(),
        "a project-denied host was written anyway:\n{text}"
    );
}

/// The request the repo made is disclosed at the gate, and the ceiling file is
/// named — the human can see both what was asked for and where the limit lives.
#[test]
fn the_wider_request_is_disclosed_at_the_consent_gate() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = fixture(tmp.path(), "evil.example");
    let (text, _ok) = run(&["trust", "--preview"], &home, &proj);
    let p: serde_json::Value = serde_json::from_str(&text).unwrap();

    let requested = serde_json::to_string(&p["policy_requested"]).unwrap();
    assert!(
        requested.contains("evil.example") && requested.contains("RT_CEILING_TOKEN"),
        "the repo's policy request was not disclosed to the human: {requested}"
    );
    assert!(
        p["machine_policy_ceiling"]
            .as_str()
            .unwrap_or_default()
            .contains("agentstack.toml"),
        "the preview must say where the ceiling lives: {p}"
    );
}
