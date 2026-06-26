//! Runtime MCP gateway (v1). Connects to the HTTP MCP servers a project's
//! manifest declares and re-exposes their tools through `agentstack mcp`, so the
//! manifest plus a one-time registration is enough — no `apply`, nothing written
//! into a native config, secrets resolved per-machine at call time.
//!
//! Scope of v1, deliberately bounded:
//! - HTTP servers only (stdio child-process servers are a follow-up).
//! - The manifest is resolved once per launch — one project per process. No cwd
//!   watching and no `tools/list_changed`; a new project means a new launch.
//! - Discovery is lazy (on first `tools/list`) with a per-server timeout and
//!   partial results: an upstream that's slow or down is skipped, not fatal.
//! - Upstream tool descriptions are forwarded with a `[via <server>]` provenance
//!   prefix and a length cap — the manifest is the allowlist; this is a first
//!   guard against tool-poisoning via aggregated descriptions.

use std::cell::{Cell, RefCell};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};

use crate::manifest::ServerType;

const TIMEOUT: Duration = Duration::from_secs(5);
const PROTOCOL: &str = "2025-06-18";
const DESC_CAP: usize = 600;

/// One upstream HTTP MCP server, with a minimal Streamable-HTTP JSON-RPC client.
pub struct Upstream {
    pub name: String,
    url: String,
    headers: Vec<(String, String)>,
    client: reqwest::blocking::Client,
    session: RefCell<Option<String>>,
    initialized: RefCell<bool>,
    next_id: Cell<i64>,
}

