// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

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

use agentstack::cli::{ApplyArgs, LockArgs, TrustArgs, UseArgs};
use agentstack::commands::{apply, lock as lock_cmd, trust as trust_cmd, use_profile};
use agentstack::scope::Scope;
use agentstack::trust::{self, TrustState};

// These tests mutate the process-global HOME; serialize them.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn use_args(write: bool) -> UseArgs {
    UseArgs {
        profile: Some("p".into()),
        targets: vec!["claude-code".into()],
        scope: None,
        write,
        allow_unresolved: false,
        prune_foreign: false,
        no_gitignore: true,
        list: false,
        json: false,
    }
}

/// §7.2: a non-interactive grant must present the surface digest a preview
/// would emit at this moment — the same two-step an external UI performs.
/// Computed fresh per call: `lock`/edits between calls change the digest.
fn grant_args(proj: &Path) -> TrustArgs {
    TrustArgs {
        path: Some(proj.to_path_buf()),
        list: false,
        revoke: false,
        yes: true,
        consented_digest: trust::digest_for(proj),
        preview: false,
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
    trust::trust_unreviewed(&proj).unwrap();
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
    lock_cmd::run(&LockArgs::default(), Some(&proj)).unwrap();
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
    trust::trust_unreviewed(&proj).unwrap();
    use_profile::run(&use_args(true), Some(&proj)).unwrap();
}

/// `agentstack trust` refuses to pin over an unpinned or drifted loadable
/// surface: `agentstack lock` is a prerequisite of trust, so the digest the
/// human blesses always covers pins that match the reviewed content.
#[test]
fn trust_grant_requires_a_pinned_matching_surface() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    write_project(&proj);

    // Unpinned inline skill → refused, pointing at `agentstack lock`.
    let err = trust_cmd::run(&grant_args(&proj)).unwrap_err().to_string();
    assert!(err.contains("isn't fully pinned"), "{err}");
    assert!(err.contains("helper"), "{err}");
    assert!(err.contains("`agentstack lock`"), "{err}");
    assert_eq!(trust::check(&proj), TrustState::Untrusted);

    // Pin it → trust grants.
    lock_cmd::run(&LockArgs::default(), Some(&proj)).unwrap();
    trust_cmd::run(&grant_args(&proj)).unwrap();
    assert_eq!(trust::check(&proj), TrustState::Trusted);

    // Drift the body → re-granting refuses until re-locked.
    fs::write(proj.join("skills/helper/SKILL.md"), "# helper v2\n").unwrap();
    let err = trust_cmd::run(&grant_args(&proj)).unwrap_err().to_string();
    assert!(err.contains("drifted"), "{err}");

    lock_cmd::run(&LockArgs::default(), Some(&proj)).unwrap();
    trust_cmd::run(&grant_args(&proj)).unwrap();
    assert_eq!(trust::check(&proj), TrustState::Trusted);
}

/// `trust --preview` is read-only: it must never grant, even on a surface a
/// real grant would accept. It is the UI consent screen, not consent itself.
#[test]
fn trust_preview_grants_nothing() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    write_project(&proj);
    let preview_args = TrustArgs {
        path: Some(proj.clone()),
        list: false,
        revoke: false,
        yes: false,
        consented_digest: None,
        preview: true,
    };

    // Even after pinning (a grant would succeed here), preview leaves the
    // project untrusted — it only reports the surface.
    lock_cmd::run(&LockArgs::default(), Some(&proj)).unwrap();
    trust_cmd::run(&preview_args).unwrap();
    assert_eq!(trust::check(&proj), TrustState::Untrusted);
}

/// A project declaring `[policy.tools]` gets that surfaced in the trust
/// review — the human sees what the bundle REQUESTS on every policy
/// dimension, not just servers/skills/instructions — and trust still grants
/// once the loadable surface is pinned (policy is display-only here: it
/// can only narrow at runtime, never widen, so it is never a blocker).
#[test]
fn trust_grant_surfaces_requested_policy() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    write_project(&proj);
    // Append a policy declaration onto the existing project manifest.
    let mut manifest_toml = fs::read_to_string(proj.join("agentstack.toml")).unwrap();
    manifest_toml.push_str("[policy.tools]\ngithub = [\"!delete_*\"]\n");
    fs::write(proj.join("agentstack.toml"), manifest_toml).unwrap();

    // The pure line-builder shows the requested rule (no need to capture
    // `grant`'s stdout — it prints exactly these lines).
    let loaded = agentstack::manifest::load_from_dir(&proj).unwrap();
    let lines = trust_cmd::policy_requested_lines(&loaded.manifest.policy);
    assert!(
        lines
            .iter()
            .any(|l| l.contains("tools") && l.contains("github: !delete_*")),
        "expected a tools/github requested line, got: {lines:?}"
    );

    // And granting still succeeds once the (unrelated) skill surface is pinned
    // — requested policy is reviewed, never blocking.
    lock_cmd::run(&LockArgs::default(), Some(&proj)).unwrap();
    trust_cmd::run(&grant_args(&proj)).unwrap();
    assert_eq!(trust::check(&proj), TrustState::Trusted);
}

