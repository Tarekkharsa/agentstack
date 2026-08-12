//! HTTP MCP endpoint for the in-process gateway (gateway-unification
//! Session 1).
//!
//! A sandboxed harness cannot spawn a host process, so it cannot reach the
//! gateway over stdio the way a `connect`-ed harness does. The socket and
//! per-run token gate remain here; MCP parsing, lifecycle negotiation, and
//! response headers are delegated to the official RMCP SDK.
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
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};
use tiny_http::{Header, Method, Response, Server};

use crate::gateway::Gateway;

/// Decrements the in-flight counter when a served request finishes, even on a
/// panic in `serve_one` — so a handler panic can't permanently consume a slot.
struct InflightGuard(Arc<AtomicUsize>);
impl Drop for InflightGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Release);
    }
}

/// Hard cap on a request body (CLAUDE.md rule 7 — bound sizes on hostile
/// input). This endpoint is reachable by the untrusted sandboxed container
/// over the network (unlike the loopback-only code-mode endpoint), so a
/// declared-huge `Content-Length` or chunked stream must not be buffered
/// whole into memory. MCP JSON-RPC messages are kilobytes; 4 MiB is a
/// generous ceiling that still refuses an OOM attempt.
const MAX_BODY_BYTES: u64 = 4 * 1024 * 1024;

/// Cap on concurrently-served requests (CLAUDE.md rule 7, same reasoning as the
/// body cap). Each request is served on its own OS thread; without a cap a
/// compromised container could open thousands of connections and exhaust host
/// threads (a `thread::spawn` panic would then take the whole endpoint — the
/// harness's only MCP transport — down). One harness's MCP client never needs
/// this many in flight; excess requests get a fast `503` instead of a thread.
const MAX_INFLIGHT: usize = 64;

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
    let protocol = Arc::new(
        agentstack_mcp::HttpServer::new(
            Arc::new(GatewayBackend(Arc::clone(&gateway))),
            "agentstack-gateway",
            env!("CARGO_PKG_VERSION"),
        )
        .ok()?,
    );

    let token_for_thread = token.clone();
    let inflight = Arc::new(AtomicUsize::new(0));
    std::thread::spawn(move || {
        // Accept loop only; each request gets its own thread. The gateway is
        // Sync with per-upstream locking, so one slow upstream call doesn't
        // block the endpoint (mirrors the code-mode endpoint's model).
        for req in server.incoming_requests() {
            // Bounded concurrency: shed load with a fast 503 rather than
            // spawning an unbounded number of host threads for a hostile
            // container. `fetch_add` then compare so the check + reserve is one
            // step against the accept loop's single thread.
            if inflight.fetch_add(1, Ordering::AcqRel) >= MAX_INFLIGHT {
                inflight.fetch_sub(1, Ordering::Release);
                let resp = Response::from_string(json!({ "error": "server busy" }).to_string())
                    .with_status_code(503)
                    .with_header(json_ctype());
                let _ = req.respond(resp);
                continue;
            }
            let guard = InflightGuard(Arc::clone(&inflight));
            let token = token_for_thread.clone();
            let protocol = Arc::clone(&protocol);
            std::thread::spawn(move || {
                let _guard = guard; // released (decrementing) on thread exit
                serve_one(req, &protocol, &token);
            });
        }
    });

    Some(GatewayHttp { port, token })
}

/// Handle one HTTP request: token first, then method, then MCP dispatch.
fn serve_one(
    mut req: tiny_http::Request,
    protocol: &agentstack_mcp::HttpServer<GatewayBackend>,
    token: &str,
) {
    let authed = req.headers().iter().any(|h| {
        h.field.equiv("X-Agentstack-Token")
            && crate::util::ct_eq(h.value.as_str().as_bytes(), token.as_bytes())
    });
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
            forward(req, protocol, "POST", body.into_bytes());
        }
        // The optional SSE channel: refusing it is spec-legal and
        // spike-verified — the client proceeds without a stream.
        Method::Get => {
            let resp = Response::from_string(String::new())
                .with_status_code(405)
                .with_header(allow_post());
            let _ = req.respond(resp);
        }
        Method::Delete => {
            forward(req, protocol, "DELETE", Vec::new());
        }
        _ => {
            let resp = Response::from_string(String::new())
                .with_status_code(405)
                .with_header(allow_post());
            let _ = req.respond(resp);
        }
    }
}

