//! Reverse of [`render`](super::render): read an existing CLI config (already
//! parsed to a JSON-shaped value tree) and recover manifest [`Server`]s, using
//! the same adapter descriptor that drives rendering. Values are recovered
//! verbatim; secret-lifting is a separate policy step in `init`.

use indexmap::IndexMap;
use serde_json::Value;

use super::descriptor::AdapterDescriptor;
use agentstack_core::manifest::{Server, ServerType};

/// Extract the settings worth importing from a CLI's parsed settings file:
/// every top-level key that has at least one catalog field. Whole top-level
/// values are taken (so e.g. `permissions` keeps its `allow`/`deny` alongside
/// the catalogued `defaultMode`) — this matches the top-level ownership model so
/// re-applying never drops sibling keys.
pub fn extract_settings(desc: &AdapterDescriptor, root: &Value) -> serde_json::Map<String, Value> {
    let Some(spec) = desc.settings.as_ref() else {
        return Default::default();
    };
    let catalog: std::collections::HashSet<&str> = spec
        .fields
        .iter()
        .map(|f| f.key.split('.').next().unwrap_or(&f.key))
        .collect();
    root.as_object()
        .map(|o| {
            o.iter()
                .filter(|(k, _)| catalog.contains(k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// Extract `(name, Server)` pairs from a target config's value tree, in file
/// order. Entries that don't look like MCP servers are skipped.
pub fn extract_servers(desc: &AdapterDescriptor, root: &Value) -> Vec<(String, Server)> {
    let Some(mcp) = desc.mcp.as_ref() else {
        return Vec::new();
    };
    let Some(section) = navigate(root, &mcp.location).and_then(Value::as_object) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for (name, body) in section {
        let Some(obj) = body.as_object() else {
            continue;
        };

        let get_str = |field: &Option<String>| -> Option<String> {
            field
                .as_ref()
                .and_then(|f| obj.get(f))
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        let get_map = |field: &Option<String>| -> IndexMap<String, String> {
            field
                .as_ref()
                .and_then(|f| obj.get(f))
                .and_then(Value::as_object)
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .unwrap_or_default()
        };

        let url = get_str(&mcp.fields.url);
        let command = get_str(&mcp.fields.command);
        let args = mcp
            .fields
            .args
            .as_ref()
            .and_then(|f| obj.get(f))
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let cwd = get_str(&mcp.fields.cwd);
        let headers = get_map(&mcp.fields.headers);
        let env = get_map(&mcp.fields.env);

        let server_type = infer_type(mcp, obj, &url, &command);

        // Skip entries that are neither http nor stdio shaped.
        if url.is_none() && command.is_none() {
            continue;
        }

        // Keys the descriptor maps (plus the transport tag) are canonical;
        // anything else is a hand-tuned native key (e.g. Codex
        // `startup_timeout_sec`) that must round-trip rather than be dropped
        // on the next apply — keep it under `extra.<adapter id>`.
        let known: Vec<&str> = [
            mcp.fields.url.as_deref(),
            mcp.fields.command.as_deref(),
            mcp.fields.args.as_deref(),
            mcp.fields.cwd.as_deref(),
            mcp.fields.headers.as_deref(),
            mcp.fields.env.as_deref(),
            mcp.transport.as_ref().map(|t| t.key.as_str()),
        ]
        .into_iter()
        .flatten()
        .collect();
        let unknown: IndexMap<String, Value> = obj
            .iter()
            .filter(|(k, _)| !known.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let mut extra = IndexMap::new();
        if !unknown.is_empty() {
            extra.insert(desc.id.clone(), unknown);
        }

        out.push((
            name.clone(),
            Server {
                server_type,
                url,
                command,
                args,
                cwd,
                targets: agentstack_core::manifest::model::all_targets(),
                owner: None,
                headers,
                env,
                extra,
            },
        ));
    }
    out
}

/// Determine transport: prefer an explicit tag (Claude's `type`), else infer
/// from which fields are present.
fn infer_type(
    mcp: &super::descriptor::McpSpec,
    obj: &serde_json::Map<String, Value>,
    url: &Option<String>,
    command: &Option<String>,
) -> ServerType {
    if let Some(t) = &mcp.transport {
        if let Some(tag) = obj.get(&t.key).and_then(Value::as_str) {
            if tag == t.http_value {
                return ServerType::Http;
            }
            if t.stdio_value.as_deref() == Some(tag) {
                return ServerType::Stdio;
            }
        }
    }
    if url.is_some() && command.is_none() {
        ServerType::Http
    } else {
        ServerType::Stdio
    }
}

/// Navigate a dotted `location` path (single segment in practice).
fn navigate<'a>(root: &'a Value, location: &str) -> Option<&'a Value> {
    let mut cur = root;
    for seg in location.split('.') {
        cur = cur.as_object()?.get(seg)?;
    }
    Some(cur)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Registry;
    use serde_json::json;

    #[test]
    fn extracts_claude_http_and_stdio() {
        let reg = Registry::load().unwrap();
        let desc = reg.get("claude-code").unwrap();
        let root = json!({
            "mcpServers": {
                "kibana": { "type": "http", "url": "https://k", "headers": { "Authorization": "Bearer x" } },
                "tldraw": { "type": "stdio", "command": "node", "args": ["a.js"], "env": { "K": "v" } }
            }
        });
        let servers = extract_servers(desc, &root);
        assert_eq!(servers.len(), 2);
        let kibana = &servers.iter().find(|(n, _)| n == "kibana").unwrap().1;
        assert_eq!(kibana.server_type, ServerType::Http);
        assert_eq!(kibana.url.as_deref(), Some("https://k"));
        assert_eq!(kibana.headers["Authorization"], "Bearer x");
        let tldraw = &servers.iter().find(|(n, _)| n == "tldraw").unwrap().1;
        assert_eq!(tldraw.server_type, ServerType::Stdio);
        assert_eq!(tldraw.args, vec!["a.js".to_string()]);
    }

    #[test]
    fn extract_settings_takes_catalog_keys_whole_and_skips_unknown() {
        let reg = Registry::load().unwrap();
        let desc = reg.get("claude-code").unwrap();
        let file = json!({
            "$schema": "https://x",            // not in catalog → skip
            "model": "opusplan",               // catalog → keep
            "hooks": { "PreToolUse": [] },      // not in catalog → skip
            "permissions": {                    // catalog (permissions.*) → keep WHOLE object
                "defaultMode": "auto",
                "allow": ["Bash(git:*)"],
                "deny": ["Read(./.env)"]
            }
        });
        let out = extract_settings(desc, &file);
        assert!(out.contains_key("model"));
        assert!(out.contains_key("permissions"));
        assert!(!out.contains_key("$schema"));
        assert!(!out.contains_key("hooks"));
        // The whole permissions object comes along (so apply won't drop siblings).
        let perms = out["permissions"].as_object().unwrap();
        assert!(perms.contains_key("defaultMode"));
        assert!(perms.contains_key("allow"));
        assert!(perms.contains_key("deny"));
    }

    #[test]
    fn cwd_round_trips_and_is_not_lifted_into_extras() {
        let reg = Registry::load().unwrap();
        let desc = reg.get("codex").unwrap();
        let root = json!({
            "mcp_servers": {
                "tldraw": {
                    "command": "node",
                    "args": ["dist/index.js"],
                    "cwd": "/srv/tldraw"
                }
            }
        });
        let servers = extract_servers(desc, &root);
        let tldraw = &servers.iter().find(|(n, _)| n == "tldraw").unwrap().1;
        assert_eq!(tldraw.cwd.as_deref(), Some("/srv/tldraw"));
        // `cwd` is a mapped field, not a hand-tuned native key: it must not be
        // duplicated into extras.
        assert!(tldraw.extra.is_empty(), "cwd should not become an extra");
    }

    #[test]
    fn unknown_keys_are_kept_as_per_target_extras() {
        let reg = Registry::load().unwrap();
        let desc = reg.get("codex").unwrap();
        let root = json!({
            "mcp_servers": {
                "miro": {
                    "command": "npx",
                    "args": ["-y", "@mirohq/mcp-server"],
                    "startup_timeout_sec": 20
                },
                "figma": { "url": "https://mcp.figma.com/mcp" }
            }
        });
        let servers = extract_servers(desc, &root);
        let miro = &servers.iter().find(|(n, _)| n == "miro").unwrap().1;
        assert_eq!(miro.extra["codex"]["startup_timeout_sec"], json!(20));
        assert_eq!(miro.extra["codex"].len(), 1, "mapped keys stay canonical");
        let figma = &servers.iter().find(|(n, _)| n == "figma").unwrap().1;
        assert!(figma.extra.is_empty(), "no extras → no extra table");
    }

    #[test]
    fn transport_tag_is_not_lifted_into_extras() {
        let reg = Registry::load().unwrap();
        let desc = reg.get("claude-code").unwrap();
        let root = json!({
            "mcpServers": {
                "k": { "type": "http", "url": "https://k", "custom_key": true }
            }
        });
        let servers = extract_servers(desc, &root);
        let k = &servers[0].1;
        assert_eq!(k.extra["claude-code"]["custom_key"], json!(true));
        assert!(!k.extra["claude-code"].contains_key("type"));
    }

    #[test]
    fn extracts_codex_renamed_headers() {
        let reg = Registry::load().unwrap();
        let desc = reg.get("codex").unwrap();
        let root = json!({
            "mcp_servers": {
                "kibana_mcp": { "url": "https://k", "http_headers": { "Authorization": "Bearer x" } }
            }
        });
        let servers = extract_servers(desc, &root);
        assert_eq!(servers.len(), 1);
        let (name, s) = &servers[0];
        assert_eq!(name, "kibana_mcp");
        assert_eq!(s.server_type, ServerType::Http);
        assert_eq!(s.headers["Authorization"], "Bearer x");
    }
}
