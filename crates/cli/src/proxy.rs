//! Wire proxy — the runtime, ground-truth companion to the static `footprint`
//! lens. Point a harness's `ANTHROPIC_BASE_URL` at this loopback proxy and it
//! relays every `/v1/messages` request VERBATIM to the real API while accounting
//! for what the on-wire `tools` block actually costs in input tokens per turn —
//! the same payload the harness re-sends on every turn. The numbers tie back
//! into agentstack's manifest/profiles: loaded-vs-called evidence, per-server
//! (per-capability) buckets, and demote-to-lazy / drop hints.
//!
//! Phase 1 is OBSERVE ONLY. The proxy never injects, never mutates the
//! tools/system block (that would bust the prompt-prefix cache), and never
//! delays or fails the proxied request on an accounting hiccup — telemetry is
//! strictly best-effort (same contract as `calllog::record` / `usage::bump`).
//!
//! Privacy: like `calllog`, this records only counts, capability names, token
//! estimates, the model id, best-effort usage numbers, and `tool_use` tool
//! NAMES. Never prompt/message bodies, tool arguments, secrets, or header
//! values.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tiny_http::{Header, Response, Server};

use crate::footprint;
use crate::util::paths;

/// Default loopback port the proxy listens on.
pub const DEFAULT_PORT: u16 = 8787;
/// Default upstream the proxy relays to.
pub const DEFAULT_UPSTREAM: &str = "https://api.anthropic.com";

/// At most two generations of ~5 MB, mirroring `calllog`.
const MAX_BYTES: u64 = 5 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct ProxyConfig {
    pub port: u16,
    pub upstream: String,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        ProxyConfig {
            port: DEFAULT_PORT,
            upstream: DEFAULT_UPSTREAM.to_string(),
        }
    }
}

/// What one capability's tools cost in the request's `tools` block.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapCost {
    pub tools: usize,
    pub est_tokens: u64,
}

/// The accounting derived (purely) from one `/v1/messages` request body.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequestAccounting {
    pub model: Option<String>,
    pub total_tools: usize,
    pub total_est_tokens: u64,
    pub per_capability: BTreeMap<String, CapCost>,
}

/// Bucket a tool name by capability: an `mcp__<server>__<tool>` name buckets to
/// its `<server>` segment; every other name (Read, Bash, Edit, Task, …) buckets
/// to `builtin`.
pub fn capability_of(tool_name: &str) -> String {
    match tool_name.strip_prefix("mcp__") {
        Some(rest) => {
            let server = rest.split("__").next().unwrap_or(rest);
            if server.is_empty() {
                "builtin".to_string()
            } else {
                server.to_string()
            }
        }
        None => "builtin".to_string(),
    }
}

/// Walk the request body's `tools` array and account per-capability token cost.
/// Pure and deterministic; an empty/missing/non-array `tools` yields zeros
/// without panicking.
pub fn account_request(body: &Value) -> RequestAccounting {
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .map(str::to_string);

    let mut acct = RequestAccounting {
        model,
        ..Default::default()
    };

    let Some(tools) = body.get("tools").and_then(Value::as_array) else {
        return acct;
    };

    for tool in tools {
        let name = tool.get("name").and_then(Value::as_str).unwrap_or("");
        let cap = capability_of(name);
        let chars = serde_json::to_string(tool).map(|s| s.len()).unwrap_or(0);
        let est = footprint::estimate_tokens(chars);

        acct.total_tools += 1;
        acct.total_est_tokens += est;
        let e = acct.per_capability.entry(cap).or_default();
        e.tools += 1;
        e.est_tokens += est;
    }

    acct
}

/// One proxied request, as recorded to the telemetry log. Content-free by
/// construction: names, counts, and token numbers only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestRecord {
    pub ts: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub total_tools: usize,
    pub total_est_tokens: u64,
    pub per_capability: BTreeMap<String, CapCost>,
    pub streamed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u64>,
    /// Tool NAMES that appeared in the response's `tool_use` blocks (best-effort;
    /// captured off the wire for both streamed (SSE) and non-streamed responses).
    #[serde(default)]
    pub tool_use: Vec<String>,
}

