//! Golden snapshot tests: a fixed fixture manifest rendered to each adapter's
//! native config. These lock in the exact JSON/TOML we produce, including the
//! per-format quirks (Claude `type:"http"`, Codex header rename + subtable).

use agentstack::adapter::{render_server, Registry};
use agentstack::manifest::Manifest;
use agentstack::render::{merge_json, merge_toml};
use agentstack::secret::MapResolver;
use serde_json::Value;

fn fixture() -> Manifest {
    toml::from_str(
        r#"
        version = 1

        [servers.kibana]
        type = "http"
        url = "https://kibana-mcp.example.com/mcp"
        headers = { Authorization = "Bearer ${KIBANA_TOKEN}" }

        [servers.github]
        type = "stdio"
        command = "npx"
        args = ["-y", "@modelcontextprotocol/server-github"]
        env = { GITHUB_TOKEN = "${GH_PAT}" }
        "#,
    )
    .unwrap()
}

fn resolver() -> MapResolver {
    MapResolver::from([("KIBANA_TOKEN", "kib-secret"), ("GH_PAT", "ghp-secret")])
}

fn entries(adapter_id: &str) -> Vec<(String, Value)> {
    let reg = Registry::load().unwrap();
    let desc = reg.get(adapter_id).unwrap();
    let m = fixture();
    let r = resolver();
    // Mirror `apply`: servers the adapter's format can't represent are skipped,
    // never written as empty `{}` entries.
    m.servers
        .iter()
        .filter_map(|(name, server)| {
            let rendered = render_server(desc, server, &r);
            rendered
                .representable
                .then(|| (name.clone(), rendered.value))
        })
        .collect()
}

#[test]
fn claude_code_render() {
    let out = merge_json::merge(
        "{\n  \"mcpServers\": {}\n}\n",
        "mcpServers",
        &entries("claude-code"),
    )
    .unwrap();
    insta::assert_snapshot!("claude_code_render", out);
}

#[test]
fn codex_render() {
    let out = merge_toml::merge("", "mcp_servers", &entries("codex"), true).unwrap();
    insta::assert_snapshot!("codex_render", out);
}

#[test]
fn cursor_render() {
    let out = merge_json::merge("{}", "mcpServers", &entries("cursor")).unwrap();
    // Cursor infers transport (no `type` tag) and uses plain `url`.
    assert!(!out.contains("\"type\""));
    assert!(out.contains("\"url\""));
    insta::assert_snapshot!("cursor_render", out);
}

#[test]
fn windsurf_render() {
    let out = merge_json::merge("{}", "mcpServers", &entries("windsurf")).unwrap();
    // Windsurf quirk: HTTP url is written as `serverUrl`.
    assert!(out.contains("\"serverUrl\""));
    assert!(!out.contains("\"url\""));
    insta::assert_snapshot!("windsurf_render", out);
}

#[test]
fn gemini_render() {
    let out = merge_json::merge("{}", "mcpServers", &entries("gemini")).unwrap();
    // Gemini quirk: HTTP url is written as `httpUrl`.
    assert!(out.contains("\"httpUrl\""));
    insta::assert_snapshot!("gemini_render", out);
}

#[test]
fn vscode_render() {
    let out = merge_json::merge("{}", "servers", &entries("vscode")).unwrap();
    // VS Code quirks: top-level "servers" key, transport "type" tag.
    assert!(out.contains("\"servers\""));
    assert!(out.contains("\"type\": \"http\""));
    assert!(out.contains("\"type\": \"stdio\""));
    insta::assert_snapshot!("vscode_render", out);
}

#[test]
fn copilot_cli_render() {
    let out = merge_json::merge("{}", "mcpServers", &entries("copilot-cli")).unwrap();
    // Copilot CLI quirk: transport "type" tag uses "local" for stdio (not
    // "stdio"), "http" for streamable HTTP. Standard mcpServers wrapper.
    assert!(out.contains("\"type\": \"http\""));
    assert!(out.contains("\"type\": \"local\""));
    insta::assert_snapshot!("copilot_cli_render", out);
}

#[test]
fn opencode_render() {
    let out = merge_json::merge("{}", "mcp", &entries("opencode")).unwrap();
    // OpenCode quirks: top-level "mcp" key; transport "type" is "remote"/"local";
    // local servers use a single combined "command" array (no "args"); env is
    // renamed to "environment".
    assert!(out.contains("\"mcp\""));
    assert!(out.contains("\"type\": \"remote\""));
    assert!(out.contains("\"type\": \"local\""));
    assert!(out.contains("\"environment\""));
    assert!(
        !out.contains("\"args\""),
        "command+args collapse into one array"
    );
    insta::assert_snapshot!("opencode_render", out);
}

#[test]
fn junie_render() {
    let out = merge_json::merge("{}", "mcpServers", &entries("junie")).unwrap();
    // Junie quirk: transport is INFERRED (url vs command), so no "type" tag.
    assert!(!out.contains("\"type\""));
    assert!(out.contains("\"url\""));
    assert!(out.contains("\"command\""));
    insta::assert_snapshot!("junie_render", out);
}

#[test]
fn kiro_render() {
    let out = merge_json::merge("{}", "mcpServers", &entries("kiro")).unwrap();
    // Kiro quirk: transport is INFERRED, no "type" tag; standard url/command.
    assert!(!out.contains("\"type\""));
    assert!(out.contains("\"url\""));
    assert!(out.contains("\"command\""));
    insta::assert_snapshot!("kiro_render", out);
}

#[test]
fn antigravity_render() {
    let out = merge_json::merge("{}", "mcpServers", &entries("antigravity")).unwrap();
    // Antigravity quirk (shared Codeium lineage with Windsurf): HTTP url is
    // written as "serverUrl"; transport is inferred (no "type").
    assert!(out.contains("\"serverUrl\""));
    assert!(!out.contains("\"url\""));
    assert!(!out.contains("\"type\""));
    insta::assert_snapshot!("antigravity_render", out);
}

#[test]
fn claude_desktop_render() {
    let out = merge_json::merge("{}", "mcpServers", &entries("claude-desktop")).unwrap();
    // Claude Desktop quirk: the config file schema is stdio-only. The http server
    // (kibana) is unrepresentable and therefore SKIPPED entirely (not written as
    // an empty `{}` entry) — remote servers are added in-app as Connectors. Only
    // the stdio server lands.
    assert!(out.contains("\"github\""));
    assert!(
        !out.contains("\"kibana\""),
        "http server is skipped, not emptied"
    );
    assert!(out.contains("\"command\""));
    assert!(!out.contains("\"url\""));
    assert!(!out.contains("\"serverUrl\""));
    assert!(!out.contains("\"type\""));
    insta::assert_snapshot!("claude_desktop_render", out);
}
