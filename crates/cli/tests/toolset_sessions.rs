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