impl RequestRecord {
    fn from_accounting(acct: RequestAccounting, project: Option<String>) -> Self {
        RequestRecord {
            ts: footprint::now_epoch(),
            project,
            model: acct.model,
            total_tools: acct.total_tools,
            total_est_tokens: acct.total_est_tokens,
            per_capability: acct.per_capability,
            streamed: false,
            input_tokens: None,
            output_tokens: None,
            cache_read_input_tokens: None,
            tool_use: Vec::new(),
        }
    }
}

/// `~/.agentstack/proxy/requests.jsonl` — the append-only telemetry log.
pub fn log_path() -> std::path::PathBuf {
    paths::agentstack_home()
        .join("proxy")
        .join("requests.jsonl")
}

/// Append one record. Best-effort: any failure is swallowed so proxy telemetry
/// can never affect the request it describes.
pub fn record(rec: &RequestRecord) {
    let path = log_path();
    let Some(dir) = path.parent() else { return };
    if fs::create_dir_all(dir).is_err() {
        return;
    }
    // Size-capped rotation: current → .1 (previous generation dropped).
    if fs::metadata(&path)
        .map(|m| m.len() > MAX_BYTES)
        .unwrap_or(false)
    {
        let _ = fs::rename(&path, path.with_extension("jsonl.1"));
    }
    let Ok(line) = serde_json::to_string(rec) else {
        return;
    };
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{line}");
    }
}

/// Read the log, newest last. Unparseable lines are skipped (a torn write from a
/// crash must not brick the whole log).
pub fn read_all() -> Vec<RequestRecord> {
    let Ok(text) = fs::read_to_string(log_path()) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

// ── Report aggregation ──────────────────────────────────────────────────────

/// A ranked-report row for one capability, aggregated across all records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapReport {
    pub capability: String,
    /// Max tools this capability contributed to any single request (its typical
    /// per-turn tool count).
    pub tools: usize,
    /// Average estimated tokens/turn it costs across the requests it appeared in.
    pub avg_est_tokens: u64,
    /// How many times any of its tools appeared in a response's `tool_use`.
    pub calls: u64,
    /// Modest loaded-vs-called hint: `drop / lazy`, `keep`, or `watch`.
    pub hint: String,
}

/// The whole aggregate, ready to print or serialize.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub requests: usize,
    pub total_tools: usize,
    pub total_est_tokens: u64,
    pub capabilities: Vec<CapReport>,
}

/// Aggregate raw records into a ranked, per-capability report. Pure over its
/// input so it can be unit-tested with synthetic records.
pub fn aggregate(records: &[RequestRecord]) -> Report {
    // Per-capability running totals across every request it appeared in.
    struct Acc {
        appearances: u64,
        est_tokens_sum: u64,
        max_tools: usize,
        calls: u64,
    }
    let mut caps: BTreeMap<String, Acc> = BTreeMap::new();

    // Headline "tools / tokens per turn" is the max seen in any single request
    // (a turn re-sends the whole block; summing across turns would inflate it).
    let mut headline_tools = 0usize;
    let mut headline_tokens = 0u64;

    for rec in records {
        headline_tools = headline_tools.max(rec.total_tools);
        headline_tokens = headline_tokens.max(rec.total_est_tokens);

        for (cap, cost) in &rec.per_capability {
            let acc = caps.entry(cap.clone()).or_insert(Acc {
                appearances: 0,
                est_tokens_sum: 0,
                max_tools: 0,
                calls: 0,
            });
            acc.appearances += 1;
            acc.est_tokens_sum += cost.est_tokens;
            acc.max_tools = acc.max_tools.max(cost.tools);
        }

        // Attribute each tool_use name back to its capability.
        for name in &rec.tool_use {
            let cap = capability_of(name);
            if let Some(acc) = caps.get_mut(&cap) {
                acc.calls += 1;
            } else {
                // A tool called but never seen loaded still counts.
                caps.entry(cap).or_insert(Acc {
                    appearances: 0,
                    est_tokens_sum: 0,
                    max_tools: 0,
                    calls: 1,
                });
            }
        }
    }

    let mut rows: Vec<CapReport> = caps
        .into_iter()
        .map(|(capability, acc)| {
            let avg = acc.est_tokens_sum.checked_div(acc.appearances).unwrap_or(0);
            CapReport {
                capability,
                tools: acc.max_tools,
                avg_est_tokens: avg,
                calls: acc.calls,
                hint: String::new(),
            }
        })
        .collect();

    // Rank by average cost, descending; name breaks ties for stable output.
    rows.sort_by(|a, b| {
        b.avg_est_tokens
            .cmp(&a.avg_est_tokens)
            .then(a.capability.cmp(&b.capability))
    });

    // "Top band" for the drop/lazy hint: at or above the costliest row's est
    // tokens — the loaded-but-unused weight most worth reclaiming.
    let top = rows.first().map(|r| r.avg_est_tokens).unwrap_or(0);
    for r in &mut rows {
        r.hint = hint_for(r.calls, r.avg_est_tokens, top);
    }

    Report {
        requests: records.len(),
        total_tools: headline_tools,
        total_est_tokens: headline_tokens,
        capabilities: rows,
    }
}

