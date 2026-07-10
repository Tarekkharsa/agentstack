//! Code mode (PLAN code-mode Phase 2) end-to-end: the generated client is built
//! from the *live* proxied surface, and a runtime call round-trips through the
//! gateway to a real (mock) upstream MCP server. Also proves `codemode --write`
//! materializes contained, secret-free files.

use std::sync::Mutex;
use std::thread;

use serde_json::{json, Value};
use tiny_http::{Header, Response, Server};

use agentstack::cli::CodemodeArgs;
use agentstack::codemode::endpoint;
use agentstack::gateway::Gateway;

// Tests mutate the process-global HOME/AGENTSTACK_HOME; serialize them.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// A minimal mock MCP HTTP server: answers `initialize`, `tools/list` (one
/// `echo` tool), and `tools/call` (echoes the arguments back). Returns its port.
fn start_mock_upstream() -> u16 {
    let server = Server::http("127.0.0.1:0").unwrap();
    let port = server.server_addr().to_ip().unwrap().port();
    thread::spawn(move || {
        for mut req in server.incoming_requests() {
            let mut body = String::new();
            let _ = req.as_reader().read_to_string(&mut body);
            let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
            let method = v.get("method").and_then(Value::as_str).unwrap_or("");
            let id = v.get("id").cloned().unwrap_or(Value::Null);
            let reply = match method {
                "initialize" => Some(json!({
                    "jsonrpc": "2.0", "id": id,
                    "result": { "protocolVersion": "2025-06-18", "capabilities": {}, "serverInfo": { "name": "mock", "version": "0" } }
                })),
                "tools/list" => Some(json!({
                    "jsonrpc": "2.0", "id": id,
                    "result": { "tools": [{
                        "name": "echo",
                        "description": "Echo the input back.",
                        "inputSchema": { "type": "object", "properties": { "msg": { "type": "string" } }, "required": ["msg"] }
                    }] }
                })),
                "tools/call" => {
                    let args = v
                        .get("params")
                        .and_then(|p| p.get("arguments"))
                        .cloned()
                        .unwrap_or_else(|| json!({}));
                    Some(json!({
                        "jsonrpc": "2.0", "id": id,
                        "result": { "content": [{ "type": "text", "text": "ok" }], "echoed": args }
                    }))
                }
                // notifications (e.g. notifications/initialized): accept, no body.
                _ => None,
            };
            let resp = match reply {
                Some(r) => Response::from_string(r.to_string()).with_header(
                    Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap(),
                ),
                None => Response::from_string("").with_status_code(202),
            };
            let _ = req.respond(resp);
        }
    });
    port
}

fn setup_project(home: &std::path::Path, proj: &std::path::Path, port: u16) {
    std::fs::create_dir_all(home).unwrap();
    std::env::set_var("HOME", home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));
    std::fs::create_dir_all(proj).unwrap();
    std::fs::write(
        proj.join("agentstack.toml"),
        format!(
            "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
             [servers.mock]\ntype = \"http\"\nurl = \"http://127.0.0.1:{port}/mcp\"\n"
        ),
    )
    .unwrap();
}

#[test]
fn endpoint_round_trips_through_gateway_to_mock_upstream() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let proj = tmp.path().join("proj");
    let port = start_mock_upstream();
    setup_project(&home, &proj, port);

    let gw = Gateway::from_manifest(Some(&proj));

    // The client is generated from the *live* discovered surface.
    let client = gw.generate_bindings().client_ts;
    assert!(client.contains("mock: {"), "client: {client}");
    assert!(client.contains(r#"call("mock__echo", input)"#));
    assert!(client.contains("msg: string"), "typed from schema");
    assert!(!client.contains("127.0.0.1"), "client is endpoint-agnostic");

    // A runtime call round-trips: shim → endpoint → gateway.try_call → upstream.
    let (status, body) = endpoint::handle_runtime_call(
        &gw,
        &json!({ "name": "mock__echo", "arguments": { "msg": "hi" } }).to_string(),
    );
    assert_eq!(status, 200, "body: {body}");
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["result"]["echoed"]["msg"], "hi");
}

#[test]
fn codemode_write_materializes_contained_secret_free_files() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let proj = tmp.path().join("proj");
    let port = start_mock_upstream();
    setup_project(&home, &proj, port);

    agentstack::commands::codemode::run(&CodemodeArgs { write: true }, Some(&proj)).unwrap();

    let dir = proj.join("codemode");
    let client = std::fs::read_to_string(dir.join("client.ts")).unwrap();
    assert!(client.contains(r#"call("mock__echo", input)"#));
    assert!(!client.contains("${"), "client carries no secret tokens");
    assert!(dir.join("agentstack-runtime.ts").exists());
    // endpoint.json (machine-local port+token) is gitignored, never committed.
    let gitignore = std::fs::read_to_string(dir.join(".gitignore")).unwrap();
    assert!(gitignore.contains("endpoint.json"));
}
