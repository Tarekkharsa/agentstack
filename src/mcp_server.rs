//! `agentstack mcp` — exposes agentstack itself as an MCP server over stdio, so
//! the agent can discover and propose capabilities (PLAN §9g). Newline-delimited
//! JSON-RPC.
//!
//! Trust gate (D20): writes go to the **manifest only** (commit-safe `${REF}`s,
//! nothing executed). The agent proposes; a human still runs `apply`.

use std::io::{BufRead, Write};
use std::path::Path;

use anyhow::{Context, Result};
use indexmap::IndexMap;
use serde_json::{json, Value};

use crate::catalog;
use crate::manifest::load::MANIFEST_FILE;
use crate::manifest::{Server, ServerType};
use crate::secret::Resolver;

const PROTOCOL_VERSION: &str = "2025-06-18";

pub fn serve(manifest_dir: Option<&Path>) -> Result<()> {
    let dir = manifest_dir.map(Path::to_path_buf);
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(req) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(resp) = handle(&req, dir.as_deref()) {
            writeln!(out, "{}", serde_json::to_string(&resp)?)?;
            out.flush()?;
        }
    }
    Ok(())
}

fn handle(req: &Value, dir: Option<&Path>) -> Option<Value> {
    let id = req.get("id").cloned();
    let method = req.get("method")?.as_str()?;
    match method {
        "initialize" => Some(result(
            id,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "agentstack", "version": env!("CARGO_PKG_VERSION") }
            }),
        )),
        "notifications/initialized" | "notifications/cancelled" => None,
        "tools/list" => Some(result(id, json!({ "tools": tool_defs() }))),
        "tools/call" => {
            let params = req.get("params").cloned().unwrap_or_else(|| json!({}));
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let (text, is_error) = match run_tool(name, &args, dir) {
                Ok(t) => (t, false),
                Err(e) => (format!("Error: {e}"), true),
            };
            Some(result(
                id,
                json!({ "content": [{ "type": "text", "text": text }], "isError": is_error }),
            ))
        }
        // Requests we don't implement → JSON-RPC error; notifications → silence.
        _ => id.map(|id| error(id, -32601, &format!("method not found: {method}"))),
    }
}

fn result(id: Option<Value>, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id.unwrap_or(Value::Null), "result": result })
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn tool_defs() -> Value {
    json!([
        {
            "name": "agentstack_search",
            "description": "Search the agentstack capability catalog for MCP servers by name, description, or tag. Returns matches with a ready-to-use add command.",
            "inputSchema": {
                "type": "object",
                "properties": { "query": { "type": "string", "description": "Free-text query" } }
            }
        },
        {
            "name": "agentstack_list",
            "description": "List the current agentstack manifest: servers, skills, profiles, and which secrets resolve on this machine (values are never returned).",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "agentstack_doctor",
            "description": "Summarize agentstack health: installed harnesses, server/skill counts, and resolved secrets.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "agentstack_add_server",
            "description": "Add an MCP server to the manifest (commit-safe — secrets stay as ${REF}). Does NOT apply; a human runs `agentstack apply` to render it.",
            "inputSchema": {
                "type": "object",
                "required": ["name"],
                "properties": {
                    "name": { "type": "string" },
                    "transport": { "type": "string", "enum": ["http", "stdio"], "default": "http" },
                    "url": { "type": "string" },
                    "command": { "type": "string" },
                    "args": { "type": "array", "items": { "type": "string" } },
                    "env": { "type": "object" },
                    "headers": { "type": "object" },
                    "profile": { "type": "string" }
                }
            }
        }
    ])
}

fn run_tool(name: &str, args: &Value, dir: Option<&Path>) -> Result<String> {
    match name {
        "agentstack_search" => Ok(search_text(
            args.get("query").and_then(Value::as_str).unwrap_or(""),
        )),
        "agentstack_list" => {
            let v = crate::dashboard::snapshot::build(dir)?;
            Ok(serde_json::to_string_pretty(&v)?)
        }
        "agentstack_doctor" => doctor_summary(dir),
        "agentstack_add_server" => add_server(args, dir),
        other => anyhow::bail!("unknown tool '{other}'"),
    }
}

