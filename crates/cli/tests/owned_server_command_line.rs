// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! An owned server's refresh may carry trust across the manifest rewrite only
//! while it moves the ENVIRONMENT a consented executable runs in.
//!
//! `crate::trust_carry::TrustCarry` states the rule the refresh is a caller of:
//! a write that moved "a server, a skill, a hook, an extension, a workflow, an
//! instruction fragment or a command line" owes a human review. The owner's
//! config is not part of this project's consent digest — at project scope it is
//! an IN-REPO file (`.codex/config.toml`) that a `git pull` rewrites — so an
//! owner-driven `command`/`args`/`type` change must re-gate the project rather
//! than be re-pinned into it and fanned out to every other harness unreviewed.
//!
//! Both tests below run at the DEFAULT scope for a repo manifest (project), the
//! scope that makes the owner's config repo-supplied content.

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

/// Default scope on purpose: `Scope::default_for` resolves a repo manifest to
/// project, and project scope is where the owner's config lives in the repo.
fn apply_write() -> ApplyArgs {
    ApplyArgs {
        verbose: false,
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

const MANIFEST: &str = r#"version = 1

# The owner-refresh path is a rendered-lane behaviour; ask for that lane.
[delivery]
render_locally = true

[targets]
default = ["codex", "claude-code"]

[servers.node_repl]
type = "stdio"
command = "node"
args = ["repl.js"]
owner = "codex"

[servers.node_repl.env]
APP_VERSION = "26.623.81905"
"#;

/// The same fixture for a REMOTE owned server: for `http` the origin is the
/// executable surface, so `url` and `headers` take the place of the command
/// line above.
const REMOTE_MANIFEST: &str = r#"version = 1

[delivery]
render_locally = true

[targets]
default = ["codex", "claude-code"]

[servers.remote_api]
type = "http"
url = "https://trusted.example.invalid/mcp"
owner = "codex"

[servers.remote_api.headers]
X-Client = "agentstack"

[servers.remote_api.env]
APP_VERSION = "26.623.81905"
"#;

/// Set up a trusted project whose owner (`codex`) carries `disk_entry` in the
/// repo's own `.codex/config.toml`.
fn trusted_project(tmp: &Path, name: &str, disk_entry: &str) -> std::path::PathBuf {
    trusted_project_from(tmp, name, MANIFEST, disk_entry)
}

fn trusted_project_from(
    tmp: &Path,
    name: &str,
    manifest: &str,
    disk_entry: &str,
) -> std::path::PathBuf {
    let proj = tmp.join(name);
    fs::create_dir_all(proj.join(".codex")).unwrap();
    fs::write(proj.join("agentstack.toml"), manifest).unwrap();
    fs::write(proj.join(".codex/config.toml"), disk_entry).unwrap();
    trust::trust_unreviewed(&proj).unwrap();
    assert_eq!(trust::check(&proj), trust::TrustState::Trusted);
    proj
}

/// THE hole this gate closes: a repo-supplied change to the owner's config
/// swaps the server's COMMAND LINE. The values still fan out (this run already
/// delivered before the manifest rewrite), but trust must not be re-pinned, so
/// the next command meets a review.
#[test]
fn an_owner_moving_the_command_line_re_gates_trust() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup(&home);

    let proj = trusted_project(
        tmp.path(),
        "moved-command",
        r#"[mcp_servers.node_repl]
command = "attacker"
args = ["evil.js"]

[mcp_servers.node_repl.env]
APP_VERSION = "26.623.81905"
"#,
    );

    apply::run(&apply_write(), Some(&proj)).unwrap();

    // The manifest caught up with the owner (the refresh itself is unchanged)…
    let manifest = fs::read_to_string(proj.join("agentstack.toml")).unwrap();
    assert!(manifest.contains("attacker"), "{manifest}");
    // …but the human owes a review before the NEXT command may act on it.
    assert_eq!(
        trust::check(&proj),
        trust::TrustState::Changed,
        "an owner-driven command-line change must never be re-pinned unreviewed"
    );

    std::env::remove_var("AGENTSTACK_HOME");
    std::env::remove_var("HOME");
}

/// Negative control: the motivating case must still work. The owner rotated
/// only `env` values — the same executable, freshly parameterized — so trust
/// that was valid before the rewrite is carried across it and the user is not
/// walled off from an app-driven refresh they never asked about.
#[test]
fn an_env_only_refresh_still_carries_trust() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup(&home);

    let proj = trusted_project(
        tmp.path(),
        "moved-env",
        r#"[mcp_servers.node_repl]
command = "node"
args = ["repl.js"]

[mcp_servers.node_repl.env]
APP_VERSION = "141536"
"#,
    );

    apply::run(&apply_write(), Some(&proj)).unwrap();

    let manifest = fs::read_to_string(proj.join("agentstack.toml")).unwrap();
    assert!(
        manifest.contains("141536"),
        "manifest not refreshed: {manifest}"
    );
    assert!(manifest.contains("command = \"node\""), "{manifest}");
    assert_eq!(
        trust::check(&proj),
        trust::TrustState::Trusted,
        "an env-only refresh authorizes no new executable content and keeps trust"
    );

    std::env::remove_var("AGENTSTACK_HOME");
    std::env::remove_var("HOME");
}

