//! End-to-end integration tests against a temp HOME-like directory.
//!
//! A custom adapter descriptor points at temp config files so we can exercise
//! the full `plan_target` path (render → merge → write) without touching the
//! real `~/.claude.json` / `~/.codex/config.toml`.

use std::fs;

use agentstack::adapter::{extract_servers, AdapterDescriptor, Registry};
use agentstack::discover::lift_secrets;
use agentstack::manifest::model::{Meta, Targets};
use agentstack::manifest::Manifest;
use agentstack::render::apply::{plan_target, Selection};
use agentstack::scope::Scope;
use agentstack::secret::MapResolver;
use assert_fs::prelude::*;
use indexmap::IndexMap;
use serde_json::json;
use std::path::Path;

fn manifest() -> Manifest {
    toml::from_str(
        r#"
        version = 1
        [servers.kibana_mcp]
        type = "http"
        url = "https://kibana-mcp.ghaloyalty.com/mcp"
        headers = { Authorization = "Bearer ${KIBANA_TOKEN}" }
        [profiles.backend]
        servers = ["kibana_mcp"]
        "#,
    )
    .unwrap()
}

/// Build a JSON adapter descriptor whose config path is `config_path`.
fn json_descriptor(config_path: &str) -> AdapterDescriptor {
    let yaml = format!(
        r#"
id: test-json
display: Test JSON
config:
  path: {config_path}
  format: json
mcp:
  location: mcpServers
  fields:
    url: url
    headers: headers
    command: command
    args: args
    env: env
  transport:
    key: type
    http_value: http
    stdio_value: stdio
  secret_mode: literal
"#
    );
    serde_yaml::from_str(&yaml).unwrap()
}

#[test]
fn non_destructive_merge_preserves_other_content_and_is_idempotent() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let cfg = tmp.child("claude.json");
    cfg.write_str(
        "{\n  \"numStartups\": 42,\n  \"stats\": { \"avg\": 0.9402052562189797 },\n  \"mcpServers\": {\n    \"tldraw\": { \"type\": \"stdio\", \"command\": \"node\" }\n  }\n}\n",
    )
    .unwrap();

    let m = manifest();
    let desc = json_descriptor(cfg.path().to_str().unwrap());
    let resolver = MapResolver::from([("KIBANA_TOKEN", "tok")]);

    // First plan + write.
    let plan = plan_target(
        &m,
        &desc,
        &resolver,
        &Selection::All,
        &[],
        Scope::Global,
        Path::new("/"),
    )
    .unwrap()
    .unwrap();
    assert!(plan.changed());
    assert!(plan.unresolved.is_empty());
    plan.write().unwrap();

    let after = fs::read_to_string(cfg.path()).unwrap();
    // Untouched content preserved exactly (incl. the float).
    assert!(after.contains("\"numStartups\": 42"));
    assert!(after.contains("0.9402052562189797"));
    assert!(after.contains("\"tldraw\""));
    // Our managed server + resolved secret are present.
    assert!(after.contains("\"kibana_mcp\""));
    assert!(after.contains("Bearer tok"));
    // Valid JSON.
    serde_json::from_str::<serde_json::Value>(&after).unwrap();

    // Second plan on the freshly-written file is a no-op (idempotent).
    let plan2 = plan_target(
        &m,
        &desc,
        &resolver,
        &Selection::All,
        &[],
        Scope::Global,
        Path::new("/"),
    )
    .unwrap()
    .unwrap();
    assert!(
        !plan2.changed(),
        "re-apply should be a no-op:\n{}",
        plan2.diff()
    );
}

#[test]
fn profile_selection_limits_servers() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let cfg = tmp.child("c.json");
    cfg.write_str("{\n  \"mcpServers\": {}\n}\n").unwrap();

    let mut m = manifest();
    // Add a second server NOT in the backend profile.
    m.servers.insert(
        "figma".into(),
        toml::from_str("type = \"http\"\nurl = \"https://figma\"").unwrap(),
    );

    let desc = json_descriptor(cfg.path().to_str().unwrap());
    let resolver = MapResolver::from([("KIBANA_TOKEN", "tok")]);

    let plan = plan_target(
        &m,
        &desc,
        &resolver,
        &Selection::Profile("backend".into()),
        &[],
        Scope::Global,
        Path::new("/"),
    )
    .unwrap()
    .unwrap();
    assert_eq!(plan.managed, vec!["kibana_mcp".to_string()]);
    assert!(!plan.proposed.contains("figma"));
}

