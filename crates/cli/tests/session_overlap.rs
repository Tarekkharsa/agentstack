// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Stage 2.2: sessions must stay contained and recoverable without ever
//! silently clobbering a user's files.
//!
//! - Overlapping projects: two projects each run their own session; one
//!   project's start/end never touches the other's native config, and each
//!   `end` restores only its own project's pre-session bytes.
//! - Interrupted process: a session the store still holds (the exact state a
//!   killed terminal or panel leaves behind) is visible for recovery, reads as
//!   abandoned once it has outlived a working day, refuses a second `start`
//!   that would overwrite the live config, and `end` still restores the
//!   pre-session bytes exactly.
//!
//! Serialized because these mutate the process-global `HOME`/`AGENTSTACK_HOME`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use agentstack::scope::Scope;
use agentstack::session;

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// The user's own pre-existing MCP config — a foreign server agentstack did
/// not write. If a session clobbers this, the test fails.
const USER_MCP: &str =
    "{\n  \"mcpServers\": {\n    \"user-owned\": { \"command\": \"keep-me\" }\n  }\n}\n";

/// A project with one inline server wired into `[profiles.p]`, its pins locked
/// and the project trusted — `session start` is fail-closed, so these are the
/// real preconditions, not a bypass.
fn setup_project(root: &Path, name: &str) -> PathBuf {
    let proj = root.join(name);
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [servers.srv]\ntype = \"stdio\"\ncommand = \"npx\"\nargs = [\"srv-mcp\"]\n\
         [profiles.p]\nservers = [\"srv\"]\n",
    )
    .unwrap();
    agentstack::commands::lock::run(&Default::default(), Some(&proj)).unwrap();
    agentstack::trust::trust_unreviewed(&proj).unwrap();
    proj
}

fn set_home(home: &Path) {
    fs::create_dir_all(home).unwrap();
    std::env::set_var("HOME", home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));
}

fn unset_home() {
    std::env::remove_var("AGENTSTACK_HOME");
    std::env::remove_var("HOME");
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Rewrite the started_unix of the session at `key_dir` in the store, so the
/// test can age a real session past the abandoned boundary without waiting.
fn age_session(home: &Path, proj: &Path, started_unix: u64) {
    let store = home.join(".agentstack/sessions.json");
    let key = fs::canonicalize(proj).unwrap().display().to_string();
    let mut map: BTreeMap<String, serde_json::Value> =
        serde_json::from_str(&fs::read_to_string(&store).unwrap()).unwrap();
    map.get_mut(&key).unwrap()["started_unix"] = serde_json::json!(started_unix);
    fs::write(&store, serde_json::to_string_pretty(&map).unwrap()).unwrap();
}

#[test]
fn overlapping_projects_never_touch_each_others_files() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    set_home(&home);

    let a = setup_project(tmp.path(), "a");
    let b = setup_project(tmp.path(), "b");
    let a_cfg = a.join(".mcp.json");
    let b_cfg = b.join(".mcp.json");

    // Project A already has a user-owned MCP config on disk.
    fs::write(&a_cfg, USER_MCP).unwrap();

    // Start A: it now manages A's config (the foreign server is preserved by
    // the adapter merge, but the point here is the exact-restore contract).
    session::start(Some(&a), "p", Scope::Project).unwrap();
    assert!(a_cfg.exists(), "A's session wrote its server config");
    let a_mid = fs::read_to_string(&a_cfg).unwrap();
    assert!(a_mid.contains("user-owned"), "A's foreign server survives");

    // Start B while A is live. B must write only B's file.
    session::start(Some(&b), "p", Scope::Project).unwrap();
    assert!(b_cfg.exists(), "B's session wrote its own config");
    assert_eq!(
        fs::read_to_string(&a_cfg).unwrap(),
        a_mid,
        "starting B left A's config untouched"
    );

    // Both sessions coexist in the store, keyed by their own project.
    let all = session::list_all();
    assert_eq!(all.len(), 2, "two distinct sessions: {all:?}");
    assert!(session::active(&a).is_some());
    assert!(session::active(&b).is_some());

    // End B: only B reverts; A stays live and A's bytes are unchanged.
    session::end(Some(&b)).unwrap();
    assert!(session::active(&b).is_none(), "B ended");
    assert!(session::active(&a).is_some(), "A still live after B ended");
    assert_eq!(
        fs::read_to_string(&a_cfg).unwrap(),
        a_mid,
        "ending B never touched A's config"
    );

    // End A: A's config restored byte-for-byte to the user's original.
    session::end(Some(&a)).unwrap();
    assert!(session::active(&a).is_none(), "A ended");
    assert_eq!(
        fs::read_to_string(&a_cfg).unwrap(),
        USER_MCP,
        "A restored to the user's pre-session bytes exactly"
    );

    unset_home();
}

#[test]
fn interrupted_session_is_visible_abandoned_and_never_clobbered() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    set_home(&home);

    let proj = setup_project(tmp.path(), "proj");
    let cfg = proj.join(".mcp.json");
    fs::write(&cfg, USER_MCP).unwrap();

    // A session starts, then its terminal/panel dies — the store entry is all
    // that is left. That interrupted session must stay visible for recovery.
    session::start(Some(&proj), "p", Scope::Project).unwrap();
    let mid = fs::read_to_string(&cfg).unwrap();
    assert!(
        session::active(&proj).is_some(),
        "interrupted session is visible"
    );

    // Age it past a working day: the store now reads as abandoned.
    age_session(&home, &proj, now().saturating_sub(13 * 3600));
    let sess = session::active(&proj).expect("still recorded");
    assert!(
        sess.is_abandoned(now()),
        "a 13h-old session reads as abandoned"
    );

    // A second start must refuse — never silently overwrite the live config.
    let err = session::start(Some(&proj), "p", Scope::Project).unwrap_err();
    assert!(
        format!("{err:#}").contains("already active"),
        "second start refuses instead of clobbering: {err:#}"
    );
    assert_eq!(
        fs::read_to_string(&cfg).unwrap(),
        mid,
        "the refused start left the config untouched"
    );

    // Recovery: `end` restores the user's pre-session bytes exactly.
    session::end(Some(&proj)).unwrap();
    assert!(session::active(&proj).is_none(), "recovered");
    assert_eq!(
        fs::read_to_string(&cfg).unwrap(),
        USER_MCP,
        "the interrupted session restored the original bytes"
    );

    unset_home();
}