fn forward(
    req: tiny_http::Request,
    protocol: &agentstack_mcp::HttpServer<GatewayBackend>,
    method: &str,
    body: Vec<u8>,
) {
    let headers = req
        .headers()
        .iter()
        .map(|header| {
            (
                header.field.as_str().to_string(),
                header.value.as_str().to_string(),
            )
        })
        .collect();
    let result = protocol.handle_parts(agentstack_mcp::HttpRequest {
        method: method.into(),
        uri: req.url().to_owned(),
        headers,
        body,
    });
    match result {
        Ok(result) => {
            let mut response = Response::from_data(result.body).with_status_code(result.status);
            for (name, value) in &result.headers {
                if let Ok(header) = Header::from_bytes(name.as_str().as_bytes(), value.as_bytes()) {
                    response = response.with_header(header);
                }
            }
            let _ = req.respond(response);
        }
        Err(_) => {
            let response =
                Response::from_string(json!({ "error": "MCP transport failure" }).to_string())
                    .with_status_code(500)
                    .with_header(json_ctype());
            let _ = req.respond(response);
        }
    }
}

struct GatewayBackend(Arc<Gateway>);

impl agentstack_mcp::Backend for GatewayBackend {
    fn list_tools(
        &self,
        _era: agentstack_mcp::ProtocolEra,
    ) -> Result<Vec<agentstack_mcp::ToolDefinition>, String> {
        self.0
            .namespaced_tools()
            .iter()
            .cloned()
            .map(agentstack_mcp::ToolDefinition::try_from)
            .map(|result| result.map_err(|error| error.to_string()))
            .collect()
    }

    fn call_tool(
        &self,
        name: &str,
        arguments: Value,
        _era: agentstack_mcp::ProtocolEra,
    ) -> Result<agentstack_mcp::ToolOutcome, String> {
        match self.0.try_call(name, &arguments) {
            Some(Ok(value)) => Ok(agentstack_mcp::ToolOutcome::from_mcp_result(value)),
            Some(Err(error)) => Ok(agentstack_mcp::ToolOutcome::error(Value::String(format!(
                "Error: {error}"
            )))),
            // A name this project does not proxy is a TOOL error, not a
            // protocol error — the same shape the stdio serve loop returns, so
            // an agent reaching both transports reads one answer. Returning
            // `Err` here made RMCP emit a JSON-RPC error instead, which many
            // harnesses surface as a broken server rather than a wrong call.
            None => Ok(agentstack_mcp::ToolOutcome::error(Value::String(format!(
                "Error: '{name}' is not a proxied tool for this project"
            )))),
        }
    }
}

fn json_ctype() -> Header {
    Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
        .expect("literal ASCII header name and value are always valid")
}

