// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Hooks compile into Claude Code's settings.json `hooks` key, render the exact
//! nested schema, preserve other keys, resolve secrets, and prune when removed.
//! Own test file so the HOME override runs isolated.

use std::fs;
use std::sync::{Mutex, OnceLock};

use agentstack::adapter::Registry;
use agentstack::manifest::Manifest;
use agentstack::render::plan_hooks;
use agentstack::scope::Scope;
use agentstack::secret::MapResolver;

fn claude(reg: &Registry) -> &agentstack::adapter::AdapterDescriptor {
    reg.get("claude-code").unwrap()
}

fn codex(reg: &Registry) -> &agentstack::adapter::AdapterDescriptor {
    reg.get("codex").unwrap()
}

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
}

#[test]
fn hooks_render_prune_and_preserve() {
    let _guard = env_lock();
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(home.join(".claude")).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));
    // settings.json with a hand-set key we must never clobber.
    fs::write(
        home.join(".claude/settings.json"),
        "{\n  \"model\": \"opus\"\n}\n",
    )
    .unwrap();

    let reg = Registry::load().unwrap();
    let resolver = MapResolver::from([("TOK", "sekret")]);
    let proj = tmp.path();

    let manifest: Manifest = toml::from_str(
        r#"
        version = 1
        [hooks.fmt]
        event = "PostToolUse"
        matcher = "Edit|Write"
        command = "prettier --write"
        [hooks.greet]
        event = "SessionStart"
        command = "notify ${TOK}"
        timeout = 5
        "#,
    )
    .unwrap();

    // Render: builds the hooks key, preserves model, resolves the secret.
    let plan = plan_hooks(
        &manifest,
        claude(&reg),
        &resolver,
        false,
        Scope::Global,
        proj,
        &[],
    )
    .unwrap()
    .unwrap();
    assert!(plan.unresolved.is_empty());
    plan.write().unwrap();

    let v: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(home.join(".claude/settings.json")).unwrap())
            .unwrap();
    assert_eq!(v["model"], "opus"); // preserved
    assert_eq!(v["hooks"]["PostToolUse"][0]["matcher"], "Edit|Write");
    assert_eq!(
        v["hooks"]["PostToolUse"][0]["hooks"][0]["command"],
        "prettier --write"
    );
    assert_eq!(v["hooks"]["PostToolUse"][0]["hooks"][0]["type"], "command");
    // SessionStart has no matcher key; secret resolved; timeout present.
    assert!(v["hooks"]["SessionStart"][0].get("matcher").is_none());
    assert_eq!(
        v["hooks"]["SessionStart"][0]["hooks"][0]["command"],
        "notify sekret"
    );
    assert_eq!(v["hooks"]["SessionStart"][0]["hooks"][0]["timeout"], 5);

    // Empty manifest + previously-managed → prune the hooks key, keep model.
    let empty: Manifest = toml::from_str("version = 1\n").unwrap();
    let prune = plan_hooks(
        &empty,
        claude(&reg),
        &resolver,
        true,
        Scope::Global,
        proj,
        &[],
    )
    .unwrap()
    .unwrap();
    prune.write().unwrap();
    let v2: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(home.join(".claude/settings.json")).unwrap())
            .unwrap();
    assert!(v2.get("hooks").is_none());
    assert_eq!(v2["model"], "opus");
}

/// Machine-layer hooks (the guard) render alongside the manifest's — a
/// global-scope apply that owns the whole hooks key must re-emit the guard
/// entry, not strip it. And with NO manifest hooks at all, machine hooks
/// alone are enough to produce a plan.
#[test]
fn machine_hooks_ride_along_and_survive_the_manifest() {
    let _guard = env_lock();
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(home.join(".claude")).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    let reg = Registry::load().unwrap();
    let resolver = MapResolver::from([("TOK", "sekret")]);
    let proj = tmp.path();
    let machine = vec![(
        "agentstack-guard".to_string(),
        agentstack::manifest::Hook {
            event: "PreToolUse".into(),
            matcher: None,
            command: "/bin/agentstack guard check --protocol claude".into(),
            args: vec![],
            timeout: Some(10),
            targets: vec!["claude-code".into()],
        },
    )];

    // Manifest hooks + machine hook → both render.
    let manifest: Manifest = toml::from_str(
        "version = 1\n[hooks.fmt]\nevent = \"PostToolUse\"\ncommand = \"prettier --write\"\n",
    )
    .unwrap();
    let plan = plan_hooks(
        &manifest,
        claude(&reg),
        &resolver,
        false,
        Scope::Global,
        proj,
        &machine,
    )
    .unwrap()
    .unwrap();
    plan.write().unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(home.join(".claude/settings.json")).unwrap())
            .unwrap();
    assert!(v["hooks"].get("PostToolUse").is_some());
    assert_eq!(
        v["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
        "/bin/agentstack guard check --protocol claude"
    );

    // Empty manifest, previously managed → the guard hook still renders
    // (only the manifest's own hooks are pruned).
    let empty: Manifest = toml::from_str("version = 1\n").unwrap();
    let plan = plan_hooks(
        &empty,
        claude(&reg),
        &resolver,
        true,
        Scope::Global,
        proj,
        &machine,
    )
    .unwrap()
    .unwrap();
    plan.write().unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(home.join(".claude/settings.json")).unwrap())
            .unwrap();
    assert!(v["hooks"].get("PostToolUse").is_none());
    assert!(v["hooks"].get("PreToolUse").is_some());
}