/// A modest loaded-vs-called ranking signal, not a hard verdict: a costly
/// capability that was never called is a `drop / lazy` candidate; anything
/// actually called is `keep`; the cheap-and-unused rest is `watch`.
fn hint_for(calls: u64, est_tokens: u64, top: u64) -> String {
    if calls > 0 {
        "keep".to_string()
    } else if top > 0 && est_tokens >= top {
        "drop / lazy".to_string()
    } else {
        "watch".to_string()
    }
}

// ── Networking loop ─────────────────────────────────────────────────────────

/// Headers the client library must set for the *upstream* leg; never copied
/// through from the incoming request (reqwest sets its own).
fn is_hop_by_hop(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    matches!(n.as_str(), "host" | "content-length")
}

/// Start the loopback wire proxy. Blocks, serving requests until interrupted.
pub fn serve(config: ProxyConfig) -> Result<()> {
    let addr = format!("127.0.0.1:{}", config.port);
    let server = Server::http(&addr).map_err(|e| anyhow!("binding {addr}: {e}"))?;
    let upstream = config.upstream.trim_end_matches('/').to_string();

    // reqwest sets its own timeouts to none by default here; long SSE streams
    // must not be cut off, so we don't impose a read timeout.
    let client = reqwest::blocking::Client::builder()
        .build()
        .map_err(|e| anyhow!("building http client: {e}"))?;

    eprintln!(
        "agentstack wire proxy — observing on http://127.0.0.1:{}",
        config.port
    );
    eprintln!("relaying verbatim to {upstream} (observe only; nothing is injected)");
    eprintln!("point your harness at it:");
    eprintln!("    ANTHROPIC_BASE_URL=http://127.0.0.1:{}", config.port);
    eprintln!("telemetry → {}", log_path().display());

    let project = std::env::current_dir()
        .ok()
        .map(|p| p.display().to_string());

    // Handle each request on its own thread so an in-flight SSE stream — which
    // holds its connection open for the life of the streamed response — can
    // never block accepting or forwarding concurrent requests (parallel
    // subagents, background token-count/compaction calls, tool-triggered
    // sub-calls). A multi-agent harness pointed at ANTHROPIC_BASE_URL must not
    // serialize behind one open stream. reqwest's blocking Client is an Arc
    // internally, so cloning it shares the connection pool across threads.
    for request in server.incoming_requests() {
        let client = client.clone();
        let upstream = upstream.clone();
        let project = project.clone();
        std::thread::spawn(move || {
            handle_one(request, &client, &upstream, project.as_deref());
        });
    }
    Ok(())
}

