//! Hooks compile into Claude Code's settings.json `hooks` key, render the exact
//! nested schema, preserve other keys, resolve secrets, and prune when removed.
//! Own test file so the HOME override runs isolated.

use std::fs;

use agentstack::adapter::Registry;
use agentstack::manifest::Manifest;
use agentstack::render::plan_hooks;
use agentstack::scope::Scope;
use agentstack::secret::MapResolver;

fn claude(reg: &Registry) -> &agentstack::adapter::AdapterDescriptor {
    reg.get("claude-code").unwrap()
}

#[test]
fn hooks_render_prune_and_preserve() {
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
    let prune = plan_hooks(&empty, claude(&reg), &resolver, true, Scope::Global, proj)
        .unwrap()
        .unwrap();
    prune.write().unwrap();
    let v2: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(home.join(".claude/settings.json")).unwrap())
            .unwrap();
    assert!(v2.get("hooks").is_none());
    assert_eq!(v2["model"], "opus");
}
