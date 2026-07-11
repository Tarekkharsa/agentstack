//! HTTP MCP endpoint for the in-process gateway (gateway-unification
//! Session 1).
//!
//! A sandboxed harness cannot spawn a host process, so it cannot reach the
//! gateway over stdio the way a `connect`-ed harness does. This module serves
//! the MINIMAL server side of MCP streamable HTTP that the harness's client
//! actually needs — spike-verified against claude-code 2.1.207
//! (`docs/spikes/2026-07-11-gateway-http-transport.md`):
//!
//! - plain `application/json` POST responses (no SSE — the client's optional
//!   `GET` for a server→client stream is answered `405` and tolerated),
//! - an `Mcp-Session-Id` header on every response (the client echoes it),
//! - `202` for notifications.
//!
//! `tiny_http` on detached threads, the same no-tokio pattern as the
//! code-mode endpoint (`crate::codemode::endpoint`).
//!
//! Security posture:
//! - **The token is the gate, not the bind.** The socket may be bound broadly
//!   so a container can reach it (the same argument ENFORCEMENT.md makes for
//!   the host egress proxy), so EVERY request must carry the per-run
//!   `X-Agentstack-Token` — checked before the body is even read.
//! - **Proxied tools only.** `tools/list` serves exactly the policy-filtered
//!   namespaced surface (`Gateway::namespaced_tools`), and `tools/call` goes
//!   through `Gateway::try_call` — the same two enforcement sites as every
//!   other gateway surface. None of agentstack's own control-plane tools
//!   (add/diff/explain/…) are exposed: a sandboxed agent must not be able to
//!   mutate the manifest it runs under.

use std::io::Read;
use std::sync::Arc;

use serde_json::{json, Value};
use tiny_http::{Header, Method, Response, Server};

use crate::gateway::Gateway;

/// Hard cap on a request body (CLAUDE.md rule 7 — bound sizes on hostile
/// input). This endpoint is reachable by the untrusted sandboxed container
/// over the network (unlike the loopback-only code-mode endpoint), so a
/// declared-huge `Content-Length` or chunked stream must not be buffered
/// whole into memory. MCP JSON-RPC messages are kilobytes; 4 MiB is a
/// generous ceiling that still refuses an OOM attempt.
const MAX_BODY_BYTES: u64 = 4 * 1024 * 1024;

/// A running gateway HTTP endpoint. The serve threads are detached and live
/// until the process exits — the endpoint's lifetime is the run's lifetime,
/// and `agentstack run` is a per-run process.
pub struct GatewayHttp {
    /// Port the listener actually bound (callers rewrite the host for the
    /// container's view — e.g. `host.docker.internal`).
    pub port: u16,
    /// The per-run bearer token every request must present.
    pub token: String,
}

/// Start serving `gateway` on `bind` (e.g. `"127.0.0.1:0"` for tests, a
/// broader bind for container-reachable use). Returns `None` if the socket
/// can't be bound. An EMPTY gateway is served faithfully (zero tools): the
/// trust gate upstream decides the surface; this endpoint never widens it.
pub fn start(gateway: Arc<Gateway>, bind: &str) -> Option<GatewayHttp> {
    let server = Server::http(bind).ok()?;
    let port = server.server_addr().to_ip().map(|a| a.port())?;
    let token = hex_token();
    let session_id = hex_token();

    let token_for_thread = token.clone();
    std::thread::spawn(move || {
        // Accept loop only; each request gets its own thread. The gateway is
        // Sync with per-upstream locking, so one slow upstream call doesn't
        // block the endpoint (mirrors the code-mode endpoint's model).
        for req in server.incoming_requests() {
            let gateway = Arc::clone(&gateway);
            let token = token_for_thread.clone();
            let session_id = session_id.clone();
            std::thread::spawn(move || serve_one(req, &gateway, &token, &session_id));
        }
    });

    Some(GatewayHttp { port, token })
}

