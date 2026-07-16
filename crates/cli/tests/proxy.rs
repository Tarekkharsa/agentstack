// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Wire-proxy relay + telemetry, end to end and hermetic (127.0.0.1 only).
//!
//! Stands up a stub upstream (a second tiny_http in-test), starts `proxy::serve`
//! pointed at it on ephemeral ports, and sends a POST /v1/messages with a small
//! tools array. Asserts the client gets the upstream status + body verbatim and
//! that exactly one telemetry record lands with the expected per-capability
//! buckets and best-effort response usage.

use std::sync::Mutex;

use agentstack::proxy::{self, ProxyConfig};
use tiny_http::{Header, Response, Server};

// The proxy records into `AGENTSTACK_HOME`; serialize env mutation across tests.
static ENV_LOCK: Mutex<()> = Mutex::new(());

const CANNED_RESPONSE: &str = r#"{
  "id": "msg_1",
  "type": "message",
  "role": "assistant",
  "model": "claude-opus-4-8",
  "content": [
    { "type": "text", "text": "ok" },
    { "type": "tool_use", "id": "tu_1", "name": "mcp__figma__get_file", "input": {} }
  ],
  "usage": { "input_tokens": 1234, "output_tokens": 56, "cache_read_input_tokens": 1000 }
}"#;

/// A canned JSON messages response upstream on an ephemeral loopback port.
fn spawn_stub_upstream() -> u16 {
    let server = Server::http("127.0.0.1:0").unwrap();
    let port = server.server_addr().to_ip().map(|a| a.port()).unwrap();
    std::thread::spawn(move || {
        for mut req in server.incoming_requests() {
            // Drain the request body so the socket is clean.
            let mut body = Vec::new();
            let _ = req.as_reader().read_to_end(&mut body);
            let resp = Response::from_string(CANNED_RESPONSE).with_header(
                Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap(),
            );
            let _ = req.respond(resp);
        }
    });
    port
}

fn free_port() -> u16 {
    // Bind :0, read the port, drop the listener — the proxy re-binds it.
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

#[test]
fn relays_verbatim_and_records_one_request() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    std::env::set_var("AGENTSTACK_HOME", tmp.path());

    let upstream_port = spawn_stub_upstream();
    let proxy_port = free_port();

    let config = ProxyConfig {
        port: proxy_port,
        upstream: format!("http://127.0.0.1:{upstream_port}"),
    };
    std::thread::spawn(move || {
        let _ = proxy::serve(config);
    });

    // Wait for the proxy socket to accept connections.
    wait_for_port(proxy_port);

    let request_body = serde_json::json!({
        "model": "claude-opus-4-8",
        "max_tokens": 16,
        "messages": [{ "role": "user", "content": "hi" }],
        "tools": [
            { "name": "mcp__figma__get_file", "description": "get a file", "input_schema": {} },
            { "name": "mcp__figma__create_frame", "description": "make a frame", "input_schema": {} },
            { "name": "Read", "description": "read a file", "input_schema": {} }
        ]
    });

    let client = reqwest::blocking::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{proxy_port}/v1/messages"))
        .header("x-api-key", "test-key")
        .header("anthropic-version", "2023-06-01")
        .json(&request_body)
        .send()
        .expect("proxied request should succeed");

    // (1) client gets the upstream status + body verbatim.
    assert_eq!(resp.status().as_u16(), 200);
    let text = resp.text().unwrap();
    assert_eq!(text, CANNED_RESPONSE);

    // (2) exactly one record with the expected per-capability buckets.
    let records = wait_for_records(1);
    assert_eq!(records.len(), 1, "exactly one record per proxied request");
    let rec = &records[0];
    assert_eq!(rec.model.as_deref(), Some("claude-opus-4-8"));
    assert_eq!(rec.total_tools, 3);
    assert_eq!(rec.per_capability["figma"].tools, 2);
    assert_eq!(rec.per_capability["builtin"].tools, 1);
    assert!(rec.per_capability["figma"].est_tokens > 0);
    assert!(!rec.streamed);

    // Best-effort response usage + tool_use names were captured (JSON path).
    assert_eq!(rec.input_tokens, Some(1234));
    assert_eq!(rec.output_tokens, Some(56));
    assert_eq!(rec.cache_read_input_tokens, Some(1000));
    assert_eq!(rec.tool_use, vec!["mcp__figma__get_file".to_string()]);

    std::env::remove_var("AGENTSTACK_HOME");
}

fn wait_for_port(port: u16) {
    for _ in 0..100 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("proxy never came up on port {port}");
}

fn wait_for_records(n: usize) -> Vec<proxy::RequestRecord> {
    for _ in 0..100 {
        let records = proxy::read_all();
        if records.len() >= n {
            return records;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    proxy::read_all()
}
