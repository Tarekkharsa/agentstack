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
    /// `${REF}`s in this server's URL/headers that did not resolve on this
    /// machine. A call is refused (with a clear message) rather than sent with a
    /// literal `${REF}` that would fail upstream as a confusing auth error.
    unresolved: Vec<String>,
    client: reqwest::blocking::Client,
    session: RefCell<Option<String>>,
    initialized: RefCell<bool>,
    next_id: Cell<i64>,
}

impl Upstream {
    fn new(
        name: String,
        url: String,
        headers: Vec<(String, String)>,
        unresolved: Vec<String>,
    ) -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .timeout(TIMEOUT)
            .build()?;
        Ok(Self {
            name,
            url,
            headers,
            unresolved,
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
            let msg = err
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("error");
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
        if !self.unresolved.is_empty() {
            anyhow::bail!(
                "{}: cannot call '{tool}' — secret(s) did not resolve on this machine: {}. Set them with `agentstack secret set`.",
                self.name,
                self.unresolved.join(", ")
            );
        }
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
                // Collect any `${REF}`s that don't resolve here (across URL +
                // headers) so a call can fail fast with a clear message instead
                // of sending a literal `${REF}` upstream.
                let mut unresolved = Vec::new();
                let url =
                    crate::adapter::render::substitute(url, &ctx.resolver, false, &mut unresolved);
                let headers = s
                    .headers
                    .iter()
                    .map(|(k, v)| {
                        (
                            k.clone(),
                            crate::adapter::render::substitute(
                                v,
                                &ctx.resolver,
                                false,
                                &mut unresolved,
                            ),
                        )
                    })
                    .collect();
                unresolved.sort();
                unresolved.dedup();
                match Upstream::new(name.clone(), url, headers, unresolved) {
                    Ok(u) => upstreams.push(u),
                    Err(e) => eprintln!("gateway: skipping '{name}': {e}"),
                }
            }
            if skipped_stdio > 0 {
                eprintln!(
                    "gateway: {skipped_stdio} stdio server(s) not yet proxied (v1 is HTTP-only)"
                );
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

    /// Rank the proxied tools against `query`, returning at most `limit` hits.
    ///
    /// Ranking v1 is deliberately boring and deterministic — no embeddings. We
    /// score each cached namespaced tool by substring/token matches over its
    /// bare tool name, its server name, and its (provenance-prefixed,
    /// length-capped) description, weighting an exact tool-name hit highest and a
    /// description hit lowest. Ties break alphabetically by namespaced name so
    /// the same query always yields the same order. An empty query lists every
    /// tool alphabetically.
    pub fn search(&self, query: &str, limit: usize) -> Vec<Hit> {
        let tools = self.namespaced_tools();
        let q = query.trim().to_lowercase();
        let tokens: Vec<&str> = q.split_whitespace().collect();
        let mut scored: Vec<(i32, Hit)> = Vec::new();
        for t in &tools {
            let name = t.get("name").and_then(Value::as_str).unwrap_or("");
            let desc = t.get("description").and_then(Value::as_str).unwrap_or("");
            let (server, bare) = name.split_once("__").unwrap_or(("", name));
            let score = score_tool(bare, server, desc, &tokens);
            if !tokens.is_empty() && score == 0 {
                continue;
            }
            scored.push((
                score,
                Hit {
                    name: name.to_string(),
                    summary: desc.to_string(),
                    entity: format!("{name}:tool"),
                },
            ));
        }
        // Higher score first; alphabetical name as a stable tiebreaker.
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(&b.1.name)));
        scored.into_iter().take(limit).map(|(_, h)| h).collect()
    }

    /// Full detail for one proxied tool, addressed by its entity ref
    /// (`<server>__<tool>:tool`, the form `search` emits). Returns the upstream's
    /// raw `inputSchema` plus provenance (the source server). `None` if no
    /// proxied tool matches the entity.
    pub fn describe(&self, entity: &str) -> Option<ToolDetail> {
        // The entity is `<name>:tool`; tolerate a bare name without the suffix.
        let name = entity.strip_suffix(":tool").unwrap_or(entity);
        let (server, tool) = name.split_once("__")?;
        let tools = self.namespaced_tools();
        let t = tools
            .iter()
            .find(|t| t.get("name").and_then(Value::as_str) == Some(name))?;
        Some(ToolDetail {
            name: name.to_string(),
            server: server.to_string(),
            tool: tool.to_string(),
            description: t
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            input_schema: t
                .get("inputSchema")
                .cloned()
                .unwrap_or_else(|| json!({ "type": "object" })),
        })
    }

    /// The proxied servers and, for each, the `${REF}`s that did not resolve on
    /// this machine. Drives the `codemode` command's secret-health summary and
    /// the `explain` view of the runtime surface.
    pub fn proxied_servers(&self) -> Vec<(String, Vec<String>)> {
        self.upstreams
            .iter()
            .map(|u| (u.name.clone(), u.unresolved.clone()))
            .collect()
    }

