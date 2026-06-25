//! Reverse of [`render`](super::render): read an existing CLI config (already
//! parsed to a JSON-shaped value tree) and recover manifest [`Server`]s, using
//! the same adapter descriptor that drives rendering. Values are recovered
//! verbatim; secret-lifting is a separate policy step in `init`.

use indexmap::IndexMap;
use serde_json::Value;

use super::descriptor::AdapterDescriptor;
use crate::manifest::{Server, ServerType};

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
    let Some(section) = navigate(root, &desc.mcp.location).and_then(Value::as_object) else {
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

        let url = get_str(&desc.mcp.fields.url);
        let command = get_str(&desc.mcp.fields.command);
        let args = desc
            .mcp
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
        let headers = get_map(&desc.mcp.fields.headers);
        let env = get_map(&desc.mcp.fields.env);

        let server_type = infer_type(desc, obj, &url, &command);

        // Skip entries that are neither http nor stdio shaped.
        if url.is_none() && command.is_none() {
            continue;
        }

        out.push((
            name.clone(),
            Server {
                server_type,
                url,
                command,
                args,
                headers,
                env,
            },
        ));
    }
    out
}

/// Determine transport: prefer an explicit tag (Claude's `type`), else infer
/// from which fields are present.
fn infer_type(
    desc: &AdapterDescriptor,
    obj: &serde_json::Map<String, Value>,
    url: &Option<String>,
    command: &Option<String>,
) -> ServerType {
    if let Some(t) = &desc.mcp.transport {
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
    use crate::adapter::Registry;
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