/// Handle one HTTP request: token first, then method, then MCP dispatch.
fn serve_one(mut req: tiny_http::Request, gateway: &Gateway, token: &str, session_id: &str) {
    let authed = req
        .headers()
        .iter()
        .any(|h| h.field.equiv("X-Agentstack-Token") && h.value.as_str() == token);
    if !authed {
        let resp = Response::from_string(json!({ "error": "unauthorized" }).to_string())
            .with_status_code(401)
            .with_header(json_ctype());
        let _ = req.respond(resp);
        return;
    }
    match req.method() {
        Method::Post => {
            // Reject an oversized declared length outright, then bound the
            // actual read so a lying/chunked Content-Length can't blow past it.
            if req.body_length().is_some_and(|n| n as u64 > MAX_BODY_BYTES) {
                let resp =
                    Response::from_string(json!({ "error": "request body too large" }).to_string())
                        .with_status_code(413)
                        .with_header(json_ctype());
                let _ = req.respond(resp);
                return;
            }
            let mut body = String::new();
            // `take` caps the reader: a stream that keeps sending past the cap
            // is truncated here rather than buffered whole, and the truncated
            // (now invalid) JSON is rejected by the handler as a 400.
            let _ = req
                .as_reader()
                .take(MAX_BODY_BYTES + 1)
                .read_to_string(&mut body);
            if body.len() as u64 > MAX_BODY_BYTES {
                let resp =
                    Response::from_string(json!({ "error": "request body too large" }).to_string())
                        .with_status_code(413)
                        .with_header(json_ctype());
                let _ = req.respond(resp);
                return;
            }
            let (status, payload) = handle_mcp_post(gateway, &body);
            let resp = match payload {
                Some(p) => Response::from_string(p)
                    .with_status_code(status)
                    .with_header(json_ctype())
                    .with_header(session_header(session_id)),
                None => Response::from_string(String::new())
                    .with_status_code(status)
                    .with_header(session_header(session_id)),
            };
            let _ = req.respond(resp);
        }
        // The optional SSE channel: refusing it is spec-legal and
        // spike-verified — the client proceeds without a stream.
        Method::Get => {
            let resp = Response::from_string(String::new())
                .with_status_code(405)
                .with_header(allow_post())
                .with_header(session_header(session_id));
            let _ = req.respond(resp);
        }
        // Session teardown: nothing to tear down server-side (the endpoint is
        // single-session by construction), acknowledge politely.
        Method::Delete => {
            let resp = Response::from_string(String::new())
                .with_status_code(200)
                .with_header(session_header(session_id));
            let _ = req.respond(resp);
        }
        _ => {
            let resp = Response::from_string(String::new())
                .with_status_code(405)
                .with_header(allow_post());
            let _ = req.respond(resp);
        }
    }
}

/// Dispatch one POSTed JSON-RPC message. Pure over the gateway (no socket),
/// so it is unit-testable — the same split as the code-mode endpoint.
/// Returns `(http_status, Some(json_body))`, or `(202, None)` for
/// notifications and client-to-server responses, which get no body.
pub fn handle_mcp_post(gateway: &Gateway, body: &str) -> (u16, Option<String>) {
    let Ok(msg) = serde_json::from_str::<Value>(body) else {
        return (
            400,
            Some(json!({ "error": "invalid JSON in request body" }).to_string()),
        );
    };
    let method = msg.get("method").and_then(Value::as_str);
    let id = msg.get("id").filter(|v| !v.is_null()).cloned();
    match (method, id) {
        (Some("initialize"), Some(id)) => {
            // Echo the client's protocol version: this endpoint implements the
            // subset that is stable across the revisions the harness speaks.
            let ver = msg
                .pointer("/params/protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or("2025-03-26");
            (
                200,
                Some(rpc_result(
                    id,
                    json!({
                        "protocolVersion": ver,
                        "capabilities": { "tools": {} },
                        "serverInfo": {
                            "name": "agentstack-gateway",
                            "version": env!("CARGO_PKG_VERSION"),
                        },
                    }),
                )),
            )
        }
        (Some("ping"), Some(id)) => (200, Some(rpc_result(id, json!({})))),
        (Some("tools/list"), Some(id)) => {
            // The policy-filtered namespaced surface, verbatim — denied tools
            // were already filtered out of discovery by the gateway.
            let tools: Vec<Value> = gateway.namespaced_tools().iter().cloned().collect();
            (200, Some(rpc_result(id, json!({ "tools": tools }))))
        }
        (Some("tools/call"), Some(id)) => {
            let name = msg
                .pointer("/params/name")
                .and_then(Value::as_str)
                .unwrap_or("");
            let args = msg
                .pointer("/params/arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            match gateway.try_call(name, &args) {
                Some(Ok(v)) => (200, Some(rpc_result(id, v))),
                // Policy denials and upstream failures ride the MCP tool-error
                // shape (isError result), not a protocol error — same shaping
                // as the stdio serve loop.
                Some(Err(e)) => (
                    200,
                    Some(rpc_result(id, tool_error(&format!("Error: {e}")))),
                ),
                None => (
                    200,
                    Some(rpc_result(
                        id,
                        tool_error(&format!(
                            "Error: '{name}' is not a proxied tool for this project"
                        )),
                    )),
                ),
            }
        }
        (Some(m), Some(id)) => (
            200,
            Some(rpc_error(id, -32601, &format!("method not found: {m}"))),
        ),
        // A notification (no id) or a client→server response (no method):
        // accepted with no body, per streamable-HTTP.
        _ => (202, None),
    }
}

fn rpc_result(id: Value, result: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

fn rpc_error(id: Value, code: i64, message: &str) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }).to_string()
}