#[test]
fn prunes_servers_that_left_the_selection() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let cfg = tmp.child("c.json");
    cfg.write_str(
        "{\n  \"mcpServers\": {\n    \"kibana_mcp\": { \"type\": \"http\", \"url\": \"https://old\" },\n    \"legacy\": { \"type\": \"http\", \"url\": \"https://legacy\" }\n  }\n}\n",
    )
    .unwrap();

    let m = manifest(); // only defines kibana_mcp
    let desc = json_descriptor(cfg.path().to_str().unwrap());
    let resolver = MapResolver::from([("KIBANA_TOKEN", "tok")]);

    // We used to manage both; "legacy" is no longer in the manifest.
    let previously = vec!["kibana_mcp".to_string(), "legacy".to_string()];
    let plan = plan_target(
        &m,
        &desc,
        &resolver,
        &Selection::All,
        &previously,
        Scope::Global,
        Path::new("/"),
    )
    .unwrap()
    .unwrap();

    assert_eq!(plan.removed, vec!["legacy".to_string()]);
    assert!(!plan.proposed.contains("legacy"));
    assert!(plan.proposed.contains("kibana_mcp"));
}

#[test]
fn init_pipeline_roundtrips_through_valid_toml() {
    // Import from a Claude-shaped config, lift secrets, build a manifest,
    // serialize to TOML, and parse it back — guards TOML field ordering
    // (scalars before subtables) and the import→lift→manifest flow.
    let reg = Registry::load().unwrap();
    let desc = reg.get("claude-code").unwrap();
    let root = json!({
        "mcpServers": {
            "kibana": {
                "type": "http",
                "url": "https://k/mcp",
                "headers": { "Authorization": "Bearer raw-secret-token" }
            },
            "gh": {
                "type": "stdio",
                "command": "npx",
                "args": ["-y", "server-github"],
                "env": { "GITHUB_TOKEN": "ghp_rawvalue", "PORT": "3000" }
            }
        }
    });

    let mut servers: IndexMap<_, _> = extract_servers(desc, &root).into_iter().collect();
    let lifted = lift_secrets(&mut servers);
    // Both the bearer token and the env token were lifted.
    assert_eq!(lifted.len(), 2);
    assert!(servers["kibana"].headers["Authorization"].contains("${"));
    assert_eq!(servers["gh"].env["GITHUB_TOKEN"], "${GITHUB_TOKEN}");
    assert_eq!(servers["gh"].env["PORT"], "3000");

    let manifest = Manifest {
        version: 1,
        meta: Meta { name: None },
        servers,
        skills: IndexMap::new(),
        profiles: IndexMap::new(),
        instructions: IndexMap::new(),
        targets: Targets {
            default: vec!["claude-code".into()],
        },
    };

    let toml_text = toml::to_string_pretty(&manifest).unwrap();
    let parsed: Manifest = toml::from_str(&toml_text).unwrap();
    assert_eq!(parsed, manifest);
    // No raw secrets leaked into the manifest.
    assert!(!toml_text.contains("raw-secret-token"));
    assert!(!toml_text.contains("ghp_rawvalue"));
}

#[test]
fn missing_secret_is_reported_not_blanked() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let cfg = tmp.child("c.json");
    cfg.write_str("{\n  \"mcpServers\": {}\n}\n").unwrap();

    let m = manifest();
    let desc = json_descriptor(cfg.path().to_str().unwrap());
    let resolver = MapResolver::default(); // nothing resolves

    let plan = plan_target(
        &m,
        &desc,
        &resolver,
        &Selection::All,
        &[],
        Scope::Global,
        Path::new("/"),
    )
    .unwrap()
    .unwrap();
    assert_eq!(plan.unresolved.len(), 1);
    assert!(plan.unresolved[0].contains("KIBANA_TOKEN"));
    // The placeholder is left intact, never silently emptied.
    assert!(plan.proposed.contains("${KIBANA_TOKEN}"));
}

