// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! RED TEAM — a hostile manifest tries to leave its own directory.
//!
//! Invariant 7 ("all repository content is hostile input") and the contract's
//! canonical-path rule say a repository-relative integrity surface may never
//! be resolved through `..` or through a symlink: a link target can change
//! without changing any pinned byte, so a link is a hole in content binding,
//! not a convenience.
//!
//! These tests attack that rule from three directions with a real binary and
//! an isolated HOME, and they assert the refusal reaches the *gate*, not just
//! the pin step — a project that cannot be pinned must also be ungrantable.
//!
//! The third case is deliberately the uncomfortable one. An ABSOLUTE command
//! is not refused; it is treated like `npx` — a system command outside the
//! pinnable surface. That is a defensible design, but only because the human
//! sees the exact string before consenting. The test pins that justification:
//! if the absolute path ever stops appearing in the consent surface, the
//! design stops being defensible and this test fails.

use std::fs;
use std::path::{Path, PathBuf};

fn run(args: &[&str], home: &Path, cwd: &Path) -> (String, bool) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("HOME", home)
        .env("AGENTSTACK_HOME", home.join(".agentstack"))
        .env("PATH", "/usr/bin:/bin")
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

/// `tmp/home`, `tmp/proj`, and `tmp/outside` — the last one is the attacker's
/// objective: it is outside the project root and must stay untouched.
fn fixture(tmp: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let home = tmp.join("home");
    let proj = tmp.join("proj");
    let outside = tmp.join("outside");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&proj).unwrap();
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("evil.sh"), "#!/bin/sh\necho pwned\n").unwrap();
    (home, proj, outside)
}

fn manifest(proj: &Path, command: &str) {
    fs::write(
        proj.join("agentstack.toml"),
        format!("version = 1\n[servers.esc]\ntype = \"stdio\"\ncommand = \"{command}\"\n"),
    )
    .unwrap();
}

fn preview(home: &Path, proj: &Path) -> serde_json::Value {
    let (text, _ok) = run(&["trust", "--preview"], home, proj);
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("preview is not JSON ({e}):\n{text}"))
}

/// Every blocker reason the preview reports, joined — the machine-readable
/// evidence that the refusal is a property of the gate and not of one command.
fn blocker_text(preview: &serde_json::Value) -> String {
    preview["blockers"]
        .as_array()
        .expect("blockers is an array")
        .iter()
        .map(|b| b["reason"].as_str().unwrap_or_default().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The attack: `../` in a pinned executable path.
#[test]
fn a_traversal_command_cannot_be_pinned_and_cannot_be_trusted() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj, outside) = fixture(tmp.path());
    manifest(&proj, "../outside/evil.sh");

    let (text, ok) = run(&["lock", "--write"], &home, &proj);
    assert!(!ok, "traversal was pinned instead of refused:\n{text}");
    assert!(
        text.contains("traversal"),
        "the refusal must name the traversal:\n{text}"
    );
    assert!(
        !proj.join("agentstack.lock").exists(),
        "a refused pin must not leave a lockfile behind"
    );

    // The gate, not just the pin step: nothing about this project is grantable.
    let p = preview(&home, &proj);
    assert_eq!(
        p["grantable"], false,
        "traversal project was grantable: {p}"
    );
    assert!(
        blocker_text(&p).contains("traversal"),
        "the gate must name the traversal in a machine-readable blocker: {p}"
    );

    // …and the non-interactive grant path refuses too, so a driver that skips
    // the preview cannot consent its way past the rule.
    let digest = p["surface_digest"].as_str().unwrap().to_string();
    let (text, ok) = run(&["trust", "--yes", "--consented", &digest], &home, &proj);
    assert!(!ok, "an ungrantable project was granted:\n{text}");

    // Nothing was created outside the project root.
    let leaked: Vec<_> = fs::read_dir(&outside)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(leaked.len(), 1, "something was written outside: {leaked:?}");
}

/// The subtler attack: a path that *looks* contained. `./link.sh` never leaves
/// the project lexically — only the filesystem knows it points outside, and
/// only a `symlink_metadata` check on every component sees it.
#[test]
fn a_symlink_inside_the_project_cannot_be_pinned_or_trusted() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj, outside) = fixture(tmp.path());
    std::os::unix::fs::symlink(outside.join("evil.sh"), proj.join("link.sh")).unwrap();
    manifest(&proj, "./link.sh");

    let (text, ok) = run(&["lock", "--write"], &home, &proj);
    assert!(!ok, "a symlink was pinned instead of refused:\n{text}");
    assert!(
        text.contains("symlink"),
        "the refusal must name the symlink:\n{text}"
    );

    let p = preview(&home, &proj);
    assert_eq!(p["grantable"], false, "symlink project was grantable: {p}");
    assert!(
        blocker_text(&p).contains("symlink"),
        "the gate must name the symlink in a machine-readable blocker: {p}"
    );
}

/// The honest limit of the rule, pinned so it stays honest.
///
/// An absolute command is NOT refused — it is out of the pinnable surface, the
/// same as `npx`. The only thing standing between it and execution is that the
/// human reads it at the gate. So the consent surface must show the literal
/// string, and the lockfile must not claim a pin it does not have.
#[test]
fn an_absolute_command_is_unpinned_but_never_hidden_from_consent() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj, outside) = fixture(tmp.path());
    let abs = outside.join("evil.sh");
    let abs = abs.to_str().unwrap().to_string();
    manifest(&proj, &abs);

    let (text, ok) = run(&["lock", "--write"], &home, &proj);
    assert!(
        ok,
        "absolute commands are out of scope, not errors:\n{text}"
    );
    let lock = fs::read_to_string(proj.join("agentstack.lock")).unwrap();
    assert!(
        !lock.contains(&abs),
        "an unpinnable absolute command must not be recorded as a pinned \
         executable — that would claim content binding it does not have:\n{lock}"
    );

    let p = preview(&home, &proj);
    let shown = serde_json::to_string(&p).unwrap();
    assert!(
        shown.contains(&abs),
        "the absolute command is the ONLY thing the human can judge, and it is \
         missing from the consent surface: {p}"
    );
    let runs = p["review"]["items"]
        .as_array()
        .expect("review items")
        .iter()
        .filter(|i| i["kind"] == "server")
        .flat_map(|i| i["runs"].as_array().cloned().unwrap_or_default())
        .filter_map(|r| r.as_str().map(str::to_string))
        .collect::<Vec<_>>();
    assert!(
        runs.iter().any(|r| r == &abs),
        "the reviewed server must declare what it runs, verbatim: {runs:?}"
    );
}
