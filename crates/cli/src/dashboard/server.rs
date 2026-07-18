//! Embedded localhost dashboard server (PLAN §9f). Binds 127.0.0.1 only, gates
//! the JSON API behind a one-time token, and serves a self-contained UI baked
//! into the binary. No Node, no external assets — still one auditable binary.
//!
//! The dashboard is a **read-only lens**: it only ever answers GET requests
//! (snapshot, diffs, doctor, runs, audited calls, search). Every mutation lives
//! in the CLI, so the server exposes no write route at all — a POST to any path
//! simply falls through to 404. The read-only property is a property of the
//! router here, not of the UI.

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

    println!("{} dashboard at {}", "✓".green(), url.bold());
    println!("  (localhost only · token-gated · read-only · Ctrl-C to stop)");
    if !args.no_open {
        let _ = open_browser(&url);
    }

    for request in server.incoming_requests() {
        handle(request, &token, dir.as_deref());
    }
    Ok(())
}

fn handle(request: Request, token: &str, dir: Option<&Path>) {
    let method = request.method().clone();
    let url = request.url().to_string();
    let (path, query) = split_url(&url);
    let authed = token_ok(query, &request, token);

    let response = route(&method, path, query, authed, dir);
    let _ = request.respond(response);
}

/// The router only knows read (GET) routes. Anything else — including every
/// former write endpoint (`/api/apply`, `/api/toggle`, `/api/secret`, …) — has
/// no arm and falls through to 404. The dashboard cannot mutate disk: there is
/// no code path from an HTTP request to a write.
fn route(method: &Method, path: &str, query: &str, authed: bool, dir: Option<&Path>) -> Resp {
    match (method, path) {
        (Method::Get, "/") => html(INDEX_HTML),
        (Method::Get, "/app.js") => asset(APP_JS, "application/javascript"),
        (Method::Get, "/styles.css") => asset(STYLES_CSS, "text/css"),
        (Method::Get, "/api/state") => {
            if !authed {
                return unauthorized();
            }
            match crate::dashboard::snapshot::state(dir) {
                Ok(v) => json(&serde_json::to_string(&v).unwrap_or_default()),
                Err(e) => json(&format!("{{\"error\":{:?}}}", e.to_string())),
            }
        }
        (Method::Get, "/api/diff") => {
            if !authed {
                return unauthorized();
            }
            let all = query_param(query, "all").as_deref() == Some("1");
            match crate::dashboard::snapshot::diffs(dir, scope_of_query(query), all) {
                Ok(v) => json(&serde_json::to_string(&v).unwrap_or_default()),
                Err(e) => json(&format!("{{\"error\":{:?}}}", e.to_string())),
            }
        }
        (Method::Get, "/api/doctor") => {
            if !authed {
                return unauthorized();
            }
            // Same checks as `agentstack doctor` (fix/live off), structured.
            match crate::commands::doctor::collect(dir) {
                Ok(v) => json(&serde_json::to_string(&v).unwrap_or_default()),
                Err(e) => json(&format!("{{\"error\":{:?}}}", e.to_string())),
            }
        }
        (Method::Get, "/api/explain") => {
            if !authed {
                return unauthorized();
            }
            let name = query_param(query, "name").unwrap_or_default();
            match crate::commands::explain::explain_text(&name, dir) {
                Ok(text) => json(
                    &serde_json::to_string(&serde_json::json!({ "text": text }))
                        .unwrap_or_default(),
                ),
                Err(e) => json(&format!("{{\"error\":{:?}}}", e.to_string())),
            }
        }
        (Method::Get, "/api/history") => {
            if !authed {
                return unauthorized();
            }
            // Metadata only — the captured file contents stay on disk, not on the wire.
            let arr: Vec<Value> = crate::history::list()
                .iter()
                .map(|e| {
                    serde_json::json!({
                        "id": e.id,
                        "timeUnix": e.time_unix,
                        "scope": e.scope,
                        "summary": e.summary,
                        "targets": e.targets,
                        "undone": e.undone,
                        "files": e.files.iter().map(|f| serde_json::json!({
                            "path": f.path, "label": f.label, "existed": f.before.is_some(),
                        })).collect::<Vec<_>>(),
                    })
                })
                .collect();
            json(&serde_json::to_string(&serde_json::json!({ "entries": arr })).unwrap_or_default())
        }
        (Method::Get, "/api/runs") => {
            if !authed {
                return unauthorized();
            }
            let arr: Vec<Value> = crate::runs::list()
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "id": r.id, "harness": r.harness, "display": r.display,
                        "pid": r.pid, "profile": r.profile, "cwd": r.cwd,
                        "startedUnix": r.started_unix,
                    })
                })
                .collect();
            json(&serde_json::to_string(&serde_json::json!({ "runs": arr })).unwrap_or_default())
        }
        (Method::Get, "/api/calls") => {
            if !authed {
                return unauthorized();
            }
            // Runtime audit log — digests only, never argument values. Newest
            // first, bounded, filterable by run id and server.
            let run = query_param(query, "run");
            let server = query_param(query, "server");
            // The bounded tail read is only correct when NOTHING is filtered
            // out afterwards: tail-then-filter would show a filtered run only
            // if it appears in the 500 newest GLOBAL rows, silently
            // under-reporting older activity on an audit surface. Filtered
            // views pay the full read; the common unfiltered poll stays cheap.
            let mut entries = if run.is_some() || server.is_some() {
                agentstack_recorder::read_all()
            } else {
                agentstack_recorder::read_tail(500)
            };
            entries.reverse();
            let filtered: Vec<_> = entries
                .into_iter()
                .filter(|e| match run.as_deref() {
                    Some(r) => e.run.as_deref() == Some(r),
                    None => true,
                })
                .filter(|e| match server.as_deref() {
                    Some(s) => e.server == s,
                    None => true,
                })
                .take(500)
                .collect();
            json(
                &serde_json::to_string(&serde_json::json!({ "calls": filtered }))
                    .unwrap_or_default(),
            )
        }
        (Method::Get, "/api/search") => {
            if !authed {
                return unauthorized();
            }
            let q = query_param(query, "q").unwrap_or_default();
            match crate::dashboard::snapshot::search(dir, &q) {
                Ok(v) => json(&serde_json::to_string(&v).unwrap_or_default()),
                Err(e) => json(&format!("{{\"error\":{:?}}}", e.to_string())),
            }
        }
        _ => Response::from_string("not found").with_status_code(404),
    }
}