#[test]
fn instructions_compile_shared_and_harness_specific_blocks() {
    use agentstack::render::instructions::plan_instructions;

    let tmp = assert_fs::TempDir::new().unwrap();
    tmp.child("instructions/shared.md")
        .write_str("Shared rule.\n")
        .unwrap();
    tmp.child("instructions/claude.md")
        .write_str("Claude only.\n")
        .unwrap();

    let m: Manifest = toml::from_str(
        r#"
        version = 1
        [instructions.shared]
        path = "./instructions/shared.md"
        targets = ["*"]
        [instructions.claudeonly]
        path = "./instructions/claude.md"
        targets = ["claude-code"]
        "#,
    )
    .unwrap();

    let reg = Registry::load().unwrap();
    let claude = reg.get("claude-code").unwrap();
    let codex = reg.get("codex").unwrap();

    // Claude (global scope) gets both fragments.
    let cp = plan_instructions(&m, claude, Scope::Global, tmp.path()).unwrap();
    assert_eq!(
        cp.fragments,
        vec!["shared".to_string(), "claudeonly".to_string()]
    );
    assert!(cp.proposed.contains("Shared rule."));
    assert!(cp.proposed.contains("Claude only."));
    assert!(cp.proposed.contains("<!-- agentstack:start -->"));

    // Codex gets only the shared fragment.
    let xp = plan_instructions(&m, codex, Scope::Global, tmp.path()).unwrap();
    assert_eq!(xp.fragments, vec!["shared".to_string()]);
    assert!(xp.proposed.contains("Shared rule."));
    assert!(!xp.proposed.contains("Claude only."));
}

#[test]
fn store_resolves_path_skill_and_lock_roundtrips() {
    use agentstack::lock::{Lock, LockedSkill};
    use agentstack::store::Store;

    let tmp = assert_fs::TempDir::new().unwrap();
    tmp.child("skills/x/SKILL.md").write_str("# x\n").unwrap();
    let skill: agentstack::manifest::Skill = toml::from_str("path = \"./skills/x\"").unwrap();

    let store = Store::with_root(tmp.child("store").path().to_path_buf());
    let resolved = store.resolve(&skill, tmp.path(), None).unwrap();
    assert_eq!(resolved.source_kind, "path");
    assert!(!resolved.checksum.is_empty());

    // Lock upsert + reload reproduces the same checksum.
    let mut lock = Lock::default();
    lock.upsert(LockedSkill {
        name: "x".into(),
        source: "path".into(),
        path: Some("./skills/x".into()),
        git: None,
        rev: None,
        checksum: resolved.checksum.clone(),
    });
    lock.save(tmp.path()).unwrap();
    let reloaded = Lock::load(tmp.path()).unwrap();
    assert_eq!(reloaded.get("x").unwrap().checksum, resolved.checksum);
}

#[test]
fn adopt_inserts_new_server_into_manifest_with_lifted_secret() {
    // Simulates `adopt`: extract a hand-added server from a config, lift its
    // secret, and insert it into the existing manifest text (comments kept).
    let reg = Registry::load().unwrap();
    let desc = reg.get("claude-code").unwrap();
    let config = json!({
        "mcpServers": {
            "kibana_mcp": { "type": "http", "url": "https://k" },
            "linear": { "type": "http", "url": "https://mcp.linear.app/mcp",
                        "headers": { "Authorization": "Bearer lin_api_SECRETVALUE" } }
        }
    });

    let m = manifest(); // has kibana_mcp only
    let mut collected: IndexMap<String, agentstack::manifest::Server> = IndexMap::new();
    for (name, server) in extract_servers(desc, &config) {
        if !m.servers.contains_key(&name) {
            collected.insert(name, server);
        }
    }
    assert_eq!(
        collected.keys().cloned().collect::<Vec<_>>(),
        vec!["linear"]
    );

    let lifted = lift_secrets(&mut collected);
    assert_eq!(lifted.len(), 1);

    let entries: Vec<(String, serde_json::Value)> = collected
        .iter()
        .map(|(n, s)| (n.clone(), serde_json::to_value(s).unwrap()))
        .collect();
    let manifest_text =
        "version = 1\n\n# keep me\n[servers.kibana_mcp]\ntype = \"http\"\nurl = \"https://k\"\n";
    let new_text =
        agentstack::render::merge_toml::merge(manifest_text, "servers", &entries, true).unwrap();

    // Comment preserved, new server added, and it parses back as a manifest.
    assert!(new_text.contains("# keep me"));
    assert!(new_text.contains("[servers.linear]"));
    assert!(!new_text.contains("lin_api_SECRETVALUE")); // secret lifted out
    let parsed: Manifest = toml::from_str(&new_text).unwrap();
    assert!(parsed.servers.contains_key("linear"));
}
