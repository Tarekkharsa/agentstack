//! Phase 1 content-pinning: the referenced-file re-gate gap, end to end.
//!
//! The trust digest hashes manifest + local overlay + lockfile bytes — NOT
//! skill bodies. So editing `./skills/x/SKILL.md` after trusting leaves the
//! project "Trusted" while its content no longer matches what was reviewed.
//! These tests prove the gap is closed at the use site: activation verifies
//! resolved content against the lock pin and fails closed on drift, and the
//! explicit `agentstack lock` acceptance is what re-gates trust (lock bytes
//! change → trust digest flips → auto mode drops to control-plane only).

use std::fs;
use std::path::Path;
use std::sync::Mutex;

use agentstack::cli::{LockArgs, UseArgs};
use agentstack::commands::{lock as lock_cmd, use_profile};
use agentstack::trust::{self, TrustState};

// These tests mutate the process-global HOME; serialize them.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn use_args(write: bool) -> UseArgs {
    UseArgs {
        profile: "p".into(),
        targets: vec!["claude-code".into()],
        scope: None,
        write,
        allow_unresolved: false,
        prune_foreign: false,
        no_gitignore: true,
    }
}

fn write_project(proj: &Path) {
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [skills.helper]\npath = \"./skills/helper\"\n\
         [profiles.p]\nskills = [\"helper\"]\n",
    )
    .unwrap();
    fs::create_dir_all(proj.join("skills/helper")).unwrap();
    fs::write(proj.join("skills/helper/SKILL.md"), "# helper v1\n").unwrap();
}

/// The whole story in one flow: activate + trust, drift the skill body
/// (trust digest does NOT flip — the gap), watch `use --write` fail closed,
/// accept with `agentstack lock` (trust re-gates), re-trust, activate again.
#[test]
fn inline_skill_drift_blocks_activation_until_relocked() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    write_project(&proj);

    // First activation pins the lock (the pinning act), then the human trusts.
    use_profile::run(&use_args(true), Some(&proj)).unwrap();
    let lock_path = proj.join("agentstack.lock");
    let lock_before = fs::read_to_string(&lock_path).unwrap();
    assert!(lock_before.contains("helper"), "lock pinned the skill");
    trust::trust(&proj).unwrap();
    assert_eq!(trust::check(&proj), TrustState::Trusted);

    // Drift the skill body. Manifest and lock bytes are untouched, so the
    // trust digest does NOT flip — this is exactly the gap being closed.
    fs::write(proj.join("skills/helper/SKILL.md"), "# helper EVIL\n").unwrap();
    assert_eq!(
        trust::check(&proj),
        TrustState::Trusted,
        "precondition: skill bodies are outside the trust digest"
    );

    // The use site fails closed — and must not have absorbed the drift into
    // the lock (the old behavior this increment removes).
    let err = use_profile::run(&use_args(true), Some(&proj))
        .unwrap_err()
        .to_string();
    assert!(err.contains("drifted"), "gate names the drift: {err}");
    assert!(err.contains("helper"), "gate names the skill: {err}");
    assert_eq!(
        fs::read_to_string(&lock_path).unwrap(),
        lock_before,
        "a blocked activation must never rewrite the lock"
    );

    // Explicit acceptance: `agentstack lock` re-pins, and because the lock
    // bytes are part of the trust digest, the project re-gates automatically.
    lock_cmd::run(&LockArgs { profile: None }, Some(&proj)).unwrap();
    assert_ne!(
        fs::read_to_string(&lock_path).unwrap(),
        lock_before,
        "re-locking recorded the new content digest"
    );
    assert_eq!(
        trust::check(&proj),
        TrustState::Changed,
        "accepting new content re-gates trust"
    );

    // After human review: re-trust, and activation flows again.
    trust::trust(&proj).unwrap();
    use_profile::run(&use_args(true), Some(&proj)).unwrap();
}

/// First activation of an unpinned project proceeds: recording the first pin
/// IS the pinning act (explicit invocation is consent in host mode).
#[test]
fn unpinned_first_activation_proceeds_and_pins() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    write_project(&proj);

    assert!(!proj.join("agentstack.lock").exists());
    use_profile::run(&use_args(true), Some(&proj)).unwrap();
    let lock = fs::read_to_string(proj.join("agentstack.lock")).unwrap();
    assert!(lock.contains("helper"), "first activation pinned: {lock}");
}
