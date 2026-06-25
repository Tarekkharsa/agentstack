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
        url = "https://kibana-mcp.ghaloyalty.com/mcp"
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
    m.servers
        .iter()
        .map(|(name, server)| (name.clone(), render_server(desc, server, &r).value))
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
