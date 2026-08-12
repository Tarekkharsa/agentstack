//! Minimal unauthenticated HTTP host for the official MCP conformance runner.
//!
//! Production AgentStack keeps its token gate outside `HttpServer`; this
//! example intentionally omits that application security wrapper so the
//! protocol adapter can be exercised directly:
//!
//! `cargo run -p agentstack-mcp --example conformance_server -- 127.0.0.1:3030`

use std::{io::Read, sync::Arc};

use agentstack_mcp::{Backend, HttpRequest, HttpServer, ProtocolEra, ToolDefinition, ToolOutcome};
use serde_json::{json, Value};
use tiny_http::{Header, Method, Response, Server};

struct ConformanceBackend;

impl Backend for ConformanceBackend {
    fn list_tools(&self, _era: ProtocolEra) -> Result<Vec<ToolDefinition>, String> {
        vec![json!({
            "name": "echo",
            "description": "Echo the supplied text.",
            "inputSchema": {
                "type": "object",
                "properties": { "text": { "type": "string" } }
            }
        })]
        .into_iter()
        .map(ToolDefinition::try_from)
        .map(|result| result.map_err(|error| error.to_string()))
        .collect()
    }

    fn call_tool(
        &self,
        name: &str,
        arguments: Value,
        _era: ProtocolEra,
    ) -> Result<ToolOutcome, String> {
        if name != "echo" {
            return Err(format!("unknown tool: {name}"));
        }
        Ok(ToolOutcome::success(
            arguments.get("text").cloned().unwrap_or(Value::Null),
        ))
    }
}

fn main() -> anyhow::Result<()> {
    let address = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:3030".to_owned());
    let listener = Server::http(&address).map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let protocol = Arc::new(HttpServer::new(
        Arc::new(ConformanceBackend),
        "agentstack-conformance",
        env!("CARGO_PKG_VERSION"),
    )?);
    eprintln!("agentstack-mcp conformance endpoint: http://{address}/mcp");

    for request in listener.incoming_requests() {
        let protocol = Arc::clone(&protocol);
        std::thread::spawn(move || serve(request, &protocol));
    }
    Ok(())
}

fn serve(mut request: tiny_http::Request, protocol: &HttpServer<ConformanceBackend>) {
    let method = match request.method() {
        Method::Post => "POST",
        Method::Delete => "DELETE",
        Method::Get => "GET",
        _ => {
            let _ = request.respond(Response::empty(405));
            return;
        }
    };
    let headers = request
        .headers()
        .iter()
        .map(|header| {
            (
                header.field.as_str().to_string(),
                header.value.as_str().to_string(),
            )
        })
        .collect();
    let mut body = Vec::new();
    if request
        .as_reader()
        .take(4 * 1024 * 1024)
        .read_to_end(&mut body)
        .is_err()
    {
        let _ = request.respond(Response::empty(400));
        return;
    }
    let Ok(result) = protocol.handle_parts(HttpRequest {
        method: method.to_owned(),
        uri: request.url().to_owned(),
        headers,
        body,
    }) else {
        let _ = request.respond(Response::empty(500));
        return;
    };
    let mut response = Response::from_data(result.body).with_status_code(result.status);
    for (name, value) in &result.headers {
        if let Ok(header) = Header::from_bytes(name.as_str().as_bytes(), value.as_bytes()) {
            response = response.with_header(header);
        }
    }
    let _ = request.respond(response);
}
