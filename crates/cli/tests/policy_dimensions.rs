//! Policy dimensions, end to end: per-server secret access is enforced
//! fail-closed at BOTH substitution sites (rendered configs and the gateway),
//! the write-time egress check refuses a disallowed declared host, machine
//! denies win over anything a repo says, and an unmentioned server is
//! untouched (uniform allow-by-default — upgrade safety).

use std::fs;
use std::path::Path;
use std::sync::Mutex;

use agentstack::cli::ApplyArgs;
use agentstack::commands::apply;
use agentstack::gateway::Gateway;
use agentstack::scope::Scope;
use serde_json::json;

// These tests mutate the process-global HOME + env secrets; serialize them.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn setup_home(home: &Path) {
    fs::create_dir_all(home).unwrap();
    std::env::set_var("HOME", home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));
}

/// The machine manifest (`~/.agentstack/agentstack.toml`) — the layer no repo
/// can see, shadow, or loosen.
fn write_machine_policy(home: &Path, policy: &str) {
    let dir = home.join(".agentstack");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("agentstack.toml"),
        format!("version = 1\n{policy}"),
    )
    .unwrap();
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

/// Machine `[policy.secrets] "*" = ["!EVIL_*"]` blocks a repo server's
/// `${EVIL_TOKEN}` at render time — even though the secret IS resolvable
/// (denied must never read as "not set"), even under --allow-unresolved
/// (a convenience flag never overrides policy), and rename-proof.
#[test]
fn machine_secret_deny_blocks_render_fail_closed() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup_home(&home);
    write_machine_policy(&home, "[policy.secrets]\n\"*\" = [\"!EVIL_*\"]\n");
    std::env::set_var("EVIL_TOKEN", "leak-me-not-xyz");

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [servers.sneaky]\ntype = \"http\"\nurl = \"https://api.example/mcp\"\n\
         headers = { Authorization = \"Bearer ${EVIL_TOKEN}\" }\n",
    )
    .unwrap();

    let claude_cfg = home.join(".claude.json");
    apply::run(&apply_args(), Some(&proj)).unwrap();
    assert!(
        !claude_cfg.exists(),
        "policy-denied secret must block the write"
    );

    // --allow-unresolved must NOT bypass the policy.
    let mut args = apply_args();
    args.allow_unresolved = true;
    apply::run(&args, Some(&proj)).unwrap();
    assert!(
        !claude_cfg.exists(),
        "--allow-unresolved must never override [policy.secrets]"
    );

    std::env::remove_var("EVIL_TOKEN");
}

/// The same machine deny bites at the gateway: the ref never resolves for the
/// upstream, the call fails fast naming [policy.secrets], and the secret
/// value never appears anywhere.
#[test]
fn machine_secret_deny_blocks_gateway_calls() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup_home(&home);
    write_machine_policy(&home, "[policy.secrets]\n\"*\" = [\"!EVIL_*\"]\n");
    std::env::set_var("EVIL_TOKEN", "leak-me-not-xyz");

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [servers.sneaky]\ntype = \"stdio\"\ncommand = \"/bin/echo\"\n\
         env = { TOKEN = \"${EVIL_TOKEN}\" }\n",
    )
    .unwrap();

    let gw = Gateway::from_manifest(Some(&proj));
    let err = gw
        .try_call("sneaky__anything", &json!({}))
        .expect("routed to the upstream")
        .expect_err("must fail fast on the denied ref");
    let msg = format!("{err:#}");
    assert!(msg.contains("[policy.secrets]"), "{msg}");
    assert!(msg.contains("machine policy"), "{msg}");
    assert!(!msg.contains("leak-me-not-xyz"), "value leaked: {msg}");

    std::env::remove_var("EVIL_TOKEN");
}

/// Write-time egress: an HTTP server whose declared host fails the machine
/// [policy.egress] is refused at render AND never built as a gateway
/// upstream. Rename-proof via the "*" key.
#[test]
fn egress_denied_host_is_refused_at_write_time() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup_home(&home);
    write_machine_policy(&home, "[policy.egress]\n\"*\" = [\"!evil.example\"]\n");

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [servers.innocent-name]\ntype = \"http\"\nurl = \"https://evil.example/mcp\"\n",
    )
    .unwrap();

    // Render path: blocked, nothing written.
    let claude_cfg = home.join(".claude.json");
    apply::run(&apply_args(), Some(&proj)).unwrap();
    assert!(
        !claude_cfg.exists(),
        "egress-denied declared host must block the write"
    );

    // Gateway path: the upstream is never constructed.
    let gw = Gateway::from_manifest(Some(&proj));
    assert!(gw.is_empty(), "egress-denied upstream must not be built");
}

/// Allow-by-default: a server no policy names renders and proxies untouched,
/// even when OTHER servers are constrained — upgrade safety for every
/// existing manifest.
#[test]
fn unmentioned_server_is_unaffected() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup_home(&home);
    write_machine_policy(
        &home,
        "[policy.secrets]\nother-server = [\"!*\"]\n[policy.egress]\nother-server = [\"api.only\"]\n",
    );
    std::env::set_var("FINE_TOKEN", "resolves-fine");

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [servers.kibana]\ntype = \"http\"\nurl = \"https://kibana.example/mcp\"\n\
         headers = { Authorization = \"Bearer ${FINE_TOKEN}\" }\n",
    )
    .unwrap();

    let claude_cfg = home.join(".claude.json");
    apply::run(&apply_args(), Some(&proj)).unwrap();
    let written = fs::read_to_string(&claude_cfg).unwrap();
    assert!(written.contains("kibana"), "{written}");
    assert!(
        written.contains("resolves-fine"),
        "unconstrained server resolves normally: {written}"
    );

    std::env::remove_var("FINE_TOKEN");
}
