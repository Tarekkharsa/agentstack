//! Runtime MCP gateway. Connects to the MCP servers a project's manifest
//! declares — HTTP and stdio — and re-exposes their tools through
//! `agentstack mcp`, so the manifest plus a one-time registration is enough —
//! no `apply`, nothing written into a native config, secrets resolved
//! per-machine at call time.
//!
//! Scope, deliberately bounded:
//! - The manifest is resolved once per launch — one project per process. No cwd
//!   watching and no `tools/list_changed`; a new project means a new launch.
//! - Discovery is lazy (on first `tools/list`) with a per-server timeout and
//!   partial results: an upstream that's slow or down is skipped, not fatal.
//! - Upstream tool descriptions are forwarded with a `[via <server>]` provenance
//!   prefix and a length cap — the manifest is the allowlist; this is a first
//!   guard against tool-poisoning via aggregated descriptions.
//!
//! Stdio lifecycle: a stdio server's child process is spawned lazily on the
//! first message that needs it, in its own process group (the same pattern as
//! `agentstack run`). Dropping the gateway — the MCP session ending — closes the
//! child's stdin (EOF, the polite MCP shutdown), then SIGTERMs and finally
//! SIGKILLs the whole group, so nothing it spawned outlives the session. If
//! agentstack itself dies without cleanup (`kill -9`), the kernel closes the
//! pipes and a well-behaved MCP server exits on stdin EOF. Secrets are resolved
//! into the child's env at spawn time and are never logged.

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};

use crate::manifest::ServerType;

const TIMEOUT: Duration = Duration::from_secs(5);
const PROTOCOL: &str = "2025-06-18";
const DESC_CAP: usize = 600;

