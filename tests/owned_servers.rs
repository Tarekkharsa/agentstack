//! Owned servers (`[servers.X] owner = "codex"`): the owning harness app
//! rewrites its own config entry (observed: the Codex desktop app refreshes
//! node_repl env values on every self-update). The manifest must follow the
//! app, never fight it — `apply --write` fans the app's fresh values out to
//! the OTHER targets and refreshes the stale manifest entry, and it must never
//! downgrade the owner's config back to the stale manifest values.

use std::fs;
use std::path::Path;
use std::sync::Mutex;

use agentstack::cli::ApplyArgs;
use agentstack::commands::apply;
use agentstack::trust;

// apply mutates the process-global HOME; serialize these tests.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn setup(home: &Path) {
    fs::create_dir_all(home).unwrap();
    std::env::set_var("HOME", home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));
}

fn apply_write() -> ApplyArgs {
    ApplyArgs {
        targets: vec![],
        profile: None,
        dry_run: false,
        write: true,
        scope: None,
        allow_unresolved: false,
        prune_foreign: false,
        no_gitignore: true,
    }
}

const STALE_MANIFEST: &str = r#"version = 1

[targets]
default = ["codex", "claude-code"]

[servers.node_repl]
type = "stdio"
command = "node"
args = ["repl.js"]
owner = "codex"

[servers.node_repl.env]
NODE_REPL_TRUSTED_BROWSER_CLIENT_SHA256S = "cb79053f"
BROWSER_USE_CODEX_APP_VERSION = "26.623.81905"
"#;

/// What the Codex app left in its own config after a self-update: same server,
/// rotated env values.
const FRESH_CODEX_CONFIG: &str = r#"model = "gpt-5.5"

[mcp_servers.node_repl]
command = "node"
args = ["repl.js"]

[mcp_servers.node_repl.env]
NODE_REPL_TRUSTED_BROWSER_CLIENT_SHA256S = "97669f77"
BROWSER_USE_CODEX_APP_VERSION = "141536"
"#;

fn project_with_stale_manifest(tmp: &Path) -> std::path::PathBuf {
    let proj = tmp.join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("agentstack.toml"), STALE_MANIFEST).unwrap();
    proj
}

#[test]
fn apply_never_downgrades_the_owner_and_refreshes_manifest_and_fanout() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup(&home);
    fs::create_dir_all(home.join(".codex")).unwrap();
    fs::write(home.join(".codex/config.toml"), FRESH_CODEX_CONFIG).unwrap();
    let proj = project_with_stale_manifest(tmp.path());

    apply::run(&apply_write(), Some(&proj)).unwrap();

    // The owner's config keeps the app's fresh values — no downgrade.
    let codex = fs::read_to_string(home.join(".codex/config.toml")).unwrap();
    assert!(codex.contains("97669f77"), "{codex}");
    assert!(codex.contains("141536"), "{codex}");
    assert!(!codex.contains("cb79053f"), "downgraded! {codex}");
    // Unmanaged user content survives the merge.
    assert!(codex.contains("model = \"gpt-5.5\""), "{codex}");

    // The other target got the FRESH values fanned out, not the stale ones.
    let claude = fs::read_to_string(home.join(".claude.json")).unwrap();
    assert!(claude.contains("97669f77"), "{claude}");
    assert!(!claude.contains("cb79053f"), "stale fan-out! {claude}");

    // The manifest entry caught up with the app, keeping its bookkeeping.
    let manifest = fs::read_to_string(proj.join("agentstack.toml")).unwrap();
    assert!(manifest.contains("97669f77"), "{manifest}");
    assert!(manifest.contains("141536"), "{manifest}");
    assert!(!manifest.contains("cb79053f"), "{manifest}");
    assert!(manifest.contains("owner = \"codex\""), "{manifest}");

    // A second apply is a no-op: everything converged.
    apply::run(&apply_write(), Some(&proj)).unwrap();
    assert_eq!(
        fs::read_to_string(home.join(".codex/config.toml")).unwrap(),
        codex
    );
    assert_eq!(
        fs::read_to_string(proj.join("agentstack.toml")).unwrap(),
        manifest
    );

    std::env::remove_var("AGENTSTACK_HOME");
    std::env::remove_var("HOME");
}

