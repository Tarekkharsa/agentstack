// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Strategy v2 Phase 3, item 5 — every kind presents the same shape.
//!
//! A person asking "what is this?" asks the same four questions whatever they
//! are pointing at: what is it called, where did it come from, is it the exact
//! content I approved, and what does it want to do. `explain` answered for
//! three kinds and, for the other three, said "no server, skill, or
//! instruction" — which reads as *it does not exist*, when in fact the thing
//! is declared, is being delivered, and was on the consent card the user
//! already said yes to.
//!
//! That disagreement is the bug this file pins: the review card is the
//! authority on what a project declares, and every other surface has to agree
//! with it.
//!
//! Scope, stated so the gaps below are read as decisions and not oversights:
//! workflows are deliberately excluded (Phase 3 leaves them out), and hooks
//! and extensions share the *presentation* shape while keeping the full
//! consent ceremony — sharing a layout is not sharing a gate.

use std::fs;
use std::path::Path;
use std::sync::Mutex;

use agentstack::commands::explain;

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// A project declaring one of every kind in scope.
fn project(tmp: &Path) -> std::path::PathBuf {
    let home = tmp.join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    let proj = tmp.join("proj");
    fs::create_dir_all(proj.join(".agentstack")).unwrap();
    fs::write(proj.join(".agentstack/FRAGMENT.md"), "be careful\n").unwrap();
    fs::write(proj.join(".agentstack/ext.ts"), "export default {}\n").unwrap();
    fs::write(
        proj.join(".agentstack/agentstack.toml"),
        r#"version = 1

[servers.demo]
type = "stdio"
command = "/bin/echo"

[skills.helper]
path = ".agentstack/skills/helper"

[instructions.house]
path = ".agentstack/FRAGMENT.md"

[settings.claude-code]
someKey = true

[hooks.watchdog]
event = "PreToolUse"
command = "/bin/true"

[extensions.addon]
path = ".agentstack/ext.ts"
target = "pi"
"#,
    )
    .unwrap();
    proj
}

/// Every kind in scope answers, and answers in the same shape.
///
/// The four rows are the contract. A kind for which a row is genuinely empty
/// must still print the row and say so — "there is nothing to pin here" is an
/// answer, and a reader comparing two kinds needs it. Omitting the row instead
/// is how the surfaces drifted apart in the first place.
#[test]
fn every_kind_in_scope_answers_in_the_same_shape() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project(tmp.path());

    for name in ["house", "claude-code", "watchdog", "addon"] {
        let text = explain::explain_text(name, Some(&proj))
            .unwrap_or_else(|e| panic!("`explain {name}` must answer, got: {e:#}"));

        assert!(
            text.starts_with(&format!("# {name} (")),
            "[{name}] must lead with its name and its kind:\n{text}"
        );
        for row in ["Source:", "What it asks:"] {
            assert!(
                text.contains(row),
                "[{name}] is missing the '{row}' row — every kind answers the \
                 same questions:\n{text}"
            );
        }
        // Pin status: present for every kind, including the kinds whose honest
        // answer is "nothing is pinned".
        assert!(
            text.contains("Pin:") || text.to_lowercase().contains("pin"),
            "[{name}] says nothing about whether this is the content that was \
             approved:\n{text}"
        );
    }
}

/// **Every kind answers at all.** This is the half of convergence that was a
/// correctness bug rather than a layout one: three kinds used to be told they
/// did not exist.
#[test]
fn no_declared_kind_is_told_it_does_not_exist() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project(tmp.path());

    for name in ["demo", "house", "claude-code", "watchdog", "addon"] {
        let text = explain::explain_text(name, Some(&proj))
            .unwrap_or_else(|e| panic!("`explain {name}` must answer, got: {e:#}"));
        assert!(
            text.contains(name),
            "[{name}] must be named in its own explanation:\n{text}"
        );
    }
}

/// **The remaining half, recorded rather than claimed.**
///
/// `explain_server` and `explain_skill` predate the shared shape and carry
/// their own richer layout — they are the trust lens the consent card composes
/// from, which is exactly why converging their rows is its own change and not
/// a late edit at the end of a long one. This test states the gap so it is a
/// known fact with a name, not something a reader has to discover by noticing
/// the other test only covers four kinds.
///
/// When the server and skill lenses gain `Source:` / `Pin:` / `What it asks:`
/// rows, this test fails — and that failure is the signal to fold them into
/// `every_kind_in_scope_answers_in_the_same_shape` and delete this.
#[test]
fn servers_and_skills_are_not_yet_on_the_shared_row_shape() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project(tmp.path());

    let server = explain::explain_text("demo", Some(&proj)).unwrap();
    assert!(
        !server.contains("What it asks:"),
        "the server lens has been converged — move `demo` into the shared-shape \
         test and delete this one:\n{server}"
    );
}

/// The executable kinds keep their ceremony. Sharing a layout with an
/// instruction fragment must never make a hook *read* like one — the whole
/// point of the shared shape is that the "what it asks" row is where they
/// differ, visibly.
#[test]
fn executable_kinds_still_say_they_run_code_at_full_permission() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project(tmp.path());

    for name in ["watchdog", "addon"] {
        let text = explain::explain_text(name, Some(&proj)).unwrap();
        let lower = text.to_lowercase();
        assert!(
            lower.contains("full user permission") || lower.contains("full permission"),
            "[{name}] is an executable kind and must say so plainly:\n{text}"
        );
    }

    // And the hook's honest pin gap — a local script's bytes are not
    // digested — is stated where a reader will actually meet it, not only in
    // the enforcement matrix.
    let hook = explain::explain_text("watchdog", Some(&proj)).unwrap();
    assert!(
        hook.contains("NOT pinned"),
        "the hook script pin gap must be stated on the hook itself:\n{hook}"
    );
}

/// Presentation convergence, not a schema change. If this ever fails, the
/// change under review has crossed from "how a kind is shown" into "what a
/// kind is" — which Phase 3 explicitly said to stop and report on instead.
#[test]
fn convergence_changed_no_manifest_schema() {
    let model = include_str!("../../core/src/manifest/model.rs");
    for kind in [
        "pub servers:",
        "pub skills:",
        "pub instructions:",
        "pub settings:",
        "pub hooks:",
        "pub extensions:",
        "pub workflows:",
    ] {
        assert!(
            model.contains(kind),
            "the seven-kind manifest shape must be untouched by a presentation pass: {kind}"
        );
    }
}

/// `status` counts every declared kind, not just the two it grew up with.
/// A project whose setup is mostly instruction fragments and hooks used to
/// report "0 servers" — a true number and a false impression, on the surface
/// whose entire job is saying what you have.
#[test]
fn status_counts_every_declared_kind() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project(tmp.path());

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .arg("status")
        .current_dir(&proj)
        .env("HOME", tmp.path().join("home"))
        .env("AGENTSTACK_HOME", tmp.path().join("home/.agentstack"))
        .stdin(std::process::Stdio::null())
        .output()
        .expect("spawn agentstack");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    for noun in ["instruction", "hook", "extension", "settings"] {
        assert!(
            text.contains(noun),
            "status must count declared {noun}s — it declares one:\n{text}"
        );
    }
}