fn search_text(query: &str) -> String {
    let results = catalog::search(query);
    if results.is_empty() {
        return format!("No catalog matches for '{query}'.");
    }
    let mut out = format!("{} match(es):\n", results.len());
    for e in results {
        out.push_str(&format!(
            "\n- {} [{}]: {}\n  tags: {}\n  add: {}\n",
            e.name,
            e.kind,
            e.description,
            e.tags.join(", "),
            e.add_command()
        ));
    }
    out
}

fn doctor_summary(dir: Option<&Path>) -> Result<String> {
    let ctx = crate::commands::load(dir)?;
    let m = &ctx.loaded.manifest;
    let installed = ctx.registry.iter().filter(|d| d.is_installed()).count();
    let refs = m.referenced_secrets();
    let resolved = refs
        .iter()
        .filter(|n| ctx.resolver.resolve(n).is_some())
        .count();
    Ok(format!(
        "Harnesses installed: {installed}/{}\nServers: {}\nSkills: {}\nSecrets resolved: {resolved}/{}",
        ctx.registry.ids().count(),
        m.servers.len(),
        m.skills.len(),
        refs.len()
    ))
}

fn add_server(args: &Value, dir: Option<&Path>) -> Result<String> {
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .context("`name` is required")?;
    let transport = args
        .get("transport")
        .and_then(Value::as_str)
        .unwrap_or("http");
    let server = Server {
        server_type: if transport == "stdio" {
            ServerType::Stdio
        } else {
            ServerType::Http
        },
        url: str_field(args, "url"),
        command: str_field(args, "command"),
        args: args
            .get("args")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        headers: obj_to_map(args.get("headers")),
        env: obj_to_map(args.get("env")),
    };
    match server.server_type {
        ServerType::Http if server.url.is_none() => anyhow::bail!("http server needs `url`"),
        ServerType::Stdio if server.command.is_none() => {
            anyhow::bail!("stdio server needs `command`")
        }
        _ => {}
    }

    let mdir = match dir {
        Some(d) => d.to_path_buf(),
        None => std::env::current_dir()?,
    };
    let manifest_path = mdir.join(MANIFEST_FILE);
    let original = std::fs::read_to_string(&manifest_path).with_context(|| {
        format!(
            "no manifest at {} (run `agentstack init`)",
            manifest_path.display()
        )
    })?;
    let parsed: crate::manifest::Manifest =
        toml::from_str(&original).context("parsing manifest")?;
    if parsed.servers.contains_key(name) {
        anyhow::bail!("server '{name}' already exists");
    }

    let body = serde_json::to_value(&server)?;
    let profile = args.get("profile").and_then(Value::as_str);
    let new_text =
        crate::commands::add::build_manifest_with(&original, "servers", name, &body, profile)?;
    std::fs::write(&manifest_path, &new_text)
        .with_context(|| format!("writing {}", manifest_path.display()))?;

    let secret_hint = if !server.headers.is_empty() || !server.env.is_empty() {
        " If it references a ${SECRET}, set it with `agentstack secret set`."
    } else {
        ""
    };
    Ok(format!(
        "Added server '{name}' to the manifest (not yet applied). A human should review and run `agentstack apply` to render it into the harnesses.{secret_hint}"
    ))
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(String::from)
}

fn obj_to_map(v: Option<&Value>) -> IndexMap<String, String> {
    v.and_then(Value::as_object)
        .map(|o| {
            o.iter()
                .filter_map(|(k, val)| val.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_returns_server_info() {
        let req = json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize" });
        let resp = handle(&req, None).unwrap();
        assert_eq!(resp["result"]["serverInfo"]["name"], "agentstack");
        assert_eq!(resp["id"], 1);
    }

    #[test]
    fn tools_list_includes_search_and_add() {
        let req = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" });
        let resp = handle(&req, None).unwrap();
        let names: Vec<&str> = resp["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"agentstack_search"));
        assert!(names.contains(&"agentstack_add_server"));
    }

    #[test]
    fn notifications_get_no_response() {
        let req = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        assert!(handle(&req, None).is_none());
    }

    #[test]
    fn search_tool_finds_github() {
        let req = json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": { "name": "agentstack_search", "arguments": { "query": "github" } }
        });
        let resp = handle(&req, None).unwrap();
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("github"));
        assert_eq!(resp["result"]["isError"], false);
    }
}