/// Relay one request end to end. Any forwarding failure returns a 502 to the
/// client but keeps the accept loop alive; all accounting is best-effort and can
/// never fail or delay the proxied request.
fn handle_one(
    mut request: tiny_http::Request,
    client: &reqwest::blocking::Client,
    upstream: &str,
    project: Option<&str>,
) {
    let method = request.method().clone();
    let url = request.url().to_string();
    let is_messages = url.contains("/v1/messages");

    // Copy request headers to forward (skip hop-by-hop ones the client resets).
    let fwd_headers: Vec<(String, String)> = request
        .headers()
        .iter()
        .filter(|h| !is_hop_by_hop(h.field.as_str().as_str()))
        .map(|h| {
            (
                h.field.as_str().as_str().to_string(),
                h.value.as_str().to_string(),
            )
        })
        .collect();

    // Read the full request body.
    let mut body: Vec<u8> = Vec::new();
    let _ = request.as_reader().read_to_end(&mut body);

    // Best-effort accounting: only on the messages endpoint, only if JSON parses.
    let mut accounting: Option<RequestAccounting> = None;
    if is_messages {
        if let Ok(v) = serde_json::from_slice::<Value>(&body) {
            accounting = Some(account_request(&v));
        }
    }

    // Forward upstream with the same method, headers, and body.
    let target = format!("{upstream}{url}");
    let rmethod = match reqwest::Method::from_bytes(method.as_str().as_bytes()) {
        Ok(m) => m,
        Err(_) => reqwest::Method::POST,
    };
    let mut builder = client.request(rmethod, &target).body(body);
    for (k, v) in &fwd_headers {
        builder = builder.header(k.as_str(), v.as_str());
    }

    let upstream_resp = match builder.send() {
        Ok(r) => r,
        Err(e) => {
            let msg = format!("agentstack proxy: upstream request failed: {e}");
            let resp = Response::from_string(msg).with_status_code(502);
            let _ = request.respond(resp);
            return;
        }
    };

    // Copy status + response headers through (especially Content-Type).
    let status = upstream_resp.status().as_u16();
    let content_type = upstream_resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let is_json = content_type.starts_with("application/json");
    let is_sse = content_type.starts_with("text/event-stream");

    let mut resp_headers: Vec<Header> = Vec::new();
    for (name, value) in upstream_resp.headers().iter() {
        if is_hop_by_hop(name.as_str()) {
            continue;
        }
        if let Ok(h) = Header::from_bytes(name.as_str().as_bytes(), value.as_bytes()) {
            resp_headers.push(h);
        }
    }

    // Seed the record from the request accounting (or an empty one on a
    // non-messages / non-JSON request), then attach response-side numbers.
    let mut rec =
        RequestRecord::from_accounting(accounting.unwrap_or_default(), project.map(str::to_string));

    if is_sse {
        // Stream the body straight through — never buffer an SSE response. The
        // Response wraps a pass-through tee reader, so bytes flow to the client
        // as they arrive (no added buffering or delay), while the tee absorbs
        // `tool_use` names and usage numbers off the wire into `cap`. Parsing is
        // best-effort and can never affect the stream (observe-only contract).
        rec.streamed = true;
        let cap = Arc::new(Mutex::new(SseCapture::default()));
        let tee = SseTee::new(upstream_resp, cap.clone());
        let resp = Response::new(tiny_http::StatusCode(status), resp_headers, tee, None, None);
        let _ = request.respond(resp);
        // respond() has now fully drained the stream, so the capture is
        // complete. Fold it into the record before logging.
        if let Ok(cap) = cap.lock() {
            rec.tool_use = cap.tool_use.clone();
            rec.input_tokens = cap.input_tokens;
            rec.output_tokens = cap.output_tokens;
            rec.cache_read_input_tokens = cap.cache_read_input_tokens;
        }
        if is_messages {
            record(&rec);
        }
        return;
    }

    // Non-SSE: read the full body so we can (best-effort) parse JSON usage.
    let mut resp_body: Vec<u8> = Vec::new();
    let mut reader = upstream_resp;
    let _ = reader.read_to_end(&mut resp_body);

    if is_json {
        if let Ok(v) = serde_json::from_slice::<Value>(&resp_body) {
            attach_response_usage(&mut rec, &v);
        }
    }

    let data_length = resp_body.len();
    let resp = Response::new(
        tiny_http::StatusCode(status),
        resp_headers,
        std::io::Cursor::new(resp_body),
        Some(data_length),
        None,
    );
    let _ = request.respond(resp);

    if is_messages {
        record(&rec);
    }
}

