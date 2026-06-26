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

use crate::manifest::load::MANIFEST_FILE;
use crate::manifest::{Server, ServerType};
use crate::secret::Resolver;
use crate::store::{local_source_dir, Store};

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
            "name": "agentstack_add_from",
            "description": "Add a capability discovered via agentstack_search (catalog name or official MCP Registry id) to the manifest, commit-safe. Does NOT apply; a human runs `agentstack apply`.",
            "inputSchema": {
                "type": "object",
                "required": ["id"],
                "properties": {
                    "id": { "type": "string", "description": "Catalog name or registry id from search" },
                    "profile": { "type": "string" }
                }
            }
        },
        {
            "name": "agentstack_list_loadable",
            "description": "List the skills you're allowed to load right now, each with a one-line description (the cheap catalog — not the full instructions). When a session is active the list is fenced to that session's profile. Call this first, read the descriptions, then load only what the task needs.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "agentstack_load",
            "description": "Load one skill by name for the rest of this session and return its full instructions. Only names from agentstack_list_loadable are allowed. Loads are sticky within a session and logged with your reason.",
            "inputSchema": {
                "type": "object",
                "required": ["name", "reason"],
                "properties": {
                    "name": { "type": "string", "description": "Skill name from agentstack_list_loadable" },
                    "reason": { "type": "string", "description": "Why this task needs it (recorded for replay)" }
                }
            }
        },
        {
            "name": "agentstack_add_server",
            "description": "Add an MCP server to the manifest by hand (commit-safe — secrets stay as ${REF}). Does NOT apply; a human runs `agentstack apply` to render it.",
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
        "agentstack_add_from" => add_from(args, dir),
        "agentstack_add_server" => add_server(args, dir),
        "agentstack_list_loadable" => list_loadable(dir),
        "agentstack_load" => load_capability(args, dir),
        other => anyhow::bail!("unknown tool '{other}'"),
    }
}

