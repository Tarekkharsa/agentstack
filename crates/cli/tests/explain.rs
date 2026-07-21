// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! `agentstack explain` surfaces the trust facts for a capability: provenance,
//! the secrets it needs and whether they resolve, and its safety signals.

use std::fs;

use agentstack::commands::explain::{explain_json, explain_text};
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
    // Delivery-mode honesty: the secrets bullet must not claim static native
    // configs never receive plaintext (a static render may resolve values into
    // them); the ${REF} guarantee is scoped to the manifest and lockfile.
    assert!(
        !out.contains("never written as plaintext"),
        "must not overclaim secret placement: {out}"
    );
    assert!(
        out.contains("a static render resolves values into native configs"),
        "states where a static render may place a value: {out}"
    );
    let structured = explain_json("kibana", Some(&proj)).unwrap();
    assert_eq!(structured["kind"], "server");
    assert_eq!(structured["transport"], "http");
    assert_eq!(structured["safety"]["networkEgressDeclared"], true);
    assert_eq!(structured["safety"]["needsSecret"], true);
    assert!(structured["secrets"]
        .as_array()
        .is_some_and(|s| s.len() == 2));
    std::env::remove_var("ZZ_ENV_ORG");

    // Unknown capability → a helpful error, not a panic.
    assert!(explain_text("nope-not-here", Some(&proj)).is_err());
}

/// The egress/secret policy dimensions surface for a server, both project and
/// machine layers, and the "connects out to" safety bullet says whether the
/// declared host actually passes the compiled [policy.egress].
#[test]
fn explain_server_reports_egress_and_secret_policy() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    // Machine layer: a "*" secrets rule, rename-proof across every server.
    let machine_dir = home.join(".agentstack");
    fs::create_dir_all(&machine_dir).unwrap();
    fs::write(
        machine_dir.join("agentstack.toml"),
        "version = 1\n[policy.secrets]\n\"*\" = [\"!SUPER_SECRET\"]\n",
    )
    .unwrap();

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [servers.kibana]\ntype = \"http\"\nurl = \"https://kibana.example/mcp\"\n\
         [policy.egress]\nkibana = [\"!kibana.example\"]\n\
         [policy.secrets]\nkibana = [\"KIBANA_TOKEN\"]\n",
    )
    .unwrap();

    let out = explain_text("kibana", Some(&proj)).unwrap();
    assert!(
        out.contains("Egress (policy)") && out.contains("deny [kibana.example]"),
        "shows the project egress rule: {out}"
    );
    assert!(
        out.contains("Secret access (policy)") && out.contains("allow only [KIBANA_TOKEN]"),
        "shows the project secret rule: {out}"
    );
    assert!(
        out.contains("Secret access (machine)")
            && out.contains("SUPER_SECRET")
            && out.contains("(via \"*\")"),
        "shows the machine secret rule via the \"*\" key: {out}"
    );
    // The declared host is denied by the project's own egress rule, so the
    // safety bullet must say so rather than silently pass.
    assert!(
        out.contains("BLOCKED by [policy.egress]"),
        "annotates the declared host as blocked: {out}"
    );

    std::env::remove_var("AGENTSTACK_HOME");
    std::env::remove_var("HOME");
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
    // P21: the mental-model line — a hand-authored inline block owned by no pack.
    assert!(
        out.contains("inline manifest"),
        "explain names the inline-manifest model: {out}"
    );
    assert!(
        out.contains("not locked"),
        "explain shows the skill has no lock pin yet"
    );

    std::env::remove_var("AGENTSTACK_HOME");
    std::env::remove_var("HOME");
}

#[test]
fn explain_instruction_names_receiving_and_unsupported_targets() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("house.md"), "House rule.\n").unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[instructions.house]\npath = \"./house.md\"\n",
    )
    .unwrap();

    let out = explain_text("house", Some(&proj)).unwrap();
    // The CLIs that actually receive it, with their instruction file…
    assert!(
        out.contains("Claude Code (CLAUDE.md)") && out.contains("Codex CLI (AGENTS.md)"),
        "names receiving targets with their file: {out}"
    );
    // …and the ones that match `"*"` but have no instruction file.
    assert!(
        out.contains("not supported by:") && out.contains("Cursor"),
        "names unsupported targets: {out}"
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
    // P21: the mental-model line — a by-name library reference, resolved fresh.
    assert!(
        out.contains("library reference by name"),
        "explain names the library-reference model: {out}"
    );
    assert!(out.contains("A central skill"), "shows the description");
    assert!(out.contains("yes — available locally"), "resolved offline");

    // A name in neither manifest nor library still errors helpfully.
    assert!(explain_text("ghost-skill", Some(&proj)).is_err());

    std::env::remove_var("AGENTSTACK_HOME");
    std::env::remove_var("HOME");
}

/// P21: an inline skill a `[packs.*]` ledger owns is named a vendored pack copy —
/// the distinction "Resolves: inline" alone cannot make (a hand-authored inline
/// block and a pack member both resolve inline).
#[test]
fn explain_names_vendored_pack_model() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    let proj = tmp.path().join("proj");
    fs::create_dir_all(proj.join("skills/pack-skill")).unwrap();
    fs::write(
        proj.join("skills/pack-skill/SKILL.md"),
        "---\ndescription: A pack member\n---\n# body\n",
    )
    .unwrap();
    // The skill rides the normal `[skills]` section; the `[packs.linear]` ledger
    // records that it owns it (the vendored-copy bookkeeping).
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n\
         [skills.pack-skill]\npath = \"./skills/pack-skill\"\n\
         [packs.linear]\nversion = \"1.2.0\"\ndescription = \"Linear pack\"\nskills = [\"pack-skill\"]\n",
    )
    .unwrap();

    let out = explain_text("pack-skill", Some(&proj)).unwrap();
    assert!(
        out.contains("vendored pack copy"),
        "names the vendored-pack model: {out}"
    );
    assert!(
        out.contains("[packs.linear]"),
        "names the owning pack: {out}"
    );

    std::env::remove_var("AGENTSTACK_HOME");
    std::env::remove_var("HOME");
}
