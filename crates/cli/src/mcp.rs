//! Minimal MCP (Model Context Protocol) HTTP handshake for `doctor --live`.
//!
//! Performs the Streamable-HTTP `initialize` → `notifications/initialized` →
//! `tools/list` sequence and reports server identity + tool count, or a
//! classified error (auth, http, connect, protocol). Just enough to prove a
//! server is reachable and accepts the configured credentials.

use std::time::Duration;

use indexmap::IndexMap;
use serde_json::{json, Value};

const PROTOCOL_VERSION: &str = "2025-06-18";

#[derive(Debug)]
pub struct Handshake {
    pub server_name: Option<String>,
    pub protocol: Option<String>,
    pub tool_count: Option<usize>,
}

#[derive(Debug)]
pub enum LiveError {
    /// 401/403 — credentials missing or rejected.
    Auth(u16),
    /// Other non-success HTTP status.
    Http(u16),
    /// Could not connect / timed out / TLS error.
    Connect(String),
    /// Connected, but the response wasn't a usable MCP handshake.
    Protocol(String),
}

impl std::fmt::Display for LiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LiveError::Auth(code) => write!(f, "{code} unauthorized"),
            LiveError::Http(code) => write!(f, "HTTP {code}"),
            LiveError::Connect(e) => write!(f, "connection failed: {e}"),
            LiveError::Protocol(e) => write!(f, "protocol error: {e}"),
        }
    }
}

/// Run the handshake against an HTTP MCP server.
pub fn handshake(
    url: &str,
    headers: &IndexMap<String, String>,
    timeout: Duration,
) -> Result<Handshake, LiveError> {
    let client = reqwest::blocking::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|e| LiveError::Connect(e.to_string()))?;

    let init = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "agentstack", "version": env!("CARGO_PKG_VERSION") }
        }
    });

    let resp = post(&client, url, headers, None, &init)?;
    let status = resp.status();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err(LiveError::Auth(status.as_u16()));
    }
    if !status.is_success() {
        return Err(LiveError::Http(status.as_u16()));
    }
    let session = resp
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let body = resp
        .text()
        .map_err(|e| LiveError::Protocol(e.to_string()))?;
    let result = extract_result(&body)
        .ok_or_else(|| LiveError::Protocol("no result in initialize response".into()))?;

    let server_name = result
        .get("serverInfo")
        .and_then(|s| s.get("name"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let protocol = result
        .get("protocolVersion")
        .and_then(Value::as_str)
        .map(str::to_string);

    // Best-effort: complete the handshake and count tools. Failures here don't
    // invalidate a successful initialize.
    let tool_count = count_tools(&client, url, headers, session.as_deref());

    Ok(Handshake {
        server_name,
        protocol,
        tool_count,
    })
}

fn count_tools(
    client: &reqwest::blocking::Client,
    url: &str,
    headers: &IndexMap<String, String>,
    session: Option<&str>,
) -> Option<usize> {
    let initialized = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
    let _ = post(client, url, headers, session, &initialized);

    let list = json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}});
    let resp = post(client, url, headers, session, &list).ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body = resp.text().ok()?;
    let result = extract_result(&body)?;
    result
        .get("tools")
        .and_then(Value::as_array)
        .map(|t| t.len())
}

fn post(
    client: &reqwest::blocking::Client,
    url: &str,
    headers: &IndexMap<String, String>,
    session: Option<&str>,
    body: &Value,
) -> Result<reqwest::blocking::Response, LiveError> {
    let mut req = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream");
    for (k, v) in headers {
        req = req.header(k, v);
    }
    if let Some(s) = session {
        req = req.header("Mcp-Session-Id", s);
    }
    req.json(body)
        .send()
        .map_err(|e| LiveError::Connect(e.to_string()))
}

/// Parse a JSON-RPC `result` from a body that may be plain JSON or an SSE
/// stream (`data: {...}` lines).
fn extract_result(body: &str) -> Option<Value> {
    let trimmed = body.trim_start();
    if trimmed.starts_with('{') {
        let v: Value = serde_json::from_str(trimmed).ok()?;
        return v.get("result").cloned();
    }
    // SSE: find the first data line carrying a result.
    for line in body.lines() {
        if let Some(data) = line.strip_prefix("data:") {
            if let Ok(v) = serde_json::from_str::<Value>(data.trim()) {
                if let Some(r) = v.get("result") {
                    return Some(r.clone());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_json_result() {
        let body = r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"x","serverInfo":{"name":"kibana"}}}"#;
        let r = extract_result(body).unwrap();
        assert_eq!(r["serverInfo"]["name"], "kibana");
    }

    #[test]
    fn parses_sse_result() {
        let body = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocolVersion\":\"x\"}}\n\n";
        let r = extract_result(body).unwrap();
        assert_eq!(r["protocolVersion"], "x");
    }

    #[test]
    fn no_result_returns_none() {
        assert!(extract_result("{\"error\":{}}").is_none());
        assert!(extract_result("garbage").is_none());
    }
}