/// Extract a query-string parameter, URL-decoding `+` and `%XX`.
fn query_param(query: &str, key: &str) -> Option<String> {
    query
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| urldecode(v))
}

fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                if let Ok(b) = u8::from_str_radix(hex, 16) {
                    out.push(b);
                    i += 2;
                } else {
                    out.push(bytes[i]);
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn scope_of_query(query: &str) -> Scope {
    let v = query
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .find(|(k, _)| *k == "scope")
        .map(|(_, v)| v);
    match v {
        Some("project") => Scope::Project,
        _ => Scope::Global,
    }
}

fn unauthorized() -> Resp {
    // JSON body so the client can parse + show a clear message (a stale token in
    // the URL is the usual cause).
    json("{\"error\":\"unauthorized — open the dashboard URL printed in your terminal (the token must match this server)\"}")
        .with_status_code(401)
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
    // Every caller passes a static literal, so this never fails today. Fall
    // back to a valid literal rather than panic if a future caller ever hands
    // a runtime value with bytes the header grammar rejects — a dashboard
    // Content-Type is never worth crashing the server over. The inner
    // construction is over a compile-time literal, so it is total.
    Header::from_bytes(&b"Content-Type"[..], value.as_bytes()).unwrap_or_else(|()| {
        Header::from_bytes(&b"Content-Type"[..], &b"application/octet-stream"[..])
            .expect("octet-stream is a valid literal Content-Type")
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every route that used to mutate disk. The dashboard is read-only now, so
    /// none of these exist in the router — each must fall through to 404. This
    /// is the witness that the write surface is gone: not gated, *absent*.
    const FORMER_WRITE_ROUTES: &[&str] = &[
        "/api/add_from",
        "/api/add_hook",
        "/api/add_profile",
        "/api/add_server",
        "/api/add_skill",
        "/api/adopt_all_skills",
        "/api/adopt_skill",
        "/api/apply",
        "/api/consolidate_skills",
        "/api/import_settings",
        "/api/init",
        "/api/install",
        "/api/remove",
        "/api/run_kill",
        "/api/secret",
        "/api/session_end",
        "/api/session_start",
        "/api/set_settings",
        "/api/toggle",
        "/api/toggle_skill",
        "/api/undo",
        "/api/use",
    ];

    #[test]
    fn former_write_routes_are_gone() {
        for path in FORMER_WRITE_ROUTES {
            // Authed POST — the only thing standing between this and a write
            // used to be the read-only gate. Now there's simply no route.
            let resp = route(&Method::Post, path, "", true, None);
            assert_eq!(
                resp.status_code(),
                tiny_http::StatusCode(404),
                "{path} must be absent (404), not routed"
            );
        }
    }

    #[test]
    fn reads_stay_available() {
        // Read endpoints answer (any non-404/401 shape is fine; these run
        // against whatever HOME the test machine has).
        for path in ["/api/state", "/api/doctor", "/api/history", "/api/runs"] {
            let resp = route(&Method::Get, path, "", true, None);
            assert_ne!(
                resp.status_code(),
                tiny_http::StatusCode(404),
                "{path} must stay a real read route"
            );
        }
    }

    #[test]
    fn unauthed_read_is_refused() {
        let resp = route(&Method::Get, "/api/state", "", false, None);
        assert_eq!(resp.status_code(), tiny_http::StatusCode(401));
    }
}