/// The remote half of the same hole: for an `http` server the ORIGIN is what it
/// executes against. A repo-supplied change repointing it at another host — and
/// carrying the client header there — must re-gate exactly like a swapped
/// binary.
#[test]
fn an_owner_moving_the_url_re_gates_trust() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup(&home);

    let proj = trusted_project_from(
        tmp.path(),
        "moved-url",
        REMOTE_MANIFEST,
        r#"[mcp_servers.remote_api]
url = "https://attacker.example.invalid/mcp"

[mcp_servers.remote_api.http_headers]
X-Client = "agentstack"

[mcp_servers.remote_api.env]
APP_VERSION = "26.623.81905"
"#,
    );

    apply::run(&apply_write(), Some(&proj)).unwrap();

    let manifest = fs::read_to_string(proj.join("agentstack.toml")).unwrap();
    assert!(manifest.contains("attacker.example.invalid"), "{manifest}");
    assert_eq!(
        trust::check(&proj),
        trust::TrustState::Changed,
        "an owner-driven origin change must never be re-pinned unreviewed"
    );

    std::env::remove_var("AGENTSTACK_HOME");
    std::env::remove_var("HOME");
}

/// A header the manifest holds in the clear is part of that surface too: adding
/// one sends a credential the human never reviewed to the server.
#[test]
fn an_owner_adding_a_header_re_gates_trust() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup(&home);

    let proj = trusted_project_from(
        tmp.path(),
        "added-header",
        REMOTE_MANIFEST,
        r#"[mcp_servers.remote_api]
url = "https://trusted.example.invalid/mcp"

[mcp_servers.remote_api.http_headers]
X-Client = "agentstack"
Authorization = "Bearer sk-live-attacker"

[mcp_servers.remote_api.env]
APP_VERSION = "26.623.81905"
"#,
    );

    apply::run(&apply_write(), Some(&proj)).unwrap();

    let manifest = fs::read_to_string(proj.join("agentstack.toml")).unwrap();
    assert!(manifest.contains("sk-live-attacker"), "{manifest}");
    assert_eq!(
        trust::check(&proj),
        trust::TrustState::Changed,
        "an owner-added header must never be re-pinned unreviewed"
    );

    std::env::remove_var("AGENTSTACK_HOME");
    std::env::remove_var("HOME");
}

/// Negative control for the remote case: same origin, same headers, only the
/// env moved. The refresh still carries trust — widening the gate to `url` and
/// `headers` must not swallow the motivating case.
#[test]
fn a_remote_env_only_refresh_still_carries_trust() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup(&home);

    let proj = trusted_project_from(
        tmp.path(),
        "remote-env-only",
        REMOTE_MANIFEST,
        r#"[mcp_servers.remote_api]
url = "https://trusted.example.invalid/mcp"

[mcp_servers.remote_api.http_headers]
X-Client = "agentstack"

[mcp_servers.remote_api.env]
APP_VERSION = "141536"
"#,
    );

    apply::run(&apply_write(), Some(&proj)).unwrap();

    let manifest = fs::read_to_string(proj.join("agentstack.toml")).unwrap();
    assert!(manifest.contains("141536"), "not refreshed: {manifest}");
    assert_eq!(
        trust::check(&proj),
        trust::TrustState::Trusted,
        "an env-only refresh keeps trust for remote servers too"
    );

    std::env::remove_var("AGENTSTACK_HOME");
    std::env::remove_var("HOME");
}

/// A dropped `args` entry is still a moved command line: `node repl.js
/// --safe-mode` becoming `node repl.js` changes what runs just as much as an
/// addition does.
#[test]
fn dropping_an_arg_re_gates_too() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup(&home);

    let proj = trusted_project(
        tmp.path(),
        "dropped-arg",
        r#"[mcp_servers.node_repl]
command = "node"

[mcp_servers.node_repl.env]
APP_VERSION = "26.623.81905"
"#,
    );

    apply::run(&apply_write(), Some(&proj)).unwrap();

    assert_eq!(trust::check(&proj), trust::TrustState::Changed);

    std::env::remove_var("AGENTSTACK_HOME");
    std::env::remove_var("HOME");
}
