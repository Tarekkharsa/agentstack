//! Embedded localhost dashboard server (PLAN §9f). Binds 127.0.0.1 only, gates
//! the JSON API behind a one-time token, and serves a self-contained UI baked
//! into the binary. No Node, no external assets — still one auditable binary.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Result};
use owo_colors::OwoColorize;
use serde_json::Value;
use tiny_http::{Header, Method, Request, Response, Server};

use crate::cli::DashboardArgs;
use crate::scope::Scope;

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

    let mode = if args.read_only {
        "read-only"
    } else {
        "read-write"
    };
    println!("{} dashboard at {}", "✓".green(), url.bold());
    println!("  (localhost only · token-gated · {mode} · Ctrl-C to stop)");
    if !args.no_open {
        let _ = open_browser(&url);
    }

    for request in server.incoming_requests() {
        handle(request, &token, args.read_only, dir.as_deref());
    }
    Ok(())
}

fn handle(mut request: Request, token: &str, read_only: bool, dir: Option<&Path>) {
    let method = request.method().clone();
    let url = request.url().to_string();
    let (path, query) = split_url(&url);
    let authed = token_ok(query, &request, token);

    // Read the body up front for POSTs (mutations).
    let body = if method == Method::Post {
        let mut s = String::new();
        let _ = request.as_reader().read_to_string(&mut s);
        s
    } else {
        String::new()
    };

    let response = route(&method, path, authed, read_only, &body, dir);
    let _ = request.respond(response);
}

fn route(
    method: &Method,
    path: &str,
    authed: bool,
    read_only: bool,
    body: &str,
    dir: Option<&Path>,
) -> Resp {
    match (method, path) {
        (Method::Get, "/") => html(INDEX_HTML),
        (Method::Get, "/app.js") => asset(APP_JS, "application/javascript"),
        (Method::Get, "/styles.css") => asset(STYLES_CSS, "text/css"),
        (Method::Get, "/api/state") => {
            if !authed {
                return unauthorized();
            }
            match crate::dashboard::snapshot::build(dir) {
                Ok(mut v) => {
                    if let Some(o) = v.as_object_mut() {
                        o.insert("readOnly".into(), Value::Bool(read_only));
                    }
                    json(&serde_json::to_string(&v).unwrap_or_default())
                }
                Err(e) => json(&format!("{{\"error\":{:?}}}", e.to_string())),
            }
        }
        (Method::Post, "/api/secret") => mutation(authed, read_only, || {
            let v = parse(body);
            let name = field(&v, "name")?;
            let value = field(&v, "value")?;
            crate::secret::keychain::set(&name, &value)
        }),
        (Method::Post, "/api/apply") => mutation(authed, read_only, || {
            let args = crate::cli::ApplyArgs {
                targets: vec![],
                profile: None,
                dry_run: false,
                write: true,
                scope: Some(scope_of(body)),
            };
            crate::commands::apply::run(&args, dir)
        }),
        (Method::Post, "/api/install") => mutation(authed, read_only, || {
            crate::commands::install::run(&crate::cli::InstallArgs { locked: false }, dir)
        }),
        (Method::Post, "/api/use") => mutation(authed, read_only, || {
            let v = parse(body);
            let profile = field(&v, "profile")?;
            let args = crate::cli::UseArgs {
                profile,
                targets: vec![],
                scope: Some(scope_of(body)),
                write: true,
            };
            crate::commands::use_profile::run(&args, dir)
        }),
        _ => Response::from_string("not found").with_status_code(404),
    }
}

/// Run a write action, enforcing auth + read-only, returning a JSON result.
fn mutation<F: FnOnce() -> Result<()>>(authed: bool, read_only: bool, f: F) -> Resp {
    if !authed {
        return unauthorized();
    }
    if read_only {
        return json("{\"error\":\"dashboard is read-only\"}").with_status_code(403);
    }
    match f() {
        Ok(()) => json("{\"ok\":true}"),
        Err(e) => json(&format!("{{\"error\":{:?}}}", e.to_string())).with_status_code(500),
    }
}

fn parse(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or(Value::Null)
}

fn field(v: &Value, key: &str) -> Result<String> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("missing field '{key}'"))
}

fn scope_of(body: &str) -> Scope {
    match parse(body).get("scope").and_then(Value::as_str) {
        Some("project") => Scope::Project,
        _ => Scope::Global,
    }
}

fn unauthorized() -> Resp {
    Response::from_string("unauthorized").with_status_code(401)
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
