//! Loopback runtime endpoint for code mode (PLAN code-mode Phase 2, transport
//! option "loopback HTTP, token-gated, project-scoped"). It mirrors the
//! local control endpoint's localhost-plus-token pattern: binds `127.0.0.1` only, gates
//! every call behind a one-time token, and forwards `{ name, arguments }`
//! straight through the gateway's existing `try_call` path. Secrets are resolved
//! by the gateway, never by the generated client.
//!
//! agentstack does **not** execute the agent's code here — the harness runs the
//! generated client in its own sandbox and that client POSTs here. This endpoint
//! only brokers the real upstream MCP call.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};
use tiny_http::{Header, Response, Server};

use crate::gateway::Gateway;

/// Decrements the in-flight counter when a served request finishes, even on a
/// panic in the handler — so a panic can't permanently consume a slot.
/// (Same pattern as `crate::gateway_http`.)
struct InflightGuard(Arc<AtomicUsize>);
impl Drop for InflightGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Release);
    }
}

/// Cap on concurrently-served requests. Each authed request gets its own OS
/// thread (see `serve_loop`); without a cap a runaway client could exhaust
/// host threads. The socket is loopback-only and the token gates every call,
/// so — like `gateway_http`'s identical cap — this is defense-in-depth
/// against a buggy or compromised *local* process, not a remote surface.
/// Excess requests get a fast `503` instead of a thread.
const MAX_INFLIGHT: usize = 64;

/// Hard cap on an authed request body (CLAUDE.md rule 7 — bound sizes on
/// hostile input). Matches `MAX_FRAME_BYTES` in
/// `crates/egress/src/execution_relay.rs`: code-mode call payloads are small
/// JSON, and 1 MiB is a generous ceiling that still refuses an OOM attempt.
const MAX_BODY_BYTES: u64 = 1024 * 1024;

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

/// Start the endpoint for the project at `dir`, serving calls through the
/// caller's `gateway` — the same one the MCP serve loop uses, so upstream
/// connections (and lazily spawned stdio children) exist once per process,
/// not once per surface. Best-effort and side-effect contained: returns
/// `None` when there is nothing to proxy or the loopback socket / coordinate
/// file can't be created. Serves calls on a detached thread until the process
/// exits.
pub fn start(dir: Option<&Path>, gateway: Arc<Gateway>) -> Option<RuntimeHandle> {
    if gateway.is_empty() {
        return None;
    }
    let server = Server::http("127.0.0.1:0").ok()?;
    let port = server.server_addr().to_ip().map(|a| a.port())?;
    let token = gen_token();
    let url = format!("http://127.0.0.1:{port}/call");

    let cmdir = crate::codemode::codemode_dir(dir);
    std::fs::create_dir_all(&cmdir).ok()?;
    // endpoint.json carries the bearer token for the proxied surface — it must
    // not be readable by other local users (default umask would leave it 0644).
    crate::util::restrict(&cmdir, true);
    let endpoint_path = cmdir.join("endpoint.json");
    let record = json!({ "url": url, "token": token });
    crate::util::atomic::write(&endpoint_path, &format!("{record}\n")).ok()?;
    crate::util::restrict(&endpoint_path, false);

    let token_for_thread = token;
    std::thread::spawn(move || serve_loop(server, gateway, token_for_thread));

    Some(RuntimeHandle { endpoint_path, url })
}

/// Accept loop only: each authed request is served on its own thread. The
/// gateway is Sync with per-upstream locking, so parallel code-mode calls to
/// different servers proceed concurrently — one slow upstream no longer
/// blocks the endpoint (or the stdio serve loop). Local, agent-driven
/// traffic: thread-per-request is plenty, and `MAX_INFLIGHT` bounds it.
fn serve_loop(server: Server, gateway: Arc<Gateway>, token: String) {
    let inflight = Arc::new(AtomicUsize::new(0));
    for mut req in server.incoming_requests() {
        // Bounded concurrency: shed load with a fast 503 rather than spawning
        // an unbounded number of threads. `fetch_add` then compare works as a
        // reservation because only this accept thread ever increments.
        if inflight.fetch_add(1, Ordering::AcqRel) >= MAX_INFLIGHT {
            inflight.fetch_sub(1, Ordering::Release);
            let resp = Response::from_string(json!({ "error": "server busy" }).to_string())
                .with_status_code(503)
                .with_header(json_ctype());
            let _ = req.respond(resp);
            continue;
        }
        let guard = InflightGuard(Arc::clone(&inflight));
        let gateway = Arc::clone(&gateway);
        let token = token.clone();
        std::thread::spawn(move || {
            let _guard = guard; // released (decrementing) on thread exit
            let authed = req.headers().iter().any(|h| {
                // Constant-time comparison — same reasoning as the gateway HTTP
                // endpoint (see crate::util::ct_eq): a plain `==` short-circuits
                // on the first mismatched byte and leaks a timing signal.
                h.field.equiv("X-Agentstack-Token")
                    && crate::util::ct_eq(h.value.as_str().as_bytes(), token.as_bytes())
            });
            // Token first: an unauthenticated caller is answered 401 before
            // the endpoint reads (let alone buffers) a single body byte.
            if !authed {
                let resp = Response::from_string(
                    json!({ "error": "unauthorized — endpoint token mismatch" }).to_string(),
                )
                .with_status_code(401)
                .with_header(json_ctype());
                let _ = req.respond(resp);
                return;
            }
            let mut body = String::new();
            // `take` caps the read: a body that streams past the cap is
            // truncated here rather than buffered whole, then rejected below.
            let _ = req
                .as_reader()
                .take(MAX_BODY_BYTES + 1)
                .read_to_string(&mut body);
            let (status, payload) = if body.len() as u64 > MAX_BODY_BYTES {
                (
                    413,
                    json!({ "error": "request body too large" }).to_string(),
                )
            } else {
                handle_runtime_call(&gateway, &body)
            };
            let resp = Response::from_string(payload)
                .with_status_code(status)
                .with_header(json_ctype());
            let _ = req.respond(resp);
        });
    }
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
    Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
        .expect("literal ASCII header name and value are always valid")
}

