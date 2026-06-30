//! Loopback runtime endpoint for code mode (PLAN code-mode Phase 2, transport
//! option "loopback HTTP, token-gated, project-scoped"). It mirrors the
//! dashboard server's localhost+token pattern: binds `127.0.0.1` only, gates
//! every call behind a one-time token, and forwards `{ name, arguments }`
//! straight through the gateway's existing `try_call` path. Secrets are resolved
//! by the gateway, never by the generated client.
//!
//! agentstack does **not** execute the agent's code here — the harness runs the
//! generated client in its own sandbox and that client POSTs here. This endpoint
//! only brokers the real upstream MCP call.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use tiny_http::{Header, Response, Server};

use crate::gateway::Gateway;

/// A running runtime endpoint. Dropping/`shutdown`-ing removes the machine-local
/// `endpoint.json` so a stale port+token isn't left pointing at a dead socket.
pub struct RuntimeHandle {
    endpoint_path: PathBuf,
    /// The base loopback URL the shim POSTs to (for logging).
    pub url: String,
}

impl RuntimeHandle {
    /// Best-effort cleanup of the endpoint coordinate file.
    pub fn shutdown(self) {
        let _ = std::fs::remove_file(&self.endpoint_path);
    }
}

/// Start the endpoint for the project at `dir`. Best-effort and side-effect
/// contained: returns `None` when there are no HTTP upstreams to proxy or the
/// loopback socket / coordinate file can't be created. Serves calls on a
/// detached thread until the process exits.
pub fn start(dir: Option<&Path>) -> Option<RuntimeHandle> {
    let gateway = Gateway::from_manifest(dir);
    if gateway.is_empty() {
        return None;
    }
    let server = Server::http("127.0.0.1:0").ok()?;
    let port = server.server_addr().to_ip().map(|a| a.port())?;
    let token = gen_token();
    let url = format!("http://127.0.0.1:{port}/call");

    let cmdir = crate::codemode::codemode_dir(dir);
    std::fs::create_dir_all(&cmdir).ok()?;
    let endpoint_path = cmdir.join("endpoint.json");
    let record = json!({ "url": url, "token": token });
    crate::util::atomic::write(&endpoint_path, &format!("{record}\n")).ok()?;

    let token_for_thread = token;
    std::thread::spawn(move || {
        // Single-threaded request loop: the gateway is `Send` but not `Sync`, so
        // we never share it across threads — one thread owns it for its lifetime.
        for mut req in server.incoming_requests() {
            let authed = req.headers().iter().any(|h| {
                h.field.equiv("X-Agentstack-Token") && h.value.as_str() == token_for_thread
            });
            let mut body = String::new();
            let _ = req.as_reader().read_to_string(&mut body);
            let (status, payload) = if !authed {
                (
                    401,
                    json!({ "error": "unauthorized — endpoint token mismatch" }).to_string(),
                )
            } else {
                handle_runtime_call(&gateway, &body)
            };
            let resp = Response::from_string(payload)
                .with_status_code(status)
                .with_header(json_ctype());
            let _ = req.respond(resp);
        }
    });

    Some(RuntimeHandle { endpoint_path, url })
}

/// Forward one `{ name, arguments }` call through the gateway and shape the HTTP
/// reply. Returns `(status, json_body)`. Pure over the gateway, so it is
/// unit-testable without a socket. The body is always `{ "result": … }` or
/// `{ "error": … }`.
pub fn handle_runtime_call(gateway: &Gateway, body: &str) -> (u16, String) {
    let v: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => {
            return (
                400,
                json!({ "error": format!("invalid JSON: {e}") }).to_string(),
            )
        }
    };
    let name = v.get("name").and_then(Value::as_str).unwrap_or("");
    if name.is_empty() {
        return (
            400,
            json!({ "error": "missing 'name' (expected \"<server>__<tool>\")" }).to_string(),
        );
    }
    let args = v.get("arguments").cloned().unwrap_or_else(|| json!({}));
    match gateway.try_call(name, &args) {
        Some(Ok(result)) => (200, json!({ "result": result }).to_string()),
        // try_call surfaces unresolved-secret and upstream errors with a clear
        // message — pass it straight to the caller.
        Some(Err(e)) => (502, json!({ "error": e.to_string() }).to_string()),
        None => (
            404,
            json!({
                "error": format!(
                    "'{name}' is not a proxied tool — it must be <server>__<tool> for a server this manifest declares"
                )
            })
            .to_string(),
        ),
    }
}

fn json_ctype() -> Header {
    Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap()
}

/// A localhost one-time token (same role as the dashboard's): not a hard
/// boundary — the socket is already 127.0.0.1-only — it just stops other local
/// processes from poking the endpoint.
fn gen_token() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seed = nanos ^ ((std::process::id() as u128) << 64);
    let mut h: u64 = 0xcbf29ce484222325;
    for b in seed.to_le_bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    let mut h2: u64 = h ^ 0x9e3779b97f4a7c15;
    for b in nanos.to_be_bytes() {
        h2 ^= b as u64;
        h2 = h2.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}{h2:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_tool_is_404() {
        // An empty gateway proxies nothing, so any name is "not a proxied tool".
        let gw = Gateway::empty();
        let (status, body) =
            handle_runtime_call(&gw, &json!({ "name": "figma__get_file" }).to_string());
        assert_eq!(status, 404);
        assert!(body.contains("not a proxied tool"));
    }

    #[test]
    fn malformed_requests_are_400() {
        let gw = Gateway::empty();
        let (s1, _) = handle_runtime_call(&gw, "{not json");
        assert_eq!(s1, 400);
        let (s2, b2) = handle_runtime_call(&gw, &json!({ "arguments": {} }).to_string());
        assert_eq!(s2, 400);
        assert!(b2.contains("missing 'name'"));
    }

    #[test]
    fn tokens_are_stable_length_hex() {
        let t = gen_token();
        assert_eq!(t.len(), 32);
        assert!(t.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