impl Upstream {
    fn new(name: String, url: String, headers: Vec<(String, String)>) -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .timeout(TIMEOUT)
            .build()?;
        Ok(Self {
            name,
            url,
            headers,
            client,
            session: RefCell::new(None),
            initialized: RefCell::new(false),
            next_id: Cell::new(1),
        })
    }

    /// POST a JSON-RPC message; parse a JSON or SSE response. `None` for an
    /// accepted notification with no body.
    fn post(&self, body: &Value) -> Result<Option<Value>> {
        let mut req = self
            .client
            .post(&self.url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", PROTOCOL);
        for (k, v) in &self.headers {
            req = req.header(k, v);
        }
        if let Some(sid) = self.session.borrow().as_ref() {
            req = req.header("Mcp-Session-Id", sid);
        }
        let resp = req
            .json(body)
            .send()
            .with_context(|| format!("contacting {}", self.name))?;
        if let Some(sid) = resp
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
        {
            *self.session.borrow_mut() = Some(sid.to_string());
        }
        let ctype = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let text = resp.text()?;
        if text.trim().is_empty() {
            return Ok(None);
        }
        let val = if ctype.contains("text/event-stream") {
            parse_sse(&text)
        } else {
            serde_json::from_str(&text).ok()
        };
        Ok(val)
    }

    fn rpc(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        let body = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let resp = self
            .post(&body)?
            .ok_or_else(|| anyhow!("{}: empty response to {method}", self.name))?;
        if let Some(err) = resp.get("error") {
            let msg = err.get("message").and_then(Value::as_str).unwrap_or("error");
            anyhow::bail!("{}: {msg}", self.name);
        }
        Ok(resp.get("result").cloned().unwrap_or(Value::Null))
    }

    fn ensure_init(&self) -> Result<()> {
        if *self.initialized.borrow() {
            return Ok(());
        }
        self.rpc(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL,
                "capabilities": {},
                "clientInfo": { "name": "agentstack-gateway", "version": env!("CARGO_PKG_VERSION") }
            }),
        )?;
        let _ = self.post(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        *self.initialized.borrow_mut() = true;
        Ok(())
    }

    fn list_tools(&self) -> Result<Vec<Value>> {
        self.ensure_init()?;
        let r = self.rpc("tools/list", json!({}))?;
        Ok(r.get("tools")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    fn call_tool(&self, tool: &str, args: Value) -> Result<Value> {
        self.ensure_init()?;
        self.rpc("tools/call", json!({ "name": tool, "arguments": args }))
    }
}

/// All HTTP upstreams a manifest declares, plus a discovered-tools cache.
pub struct Gateway {
    upstreams: Vec<Upstream>,
    cache: RefCell<Option<Vec<Value>>>,
}

impl Gateway {
    /// Build from the manifest at `dir`, resolving `${REF}`s in URLs/headers from
    /// the live resolver. Best-effort: returns an empty gateway if the manifest
    /// can't load. Only HTTP servers are included in v1.
    pub fn from_manifest(dir: Option<&std::path::Path>) -> Gateway {
        let mut upstreams = Vec::new();
        if let Ok(ctx) = crate::commands::load(dir) {
            let mut skipped_stdio = 0;
            for (name, s) in &ctx.loaded.manifest.servers {
                if s.server_type != ServerType::Http {
                    skipped_stdio += 1;
                    continue;
                }
                let Some(url) = &s.url else { continue };
                let mut junk = Vec::new();
                let url = crate::adapter::render::substitute(url, &ctx.resolver, false, &mut junk);
                let headers = s
                    .headers
                    .iter()
                    .map(|(k, v)| {
                        (
                            k.clone(),
                            crate::adapter::render::substitute(v, &ctx.resolver, false, &mut junk),
                        )
                    })
                    .collect();
                match Upstream::new(name.clone(), url, headers) {
                    Ok(u) => upstreams.push(u),
                    Err(e) => eprintln!("gateway: skipping '{name}': {e}"),
                }
            }
            if skipped_stdio > 0 {
                eprintln!("gateway: {skipped_stdio} stdio server(s) not yet proxied (v1 is HTTP-only)");
            }
        }
        Gateway {
            upstreams,
            cache: RefCell::new(None),
        }
    }

    /// An empty gateway (no upstreams) — used as a default and in tests.
    pub fn empty() -> Gateway {
        Gateway {
            upstreams: Vec::new(),
            cache: RefCell::new(Some(Vec::new())),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.upstreams.is_empty()
    }

    /// Discover every upstream's tools, namespaced `<server>__<tool>`. Cached
    /// after the first call. Per-server failures are skipped (logged to stderr)
    /// so one slow/down server can't fail the whole list.
    pub fn namespaced_tools(&self) -> Vec<Value> {
        if let Some(cached) = self.cache.borrow().as_ref() {
            return cached.clone();
        }
        let mut out = Vec::new();
        for up in &self.upstreams {
            match up.list_tools() {
                Ok(tools) => {
                    for t in tools {
                        out.push(namespace_tool(&up.name, &t));
                    }
                }
                Err(e) => eprintln!("gateway: '{}' unavailable, skipping: {e}", up.name),
            }
        }
        *self.cache.borrow_mut() = Some(out.clone());
        out
    }

    /// If `name` is `<server>__<tool>` and we own that server, forward the call.
    pub fn try_call(&self, name: &str, args: &Value) -> Option<Result<Value>> {
        let (server, tool) = name.split_once("__")?;
        let up = self.upstreams.iter().find(|u| u.name == server)?;
        Some(up.call_tool(tool, args.clone()))
    }
}

fn namespace_tool(server: &str, tool: &Value) -> Value {
    let bare = tool.get("name").and_then(Value::as_str).unwrap_or("tool");
    let mut desc = format!(
        "[via {server}] {}",
        tool.get("description").and_then(Value::as_str).unwrap_or("")
    );
    if desc.len() > DESC_CAP {
        desc.truncate(DESC_CAP);
        desc.push('…');
    }
    json!({
        "name": format!("{server}__{bare}"),
        "description": desc,
        "inputSchema": tool.get("inputSchema").cloned().unwrap_or_else(|| json!({ "type": "object" })),
    })
}

/// Concatenate `data:` lines of an SSE body and parse the JSON-RPC message.
fn parse_sse(text: &str) -> Option<Value> {
    let mut data = String::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("data:") {
            data.push_str(rest.trim());
        }
    }
    serde_json::from_str(&data).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespaces_and_caps_descriptions() {
        let t = json!({ "name": "get_file", "description": "x".repeat(900), "inputSchema": { "type": "object" } });
        let n = namespace_tool("figma", &t);
        assert_eq!(n["name"], "figma__get_file");
        assert!(n["description"].as_str().unwrap().starts_with("[via figma] "));
        assert!(n["description"].as_str().unwrap().chars().count() <= DESC_CAP + 13);
    }

    #[test]
    fn parses_sse_payload() {
        let body = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ok\":true}}\n\n";
        assert_eq!(parse_sse(body).unwrap()["result"]["ok"], true);
    }
}