/// Pull best-effort usage numbers and `tool_use` tool NAMES out of a JSON
/// messages response. Never records anything but counts and names.
fn attach_response_usage(rec: &mut RequestRecord, v: &Value) {
    if let Some(usage) = v.get("usage") {
        rec.input_tokens = usage.get("input_tokens").and_then(Value::as_u64);
        rec.output_tokens = usage.get("output_tokens").and_then(Value::as_u64);
        rec.cache_read_input_tokens = usage.get("cache_read_input_tokens").and_then(Value::as_u64);
    }
    if let Some(content) = v.get("content").and_then(Value::as_array) {
        for block in content {
            if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                if let Some(name) = block.get("name").and_then(Value::as_str) {
                    rec.tool_use.push(name.to_string());
                }
            }
        }
    }
}

/// Content-free tally accumulated from an SSE stream as it flows through: the
/// `tool_use` tool NAMES the model emitted and best-effort usage numbers.
#[derive(Debug, Default)]
struct SseCapture {
    tool_use: Vec<String>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
}

/// Feed one decoded SSE `data:` JSON payload into the capture. Handles the two
/// event shapes that carry what we account for, and NOTHING else — no prompt,
/// message, or argument bodies are ever touched. Same privacy contract as the
/// non-streaming path.
fn absorb_sse_event(cap: &mut SseCapture, v: &Value) {
    match v.get("type").and_then(Value::as_str) {
        // A `content_block_start` for a tool_use block carries the tool name.
        Some("content_block_start") => {
            let block = v.get("content_block");
            if block.and_then(|b| b.get("type")).and_then(Value::as_str) == Some("tool_use") {
                if let Some(name) = block.and_then(|b| b.get("name")).and_then(Value::as_str) {
                    cap.tool_use.push(name.to_string());
                }
            }
        }
        // `message_start` seeds input/cache usage; `message_delta` carries the
        // final output_tokens. Take the last non-null value we see for each.
        Some("message_start") => {
            if let Some(usage) = v.get("message").and_then(|m| m.get("usage")) {
                if let Some(n) = usage.get("input_tokens").and_then(Value::as_u64) {
                    cap.input_tokens = Some(n);
                }
                if let Some(n) = usage.get("cache_read_input_tokens").and_then(Value::as_u64) {
                    cap.cache_read_input_tokens = Some(n);
                }
            }
        }
        Some("message_delta") => {
            if let Some(usage) = v.get("usage") {
                if let Some(n) = usage.get("output_tokens").and_then(Value::as_u64) {
                    cap.output_tokens = Some(n);
                }
            }
        }
        _ => {}
    }
}

/// A pass-through reader that tees an SSE response: every byte read is returned
/// to the caller UNCHANGED (so the client receives the verbatim stream with no
/// added buffering or delay — the observe-only, never-delay contract), while a
/// side buffer accumulates complete `data:` lines and parses out `tool_use`
/// names and usage numbers into the shared `SseCapture`. Parsing is strictly
/// best-effort: any malformed line is skipped and never affects the stream.
struct SseTee<R: Read> {
    inner: R,
    cap: Arc<Mutex<SseCapture>>,
    /// Bytes of the current, not-yet-terminated line.
    line: Vec<u8>,
}

impl<R: Read> SseTee<R> {
    fn new(inner: R, cap: Arc<Mutex<SseCapture>>) -> Self {
        SseTee {
            inner,
            cap,
            line: Vec::new(),
        }
    }

    /// Scan freshly-read bytes for complete lines and absorb any `data:` payloads.
    fn feed(&mut self, bytes: &[u8]) {
        for &b in bytes {
            if b == b'\n' {
                self.flush_line();
            } else if b != b'\r' {
                // Guard the side buffer so a pathological line can't grow the
                // proxy's memory without bound; SSE data lines are small.
                if self.line.len() < 64 * 1024 {
                    self.line.push(b);
                }
            }
        }
    }

    fn flush_line(&mut self) {
        let line = std::mem::take(&mut self.line);
        let Ok(text) = std::str::from_utf8(&line) else {
            return;
        };
        let Some(payload) = text.strip_prefix("data:") else {
            return;
        };
        let payload = payload.trim();
        if payload.is_empty() || payload == "[DONE]" {
            return;
        }
        let Ok(v) = serde_json::from_str::<Value>(payload) else {
            return;
        };
        if let Ok(mut cap) = self.cap.lock() {
            absorb_sse_event(&mut cap, &v);
        }
    }
}

