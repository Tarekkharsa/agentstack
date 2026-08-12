//! Witnesses for the three refusals a hands-on `workflow` walkthrough found
//! pointing the wrong way. Each one is about what the message SENDS THE READER
//! TO DO, so each is driven through the real binary — an internal seam can
//! carry the right error kind while the sentence the user reads names the wrong
//! file, the wrong line, or nothing at all.
//!
//! 1. **A syntax error is not a `meta` fault.** `workflow explain` folded every
//!    `extract_meta` refusal into "has an unusable meta block", so an ordinary
//!    unbalanced paren sent the author to inspect a `meta` block that was never
//!    wrong. The error kind already told the two apart; now the message does.
//! 2. **A parse error names the author's own line.** Both engine wrappers open
//!    with one line before the script's first byte, so every position Boa
//!    reported was one line too far down — open the file at the reported line
//!    and the mistake is above you. (The rebase itself is unit-tested in
//!    `agentstack-workflow`; this is the end-to-end witness that it reaches the
//!    terminal.)
//! 3. **`--resume` with an unknown id says what to do next.** Every other
//!    refusal on that flag — a child `r-…` id, a completed run, a diverged name
//!    — names a way forward. The missing-journal case alone emitted a raw
//!    `No such file or directory (os error 2)`.

#![cfg(unix)]
// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::process::Command;

use assert_fs::prelude::*;

fn agentstack(home: &Path, cwd: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(args)
        .current_dir(cwd)
        .env("AGENTSTACK_HOME", home)
        .env("NO_COLOR", "1")
        .output()
        .expect("agentstack binary runs")
}

/// A trusted project whose one pinned workflow has the given script body.
///
/// The script is pinned and trusted even when it does not parse: that is the
/// case under test. Trust gates on the BYTES, not on their meaning, so a script
/// with a syntax error is admitted exactly as far as the parser and no further
/// — which is where these messages are read.
fn project(home: &Path, script: &str) -> assert_fs::TempDir {
    let proj = assert_fs::TempDir::new().unwrap();
    proj.child("workflows/main.js").write_str(script).unwrap();
    proj.child("agentstack.toml")
        .write_str(
            "version = 1\n\
             [profiles.worker]\n\
             servers = []\n\
             skills = []\n\
             [workflows.demo]\n\
             path = \"./workflows/main.js\"\n\
             roles = [\"worker\"]\n",
        )
        .unwrap();
    let lock = agentstack(home, proj.path(), &["lock", "--write"]);
    assert!(
        lock.status.success(),
        "lock failed: {}",
        String::from_utf8_lossy(&lock.stderr)
    );
    let consent = agentstack::trust::digest_for(proj.path()).unwrap();
    let trust = agentstack(
        home,
        proj.path(),
        &["trust", ".", "--yes", "--consented-digest", &consent],
    );
    assert!(
        trust.status.success(),
        "trust failed: {}",
        String::from_utf8_lossy(&trust.stderr)
    );
    proj
}

/// Witnesses 1 and 2 together, because they are read in one sentence: a script
/// that does not parse must be called a parse failure, at the line the author
/// can actually open.
#[test]
fn a_syntax_error_is_reported_as_one_at_the_scripts_own_line() {
    let home = assert_fs::TempDir::new().unwrap();
    // Three lines. `meta` on line 1 is impeccable; the call on line 2 is never
    // closed, which the parser only discovers at `return` on line 3.
    let proj = project(
        home.path(),
        "export const meta = { name: 'demo', roles: ['worker'] }\n\
         const x = await agent('hi', { role: 'worker' }\n\
         return x\n",
    );

    for cmd in [
        vec!["workflow", "explain", "demo"],
        vec!["workflow", "run", "demo"],
    ] {
        let out = agentstack(home.path(), proj.path(), &cmd);
        let msg = String::from_utf8_lossy(&out.stderr).to_string();
        assert!(!out.status.success(), "{cmd:?} should refuse: {msg}");
        assert!(
            msg.contains("at line 3,"),
            "{cmd:?} must name the script's own line 3: {msg}"
        );
        assert!(
            !msg.contains("at line 4,"),
            "{cmd:?} must not leak the wrapper's line numbering: {msg}"
        );
    }

    // `explain` additionally has to stop blaming `meta`, and has to say where
    // the file is — the pinned copy is not the source it was declared from.
    let explain = agentstack(home.path(), proj.path(), &["workflow", "explain", "demo"]);
    let msg = String::from_utf8_lossy(&explain.stderr).to_string();
    assert!(
        msg.contains("does not parse"),
        "a syntax error must be called one: {msg}"
    );
    assert!(
        !msg.contains("meta block"),
        "a parse failure must not be blamed on the meta block: {msg}"
    );
    assert!(
        msg.contains("workflows/main.js"),
        "the refusal must name the file to fix: {msg}"
    );
}

/// The other half of witness 1: a script that DOES parse but whose `meta`
/// breaks the pure-literal rule still gets the meta-block message. Splitting
/// the two apart is only an improvement if the real meta faults keep landing on
/// the real meta sentence.
#[test]
fn a_genuine_meta_fault_still_names_the_meta_block() {
    let home = assert_fs::TempDir::new().unwrap();
    // Parses cleanly; `roles` is a call expression, which the pure-literal rule
    // refuses without ever evaluating it.
    let proj = project(
        home.path(),
        "const pick = () => 'worker'\n\
         const meta = { name: 'demo', roles: [pick()] }\n\
         return 1\n",
    );

    let explain = agentstack(home.path(), proj.path(), &["workflow", "explain", "demo"]);
    let msg = String::from_utf8_lossy(&explain.stderr).to_string();
    assert!(!explain.status.success(), "should refuse: {msg}");
    assert!(
        msg.contains("unusable meta block"),
        "a meta-rule violation keeps the meta message: {msg}"
    );
    assert!(
        !msg.contains("does not parse"),
        "a script that parses must not be called unparseable: {msg}"
    );
    assert!(
        msg.contains("workflows/main.js"),
        "the refusal must name the file to fix: {msg}"
    );
}

/// Witness 3: `--resume` with an id that has no evidence directory names the
/// command that lists resumable runs, instead of handing over an errno.
#[test]
fn resuming_an_unknown_run_id_names_the_way_to_find_one() {
    let home = assert_fs::TempDir::new().unwrap();
    let proj = project(
        home.path(),
        "export const meta = { name: 'demo', roles: ['worker'] }\n\
         return 1\n",
    );

    let out = agentstack(
        home.path(),
        proj.path(),
        &["workflow", "run", "demo", "--resume", "w-nosuchrun"],
    );
    let msg = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(!out.status.success(), "should refuse: {msg}");
    assert!(
        msg.contains("agentstack workflow runs"),
        "the refusal must name the surface that lists resumable runs: {msg}"
    );
    assert!(
        !msg.contains("os error"),
        "a raw errno is not a way forward: {msg}"
    );
}