    /// Generate the code-mode client (typed TS client + runtime shim) for the
    /// proxied surface. Secret-free: the client only carries tool names; secrets
    /// are resolved here, per call, when the shim forwards through `try_call`.
    pub fn generate_bindings(&self) -> crate::codemode::Bindings {
        crate::codemode::Bindings {
            client_ts: crate::codemode::render_client(&self.namespaced_tools()),
            runtime_ts: crate::codemode::runtime_shim().to_string(),
        }
    }

    /// Build a gateway directly from a pre-discovered namespaced tool list,
    /// bypassing the network. Test-only fixture seam.
    #[cfg(test)]
    pub(crate) fn with_tools(tools: Vec<Value>) -> Gateway {
        Gateway {
            upstreams: Vec::new(),
            cache: RefCell::new(Some(tools)),
        }
    }
}

/// One ranked discovery result: the namespaced tool name, its one-line summary
/// (the capped, `[via <server>]`-prefixed description), and the entity ref to
/// inspect it with.
pub struct Hit {
    pub name: String,
    pub summary: String,
    pub entity: String,
}

/// Full detail for a single proxied tool, as returned by [`Gateway::describe`].
pub struct ToolDetail {
    pub name: String,
    pub server: String,
    pub tool: String,
    pub description: String,
    pub input_schema: Value,
}

/// Deterministic relevance score for one tool against the query tokens. Weights:
/// exact bare-name match 10, bare-name substring 3, server-name substring 2,
/// description substring 1. Summed over tokens.
fn score_tool(bare: &str, server: &str, desc: &str, tokens: &[&str]) -> i32 {
    let bare = bare.to_lowercase();
    let server = server.to_lowercase();
    let desc = desc.to_lowercase();
    let mut score = 0;
    for tok in tokens {
        if bare == *tok {
            score += 10;
        }
        if bare.contains(tok) {
            score += 3;
        }
        if server.contains(tok) {
            score += 2;
        }
        if desc.contains(tok) {
            score += 1;
        }
    }
    score
}

fn namespace_tool(server: &str, tool: &Value) -> Value {
    let bare = tool.get("name").and_then(Value::as_str).unwrap_or("tool");
    let mut desc = format!(
        "[via {server}] {}",
        tool.get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
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
        assert!(n["description"]
            .as_str()
            .unwrap()
            .starts_with("[via figma] "));
        assert!(n["description"].as_str().unwrap().chars().count() <= DESC_CAP + 13);
    }

    #[test]
    fn parses_sse_payload() {
        let body =
            "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ok\":true}}\n\n";
        assert_eq!(parse_sse(body).unwrap()["result"]["ok"], true);
    }

    /// A small fixture of two upstreams' worth of namespaced tools, shaped exactly
    /// like `namespaced_tools()` produces.
    fn fixture_tools() -> Vec<Value> {
        vec![
            namespace_tool(
                "figma",
                &json!({ "name": "get_file", "description": "Get a file's node tree.", "inputSchema": { "type": "object", "properties": { "fileKey": { "type": "string" } } } }),
            ),
            namespace_tool(
                "figma",
                &json!({ "name": "create_frame", "description": "Create a new frame on the canvas." }),
            ),
            namespace_tool(
                "github",
                &json!({ "name": "list_issues", "description": "List issues in a repository." }),
            ),
        ]
    }

    #[test]
    fn search_ranks_known_server_for_query() {
        let gw = Gateway::with_tools(fixture_tools());
        let hits = gw.search("file", 10);
        assert_eq!(hits.first().unwrap().name, "figma__get_file");
        assert_eq!(hits.first().unwrap().entity, "figma__get_file:tool");
        // server-name query surfaces that server's tools
        let gh = gw.search("github", 10);
        assert!(gh.iter().all(|h| h.name.starts_with("github__")));
        assert_eq!(gh.len(), 1);
    }

    #[test]
    fn search_respects_limit_and_empty_query() {
        let gw = Gateway::with_tools(fixture_tools());
        // empty query lists everything, alphabetical, bounded by limit
        let all = gw.search("", 2);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].name, "figma__create_frame");
        // no match → no hits
        assert!(gw.search("nonexistent-xyz", 10).is_empty());
    }

    #[test]
    fn describe_returns_schema_and_provenance() {
        let gw = Gateway::with_tools(fixture_tools());
        let d = gw.describe("figma__get_file:tool").unwrap();
        assert_eq!(d.server, "figma");
        assert_eq!(d.tool, "get_file");
        assert_eq!(d.input_schema["properties"]["fileKey"]["type"], "string");
        // a bare name (no `:tool` suffix) also resolves
        assert!(gw.describe("github__list_issues").is_some());
        // unknown entity → None
        assert!(gw.describe("figma__nope:tool").is_none());
        assert!(gw.describe("garbage").is_none());
    }
}