/// P13.2: Codex loads hooks from BOTH `~/.codex/hooks.json` (written by
/// `guard install`) AND `config.toml [hooks]` (this renderer). The guard must
/// end up registered exactly ONCE. With guard install's hooks.json in place and
/// a manifest `[hooks.*]` entry that is itself a guard-check hook, the renderer
/// must DROP the guard from config.toml (deferring to hooks.json) while keeping
/// the manifest's non-guard hooks.
#[test]
fn codex_guard_is_registered_exactly_once_across_both_hook_files() {
    let _guard = env_lock();
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(home.join(".codex")).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    // Simulate `guard install`: the guard hook lives in ~/.codex/hooks.json.
    let hooks_json = home.join(".codex/hooks.json");
    fs::write(
        &hooks_json,
        r#"{"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"/bin/agentstack guard check --protocol codex","timeout":10}]}]}}"#,
    )
    .unwrap();

    let reg = Registry::load().unwrap();
    // No secrets in these commands; an empty resolver suffices.
    let resolver = MapResolver::from([] as [(&str, &str); 0]);
    let proj = tmp.path();

    // A manifest [hooks.*] with BOTH a normal hook and a guard-check hook,
    // both targeting codex.
    let manifest: Manifest = toml::from_str(
        r#"
        version = 1
        [hooks.fmt]
        event = "PostToolUse"
        command = "prettier --write"
        targets = ["codex"]
        [hooks.guard]
        event = "PreToolUse"
        command = "/bin/agentstack guard check --protocol codex"
        targets = ["codex"]
        "#,
    )
    .unwrap();

    let plan = plan_hooks(
        &manifest,
        codex(&reg),
        &resolver,
        false,
        Scope::Global,
        proj,
        &[],
    )
    .unwrap()
    .unwrap();
    plan.write().unwrap();

    let config = fs::read_to_string(home.join(".codex/config.toml")).unwrap();
    // The non-guard hook survives; the guard hook is deferred to hooks.json.
    assert!(config.contains("prettier --write"), "manifest hook kept");
    assert!(
        !config.contains("guard check --protocol"),
        "guard must NOT be re-rendered into config.toml: {config}"
    );

    // Across BOTH files Codex loads, the guard appears exactly once.
    let both = fs::read_to_string(&hooks_json).unwrap() + &config;
    let count = both.matches("guard check --protocol").count();
    assert_eq!(count, 1, "exactly one guard registration for Codex");
}

#[test]
fn codex_hooks_render_to_config_toml_and_preserve_mcp() {
    let _guard = env_lock();
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(home.join(".codex")).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));
    fs::write(
        home.join(".codex/config.toml"),
        "model = \"gpt-5.5\"\n\n[mcp_servers.figma]\nurl = \"https://mcp.figma.com/mcp\"\n",
    )
    .unwrap();

    let reg = Registry::load().unwrap();
    let resolver = MapResolver::from([("TOK", "sekret")]);
    let proj = tmp.path();

    let manifest: Manifest = toml::from_str(
        r#"
        version = 1
        [hooks.fmt]
        event = "PostToolUse"
        matcher = "Edit|Write"
        command = "prettier --write ${TOK}"
        timeout = 5
        targets = ["codex"]
        "#,
    )
    .unwrap();

    let plan = plan_hooks(
        &manifest,
        codex(&reg),
        &resolver,
        false,
        Scope::Global,
        proj,
        &[],
    )
    .unwrap()
    .unwrap();
    assert!(plan.unresolved.is_empty());
    plan.write().unwrap();

    let text = fs::read_to_string(home.join(".codex/config.toml")).unwrap();
    assert!(text.contains("model = \"gpt-5.5\""));
    assert!(text.contains("[mcp_servers.figma]"));
    assert!(text.contains("[hooks]"));
    assert!(text.contains("PostToolUse = [{ matcher = \"Edit|Write\""));
    assert!(text.contains("prettier --write sekret"));
    let parsed: toml::Value = toml::from_str(&text).unwrap();
    assert!(parsed.get("hooks").is_some());

    let empty: Manifest = toml::from_str("version = 1\n").unwrap();
    let prune = plan_hooks(
        &empty,
        codex(&reg),
        &resolver,
        true,
        Scope::Global,
        proj,
        &[],
    )
    .unwrap()
    .unwrap();
    prune.write().unwrap();
    let pruned = fs::read_to_string(home.join(".codex/config.toml")).unwrap();
    assert!(!pruned.contains("[hooks]"));
    assert!(pruned.contains("[mcp_servers.figma]"));
}
