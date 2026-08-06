// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Slice 2 witness (`sessions-v1`): the `use --list --json` body carries the
//! active-session state a toolset picker renders — per-profile `active` and
//! the top-level `session` object — and the state comes from the CLI's own
//! session store on every read. That is what makes interrupted-session
//! recovery possible: a supervising UI that died mid-session reads the truth
//! back on its next load instead of trusting its own memory.

use std::fs;
use std::sync::Mutex;

use agentstack::commands::use_profile;

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn list_json_reports_active_session_even_after_a_supervisor_died() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let as_home = home.join(".agentstack");
    fs::create_dir_all(&as_home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", &as_home);

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        r#"version = 1

[servers.srv]
type = "stdio"
command = "npx"
args = ["srv-mcp"]

[profiles.dev]
servers = ["srv"]
"#,
    )
    .unwrap();

    // No session: the listing says so, in both shapes a picker reads.
    let out = use_profile::list_json_value(Some(&proj)).unwrap();
    assert_eq!(out["profiles"][0]["name"], "dev");
    assert_eq!(out["profiles"][0]["active"], false);
    assert!(out["session"].is_null());

    // A session exists in the CLI's store, but the process that started it is
    // gone (simulated by writing the store directly — the exact state an
    // interrupted UI leaves behind). The key is the canonicalized manifest
    // dir, as `session::start` records it.
    let key = fs::canonicalize(&proj).unwrap().display().to_string();
    fs::write(
        as_home.join("sessions.json"),
        serde_json::json!({
            &key: {
                "dir": &key,
                "profile": "dev",
                "scope": "project",
                "started_unix": 1_753_000_000u64,
                "history_id": null,
                "skill_adds": [],
                "loads": [],
            }
        })
        .to_string(),
    )
    .unwrap();

    let out = use_profile::list_json_value(Some(&proj)).unwrap();
    assert_eq!(
        out["profiles"][0]["active"], true,
        "the picker's row shows in-use: {out}"
    );
    assert_eq!(out["session"]["profile"], "dev");
    assert_eq!(out["session"]["scope"], "project");
    assert_eq!(out["session"]["started_unix"], 1_753_000_000u64);

    std::env::remove_var("HOME");
    std::env::remove_var("AGENTSTACK_HOME");
}

/// Run the real binary against the isolated HOME the caller just set.
/// `toolset create`'s footer is printed, so a subprocess is the only witness.
/// Markers are coloured unconditionally, so assertions match the sentence and
/// not the punctuation in front of it.
fn cli(proj: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(args)
        .current_dir(proj)
        .env_clear()
        .env("HOME", std::env::var("HOME").unwrap())
        .env("AGENTSTACK_HOME", std::env::var("AGENTSTACK_HOME").unwrap())
        .env("PATH", "/usr/bin:/bin")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("spawn agentstack");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// G22: `toolset create` names the command that unblocks the state it left.
///
/// Creating a toolset writes the manifest and re-locks, and both are the
/// consent digest — so on a project that was trusted, the create itself makes
/// the review come due, and the `use <name> --write` it used to offer as the
/// one next step refuses every target. The review goes first now.
///
/// The other half of the contract — that a project needing no review keeps
/// today's single "Switch to it" line, byte for byte — is a unit test on
/// `panel_edit::switch_lines`, because no end-to-end create can produce that
/// state: the create's own manifest write is what takes it away.
#[test]
fn create_names_the_review_it_just_made_come_due() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let as_home = home.join(".agentstack");
    fs::create_dir_all(&as_home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", &as_home);

    let proj = tmp.path().join("proj");
    fs::create_dir_all(proj.join("skills/seed")).unwrap();
    fs::write(
        proj.join("skills/seed/SKILL.md"),
        "---\ndescription: seed\n---\n# seed\n",
    )
    .unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [skills.seed]\npath = \"./skills/seed\"\n\
         [profiles.default]\nskills = [\"seed\"]\n",
    )
    .unwrap();
    cli(&proj, &["lock", "--write"]);
    agentstack::trust::trust_unreviewed(&proj).unwrap();
    assert_eq!(
        agentstack::trust::check(&proj),
        agentstack::trust::TrustState::Trusted
    );

    // The panel's two-step: review the plan, then apply the exact digest.
    let preview = cli(
        &proj,
        &[
            "toolset",
            "create",
            "backend",
            "--skill",
            "seed",
            "--preview",
        ],
    );
    let plan: serde_json::Value =
        serde_json::from_str(&preview).unwrap_or_else(|e| panic!("{e}: {preview}"));
    let digest = plan["consent_digest"].as_str().unwrap().to_string();
    let out = cli(
        &proj,
        &[
            "toolset",
            "create",
            "backend",
            "--skill",
            "seed",
            "--yes",
            "--consented",
            &digest,
        ],
    );

    assert!(out.contains("toolset"), "the create ran:\n{out}");
    assert_eq!(
        agentstack::trust::check(&proj),
        agentstack::trust::TrustState::Changed,
        "precondition: the create's own writes re-gated the project:\n{out}"
    );
    assert!(
        out.contains("Review the new pins:  agentstack trust ."),
        "the footer names the review first:\n{out}"
    );
    assert!(
        out.contains("Then switch to it:  agentstack use backend --write"),
        "and the switch after it, in the order it has to happen:\n{out}"
    );
    assert!(
        !out.contains("Switch to it:  agentstack use backend --write"),
        "the old single line offered a command that refuses every target:\n{out}"
    );
}