fn apply_args() -> ApplyArgs {
    ApplyArgs {
        targets: vec!["claude-code".into()],
        profile: None,
        dry_run: false,
        write: true,
        scope: Some(Scope::Global),
        allow_unresolved: false,
        prune_foreign: false,
        no_gitignore: true,
    }
}

/// Instruction fragments walk the same re-gate chain as skills: first apply
/// pins, trust requires the pin, drift blocks the compile and the re-grant,
/// `agentstack lock` accepts (flipping trust to Changed), re-trust restores.
#[test]
fn instruction_drift_blocks_apply_until_relocked() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    let proj = tmp.path().join("proj");
    fs::create_dir_all(proj.join("instructions")).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [instructions.house]\npath = \"./instructions/house.md\"\n",
    )
    .unwrap();
    fs::write(proj.join("instructions/house.md"), "Be kind.\n").unwrap();

    // Unpinned instruction → trust refuses.
    let err = trust_cmd::run(&grant_args(&proj)).unwrap_err().to_string();
    assert!(err.contains("isn't fully pinned"), "{err}");
    assert!(err.contains("house"), "{err}");

    // First apply --write compiles AND records the first pin.
    apply::run(&apply_args(), Some(&proj)).unwrap();
    let lock_path = proj.join("agentstack.lock");
    let lock_before = fs::read_to_string(&lock_path).unwrap();
    assert!(
        lock_before.contains("[[instruction]]") && lock_before.contains("house"),
        "apply pinned the fragment: {lock_before}"
    );
    let compiled = home.join(".claude/CLAUDE.md");
    assert!(
        fs::read_to_string(&compiled).unwrap().contains("Be kind."),
        "fragment compiled into the instruction file"
    );

    // Pinned → trust grants.
    trust_cmd::run(&grant_args(&proj)).unwrap();
    assert_eq!(trust::check(&proj), TrustState::Trusted);

    // Drift the fragment: manifest + lock untouched → trust digest holds.
    fs::write(proj.join("instructions/house.md"), "Be EVIL.\n").unwrap();
    assert_eq!(trust::check(&proj), TrustState::Trusted);

    // The compile gate fails closed, and the lock isn't silently rewritten.
    let err = apply::run(&apply_args(), Some(&proj))
        .unwrap_err()
        .to_string();
    assert!(err.contains("refusing to compile instructions"), "{err}");
    assert!(err.contains("house"), "{err}");
    assert_eq!(fs::read_to_string(&lock_path).unwrap(), lock_before);
    assert!(
        !fs::read_to_string(&compiled).unwrap().contains("Be EVIL."),
        "drifted content never reached the instruction file"
    );

    // Re-granting trust refuses over the drift.
    let err = trust_cmd::run(&grant_args(&proj)).unwrap_err().to_string();
    assert!(err.contains("drifted"), "{err}");

    // Accept via `agentstack lock` (zero profiles — instructions still pin),
    // which re-gates trust through the lock bytes.
    lock_cmd::run(&LockArgs::default(), Some(&proj)).unwrap();
    assert_eq!(trust::check(&proj), TrustState::Changed);

    // Re-trust → apply flows and the accepted content compiles.
    trust_cmd::run(&grant_args(&proj)).unwrap();
    apply::run(&apply_args(), Some(&proj)).unwrap();
    assert!(fs::read_to_string(&compiled).unwrap().contains("Be EVIL."));
}

/// Machine-layer fragments are the user's own content: they never pin, never
/// block the gates, and never appear in the trust review.
#[test]
fn machine_layer_fragments_are_exempt_from_pinning() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let ast_home = home.join(".agentstack");
    fs::create_dir_all(ast_home.join("instructions")).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", &ast_home);

    // Machine manifest declares a fragment (like setup's house rules).
    fs::write(
        ast_home.join("agentstack.toml"),
        "version = 1\n[instructions.style]\npath = \"./instructions/style.md\"\n",
    )
    .unwrap();
    fs::write(ast_home.join("instructions/style.md"), "Machine style.\n").unwrap();

    // Project declares its own.
    let proj = tmp.path().join("proj");
    fs::create_dir_all(proj.join("instructions")).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [instructions.house]\npath = \"./instructions/house.md\"\n",
    )
    .unwrap();
    fs::write(proj.join("instructions/house.md"), "Project rule.\n").unwrap();

    // `agentstack lock` pins the project fragment only.
    lock_cmd::run(&LockArgs::default(), Some(&proj)).unwrap();
    let lock = fs::read_to_string(proj.join("agentstack.lock")).unwrap();
    assert!(lock.contains("house"), "{lock}");
    assert!(
        !lock.contains("style"),
        "machine fragment must not pin into the project lock: {lock}"
    );

    // Trust grants (the machine fragment is invisible to the review), and
    // apply --write compiles both layers without the machine fragment ever
    // blocking or pinning.
    trust_cmd::run(&grant_args(&proj)).unwrap();
    apply::run(&apply_args(), Some(&proj)).unwrap();
    let compiled = fs::read_to_string(home.join(".claude/CLAUDE.md")).unwrap();
    assert!(compiled.contains("Machine style."));
    assert!(compiled.contains("Project rule."));
    let lock = fs::read_to_string(proj.join("agentstack.lock")).unwrap();
    assert!(
        !lock.contains("style"),
        "apply must not pin machine fragments"
    );
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