impl<R: Read> Read for SseTee<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n > 0 {
            self.feed(&buf[..n]);
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool(name: &str) -> Value {
        json!({
            "name": name,
            "description": "a tool that does a thing with some words in it",
            "input_schema": { "type": "object", "properties": {} }
        })
    }

    #[test]
    fn capability_bucketing() {
        assert_eq!(capability_of("mcp__figma__get_file"), "figma");
        assert_eq!(capability_of("mcp__github__list_issues"), "github");
        assert_eq!(capability_of("Read"), "builtin");
        assert_eq!(capability_of("Bash"), "builtin");
        // Malformed mcp names fall back sanely.
        assert_eq!(capability_of("mcp__"), "builtin");
        assert_eq!(capability_of("mcp__solo"), "solo");
    }

    #[test]
    fn account_request_buckets_and_totals_add_up() {
        let body = json!({
            "model": "claude-opus-4-8",
            "tools": [
                tool("mcp__figma__get_file"),
                tool("mcp__figma__create_frame"),
                tool("Read"),
                tool("Bash"),
            ]
        });
        let acct = account_request(&body);
        assert_eq!(acct.model.as_deref(), Some("claude-opus-4-8"));
        assert_eq!(acct.total_tools, 4);
        assert_eq!(acct.per_capability.len(), 2);
        assert_eq!(acct.per_capability["figma"].tools, 2);
        assert_eq!(acct.per_capability["builtin"].tools, 2);
        // Totals equal the sum of the buckets.
        let bucket_sum: u64 = acct.per_capability.values().map(|c| c.est_tokens).sum();
        assert_eq!(acct.total_est_tokens, bucket_sum);
        assert!(acct.total_est_tokens > 0);
    }

    #[test]
    fn account_request_empty_and_missing_tools_yield_zeros() {
        let empty = account_request(&json!({ "model": "m", "tools": [] }));
        assert_eq!(empty.total_tools, 0);
        assert_eq!(empty.total_est_tokens, 0);
        assert!(empty.per_capability.is_empty());

        let missing = account_request(&json!({ "model": "m" }));
        assert_eq!(missing.total_tools, 0);
        assert!(missing.per_capability.is_empty());
        assert_eq!(missing.model.as_deref(), Some("m"));

        // Non-array tools must not panic.
        let weird = account_request(&json!({ "tools": "nope" }));
        assert_eq!(weird.total_tools, 0);
        assert!(weird.model.is_none());
    }

    #[test]
    fn request_record_serde_round_trip() {
        let mut per = BTreeMap::new();
        per.insert(
            "figma".to_string(),
            CapCost {
                tools: 2,
                est_tokens: 400,
            },
        );
        let rec = RequestRecord {
            ts: 1234,
            project: Some("/tmp/proj".to_string()),
            model: Some("claude-opus-4-8".to_string()),
            total_tools: 3,
            total_est_tokens: 600,
            per_capability: per,
            streamed: false,
            input_tokens: Some(500),
            output_tokens: Some(20),
            cache_read_input_tokens: Some(480),
            tool_use: vec!["mcp__figma__get_file".to_string()],
        };
        let line = serde_json::to_string(&rec).unwrap();
        let back: RequestRecord = serde_json::from_str(&line).unwrap();
        assert_eq!(rec, back);
    }