fn allow_post() -> Header {
    Header::from_bytes(&b"Allow"[..], &b"POST"[..])
        .expect("literal ASCII header name and value are always valid")
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

    /// Both transports answer an unproxied name the same way: an MCP tool
    /// result carrying `isError`, never a JSON-RPC protocol error. Policy
    /// denials and upstream failures already ride that shape on this path, and
    /// a name the project does not proxy is the same class of answer — the
    /// call was wrong, the server is not.
    #[test]
    fn unknown_tool_is_a_tool_error_not_a_protocol_error() {
        use agentstack_mcp::Backend;

        let backend = GatewayBackend(Arc::new(Gateway::empty()));
        let outcome = backend
            .call_tool(
                "figma__get_file",
                json!({}),
                agentstack_mcp::ProtocolEra::Legacy,
            )
            .expect("an unproxied name must not fail the protocol");
        assert!(outcome.is_error, "the tool result must be flagged an error");
        assert!(
            outcome
                .value
                .as_str()
                .is_some_and(|text| text.contains("not a proxied tool")),
            "the text must say what went wrong: {:?}",
            outcome.value
        );
    }

    /// A sandboxed harness reaches this endpoint over a container alias and
    /// sends `Accept: application/json` — it predates the 2025 rule that a POST
    /// must also accept `text/event-stream`. The hand-written bridge answered
    /// such a client in plain JSON, and it must keep doing so: inside a sandbox
    /// there is no other route to the gateway, and a 406 turns every proxied
    /// call (including the DENIED ones the audit log exists to record) into a
    /// transport failure that never reaches the gateway at all.
    #[test]
    fn a_json_only_client_is_answered_in_plain_json() {
        let handle = start(std::sync::Arc::new(Gateway::empty()), "127.0.0.1:0").unwrap();
        let url = format!("http://127.0.0.1:{}/mcp", handle.port);
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .build()
            .unwrap();
        let post = |accept: Option<&str>, body: &Value| {
            let mut request = client
                .post(&url)
                .header("X-Agentstack-Token", &handle.token)
                .header("Content-Type", "application/json");
            if let Some(accept) = accept {
                request = request.header("Accept", accept);
            }
            request.body(body.to_string()).send().unwrap()
        };
        let json_body = |resp: reqwest::blocking::Response| -> Value {
            let ctype = resp
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string();
            assert_eq!(resp.status().as_u16(), 200);
            assert!(
                ctype.starts_with("application/json"),
                "a json-only client must never be handed a stream: {ctype}"
            );
            resp.json().unwrap()
        };

        let init = json!({
            "jsonrpc": "2.0", "id": 0, "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "sandboxed-harness", "version": "1" }
            }
        });
        // Every shape an older client sends: json only, `*/*`, and no header.
        for accept in [Some("application/json"), Some("*/*"), None] {
            let resp = post(accept, &init);
            assert_eq!(
                resp.status().as_u16(),
                200,
                "Accept {accept:?} was refused: {}",
                resp.text().unwrap_or_default()
            );
            let body = json_body(resp);
            assert_eq!(body["result"]["serverInfo"]["name"], "agentstack-gateway");
        }

        // The call is the point, and this is the exact shape the sandboxed
        // client sends: a bare `tools/call`, no handshake, no session. There is
        // no other route to the gateway from inside a container, so a refusal
        // here loses the call entirely — including the DENIED ones the audit
        // log exists to record.
        // `*/*` is what the container's `fetch` sends when the caller sets only
        // content-type, which is exactly what the sandbox client does.
        let body = json_body(post(
            Some("*/*"),
            &json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": "figma__get_file", "arguments": {} }
            }),
        ));
        assert_eq!(body["result"]["isError"], true, "{body}");

        // And discovery on the same terms.
        let body = json_body(post(
            Some("application/json"),
            &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
        ));
        assert_eq!(body["result"]["tools"], json!([]));
    }

    /// The outer token gate remains authoritative, and every request is served
    /// on its own — no session to establish, in either protocol era. That was
    /// the hand-written bridge's contract and it is the only one a sandboxed
    /// client can hold up its end of.
    #[test]
    fn socket_gates_token_and_serves_both_protocol_eras_without_a_session() {
        let handle = start(std::sync::Arc::new(Gateway::empty()), "127.0.0.1:0").unwrap();
        let url = format!("http://127.0.0.1:{}/mcp", handle.port);
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .build()
            .unwrap();
        let init = json!({
            "jsonrpc": "2.0", "id": 0, "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "agentstack-test", "version": "1" }
            }
        });

        // No token → 401 before any MCP handling.
        let resp = client.post(&url).json(&init).send().unwrap();
        assert_eq!(resp.status().as_u16(), 401);

        // Authed initialize → 200, and no session is opened for it.
        let resp = client
            .post(&url)
            .header("X-Agentstack-Token", &handle.token)
            .header("Accept", "application/json, text/event-stream")
            .json(&init)
            .send()
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        assert!(resp.headers().get("mcp-session-id").is_none());

        let discover = json!({
            "jsonrpc": "2.0", "id": 1, "method": "server/discover",
            "params": { "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientInfo": { "name": "agentstack-test", "version": "1" },
                "io.modelcontextprotocol/clientCapabilities": {}
            }}
        });
        let resp = client
            .post(&url)
            .header("X-Agentstack-Token", &handle.token)
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", "2026-07-28")
            .header("Mcp-Method", "server/discover")
            .json(&discover)
            .send()
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        assert!(resp.headers().get("mcp-session-id").is_none());
        let body: Value = resp.json().unwrap();
        assert!(body["result"]["supportedVersions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|version| version == "2026-07-28"));

        let list = json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/list",
            "params": { "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientInfo": { "name": "agentstack-test", "version": "1" },
                "io.modelcontextprotocol/clientCapabilities": {}
            }}
        });
        let mut surfaces = Vec::new();
        for _ in 0..2 {
            let resp = client
                .post(&url)
                .header("X-Agentstack-Token", &handle.token)
                .header("Accept", "application/json, text/event-stream")
                .header("MCP-Protocol-Version", "2026-07-28")
                .header("Mcp-Method", "tools/list")
                .json(&list)
                .send()
                .unwrap();
            assert_eq!(resp.status().as_u16(), 200);
            assert!(resp.headers().get("mcp-session-id").is_none());
            let body: Value = resp.json().unwrap();
            surfaces.push(body["result"]["tools"].clone());
        }
        assert_eq!(surfaces[0], surfaces[1]);

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