/// D7 W1 witnesses, the workflow re-gate chain end to end:
/// - a ONE-BYTE edit to the pinned workflow source surfaces as
///   `ChecksumDrift`, blocks re-granting trust, and accepting it with
///   `agentstack lock` flips trust to `Changed` (lock bytes are in the trust
///   digest) until re-reviewed;
/// - a ROLES widening with unchanged bytes surfaces as `RolesDrift` and
///   blocks trust the same way — the pin bound the script to its reviewed
///   capability sets, not just its bytes.
#[test]
fn workflow_drift_and_roles_widening_block_trust_until_relocked() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    let proj = tmp.path().join("proj");
    fs::create_dir_all(proj.join("workflows/review")).unwrap();
    const MANIFEST: &str = "version = 1\n[profiles.reader]\n\
         [workflows.review]\npath = \"./workflows/review\"\nroles = [\"reader\"]\n";
    fs::write(proj.join("agentstack.toml"), MANIFEST).unwrap();
    fs::write(
        proj.join("workflows/review/main.js"),
        "export const meta = { name: 'review' } // v1",
    )
    .unwrap();

    // Unpinned workflow → trust refuses, naming it.
    let err = trust_cmd::run(&grant_args(&proj)).unwrap_err().to_string();
    assert!(err.contains("isn't fully pinned"), "{err}");
    assert!(err.contains("review"), "{err}");

    // Pin → trust grants.
    lock_cmd::run(&LockArgs::default(), Some(&proj)).unwrap();
    let lock_path = proj.join("agentstack.lock");
    let lock_before = fs::read_to_string(&lock_path).unwrap();
    assert!(lock_before.contains("[[workflow]]"), "{lock_before}");
    trust_cmd::run(&grant_args(&proj)).unwrap();
    assert_eq!(trust::check(&proj), TrustState::Trusted);

    // One-byte source edit: manifest + lock untouched → trust digest holds
    // (the gap), and the status is ChecksumDrift.
    fs::write(
        proj.join("workflows/review/main.js"),
        "export const meta = { name: 'review' } // v2",
    )
    .unwrap();
    assert_eq!(trust::check(&proj), TrustState::Trusted);
    {
        let manifest: agentstack::manifest::Manifest = toml::from_str(MANIFEST).unwrap();
        let lock = agentstack::lock::Lock::load(&proj).unwrap();
        let store = agentstack::store::Store::default_store();
        let status = agentstack::resolve::workflow_lock_status(
            "review",
            &manifest.workflows["review"],
            &proj,
            &store,
            &lock,
            agentstack::resolve::ResolveMode::NoFetch,
        );
        assert!(
            matches!(
                status,
                agentstack::resolve::WorkflowLockStatus::ChecksumDrift { .. }
            ),
            "{status:?}"
        );
    }
    // Drift blocks the re-grant…
    let err = trust_cmd::run(&grant_args(&proj)).unwrap_err().to_string();
    assert!(err.contains("drifted"), "{err}");
    // …and `agentstack lock` accepts it, which re-gates trust via the lock
    // bytes. Re-trust restores.
    lock_cmd::run(&LockArgs::default(), Some(&proj)).unwrap();
    assert_ne!(fs::read_to_string(&lock_path).unwrap(), lock_before);
    assert_eq!(trust::check(&proj), TrustState::Changed);
    trust_cmd::run(&grant_args(&proj)).unwrap();
    assert_eq!(trust::check(&proj), TrustState::Trusted);

    // Roles widening, bytes unchanged: add a second role to the declaration
    // only. The manifest edit itself flips the trust digest (manifest bytes
    // are in it), but the load-bearing check is the PIN: even after the
    // human re-reviews the manifest, the roles-vs-pin mismatch still blocks
    // until re-locked.
    let widened = "version = 1\n[profiles.reader]\n[profiles.writer]\n\
         [workflows.review]\npath = \"./workflows/review\"\nroles = [\"reader\", \"writer\"]\n";
    fs::write(proj.join("agentstack.toml"), widened).unwrap();
    let err = trust_cmd::run(&grant_args(&proj)).unwrap_err().to_string();
    assert!(err.contains("roles changed since locked"), "{err}");

    // Re-lock records the widened role set; trust then grants over it.
    lock_cmd::run(&LockArgs::default(), Some(&proj)).unwrap();
    let relocked = fs::read_to_string(&lock_path).unwrap();
    assert!(
        relocked.contains("writer"),
        "pin records the widened roles: {relocked}"
    );
    trust_cmd::run(&grant_args(&proj)).unwrap();
    assert_eq!(trust::check(&proj), TrustState::Trusted);
}
