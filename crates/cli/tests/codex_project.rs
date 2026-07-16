// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Codex project scope: `<repo>/.codex/config.toml` (which Codex loads only
//! for trusted projects — its gate, not ours) receives MCP servers, hooks,
//! and settings COEXISTING in the one TOML file, non-destructively; and the
//! descriptor points skills at the cross-tool `.agents/skills` convention.

use std::fs;
use std::sync::Mutex;

use agentstack::adapter::Registry;
use agentstack::cli::ApplyArgs;
use agentstack::commands::apply;
use agentstack::scope::Scope;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn set_home(home: &std::path::Path) {
    fs::create_dir_all(home).unwrap();
    std::env::set_var("HOME", home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));
}

fn args(write: bool) -> ApplyArgs {
    ApplyArgs {
        targets: vec!["codex".into()],
        profile: None,
        dry_run: !write,
        write,
        scope: Some(Scope::Project),
        allow_unresolved: false,
        no_gitignore: true,
        prune_foreign: false,
    }
}

/// The descriptor itself: project config + the documented skills locations.
/// (~/.codex/skills was never a documented Codex location.)
#[test]
fn codex_descriptor_declares_project_scope_and_agents_skills() {
    let registry = Registry::load().unwrap();
    let codex = registry.get("codex").expect("codex adapter");
    let project = codex.project.as_ref().expect("codex has project scope");
    assert_eq!(project.config, ".codex/config.toml");
    let skills = codex.skills.as_ref().expect("codex has skills");
    assert_eq!(skills.dir, "~/.agents/skills");
    assert_eq!(skills.project_dir.as_deref(), Some(".agents/skills"));
    assert_eq!(
        codex.hooks.as_ref().and_then(|h| h.project.as_deref()),
        Some(".codex/config.toml")
    );
    assert_eq!(
        codex.settings.as_ref().and_then(|s| s.project.as_deref()),
        Some(".codex/config.toml")
    );
}

/// MCP servers, hooks, and settings all render into the ONE project
/// `.codex/config.toml`, hand-written keys survive, and a second apply is
/// idempotent.
#[test]
fn project_config_hosts_servers_hooks_and_settings_together() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    set_home(&tmp.path().join("home"));

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        r#"version = 1
[targets]
default = ["codex"]

[servers.demo]
type = "stdio"
command = "/bin/echo"

[hooks.fmt]
event = "PostToolUse"
command = "cargo fmt"

[settings.codex]
model = "gpt-5.5"
"#,
    )
    .unwrap();
    // A hand-written key that must survive every managed write.
    let cfg = proj.join(".codex/config.toml");
    fs::create_dir_all(cfg.parent().unwrap()).unwrap();
    fs::write(&cfg, "# my note\npersonality = \"pragmatic\"\n").unwrap();

    apply::run(&args(true), Some(&proj)).unwrap();
    let text = fs::read_to_string(&cfg).unwrap();
    assert!(
        text.contains("[mcp_servers.demo]"),
        "server table missing:\n{text}"
    );
    assert!(text.contains("cargo fmt"), "hook missing:\n{text}");
    assert!(text.contains("gpt-5.5"), "setting missing:\n{text}");
    assert!(
        text.contains("# my note") && text.contains("personality"),
        "hand-written content clobbered:\n{text}"
    );

    // Idempotent: byte-identical on a repeat write.
    apply::run(&args(true), Some(&proj)).unwrap();
    assert_eq!(text, fs::read_to_string(&cfg).unwrap());
}