#[test]
fn manifest_ref_keys_stay_refs_through_the_refresh() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup(&home);
    fs::create_dir_all(home.join(".codex")).unwrap();
    // On disk the app carries the RESOLVED literal for the token env var.
    fs::write(
        home.join(".codex/config.toml"),
        r#"[mcp_servers.node_repl]
command = "node"

[mcp_servers.node_repl.env]
APP_VERSION = "141536"
NODE_REPL_TOKEN = "resolved-secret-123"
"#,
    )
    .unwrap();
    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        r#"version = 1

[targets]
default = ["codex"]

[servers.node_repl]
type = "stdio"
command = "node"
owner = "codex"

[servers.node_repl.env]
APP_VERSION = "26.623.81905"
NODE_REPL_TOKEN = "${NODE_REPL_TOKEN}"
"#,
    )
    .unwrap();

    // The ref resolves via env var (part of the default resolver chain).
    std::env::set_var("NODE_REPL_TOKEN", "resolved-secret-123");
    apply::run(&apply_write(), Some(&proj)).unwrap();
    std::env::remove_var("NODE_REPL_TOKEN");

    let manifest = fs::read_to_string(proj.join("agentstack.toml")).unwrap();
    // Disk-canonical key refreshed; the secret stays a ${REF}, never a literal.
    assert!(manifest.contains("141536"), "{manifest}");
    assert!(
        manifest.contains("${NODE_REPL_TOKEN}"),
        "secret ref lost: {manifest}"
    );
    assert!(
        !manifest.contains("resolved-secret-123"),
        "secret literal leaked into the manifest: {manifest}"
    );

    std::env::remove_var("AGENTSTACK_HOME");
    std::env::remove_var("HOME");
}

#[test]
fn trust_is_repinned_only_when_it_was_valid_before_the_refresh() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup(&home);
    fs::create_dir_all(home.join(".codex")).unwrap();
    fs::write(home.join(".codex/config.toml"), FRESH_CODEX_CONFIG).unwrap();

    // Trusted project: the auto-refresh is machine-derived from the owner's
    // own config, so trust follows the rewrite instead of breaking.
    let proj = project_with_stale_manifest(tmp.path());
    trust::trust(&proj).unwrap();
    assert_eq!(trust::check(&proj), trust::TrustState::Trusted);
    apply::run(&apply_write(), Some(&proj)).unwrap();
    assert_eq!(
        trust::check(&proj),
        trust::TrustState::Trusted,
        "owned refresh must re-pin previously valid trust"
    );

    // Never-trusted project: the refresh must not mint trust.
    fs::write(home.join(".codex/config.toml"), FRESH_CODEX_CONFIG).unwrap();
    let proj2 = tmp.path().join("proj2");
    fs::create_dir_all(&proj2).unwrap();
    fs::write(proj2.join("agentstack.toml"), STALE_MANIFEST).unwrap();
    apply::run(&apply_write(), Some(&proj2)).unwrap();
    assert_eq!(trust::check(&proj2), trust::TrustState::Untrusted);

    // Trust already broken by a human edit: stays broken (pending review is
    // still pending — the refresh must not silently bless unrelated changes).
    fs::write(home.join(".codex/config.toml"), FRESH_CODEX_CONFIG).unwrap();
    let proj3 = tmp.path().join("proj3");
    fs::create_dir_all(&proj3).unwrap();
    fs::write(proj3.join("agentstack.toml"), STALE_MANIFEST).unwrap();
    trust::trust(&proj3).unwrap();
    fs::write(
        proj3.join("agentstack.toml"),
        format!("{STALE_MANIFEST}\n# human edit after trust\n"),
    )
    .unwrap();
    assert_eq!(trust::check(&proj3), trust::TrustState::Changed);
    apply::run(&apply_write(), Some(&proj3)).unwrap();
    assert_eq!(
        trust::check(&proj3),
        trust::TrustState::Changed,
        "a broken trust must stay broken through the refresh"
    );

    std::env::remove_var("AGENTSTACK_HOME");
    std::env::remove_var("HOME");
}