/// A per-session bearer token for the loopback endpoint. The socket is
/// 127.0.0.1-only, but the token is a real credential (it invokes proxied
/// tools), so it comes from the OS entropy pool — not a guessable
/// time/PID-derived hash.
fn gen_token() -> String {
    crate::util::random_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

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
        assert_eq!(t.len(), 64);
        assert!(t.chars().all(|c| c.is_ascii_hexdigit()));
    }

    /// Serve an empty gateway on an ephemeral loopback port with a known
    /// token, for socket-level tests (`start` itself refuses an empty
    /// gateway and mints its own token).
    fn spawn_test_endpoint() -> u16 {
        let server = Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        std::thread::spawn(move || serve_loop(server, Arc::new(Gateway::empty()), "tok".into()));
        port
    }

    /// The token is checked BEFORE the body is read: a tokenless request
    /// declaring a huge Content-Length and sending no body still gets its
    /// 401 promptly. If the endpoint buffered the body pre-auth (the old
    /// behavior), this read would hang until the timeout.
    #[test]
    fn unauthenticated_request_gets_401_without_a_body_read() {
        let port = spawn_test_endpoint();
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        s.write_all(b"POST /call HTTP/1.1\r\nHost: x\r\nContent-Length: 10737418240\r\n\r\n")
            .unwrap();
        let mut buf = [0u8; 256];
        let n = s
            .read(&mut buf)
            .expect("401 should arrive without the body");
        let head = String::from_utf8_lossy(&buf[..n]);
        assert!(head.starts_with("HTTP/1.1 401"), "got: {head}");
    }

    /// Once `MAX_INFLIGHT` authed requests park (each handler blocks in the
    /// bounded body read, holding its slot for the whole test), the accept loop
    /// sheds every further connection with a fast 503. So flooding the endpoint
    /// with more than `MAX_INFLIGHT` such connections MUST produce at least one
    /// 503 — a guarantee that holds however the runner schedules the accept
    /// loop. (The earlier saturate-exactly-then-probe version raced on that
    /// scheduling and flaked twice on loaded CI; this is bounded by a connection
    /// count, not wall-clock time.)
    #[test]
    fn request_over_the_inflight_cap_is_shed_with_503() {
        let port = spawn_test_endpoint();
        // Hold every parked connection open — dropping one frees its slot.
        let mut held = Vec::new();
        let mut saw_503 = false;
        for _ in 0..(MAX_INFLIGHT * 3) {
            let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            // A shed 503 is written by the accept loop immediately; a pinned
            // slot's handler blocks forever in the body read. A short read
            // timeout tells the two apart.
            s.set_read_timeout(Some(std::time::Duration::from_millis(300)))
                .unwrap();
            // Content-Length > 1024 so tiny_http yields the request to the serve
            // loop (and a handler thread) before the body arrives, pinning the
            // slot; the body never comes, so the handler parks.
            s.write_all(
                b"POST /call HTTP/1.1\r\nHost: x\r\nX-Agentstack-Token: tok\r\nContent-Length: 2048\r\n\r\n",
            )
            .unwrap();
            let mut buf = [0u8; 64];
            match s.read(&mut buf) {
                Ok(n) if String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 503") => {
                    saw_503 = true;
                    break;
                }
                // Parked (read timed out) or any other reply — keep the socket
                // open so its slot stays pinned, and keep flooding.
                _ => held.push(s),
            }
        }
        assert!(
            saw_503,
            "the accept loop must shed at least one over-cap request with 503"
        );
        drop(held);
    }
}
