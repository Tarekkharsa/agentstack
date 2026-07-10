//! `agentstack explain` surfaces the trust facts for a capability: provenance,
//! the secrets it needs and whether they resolve, and its safety signals.

use std::fs;

use agentstack::commands::explain::explain_text;
use agentstack::commands::lib::{add_skill, LibSource};

/// These tests mutate the process-global `HOME`/`AGENTSTACK_HOME`; serialize them.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn explain_server_reports_secret_and_safety() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [servers.kibana]\ntype = \"http\"\nurl = \"https://kibana.example/mcp\"\n\
         headers = { Authorization = \"Bearer ${ZZ_UNSET_TOKEN}\", Org = \"${ZZ_ENV_ORG}\" }\n",
    )
    .unwrap();
    std::env::set_var("ZZ_ENV_ORG", "acme");

    let out = explain_text("kibana", Some(&proj)).unwrap();
    assert!(out.contains("MCP server · http"));
    assert!(out.contains("kibana.example"), "shows the endpoint host");
    assert!(out.contains("${ZZ_UNSET_TOKEN}") && out.contains("not set"));
    // The resolved one names its source layer.
    assert!(
        out.contains("${ZZ_ENV_ORG}") && out.contains("from env"),
        "names the layer a resolved secret comes from"
    );
    assert!(out.contains("network egress"));
    std::env::remove_var("ZZ_ENV_ORG");

    // Unknown capability → a helpful error, not a panic.
    assert!(explain_text("nope-not-here", Some(&proj)).is_err());
}

#[test]
fn explain_skill_reports_resolution_and_lock() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    let proj = tmp.path().join("proj");
    fs::create_dir_all(proj.join("skills/local-skill")).unwrap();
    fs::write(
        proj.join("skills/local-skill/SKILL.md"),
        "---\ndescription: Local test skill\n---\n# body\n",
    )
    .unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[skills.local-skill]\npath = \"./skills/local-skill\"\n",
    )
    .unwrap();

    let out = explain_text("local-skill", Some(&proj)).unwrap();
    assert!(out.contains("skill · path"));
    // Provenance/detail: names where it resolves and its lock state.
    assert!(
        out.contains("inline (this project)"),
        "explain names where the skill resolves"
    );
    assert!(
        out.contains("not locked"),
        "explain shows the skill has no lock pin yet"
    );

    std::env::remove_var("AGENTSTACK_HOME");
    std::env::remove_var("HOME");
}

#[test]
fn explain_library_only_skill() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    // Seed a skill into the central library only (not in any project manifest).
    let src = tmp.path().join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(
        src.join("SKILL.md"),
        "---\ndescription: A central skill\n---\n# body\n",
    )
    .unwrap();
    let lib_home = home.join(".agentstack/lib");
    add_skill(
        &lib_home,
        "central-skill",
        LibSource::Path(&src),
        false,
        true,
        false,
    )
    .unwrap();

    // A project that does NOT define the skill inline.
    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("agentstack.toml"), "version = 1\n").unwrap();

    let out = explain_text("central-skill", Some(&proj)).unwrap();
    assert!(out.contains("skill · path"));
    assert!(out.contains("central library"), "names its origin: {out}");
    assert!(out.contains("A central skill"), "shows the description");
    assert!(out.contains("yes — available locally"), "resolved offline");

    // A name in neither manifest nor library still errors helpfully.
    assert!(explain_text("ghost-skill", Some(&proj)).is_err());

    std::env::remove_var("AGENTSTACK_HOME");
    std::env::remove_var("HOME");
}
