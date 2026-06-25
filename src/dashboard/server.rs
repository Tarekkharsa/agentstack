//! Embedded localhost dashboard server (PLAN §9f). Binds 127.0.0.1 only, gates
//! the JSON API behind a one-time token, and serves a self-contained UI baked
//! into the binary. No Node, no external assets — still one auditable binary.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Result};
use owo_colors::OwoColorize;
use tiny_http::{Header, Response, Server};

use crate::cli::DashboardArgs;

const INDEX_HTML: &str = include_str!("assets/index.html");
const APP_JS: &str = include_str!("assets/app.js");
const STYLES_CSS: &str = include_str!("assets/styles.css");

pub fn serve(args: &DashboardArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let dir = manifest_dir.map(Path::to_path_buf);
    let addr = format!("127.0.0.1:{}", args.port.unwrap_or(0));
    let server = Server::http(&addr).map_err(|e| anyhow!("binding {addr}: {e}"))?;
    let port = server.server_addr().to_ip().map(|a| a.port()).unwrap_or(0);
    let token = gen_token();
    let url = format!("http://127.0.0.1:{port}/?token={token}");

    println!("{} dashboard at {}", "✓".green(), url.bold());
    println!("  (localhost only · token-gated · Ctrl-C to stop)");
    if !args.no_open {
        let _ = open_browser(&url);
    }

    for request in server.incoming_requests() {
        let url_str = request.url().to_string();
        let (path, query) = split_url(&url_str);

        let response = match path {
            "/" => html(INDEX_HTML),
            "/app.js" => asset(APP_JS, "application/javascript"),
            "/styles.css" => asset(STYLES_CSS, "text/css"),
            "/api/state" => {
                if !token_ok(query, &request, &token) {
                    Response::from_string("unauthorized").with_status_code(401)
                } else {
                    match crate::dashboard::snapshot::build(dir.as_deref()) {
                        Ok(v) => json(&serde_json::to_string(&v).unwrap_or_default()),
                        Err(e) => json(&format!("{{\"error\":{:?}}}", e.to_string())),
                    }
                }
            }
            _ => Response::from_string("not found").with_status_code(404),
        };
        let _ = request.respond(response);
    }
    Ok(())
}

type Resp = Response<std::io::Cursor<Vec<u8>>>;

fn html(body: &str) -> Resp {
    asset(body, "text/html; charset=utf-8")
}

fn asset(body: &str, content_type: &str) -> Resp {
    Response::from_string(body).with_header(ctype(content_type))
}

fn json(body: &str) -> Resp {
    Response::from_string(body).with_header(ctype("application/json"))
}

fn ctype(value: &str) -> Header {
    Header::from_bytes(&b"Content-Type"[..], value.as_bytes()).unwrap()
}

fn split_url(url: &str) -> (&str, &str) {
    match url.split_once('?') {
        Some((p, q)) => (p, q),
        None => (url, ""),
    }
}

/// Token must arrive as `?token=` or the `X-Agentstack-Token` header.
fn token_ok(query: &str, request: &tiny_http::Request, token: &str) -> bool {
    let from_query = query
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .any(|(k, v)| k == "token" && v == token);
    let from_header = request
        .headers()
        .iter()
        .any(|h| h.field.equiv("X-Agentstack-Token") && h.value.as_str() == token);
    from_query || from_header
}

/// A localhost one-time token. Not a hard security boundary (the server is
/// already 127.0.0.1-only) — it stops other local pages/processes from poking
/// the API.
fn gen_token() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id() as u128;
    let mut h: u64 = 0xcbf29ce484222325;
    for b in (nanos ^ (pid << 64)).to_le_bytes() {
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

fn open_browser(url: &str) -> Result<()> {
    let (cmd, args): (&str, Vec<&str>) = if cfg!(target_os = "macos") {
        ("open", vec![url])
    } else if cfg!(target_os = "windows") {
        ("cmd", vec!["/C", "start", url])
    } else {
        ("xdg-open", vec![url])
    };
    std::process::Command::new(cmd).args(args).spawn()?;
    Ok(())
}

/// Kept for potential future use (static asset dir overrides).
#[allow(dead_code)]
fn user_assets_dir() -> PathBuf {
    crate::util::paths::agentstack_home().join("dashboard")
}
