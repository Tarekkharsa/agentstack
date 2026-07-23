// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The preview → edit → apply races, end to end (UI control-plane §"Consent
//! and administrative authority"). Both consent bindings share one shape: a
//! read-only preview emits a digest of the exact reviewed content, and the
//! later write must present it back — so any byte that changed in between
//! flips the digest and the write refuses instead of blessing content nobody
//! reviewed.
//!
//! Two bindings, two witnesses:
//!  - trust:  `trust --preview` surface_digest ↔ `trust --yes --consented-digest`
//!  - setup:  `init --plan` plan_digest ↔ `init --yes --consented-plan`

use std::fs;
use std::path::Path;
use std::sync::Mutex;

use agentstack::cli::{InitArgs, TrustArgs};
use agentstack::commands::{init, trust as trust_cmd};
use agentstack::trust::{self, TrustState};

// These tests mutate the process-global HOME/AGENTSTACK_HOME; serialize them.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn fake_home(tmp: &Path) -> std::path::PathBuf {
    let home = tmp.join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));
    home
}

fn grant_args(proj: &Path, digest: Option<String>) -> TrustArgs {
    TrustArgs {
        path: Some(proj.to_path_buf()),
        list: false,
        revoke: false,
        yes: true,
        consented_digest: digest,
        preview: false,
    }
}

/// trust: the surface digest a preview handed out stops authorizing the grant
/// the moment any pinned byte changes — and the store keeps the pre-race
/// state, so nothing downstream sees a blessed-but-unreviewed manifest.
#[test]
fn trust_grant_refuses_previewed_digest_after_manifest_edit() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    fake_home(tmp.path());

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[servers.good]\ntype = \"stdio\"\ncommand = \"echo\"\n",
    )
    .unwrap();

    // The preview moment: same digest `trust --preview` emits (witnessed as
    // identical in crates/trust's snapshot tests).
    let previewed = trust::digest_for(&proj).unwrap();

    // The race: a pull/edit lands a new server between preview and grant.
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[servers.good]\ntype = \"stdio\"\ncommand = \"echo\"\n[servers.evil]\ntype = \"stdio\"\ncommand = \"curl\"\n",
    )
    .unwrap();

    let err = trust_cmd::run(&grant_args(&proj, Some(previewed))).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("consent"),
        "refusal names the consent gate: {msg}"
    );
    assert_eq!(
        trust::check(&proj),
        TrustState::Untrusted,
        "a refused grant must leave no trust record"
    );

    // Recovery is a fresh preview of the new bytes, then the grant binds.
    let fresh = trust::digest_for(&proj).unwrap();
    trust_cmd::run(&grant_args(&proj, Some(fresh))).unwrap();
    assert_eq!(trust::check(&proj), TrustState::Trusted);
}

fn init_args(yes: bool, consented_plan: Option<String>) -> InitArgs {
    InitArgs {
        global: false,
        force: false,
        dry_run: false,
        plan: false,
        // Skip keeps the witness off the real OS keychain.
        secrets: Some(agentstack::cli::SecretStore::Skip),
        no_keychain: false,
        yes,
        consented_plan,
    }
}

/// setup: the plan digest emitted by `init --plan` stops authorizing the
/// scripted import when a detected CLI config changes in between — the
/// refused apply writes nothing, and a fresh plan's digest applies cleanly.
#[test]
fn init_apply_refuses_previewed_plan_after_source_config_edit() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = fake_home(tmp.path());

    // A detected CLI: claude-code by config presence (~/.claude.json).
    fs::write(
        home.join(".claude.json"),
        r#"{"mcpServers":{"alpha":{"command":"echo","args":["hi"]}}}"#,
    )
    .unwrap();

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();

    let plan = init::plan_json(&init_args(false, None), Some(&proj)).unwrap();
    let reviewed = plan["plan_digest"].as_str().unwrap().to_string();
    assert!(reviewed.starts_with("sha256:"));
    assert!(
        plan["servers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["name"] == "alpha"),
        "plan lists the importable server"
    );

    // The race: the CLI config gains a server after the plan was reviewed.
    fs::write(
        home.join(".claude.json"),
        r#"{"mcpServers":{"alpha":{"command":"echo","args":["hi"]},"beta":{"command":"curl"}}}"#,
    )
    .unwrap();

    let err = init::run(&init_args(true, Some(reviewed.clone())), Some(&proj)).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("plan"),
        "refusal points back at the plan flow: {msg}"
    );
    assert!(
        !proj.join(".agentstack").exists() && !proj.join("agentstack.toml").exists(),
        "a refused apply must write nothing"
    );

    // Recovery: re-plan over the new state, apply with the fresh digest.
    let fresh = init::plan_json(&init_args(false, None), Some(&proj)).unwrap();
    let fresh_digest = fresh["plan_digest"].as_str().unwrap().to_string();
    assert_ne!(fresh_digest, reviewed, "a changed source flips the digest");
    init::run(&init_args(true, Some(fresh_digest)), Some(&proj)).unwrap();
    let manifest = fs::read_to_string(proj.join(".agentstack/agentstack.toml")).unwrap();
    assert!(manifest.contains("alpha") && manifest.contains("beta"));

    std::env::remove_var("HOME");
    std::env::remove_var("AGENTSTACK_HOME");
}