fn search_text(query: &str) -> String {
    let results = crate::provider::search_all(query, 20);
    if results.is_empty() {
        return format!("No matches for '{query}' (catalog or official MCP Registry).");
    }
    let mut out = format!("{} match(es):\n", results.len());
    for c in results {
        let add_id = if c.source == "catalog" {
            &c.name
        } else {
            &c.id
        };
        out.push_str(&format!(
            "\n- {} [{}]: {}\n  add: agentstack add from {}\n",
            c.name, c.source, c.description, add_id
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

fn add_from(args: &Value, dir: Option<&Path>) -> Result<String> {
    let id = args
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .context("`id` is required")?;
    let candidate = crate::provider::resolve(id)
        .with_context(|| format!("no capability '{id}' in the catalog or registry"))?;

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
    if parsed.servers.contains_key(&candidate.name) {
        anyhow::bail!("server '{}' already exists", candidate.name);
    }

    let body = serde_json::to_value(candidate.to_server())?;
    let profile = args.get("profile").and_then(Value::as_str);
    let new_text = crate::commands::add::build_manifest_with(
        &original,
        "servers",
        &candidate.name,
        &body,
        profile,
    )?;
    crate::util::atomic::write(&manifest_path, &new_text)
        .with_context(|| format!("writing {}", manifest_path.display()))?;

    Ok(format!(
        "Added '{}' (from {}) to the manifest (not yet applied). A human should review secrets and run `agentstack apply`.",
        candidate.name, candidate.source
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
    crate::util::atomic::write(&manifest_path, &new_text)
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

/// The skills loadable right now: fenced to the active session's profile, or —
/// when no session is active — the whole manifest (dev-open). This is the
/// progressive-disclosure catalog: names + one-line descriptions, not payloads.
fn loadable_skill_names(
    manifest: &crate::manifest::Manifest,
    session: Option<&crate::session::Session>,
) -> Vec<String> {
    match session.and_then(|s| manifest.profiles.get(&s.profile)) {
        Some(p) if p.loads_all_skills() => manifest.skills.keys().cloned().collect(),
        Some(p) => p
            .skills
            .iter()
            .filter(|n| manifest.skills.contains_key(*n))
            .cloned()
            .collect(),
        None => manifest.skills.keys().cloned().collect(),
    }
}

/// Read a skill's `SKILL.md` once; return (description, full body).
fn read_skill_md(source: &Path) -> (Option<String>, Option<String>) {
    let Ok(text) = std::fs::read_to_string(source.join("SKILL.md")) else {
        return (None, None);
    };
    let desc = parse_frontmatter_description(&text);
    (desc, Some(text))
}

fn parse_frontmatter_description(md: &str) -> Option<String> {
    let rest = md.trim_start().strip_prefix("---")?;
    let end = rest.find("\n---")?;
    for line in rest[..end].lines() {
        if let Some(v) = line.trim().strip_prefix("description:") {
            return Some(v.trim().trim_matches('"').trim_matches('\'').to_string());
        }
    }
    None
}

fn list_loadable(dir: Option<&Path>) -> Result<String> {
    let ctx = crate::commands::load(dir)?;
    let m = &ctx.loaded.manifest;
    let session = crate::session::active(&ctx.dir);
    let loaded: std::collections::HashSet<String> = session
        .as_ref()
        .map(|s| s.loads.iter().map(|l| l.name.clone()).collect())
        .unwrap_or_default();
    let store = Store::default_store();

    let mut entries = Vec::new();
    for name in loadable_skill_names(m, session.as_ref()) {
        let Some(skill) = m.skills.get(&name) else {
            continue;
        };
        let desc = local_source_dir(&store, skill, &ctx.dir)
            .and_then(|d| read_skill_md(&d).0)
            .unwrap_or_default();
        entries.push(json!({
            "name": name,
            "description": desc,
            "kind": "skill",
            "loaded": loaded.contains(&name),
        }));
    }
    Ok(serde_json::to_string_pretty(&json!({
        "loadable": entries,
        "fenced": session.is_some(),
        "session": session.as_ref().map(|s| s.profile.clone()),
        "note": if session.is_some() {
            "Fenced to this session's profile. Load only what the task needs."
        } else {
            "No active session — all manifest skills are loadable (dev-open). Start a session to fence + log loads."
        },
    }))?)
}

fn load_capability(args: &Value, dir: Option<&Path>) -> Result<String> {
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .context("`name` is required")?;
    let reason = args
        .get("reason")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .context("`reason` is required — say why this task needs the skill")?;

    let ctx = crate::commands::load(dir)?;
    let m = &ctx.loaded.manifest;
    let skill = m
        .skills
        .get(name)
        .with_context(|| format!("no skill '{name}' in the manifest"))?;

    let session = crate::session::active(&ctx.dir);
    // Fence: inside a session, only the profile's skills are loadable.
    if let Some(s) = &session {
        if !loadable_skill_names(m, Some(s)).iter().any(|n| n == name) {
            anyhow::bail!(
                "'{name}' is not loadable in session '{}' — add it to the profile to allow it",
                s.profile
            );
        }
    }

    let source = local_source_dir(&Store::default_store(), skill, &ctx.dir)
        .with_context(|| format!("skill '{name}' is not available locally — run `agentstack install`"))?;
    let (_, body) = read_skill_md(&source);
    let instructions = body.with_context(|| format!("skill '{name}' has no SKILL.md"))?;

    let newly = if session.is_some() {
        crate::session::record_load(&ctx.dir, name, reason)?
    } else {
        false
    };

    Ok(serde_json::to_string_pretty(&json!({
        "loaded": name,
        "instructions": instructions,
        "sticky": session.is_some(),
        "newly_loaded": newly,
        "fenced": session.is_some(),
    }))?)
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
        assert!(names.contains(&"agentstack_list_loadable"));
        assert!(names.contains(&"agentstack_load"));
    }

    #[test]
    fn frontmatter_description_parses() {
        let md = "---\nname: pdf\ndescription: Fill and merge PDFs.\n---\nbody";
        assert_eq!(
            parse_frontmatter_description(md).as_deref(),
            Some("Fill and merge PDFs.")
        );
        assert_eq!(parse_frontmatter_description("no frontmatter"), None);
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