    fn rec_with(
        per: &[(&str, usize, u64)],
        total_tools: usize,
        total_tokens: u64,
        tool_use: &[&str],
    ) -> RequestRecord {
        let mut m = BTreeMap::new();
        for (cap, tools, tok) in per {
            m.insert(
                cap.to_string(),
                CapCost {
                    tools: *tools,
                    est_tokens: *tok,
                },
            );
        }
        RequestRecord {
            ts: 0,
            project: None,
            model: None,
            total_tools,
            total_est_tokens: total_tokens,
            per_capability: m,
            streamed: false,
            input_tokens: None,
            output_tokens: None,
            cache_read_input_tokens: None,
            tool_use: tool_use.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn aggregate_math_and_hints() {
        let records = vec![
            rec_with(&[("figma", 2, 400), ("builtin", 2, 100)], 4, 500, &["Read"]),
            rec_with(
                &[("figma", 2, 600), ("builtin", 2, 100)],
                4,
                700,
                &["Read", "mcp__figma__get_file"],
            ),
        ];
        let rep = aggregate(&records);
        assert_eq!(rep.requests, 2);
        // Headline = max seen in a single turn, not a sum across turns.
        assert_eq!(rep.total_tools, 4);
        assert_eq!(rep.total_est_tokens, 700);

        // Ranked by avg est tokens, desc: figma (500) before builtin (100).
        assert_eq!(rep.capabilities[0].capability, "figma");
        assert_eq!(rep.capabilities[0].avg_est_tokens, 500); // (400+600)/2
        assert_eq!(rep.capabilities[0].calls, 1); // one figma tool_use
        assert_eq!(rep.capabilities[0].hint, "keep"); // called → keep

        assert_eq!(rep.capabilities[1].capability, "builtin");
        assert_eq!(rep.capabilities[1].avg_est_tokens, 100);
        assert_eq!(rep.capabilities[1].calls, 2); // two Read calls
        assert_eq!(rep.capabilities[1].hint, "keep");
    }

    #[test]
    fn aggregate_flags_costly_uncalled_as_drop() {
        let records = vec![rec_with(
            &[("figma", 5, 2000), ("builtin", 1, 50)],
            6,
            2050,
            &["Read"], // only builtin called; figma loaded but never called
        )];
        let rep = aggregate(&records);
        // figma is the top band and was never called → drop / lazy.
        let figma = rep
            .capabilities
            .iter()
            .find(|c| c.capability == "figma")
            .unwrap();
        assert_eq!(figma.hint, "drop / lazy");
        // builtin was called → keep, even though it's cheap.
        let builtin = rep
            .capabilities
            .iter()
            .find(|c| c.capability == "builtin")
            .unwrap();
        assert_eq!(builtin.hint, "keep");
    }

    #[test]
    fn sse_tee_captures_tool_use_and_usage_and_passes_through() {
        // A minimal but representative Anthropic streaming body: message_start
        // with input/cache usage, a content_block_start for a tool_use block,
        // and a message_delta with output_tokens. Interspersed with events we
        // ignore and a [DONE]-style terminator.
        let sse = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1200,\"cache_read_input_tokens\":1000}}}\n",
            "\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"mcp__figma__get_file\"}}\n",
            "\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{}\"}}\n",
            "\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_2\",\"name\":\"Read\"}}\n",
            "\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":42}}\n",
            "\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n",
            "\n",
        );

        let cap = Arc::new(Mutex::new(SseCapture::default()));
        let mut tee = SseTee::new(std::io::Cursor::new(sse.as_bytes().to_vec()), cap.clone());

        // Read through a tiny buffer to prove line reassembly across read
        // boundaries works, and that the bytes come out verbatim.
        let mut out = Vec::new();
        let mut buf = [0u8; 7];
        loop {
            let n = tee.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
        }
        assert_eq!(out, sse.as_bytes(), "stream must pass through verbatim");

        let cap = cap.lock().unwrap();
        assert_eq!(cap.tool_use, vec!["mcp__figma__get_file", "Read"]);
        assert_eq!(cap.input_tokens, Some(1200));
        assert_eq!(cap.cache_read_input_tokens, Some(1000));
        assert_eq!(cap.output_tokens, Some(42));
    }

    #[test]
    fn sse_tee_skips_malformed_lines_without_panic() {
        let sse = concat!(
            "data: not json at all\n",
            "data: {\"type\":\"content_block_start\",\"content_block\":{\"type\":\"text\"}}\n",
            "data: [DONE]\n",
            ": a comment line\n",
            "data:\n",
        );
        let cap = Arc::new(Mutex::new(SseCapture::default()));
        let mut tee = SseTee::new(std::io::Cursor::new(sse.as_bytes().to_vec()), cap.clone());
        let mut out = Vec::new();
        tee.read_to_end(&mut out).unwrap();
        assert_eq!(out, sse.as_bytes());
        let cap = cap.lock().unwrap();
        assert!(cap.tool_use.is_empty());
        assert!(cap.input_tokens.is_none());
    }

    #[test]
    fn empty_aggregate_is_safe() {
        let rep = aggregate(&[]);
        assert_eq!(rep.requests, 0);
        assert_eq!(rep.total_tools, 0);
        assert!(rep.capabilities.is_empty());
    }
}
