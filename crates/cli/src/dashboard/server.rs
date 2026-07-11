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

    let response = route(&method, path, query, authed, read_only, &body, dir);
    let _ = request.respond(response);
}

#[allow(clippy::too_many_arguments)]
fn route(
    method: &Method,
    path: &str,
    query: &str,
    authed: bool,
    read_only: bool,
    body: &str,
    dir: Option<&Path>,
) -> Resp {
    // Structural read-only gate: every POST is a mutation, so refuse them all
    // here (individual handlers also gate via `mutation()` — belt and braces).
    // A future endpoint that forgets `mutation()` still can't write in
    // `--read-only` mode.
    if *method == Method::Post && read_only {
        return forbidden();
    }
    match (method, path) {
        (Method::Get, "/") => html(INDEX_HTML),
        (Method::Get, "/app.js") => asset(APP_JS, "application/javascript"),
        (Method::Get, "/styles.css") => asset(STYLES_CSS, "text/css"),
        (Method::Get, "/api/state") => {
            if !authed {
                return unauthorized();
            }
            match crate::dashboard::snapshot::state(dir) {
                Ok(mut v) => {
                    if let Some(o) = v.as_object_mut() {
                        o.insert("readOnly".into(), Value::Bool(read_only));
                    }
                    json(&serde_json::to_string(&v).unwrap_or_default())
                }
                Err(e) => json(&format!("{{\"error\":{:?}}}", e.to_string())),
            }
        }
        (Method::Post, "/api/init") => mutation(authed, read_only, || {
            crate::commands::init::dashboard_init(dir).map(|_| ())
        }),
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
        (Method::Post, "/api/undo") => mutation(authed, read_only, || {
            crate::history::undo(&field(&parse(body), "id")?)
        }),
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
        (Method::Post, "/api/run_kill") => mutation(authed, read_only, || {
            let v = parse(body);
            let force = v.get("force").and_then(Value::as_bool).unwrap_or(false);
            crate::runs::kill(&field(&v, "id")?, force)
        }),
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
        (Method::Get, "/api/pi_search") => {
            if !authed {
                return unauthorized();
            }
            let q = query_param(query, "q").unwrap_or_default();
            match crate::pi_packages::search(&q, 30) {
                Ok(pkgs) => {
                    let arr: Vec<Value> = pkgs
                        .iter()
                        .map(|p| {
                            serde_json::json!({
                                "name": p.name, "version": p.version,
                                "description": p.description, "kind": p.kind,
                                "npmUrl": p.npm_url, "repoUrl": p.repo_url,
                                "install": p.install,
                            })
                        })
                        .collect();
                    json(
                        &serde_json::to_string(&serde_json::json!({ "results": arr }))
                            .unwrap_or_default(),
                    )
                }
                Err(e) => json(&format!("{{\"error\":{:?}}}", e.to_string())),
            }
        }
        (Method::Post, "/api/pi_install") => mutation(authed, read_only, || {
            crate::dashboard::actions::pi_install(&field(&parse(body), "name")?)
        }),
        (Method::Post, "/api/toggle") => mutation(authed, read_only, || {
            let v = parse(body);
            let server = field(&v, "server")?;
            let target = field(&v, "target")?;
            let enable = v.get("enable").and_then(Value::as_bool).unwrap_or(true);
            crate::dashboard::actions::toggle(dir, &server, &target, scope_of(body), enable)
        }),
        (Method::Post, "/api/toggle_skill") => mutation(authed, read_only, || {
            let v = parse(body);
            let skill = field(&v, "skill")?;
            let target = field(&v, "target")?;
            let enable = v.get("enable").and_then(Value::as_bool).unwrap_or(true);
            crate::dashboard::actions::toggle_skill(dir, &skill, &target, scope_of(body), enable)
        }),
        (Method::Post, "/api/add_server") => mutation(authed, read_only, || {
            crate::dashboard::actions::add_server(dir, &parse(body)).map(|_| ())
        }),
        (Method::Post, "/api/remove") => mutation(authed, read_only, || {
            crate::dashboard::actions::remove_capability(dir, &parse(body)).map(|_| ())
        }),
        (Method::Post, "/api/add_skill") => mutation(authed, read_only, || {
            crate::dashboard::actions::add_skill(dir, &parse(body)).map(|_| ())
        }),
        (Method::Post, "/api/adopt_skill") => mutation(authed, read_only, || {
            crate::dashboard::actions::adopt_skill(dir, &field(&parse(body), "name")?)
        }),
        (Method::Post, "/api/adopt_all_skills") => mutation(authed, read_only, || {
            crate::dashboard::actions::adopt_all_skills(dir).map(|_| ())
        }),
        (Method::Post, "/api/consolidate_skills") => mutation(authed, read_only, || {
            let v = parse(body);
            let names: Vec<String> = v
                .get("names")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            crate::dashboard::actions::consolidate_skills(dir, &names).map(|_| ())
        }),
        (Method::Post, "/api/set_settings") => mutation(authed, read_only, || {
            crate::dashboard::actions::set_settings(dir, &parse(body))
        }),
        (Method::Post, "/api/add_hook") => mutation(authed, read_only, || {
            crate::dashboard::actions::add_hook(dir, &parse(body)).map(|_| ())
        }),
        (Method::Post, "/api/add_plugin_recipe") => mutation(authed, read_only, || {
            crate::dashboard::actions::add_plugin_recipe(dir, &parse(body)).map(|_| ())
        }),
        (Method::Post, "/api/add_profile") => mutation(authed, read_only, || {
            crate::dashboard::actions::add_profile(dir, &parse(body)).map(|_| ())
        }),
        (Method::Post, "/api/session_start") => mutation(authed, read_only, || {
            let v = parse(body);
            let profile = field(&v, "profile")?;
            let plugin = v
                .get("plugin")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty());
            crate::session::start(dir, &profile, scope_of(body), plugin)
        }),
        (Method::Post, "/api/session_end") => {
            mutation(authed, read_only, || crate::session::end(dir))
        }
        (Method::Post, "/api/import_settings") => mutation(authed, read_only, || {
            let target = field(&parse(body), "target")?;
            crate::dashboard::actions::import_settings(dir, &target).map(|_| ())
        }),
        (Method::Post, "/api/add_from") => mutation(authed, read_only, || {
            let v = parse(body);
            let id = field(&v, "id")?;
            let profile = v.get("profile").and_then(Value::as_str);
            let mdir = match dir {
                Some(d) => d.to_path_buf(),
                None => std::env::current_dir()?,
            };
            crate::commands::add::write_from_provider(&mdir, &id, profile).map(|_| ())
        }),
        (Method::Post, "/api/secret") => mutation(authed, read_only, || {
            let v = parse(body);
            let name = field(&v, "name")?;
            let value = field(&v, "value")?;
            crate::secret::keychain::set(&name, &value)
        }),
        (Method::Post, "/api/apply") => mutation(authed, read_only, || {
            // Optional explicit target list (e.g. "preview all" reconciling
            // installed non-default CLIs); empty = the manifest's defaults.
            let targets: Vec<String> = parse(body)
                .get("targets")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            let args = crate::cli::ApplyArgs {
                targets,
                profile: None,
                dry_run: false,
                write: true,
                scope: Some(scope_of(body)),
                allow_unresolved: false,
                prune_foreign: false,
                no_gitignore: false,
            };
            crate::commands::apply::run(&args, dir)
        }),
        (Method::Post, "/api/install") => mutation(authed, read_only, || {
            crate::commands::install::run(
                &crate::cli::InstallArgs {
                    locked: false,
                    allow_flagged: false,
                },
                dir,
            )
        }),
        (Method::Post, "/api/plugins_sync") => mutation(authed, read_only, || {
            let v = parse(body);
            let targets: Vec<String> = v
                .get("targets")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            let ctx = crate::commands::load(dir)?;
            let valid_targets: Vec<&str> = ctx.registry.ids().collect();
            let issues =
                crate::manifest::validate_with_targets(&ctx.loaded.manifest, valid_targets);
            if let Some(issue) = issues.into_iter().find(|i| i.kind.is_error()) {
                anyhow::bail!(issue.message);
            }
            let report = crate::plugin_recipes::sync(
                &ctx.loaded.manifest,
                &ctx.registry,
                &ctx.dir,
                &crate::plugin_recipes::SyncOptions {
                    targets,
                    write: true,
                },
            )?;
            crate::plugin_recipes::ensure_no_sync_errors(&report)
        }),
        (Method::Post, "/api/plugins_install") => mutation(authed, read_only, || {
            let v = parse(body);
            let name = field(&v, "name")?;
            let targets = string_array(&v, "targets");
            crate::commands::plugins::install_recipe_native(dir, &name, &targets, true)
        }),
        (Method::Post, "/api/plugins_remove") => mutation(authed, read_only, || {
            let v = parse(body);
            let name = field(&v, "name")?;
            let targets = string_array(&v, "targets");
            crate::commands::plugins::remove_recipe_native(dir, &name, &targets, true)
        }),
        (Method::Post, "/api/use") => mutation(authed, read_only, || {
            let v = parse(body);
            let profile = field(&v, "profile")?;
            let args = crate::cli::UseArgs {
                profile,
                targets: vec![],
                scope: Some(scope_of(body)),
                write: true,
                allow_unresolved: false,
                prune_foreign: false,
                no_gitignore: false,
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

fn string_array(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn scope_of(body: &str) -> Scope {
    match parse(body).get("scope").and_then(Value::as_str) {
        Some("project") => Scope::Project,
        _ => Scope::Global,
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

fn forbidden() -> Resp {
    json("{\"error\":\"dashboard is read-only\"}").with_status_code(403)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every mutating dashboard endpoint. Keep in sync with the POST arms in
    /// `route` — the test below proves each refuses in `--read-only`, and the
    /// unknown-path case proves the refusal is the *central* gate, so a future
    /// endpoint that forgets `mutation()` still can't write.
    const POST_ROUTES: &[&str] = &[
        "/api/add_from",
        "/api/add_hook",
        "/api/add_plugin_recipe",
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
        "/api/pi_install",
        "/api/plugins_install",
        "/api/plugins_remove",
        "/api/plugins_sync",
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
    fn read_only_refuses_every_mutation() {
        for path in POST_ROUTES {
            let resp = route(&Method::Post, path, "", true, true, "{}", None);
            assert_eq!(
                resp.status_code(),
                tiny_http::StatusCode(403),
                "{path} must refuse in --read-only"
            );
        }
        // Unknown POST path: still 403 — the central gate, not per-route luck.
        let resp = route(
            &Method::Post,
            "/api/added_next_year",
            "",
            true,
            true,
            "{}",
            None,
        );
        assert_eq!(resp.status_code(), tiny_http::StatusCode(403));
    }

    #[test]
    fn reads_stay_available_in_read_only() {
        // Read endpoints answer in --read-only (any non-403 shape is fine;
        // these run against whatever HOME the test machine has).
        for path in ["/api/state", "/api/doctor", "/api/history", "/api/runs"] {
            let resp = route(&Method::Get, path, "", true, true, "", None);
            assert_ne!(
                resp.status_code(),
                tiny_http::StatusCode(403),
                "{path} must stay readable in --read-only"
            );
        }
    }

    #[test]
    fn unauthed_is_refused_before_read_only() {
        let resp = route(&Method::Post, "/api/apply", "", false, false, "{}", None);
        assert_eq!(resp.status_code(), tiny_http::StatusCode(401));
    }
}