fn tool_error(text: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "isError": true })
}

fn json_ctype() -> Header {
    Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap()
}

fn session_header(session_id: &str) -> Header {
    Header::from_bytes(&b"Mcp-Session-Id"[..], session_id.as_bytes()).unwrap()
}

fn allow_post() -> Header {
    Header::from_bytes(&b"Allow"[..], &b"POST"[..]).unwrap()
}

/// Per-run credential from the OS entropy pool — same construction as the
/// code-mode endpoint's token (it invokes proxied tools, so it is a real
/// credential, never a guessable time/PID hash).
fn hex_token() -> String {
    crate::util::random_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_echoes_protocol_version_and_names_the_server() {
        let gw = Gateway::empty();
        let req = json!({
            "jsonrpc": "2.0", "id": 0, "method": "initialize",
            "params": { "protocolVersion": "2025-11-25", "capabilities": {} }
        });
        let (status, body) = handle_mcp_post(&gw, &req.to_string());
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body.unwrap()).unwrap();
        assert_eq!(v["result"]["protocolVersion"], "2025-11-25");
        assert_eq!(v["result"]["serverInfo"]["name"], "agentstack-gateway");
    }

    #[test]
    fn notifications_are_202_with_no_body() {
        let gw = Gateway::empty();
        let req = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        let (status, body) = handle_mcp_post(&gw, &req.to_string());
        assert_eq!(status, 202);
        assert!(body.is_none());
    }

    #[test]
    fn tools_list_on_an_empty_gateway_serves_zero_tools() {
        // The untrusted-bundle path yields an empty gateway; the endpoint
        // serves that surface faithfully — it never widens it.
        let gw = Gateway::empty();
        let req = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" });
        let (status, body) = handle_mcp_post(&gw, &req.to_string());
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body.unwrap()).unwrap();
        assert_eq!(v["result"]["tools"], json!([]));
    }

    #[test]
    fn unknown_tool_is_a_tool_error_not_a_protocol_error() {
        let gw = Gateway::empty();
        let req = json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": { "name": "figma__get_file", "arguments": {} }
        });
        let (status, body) = handle_mcp_post(&gw, &req.to_string());
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body.unwrap()).unwrap();
        assert_eq!(v["result"]["isError"], true);
        assert!(v["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("not a proxied tool"));
    }

    #[test]
    fn malformed_body_is_400_and_unknown_method_is_32601() {
        let gw = Gateway::empty();
        let (s, _) = handle_mcp_post(&gw, "{not json");
        assert_eq!(s, 400);
        let req = json!({ "jsonrpc": "2.0", "id": 3, "method": "resources/list" });
        let (s, body) = handle_mcp_post(&gw, &req.to_string());
        assert_eq!(s, 200);
        let v: Value = serde_json::from_str(&body.unwrap()).unwrap();
        assert_eq!(v["error"]["code"], -32601);
    }

    /// Socket-level contract the spike proved the client needs: 401 without
    /// the token, and Mcp-Session-Id + JSON on an authed initialize.
    #[test]
    fn socket_gates_on_token_and_stamps_the_session_header() {
        let handle = start(std::sync::Arc::new(Gateway::empty()), "127.0.0.1:0").unwrap();
        let url = format!("http://127.0.0.1:{}/mcp", handle.port);
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .build()
            .unwrap();
        let init = json!({
            "jsonrpc": "2.0", "id": 0, "method": "initialize",
            "params": { "protocolVersion": "2025-03-26" }
        });

        // No token → 401 before any MCP handling.
        let resp = client.post(&url).json(&init).send().unwrap();
        assert_eq!(resp.status().as_u16(), 401);

        // Authed initialize → 200 + session header; GET (SSE probe) → 405.
        let resp = client
            .post(&url)
            .header("X-Agentstack-Token", &handle.token)
            .json(&init)
            .send()
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        assert!(resp.headers().get("mcp-session-id").is_some());
        let resp = client
            .get(&url)
            .header("X-Agentstack-Token", &handle.token)
            .send()
            .unwrap();
        assert_eq!(resp.status().as_u16(), 405);
    }

    /// An oversized body is refused with 413, not buffered whole — a hostile
    /// container must not be able to OOM the host gateway (rule 7).
    #[test]
    fn oversized_body_is_rejected_not_buffered() {
        let handle = start(std::sync::Arc::new(Gateway::empty()), "127.0.0.1:0").unwrap();
        let url = format!("http://127.0.0.1:{}/mcp", handle.port);
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .build()
            .unwrap();
        let huge = "x".repeat((MAX_BODY_BYTES as usize) + 1024);
        let resp = client
            .post(&url)
            .header("X-Agentstack-Token", &handle.token)
            .body(huge)
            .send()
            .unwrap();
        assert_eq!(resp.status().as_u16(), 413);
    }
}