/// How long a stdio tool call may run. Generous: upstream tools do real work
/// (searches, builds); the HTTP side's 5s request timeout is its own bound.
const STDIO_CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// How long a stdio server gets from spawn to its `initialize` response —
/// `npx`-style servers may install on first run. Env-overridable so tests can
/// exercise the timeout path without waiting out the real default.
fn stdio_start_timeout() -> Duration {
    std::env::var("AGENTSTACK_STDIO_START_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .map(Duration::from_millis)
        .unwrap_or(Duration::from_secs(10))
}

/// One upstream MCP server behind either transport.
pub struct Upstream {
    pub name: String,
    /// `${REF}`s in this server's URL/headers/env/args that did not resolve on
    /// this machine. A call is refused (with a clear message) rather than sent
    /// with a literal `${REF}` that would fail upstream as a confusing auth
    /// error.
    unresolved: Vec<String>,
    transport: Transport,
    initialized: RefCell<bool>,
    next_id: Cell<i64>,
}

enum Transport {
    Http(HttpTransport),
    Stdio(StdioTransport),
}

/// Minimal Streamable-HTTP JSON-RPC client.
struct HttpTransport {
    url: String,
    headers: Vec<(String, String)>,
    client: reqwest::blocking::Client,
    session: RefCell<Option<String>>,
}

/// A stdio child-process JSON-RPC client (one line = one message). The child is
/// spawned lazily; a dedicated reader thread parses its stdout into a channel so
/// requests can wait with a deadline instead of blocking forever.
struct StdioTransport {
    command: String,
    args: Vec<String>,
    env: Vec<(String, String)>,
    /// Working directory the child is spawned in: the server's manifest `cwd`
    /// (relative paths anchor at the project root) or the project root itself.
    /// Never the gateway's own cwd — that depends on where the client launched
    /// `agentstack mcp`, so relative commands/args would resolve unpredictably.
    cwd: std::path::PathBuf,
    child: RefCell<Option<StdioChild>>,
}

struct StdioChild {
    proc: std::process::Child,
    /// `Some` for the child's whole working life; taken (closed → EOF) first
    /// during Drop, because stdin EOF is the polite MCP shutdown signal.
    stdin: Option<std::process::ChildStdin>,
    rx: std::sync::mpsc::Receiver<Value>,
}

impl StdioChild {
    /// Poll `try_wait` for up to `dur`; true once the child has exited.
    fn wait_for_exit(&mut self, dur: Duration) -> bool {
        let deadline = Instant::now() + dur;
        loop {
            if matches!(self.proc.try_wait(), Ok(Some(_))) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for StdioChild {
    fn drop(&mut self) {
        // Escalation ladder: stdin EOF (polite MCP shutdown) → SIGTERM to the
        // process group → SIGKILL to the group. The child is its own group
        // leader (see spawn), so anything *it* spawned goes too — the same
        // tree-kill contract as `agentstack kill`.
        drop(self.stdin.take());
        if self.wait_for_exit(Duration::from_millis(200)) {
            return;
        }
        #[cfg(unix)]
        {
            let pid = self.proc.id() as i32;
            unsafe { libc::kill(-pid, libc::SIGTERM) };
            if self.wait_for_exit(Duration::from_millis(300)) {
                return;
            }
            unsafe { libc::kill(-pid, libc::SIGKILL) };
        }
        #[cfg(not(unix))]
        {
            let _ = self.proc.kill();
        }
        let _ = self.proc.wait();
    }
}

impl StdioTransport {
    fn spawn(&self) -> Result<StdioChild> {
        let mut cmd = std::process::Command::new(&self.command);
        cmd.args(&self.args)
            .envs(self.env.iter().cloned())
            .current_dir(&self.cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            // The child's stderr flows to ours: `agentstack mcp` keeps the
            // protocol on the real stdout, so this is debug-visible and safe.
            .stderr(std::process::Stdio::inherit());
        #[cfg(unix)]
        unsafe {
            use std::os::unix::process::CommandExt;
            // setpgid(0, 0): own process group, so Drop can tree-kill it.
            cmd.pre_exec(|| {
                if libc::setpgid(0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut proc = cmd
            .spawn()
            .with_context(|| format!("spawning '{}' in {}", self.command, self.cwd.display()))?;
        let stdin = Some(proc.stdin.take().expect("piped stdin"));
        let stdout = proc.stdout.take().expect("piped stdout");
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            use std::io::BufRead;
            for line in std::io::BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                // Skip non-JSON stdout noise; only JSON-RPC frames go through.
                if let Ok(v) = serde_json::from_str::<Value>(&line) {
                    if tx.send(v).is_err() {
                        break;
                    }
                }
            }
        });
        Ok(StdioChild { proc, stdin, rx })
    }

    /// Send one JSON-RPC message; for a request (has an id), wait for the
    /// matching response until `timeout`. Server-initiated notifications and
    /// stale replies are skipped, not fatal.
    fn request(&self, body: &Value, timeout: Duration) -> Result<Option<Value>> {
        let mut slot = self.child.borrow_mut();
        let mut child = match slot.take() {
            Some(c) => c,
            None => self.spawn()?,
        };
        use std::io::Write;
        let line = serde_json::to_string(body)?;
        let stdin = child.stdin.as_mut().expect("stdin open until drop");
        if let Err(e) = writeln!(stdin, "{line}").and_then(|()| stdin.flush()) {
            // Dead child: drop it (reaps + group-kills); the next call respawns.
            drop(child);
            anyhow::bail!("server process is not accepting input: {e}");
        }
        let Some(id) = body.get("id").cloned() else {
            *slot = Some(child);
            return Ok(None);
        };
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match child.rx.recv_timeout(remaining) {
                Ok(msg) => {
                    if msg.get("id") == Some(&id) && msg.get("method").is_none() {
                        *slot = Some(child);
                        return Ok(Some(msg));
                    }
                    // A notification, a server-initiated request, or a stale
                    // reply to a timed-out call — skip and keep waiting.
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    // Keep the child: a slow tool call is not a dead server.
                    *slot = Some(child);
                    anyhow::bail!("no response after {}s", timeout.as_secs());
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    drop(child);
                    anyhow::bail!("server process exited");
                }
            }
        }
    }
}

impl Upstream {
    fn http(
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
            unresolved,
            transport: Transport::Http(HttpTransport {
                url,
                headers,
                client,
                session: RefCell::new(None),
            }),
            initialized: RefCell::new(false),
            next_id: Cell::new(1),
        })
    }

    fn stdio(
        name: String,
        command: String,
        args: Vec<String>,
        env: Vec<(String, String)>,
        cwd: std::path::PathBuf,
        unresolved: Vec<String>,
    ) -> Self {
        Self {
            name,
            unresolved,
            transport: Transport::Stdio(StdioTransport {
                command,
                args,
                env,
                cwd,
                child: RefCell::new(None),
            }),
            initialized: RefCell::new(false),
            next_id: Cell::new(1),
        }
    }

    /// Send one JSON-RPC message over whichever transport. `None` for an
    /// accepted notification with no body. `timeout` bounds stdio waits; the
    /// HTTP client carries its own request timeout.
    fn send(&self, body: &Value, timeout: Duration) -> Result<Option<Value>> {
        match &self.transport {
            Transport::Http(h) => h.post(&self.name, body),
            Transport::Stdio(s) => s
                .request(body, timeout)
                .with_context(|| format!("contacting {}", self.name)),
        }
    }

    fn rpc(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        let timeout = if method == "initialize" {
            stdio_start_timeout()
        } else {
            STDIO_CALL_TIMEOUT
        };
        let body = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let resp = self
            .send(&body, timeout)?
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
        let _ = self.send(
            &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
            STDIO_CALL_TIMEOUT,
        );
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

impl HttpTransport {
    /// POST a JSON-RPC message; parse a JSON or SSE response. `None` for an
    /// accepted notification with no body.
    fn post(&self, name: &str, body: &Value) -> Result<Option<Value>> {
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
            .with_context(|| format!("contacting {name}"))?;
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
}

/// All upstreams a manifest declares, plus a discovered-tools cache, the
/// project's `[policy.tools]` firewall rules, and audit-log context.
pub struct Gateway {
    upstreams: Vec<Upstream>,
    cache: RefCell<Option<Vec<Value>>>,
    policy: crate::manifest::Policy,
    project: Option<String>,
}

impl Gateway {
    /// Build from the manifest at `dir`, resolving `${REF}`s in URLs, headers,
    /// commands, args, and env from the live resolver. Best-effort: returns an
    /// empty gateway if the manifest can't load. HTTP and stdio servers are both
    /// proxied; stdio children spawn lazily, on the first message that needs one.
    pub fn from_manifest(dir: Option<&std::path::Path>) -> Gateway {
        let mut upstreams = Vec::new();
        if let Ok(ctx) = crate::commands::load(dir) {
            // When a session is active, fence the proxied surface to that
            // session's profile servers — the same fence a profile already puts
            // on skills, extended to runtime tools (PLAN code-mode Phase 3).
            let session = crate::session::active(&ctx.dir);
            let fence = server_allowlist(
                &ctx.loaded.manifest,
                session.as_ref().map(|s| s.profile.as_str()),
            );
            if let Some(allow) = &fence {
                eprintln!(
                    "gateway: session active — proxying only this profile's {} server(s)",
                    allow.len()
                );
            }
            // `[policy.tools]` travels with the gateway — it is the firewall's
            // rule set; `project` is audit-log context.
            let policy = ctx.loaded.manifest.policy.clone();
            let project = Some(ctx.dir.display().to_string());
            for (name, s) in &ctx.loaded.manifest.servers {
                if fence.as_ref().is_some_and(|allow| !allow.contains(name)) {
                    continue;
                }
                // Collect any `${REF}`s that don't resolve here (across URL +
                // headers + args + env) so a call can fail fast with a clear
                // message instead of sending a literal `${REF}` upstream.
                let mut unresolved = Vec::new();
                let sub = |v: &str, unresolved: &mut Vec<String>| {
                    // The gateway resolves for an upstream request, not a diff —
                    // it doesn't display anything, so the redaction set is dropped.
                    let mut secrets = Vec::new();
                    crate::adapter::render::substitute(
                        v,
                        &ctx.resolver,
                        false,
                        unresolved,
                        &mut secrets,
                    )
                };
                let up = match s.server_type {
                    ServerType::Http => {
                        let Some(url) = &s.url else { continue };
                        let url = sub(url, &mut unresolved);
                        let headers = s
                            .headers
                            .iter()
                            .map(|(k, v)| (k.clone(), sub(v, &mut unresolved)))
                            .collect();
                        unresolved.sort();
                        unresolved.dedup();
                        match Upstream::http(name.clone(), url, headers, unresolved) {
                            Ok(u) => u,
                            Err(e) => {
                                eprintln!("gateway: skipping '{name}': {e}");
                                continue;
                            }
                        }
                    }
                    ServerType::Stdio => {
                        let Some(command) = &s.command else { continue };
                        let command = sub(command, &mut unresolved);
                        let args = s.args.iter().map(|a| sub(a, &mut unresolved)).collect();
                        let env = s
                            .env
                            .iter()
                            .map(|(k, v)| (k.clone(), sub(v, &mut unresolved)))
                            .collect();
                        // Manifest `cwd` (relative paths anchor at the project
                        // root — `join` keeps absolute ones as-is), defaulting
                        // to the project root. Mirrors what a rendered config
                        // gives a harness, whose own cwd is the project root.
                        let cwd = match &s.cwd {
                            Some(c) => ctx.dir.join(sub(c, &mut unresolved)),
                            None => ctx.dir.clone(),
                        };
                        unresolved.sort();
                        unresolved.dedup();
                        Upstream::stdio(name.clone(), command, args, env, cwd, unresolved)
                    }
                };
                upstreams.push(up);
            }
            return Gateway {
                upstreams,
                cache: RefCell::new(None),
                policy,
                project,
            };
        }
        Gateway {
            upstreams,
            cache: RefCell::new(None),
            policy: crate::manifest::Policy::default(),
            project: None,
        }
    }

    /// An empty gateway (no upstreams) — used as a default and in tests.
    pub fn empty() -> Gateway {
        Gateway {
            upstreams: Vec::new(),
            cache: RefCell::new(Some(Vec::new())),
            policy: crate::manifest::Policy::default(),
            project: None,
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
                        // The firewall filters discovery too: a policy-denied
                        // tool is invisible, not just refusable — it never
                        // reaches tools_search or code-mode bindings.
                        let bare = t.get("name").and_then(Value::as_str).unwrap_or("");
                        if self.policy.tool_allowed(&up.name, bare).is_err() {
                            continue;
                        }
                        out.push(namespace_tool(&up.name, &t));
                    }
                }
                Err(e) => eprintln!("gateway: '{}' unavailable, skipping: {e}", up.name),
            }
        }
        *self.cache.borrow_mut() = Some(out.clone());
        out
    }

    /// If `name` is `<server>__<tool>` and we own that server, forward the
    /// call — after the `[policy.tools]` firewall, and with every outcome
    /// (ok / error / denied) appended to the audit log.
    pub fn try_call(&self, name: &str, args: &Value) -> Option<Result<Value>> {
        let (server, tool) = name.split_once("__")?;
        let up = self.upstreams.iter().find(|u| u.name == server)?;
        let started = Instant::now();
        if let Err(rule) = self.policy.tool_allowed(server, tool) {
            self.log_call(server, tool, args, "denied", Some(&rule), started);
            return Some(Err(anyhow!("{server}__{tool}: call refused — {rule}")));
        }
        let result = up.call_tool(tool, args.clone());
        match &result {
            Ok(_) => self.log_call(server, tool, args, "ok", None, started),
            Err(e) => self.log_call(server, tool, args, "error", Some(&e.to_string()), started),
        }
        Some(result)
    }

    /// Append one audit record (best-effort; never fails the call). Only the
    /// argument *digest* is stored — never values, never resolved secrets.
    fn log_call(
        &self,
        server: &str,
        tool: &str,
        args: &Value,
        outcome: &str,
        detail: Option<&str>,
        started: Instant,
    ) {
        crate::calllog::record(&crate::calllog::CallRecord {
            ts: crate::calllog::now_epoch(),
            run: std::env::var(crate::calllog::RUN_ID_ENV).ok(),
            pid: std::process::id(),
            project: self.project.clone(),
            server: server.to_string(),
            tool: tool.to_string(),
            args_digest: crate::calllog::digest_args(args),
            outcome: outcome.to_string(),
            detail: detail.map(|d| {
                let mut d = d.to_string();
                d.truncate(200);
                d
            }),
            ms: started.elapsed().as_millis() as u64,
        });
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
            policy: crate::manifest::Policy::default(),
            project: None,
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

/// The proxied-server allowlist for the current context. `None` (no active
/// session, or its profile is gone) means no fence — every manifest server is
/// proxied. `Some(set)` restricts the proxied surface to that profile's servers.
fn server_allowlist(
    manifest: &crate::manifest::Manifest,
    active_profile: Option<&str>,
) -> Option<std::collections::HashSet<String>> {
    let profile = manifest.profiles.get(active_profile?)?;
    Some(profile.servers.iter().cloned().collect())
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
    fn session_fences_proxied_servers_to_profile() {
        let manifest: crate::manifest::Manifest = toml::from_str(
            "version = 1\n\
             [servers.alpha]\ntype = \"http\"\nurl = \"https://a\"\n\
             [servers.beta]\ntype = \"http\"\nurl = \"https://b\"\n\
             [profiles.solo]\nservers = [\"alpha\"]\n",
        )
        .unwrap();
        // No session → no fence (every server proxied).
        assert!(server_allowlist(&manifest, None).is_none());
        // Active session on `solo` → only its server is allowed.
        let allow = server_allowlist(&manifest, Some("solo")).unwrap();
        assert!(allow.contains("alpha"));
        assert!(!allow.contains("beta"));
        // A profile that vanished → no fence rather than fencing to nothing.
        assert!(server_allowlist(&manifest, Some("ghost")).is_none());
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
