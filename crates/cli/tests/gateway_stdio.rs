//! Gateway stdio proxying end-to-end: a manifest-declared stdio server is
//! spawned lazily, speaks real JSON-RPC over its pipes, gets its `${REF}`s
//! resolved into the child env, and is tree-killed when the gateway drops.
//! The fixture "server" is a plain `sh` script, so the test has no runtime
//! dependencies beyond a POSIX shell.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use agentstack::gateway::Gateway;

// Tests mutate the process-global HOME/AGENTSTACK_HOME and secret env; serialize.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// A minimal MCP stdio server in POSIX sh: answers `initialize`, `tools/list`
/// (one `echo` tool), and `tools/call` (echoes the `msg` argument and its own
/// `$FIX_TOKEN` env, proving env made it into the child resolved). Writes its
/// pid to `$PIDFILE` on start so tests can watch its lifetime.
const FIXTURE: &str = r#"#!/bin/sh
if [ -n "$PIDFILE" ]; then echo $$ > "$PIDFILE"; fi
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-06-18","capabilities":{},"serverInfo":{"name":"fix","version":"0"}}}\n' "$id"
      ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo","description":"Echo the input back.","inputSchema":{"type":"object","properties":{"msg":{"type":"string"}},"required":["msg"]}}]}}\n' "$id"
      ;;
    *'"method":"tools/call"'*)
      msg=$(printf '%s' "$line" | sed -n 's/.*"msg":"\([^"]*\)".*/\1/p')
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"echo:%s:token=%s"}]}}\n' "$id" "$msg" "$FIX_TOKEN"
      ;;
  esac
done
"#;

/// A deliberately slow server: sleeps 1.5s before answering a `tools/call` —
/// the fixture for proving per-upstream locking (a call here must not block a
/// call to a different server).
const SLOW_FIXTURE: &str = r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-06-18","capabilities":{},"serverInfo":{"name":"slow","version":"0"}}}\n' "$id"
      ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"wait","description":"Answer slowly.","inputSchema":{"type":"object","properties":{}}}]}}\n' "$id"
      ;;
    *'"method":"tools/call"'*)
      sleep 1.5
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"finally"}]}}\n' "$id"
      ;;
  esac
done
"#;

/// A server that starts but never answers anything — the timeout fixture.
const HANG_FIXTURE: &str = "#!/bin/sh\nexec sleep 3600\n";

/// A server whose one tool (`where`) reports the directory it runs in — the
/// fixture for cwd anchoring.
const CWD_FIXTURE: &str = r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-06-18","capabilities":{},"serverInfo":{"name":"cwd","version":"0"}}}\n' "$id"
      ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"where","description":"Report the working directory.","inputSchema":{"type":"object","properties":{}}}]}}\n' "$id"
      ;;
    *'"method":"tools/call"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"cwd:%s"}]}}\n' "$id" "$(pwd)"
      ;;
  esac
done
"#;

fn write_script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    path
}

fn setup_home(home: &Path) {
    std::fs::create_dir_all(home).unwrap();
    std::env::set_var("HOME", home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));
}

fn write_manifest(proj: &Path, servers: &str) {
    std::fs::create_dir_all(proj).unwrap();
    std::fs::write(
        proj.join("agentstack.toml"),
        format!("version = 1\n[targets]\ndefault = [\"claude-code\"]\n{servers}"),
    )
    .unwrap();
}

fn pid_alive(pid: &str) -> bool {
    std::process::Command::new("/bin/kill")
        .args(["-0", pid])
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// `try_call` returns the JSON-RPC `result` payload already unwrapped.
fn call_text(result: &Value) -> &str {
    result["content"][0]["text"].as_str().unwrap_or("")
}

#[test]
fn stdio_round_trip_env_secrets_and_group_kill() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    setup_home(&tmp.path().join("home"));
    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let script = write_script(&proj, "fix.sh", FIXTURE);
    let pidfile = proj.join("fix.pid");
    // The secret resolves from process env (first link in the chain) and must
    // land, resolved, in the child's env.
    std::env::set_var("FIX_SECRET", "tok-s3cr3t");
    write_manifest(
        &proj,
        &format!(
            "[servers.fix]\ntype = \"stdio\"\ncommand = \"/bin/sh\"\nargs = [\"{}\"]\n\
             env = {{ FIX_TOKEN = \"${{FIX_SECRET}}\", PIDFILE = \"{}\" }}\n",
            script.display(),
            pidfile.display()
        ),
    );

    let gw = Gateway::from_manifest(Some(&proj));
    // Lazy spawn: building the gateway must not start the child.
    assert!(!pidfile.exists(), "child spawned before first use");

    // Discovery spawns the child and namespaces its tools.
    let tools = gw.namespaced_tools();
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    assert_eq!(names, ["fix__echo"], "tools: {names:?}");
    assert!(tools[0]["description"]
        .as_str()
        .unwrap()
        .starts_with("[via fix] "));

    // Code-mode bindings cover stdio servers too.
    let client = gw.generate_bindings().client_ts;
    assert!(client.contains(r#"call("fix__echo", input)"#), "{client}");

    // A call round-trips, and the child saw the *resolved* secret in its env.
    let res = gw
        .try_call("fix__echo", &json!({ "msg": "hi" }))
        .expect("routed")
        .expect("call ok");
    assert_eq!(call_text(&res), "echo:hi:token=tok-s3cr3t");

    // Dropping the gateway kills the child (and its whole process group).
    let pid = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .to_string();
    assert!(pid_alive(&pid), "child should be alive while gateway lives");
    drop(gw);
    let deadline = Instant::now() + Duration::from_secs(3);
    while pid_alive(&pid) {
        assert!(
            Instant::now() < deadline,
            "child {pid} outlived the gateway"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    std::env::remove_var("FIX_SECRET");
}

/// Ask the `where` tool for the child's working directory, canonicalized (so
/// macOS's `/tmp` → `/private/tmp` symlink doesn't fail the comparison).
fn reported_cwd(gw: &Gateway, tool: &str) -> PathBuf {
    let res = gw
        .try_call(tool, &json!({}))
        .expect("routed")
        .expect("call ok");
    let text = call_text(&res);
    let dir = text.strip_prefix("cwd:").unwrap_or_else(|| {
        panic!("unexpected tool output: {text}");
    });
    PathBuf::from(dir).canonicalize().unwrap()
}

#[test]
fn stdio_spawns_in_project_root_so_relative_args_resolve() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    setup_home(&tmp.path().join("home"));
    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    write_script(&proj, "cwdfix.sh", CWD_FIXTURE);
    // A *relative* script path: it only resolves if the child is spawned from
    // the manifest's project root — the test process itself runs elsewhere.
    write_manifest(
        &proj,
        "[servers.here]\ntype = \"stdio\"\ncommand = \"/bin/sh\"\nargs = [\"./cwdfix.sh\"]\n",
    );

    let gw = Gateway::from_manifest(Some(&proj));
    assert_eq!(
        reported_cwd(&gw, "here__where"),
        proj.canonicalize().unwrap(),
        "child must run in the manifest's project root, not the gateway's cwd"
    );
}

#[test]
fn stdio_manifest_cwd_anchors_the_child_relative_to_project_root() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    setup_home(&tmp.path().join("home"));
    let proj = tmp.path().join("proj");
    let srv = proj.join("srv");
    std::fs::create_dir_all(&srv).unwrap();
    write_script(&srv, "cwdfix.sh", CWD_FIXTURE);
    // `cwd = "srv"` is relative to the project root; the relative script path
    // then resolves against that cwd, matching the rendered-config contract.
    write_manifest(
        &proj,
        "[servers.sub]\ntype = \"stdio\"\ncwd = \"srv\"\ncommand = \"/bin/sh\"\nargs = [\"./cwdfix.sh\"]\n",
    );

    let gw = Gateway::from_manifest(Some(&proj));
    assert_eq!(
        reported_cwd(&gw, "sub__where"),
        srv.canonicalize().unwrap(),
        "child must run in the manifest-declared cwd"
    );
}

/// Real failures through `try_call` must land in the call log with their
/// intended fixed class — classified from the actual anyhow chains the
/// gateway produces, not synthetic strings. (The unit test covers shapes;
/// this covers the wiring end to end.)
#[test]
fn call_log_classifies_real_failures() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    setup_home(&tmp.path().join("home"));
    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    write_script(&proj, "fix.sh", FIXTURE);
    write_manifest(
        &proj,
        "[servers.missing]\ntype = \"stdio\"\ncommand = \"/bin/definitely-not-a-binary\"\n\
         [servers.hung]\ntype = \"stdio\"\ncommand = \"/bin/sh\"\nargs = [\"-c\", \"exec sleep 3600\"]\n\
         [servers.noref]\ntype = \"stdio\"\ncommand = \"/bin/sh\"\nargs = [\"./fix.sh\"]\nenv = { TOKEN = \"${NO_SUCH_SECRET_REF}\" }\n",
    );
    // Keep the hung server's start timeout short so the test stays fast.
    std::env::set_var("AGENTSTACK_STDIO_START_MS", "300");
    let gw = Gateway::from_manifest(Some(&proj));
    assert!(gw.try_call("noref__echo", &json!({})).unwrap().is_err());
    assert!(gw.try_call("missing__echo", &json!({})).unwrap().is_err());
    assert!(gw.try_call("hung__echo", &json!({})).unwrap().is_err());
    std::env::remove_var("AGENTSTACK_STDIO_START_MS");

    let detail_for = |server: &str| {
        agentstack::calllog::read_all()
            .into_iter()
            .find(|r| r.server == server)
            .unwrap_or_else(|| panic!("no log record for {server}"))
            .detail
            .unwrap_or_default()
    };
    assert_eq!(detail_for("noref"), "unresolved-secret");
    assert_eq!(detail_for("missing"), "spawn-failed");
    assert_eq!(detail_for("hung"), "timeout");
}

/// The machine `[policy.tools]` layer (`~/.agentstack/agentstack.toml`) denies
/// with precedence: the project manifest declares NO policy at all, and the
/// call is still refused — a cloned repo cannot loosen the user's own rules.
/// Denied tools are also invisible to discovery.
#[test]
fn machine_policy_denies_with_precedence_over_the_project() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    setup_home(&tmp.path().join("home"));
    // The machine manifest carries the user's standing deny rule.
    let agentstack_home = tmp.path().join("home/.agentstack");
    std::fs::create_dir_all(&agentstack_home).unwrap();
    std::fs::write(
        agentstack_home.join("agentstack.toml"),
        "version = 1\n[policy.tools]\nfix = [\"!echo\"]\n",
    )
    .unwrap();
    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    write_script(&proj, "fix.sh", FIXTURE);
    write_manifest(
        &proj,
        "[servers.fix]\ntype = \"stdio\"\ncommand = \"/bin/sh\"\nargs = [\"./fix.sh\"]\n",
    );

    let gw = Gateway::from_manifest(Some(&proj));
    let err = gw
        .try_call("fix__echo", &json!({ "msg": "hi" }))
        .expect("routed")
        .expect_err("machine policy must refuse the call");
    assert!(err.to_string().contains("machine policy"), "{err}");
    // Invisible to discovery too — same rule filters the tool list.
    assert!(
        gw.namespaced_tools().is_empty(),
        "machine-denied tool must not be discoverable"
    );

    // The first validated load persisted the secret-free policy input. If the
    // source subsequently rots, a fresh gateway must retain the deny from that
    // last-known-good snapshot rather than falling back to project-only policy.
    drop(gw);
    std::fs::write(agentstack_home.join("agentstack.toml"), "not toml {{{").unwrap();
    let cached = Gateway::from_manifest(Some(&proj));
    let err = cached
        .try_call("fix__echo", &json!({ "msg": "hi" }))
        .expect("routed")
        .expect_err("last-known-good machine policy must retain the deny");
    assert!(err.to_string().contains("machine policy"), "{err}");
    assert!(
        cached.namespaced_tools().is_empty(),
        "last-known-good denied tool must remain undiscoverable"
    );
}

/// The serialization fix: a slow call to one upstream must not block a call to
/// a *different* upstream. Before per-upstream locking, one gateway-wide mutex
/// held for the whole round trip meant the fast call below would wait out the
/// slow server's full 1.5s.
#[test]
fn slow_upstream_does_not_block_calls_to_another_server() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    setup_home(&tmp.path().join("home"));
    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    write_script(&proj, "slow.sh", SLOW_FIXTURE);
    write_script(&proj, "fast.sh", CWD_FIXTURE);
    write_manifest(
        &proj,
        "[servers.slow]\ntype = \"stdio\"\ncommand = \"/bin/sh\"\nargs = [\"./slow.sh\"]\n\
         [servers.fast]\ntype = \"stdio\"\ncommand = \"/bin/sh\"\nargs = [\"./fast.sh\"]\n",
    );

    let gw = std::sync::Arc::new(Gateway::from_manifest(Some(&proj)));
    // Pre-warm both children and the discovery cache so the timing below
    // measures call concurrency, not spawn/discovery cost.
    assert_eq!(gw.namespaced_tools().len(), 2);
    gw.try_call("fast__where", &json!({})).unwrap().unwrap();

    // Occupy the slow server from another thread (holds its slot for ~1.5s)…
    let slow_gw = std::sync::Arc::clone(&gw);
    let slow = std::thread::spawn(move || slow_gw.try_call("slow__wait", &json!({})));
    std::thread::sleep(Duration::from_millis(150)); // let it acquire the slot

    // …and prove the other server answers immediately meanwhile.
    let t0 = Instant::now();
    gw.try_call("fast__where", &json!({})).unwrap().unwrap();
    let fast_elapsed = t0.elapsed();

    let slow_result = slow.join().unwrap().expect("routed").expect("slow call ok");
    assert!(call_text(&slow_result).contains("finally"));
    assert!(
        fast_elapsed < Duration::from_millis(1000),
        "fast call took {fast_elapsed:?} — it was serialized behind the slow upstream"
    );
}

#[test]
fn stdio_agentstack_layout_anchors_at_project_root_not_manifest_dir() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    setup_home(&tmp.path().join("home"));
    let proj = tmp.path().join("proj");
    let sub = proj.join(".agentstack");
    std::fs::create_dir_all(&sub).unwrap();
    write_script(&proj, "cwdfix.sh", CWD_FIXTURE);
    // Preferred `.agentstack/` layout: the manifest dir is NOT the project
    // root. Relative paths must anchor at the root, not at `.agentstack/`.
    std::fs::write(
        sub.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [servers.here]\ntype = \"stdio\"\ncommand = \"/bin/sh\"\nargs = [\"./cwdfix.sh\"]\n",
    )
    .unwrap();

    let gw = Gateway::from_manifest(Some(&proj));
    assert_eq!(
        reported_cwd(&gw, "here__where"),
        proj.canonicalize().unwrap(),
        "child must run in the project root, not the .agentstack/ manifest dir"
    );
}

#[test]
fn stdio_unresolved_secret_refuses_calls_but_still_lists() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    setup_home(&tmp.path().join("home"));
    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let script = write_script(&proj, "fix.sh", FIXTURE);
    std::env::remove_var("AGENTSTACK_TEST_UNSET_REF");
    write_manifest(
        &proj,
        &format!(
            "[servers.fix]\ntype = \"stdio\"\ncommand = \"/bin/sh\"\nargs = [\"{}\"]\n\
             env = {{ FIX_TOKEN = \"${{AGENTSTACK_TEST_UNSET_REF}}\" }}\n",
            script.display()
        ),
    );

    let gw = Gateway::from_manifest(Some(&proj));
    // Listing still works (parity with HTTP: an unauthed server can list)…
    assert_eq!(gw.namespaced_tools().len(), 1);
    // …but a call is refused with the ref named, before reaching the child.
    let err = gw
        .try_call("fix__echo", &json!({ "msg": "x" }))
        .expect("routed")
        .expect_err("must refuse");
    let msg = err.to_string();
    assert!(
        msg.contains("AGENTSTACK_TEST_UNSET_REF") && msg.contains("secret"),
        "unexpected refusal message: {msg}"
    );
}

#[test]
fn stdio_startup_timeout_yields_partial_results() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    setup_home(&tmp.path().join("home"));
    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let good = write_script(&proj, "fix.sh", FIXTURE);
    let hang = write_script(&proj, "hang.sh", HANG_FIXTURE);
    write_manifest(
        &proj,
        &format!(
            "[servers.fix]\ntype = \"stdio\"\ncommand = \"/bin/sh\"\nargs = [\"{}\"]\n\
             [servers.hang]\ntype = \"stdio\"\ncommand = \"/bin/sh\"\nargs = [\"{}\"]\n",
            good.display(),
            hang.display()
        ),
    );

    std::env::set_var("AGENTSTACK_STDIO_START_MS", "500");
    let gw = Gateway::from_manifest(Some(&proj));
    let start = Instant::now();
    let tools = gw.namespaced_tools();
    std::env::remove_var("AGENTSTACK_STDIO_START_MS");

    // The hung server is skipped after its startup timeout; the healthy one
    // still answers — partial results, not a wholesale failure.
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    assert_eq!(names, ["fix__echo"], "tools: {names:?}");
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "discovery should not hang: {:?}",
        start.elapsed()
    );
}

#[test]
fn stats_live_measures_context_cost_through_gateway() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup_home(&home);
    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let script = write_script(&proj, "fix.sh", FIXTURE);
    write_manifest(
        &proj,
        &format!(
            "[servers.fix]\ntype = \"stdio\"\ncommand = \"/bin/sh\"\nargs = [\"{}\"]\n",
            script.display()
        ),
    );

    // `stats --live` measures via the gateway and caches to footprint.json…
    agentstack::commands::stats::run(&agentstack::cli::StatsArgs { live: true }, Some(&proj))
        .unwrap();
    let fp = agentstack::footprint::Footprints::load().unwrap();
    let f = fp.get("fix").expect("fix measured");
    assert_eq!(f.tools, 1);
    assert!(f.est_tokens > 0, "footprint: {f:?}");

    // …and `explain` reads the cache offline (no live discovery).
    let text = agentstack::commands::explain::explain_text("fix", Some(&proj)).unwrap();
    assert!(
        text.contains("Context cost") && text.contains("tok"),
        "explain: {text}"
    );
}

#[test]
fn policy_firewall_hides_denied_tools_and_refuses_calls() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    setup_home(&tmp.path().join("home"));
    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let script = write_script(&proj, "fix.sh", FIXTURE);
    write_manifest(
        &proj,
        &format!(
            "[servers.fix]\ntype = \"stdio\"\ncommand = \"/bin/sh\"\nargs = [\"{}\"]\n\
             [policy]\ntools = {{ fix = [\"!echo\"] }}\n",
            script.display()
        ),
    );

    let gw = Gateway::from_manifest(Some(&proj));
    // The denied tool is invisible to discovery (and so to search/bindings)…
    assert!(
        gw.namespaced_tools().is_empty(),
        "denied tool must not list"
    );
    // …and a direct call is refused, naming the rule, without reaching the child.
    let err = gw
        .try_call("fix__echo", &json!({ "msg": "sentinel-value-xyz" }))
        .expect("routed")
        .expect_err("must be denied");
    let msg = err.to_string();
    assert!(msg.contains("refused") && msg.contains("!echo"), "{msg}");

    // The denial is audited — digest only, never the argument value.
    let log = std::fs::read_to_string(agentstack::calllog::log_path()).unwrap();
    assert!(
        !log.contains("sentinel-value-xyz"),
        "log leaked args: {log}"
    );
    let entries = agentstack::calllog::read_all();
    let e = entries.last().expect("one record");
    assert_eq!((e.server.as_str(), e.tool.as_str()), ("fix", "echo"));
    assert_eq!(e.outcome, "denied");
    assert!(e.detail.as_deref().unwrap_or("").contains("!echo"));
    assert_eq!(e.args_digest.len(), 12);
}

#[test]
fn audit_log_records_ok_calls_with_digest_only() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    setup_home(&tmp.path().join("home"));
    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let script = write_script(&proj, "fix.sh", FIXTURE);
    write_manifest(
        &proj,
        &format!(
            "[servers.fix]\ntype = \"stdio\"\ncommand = \"/bin/sh\"\nargs = [\"{}\"]\n",
            script.display()
        ),
    );

    let gw = Gateway::from_manifest(Some(&proj));
    gw.try_call("fix__echo", &json!({ "msg": "sentinel-ok-abc" }))
        .expect("routed")
        .expect("call ok");

    let raw = std::fs::read_to_string(agentstack::calllog::log_path()).unwrap();
    assert!(!raw.contains("sentinel-ok-abc"), "log leaked args: {raw}");
    let entries = agentstack::calllog::read_all();
    let e = entries.last().expect("one record");
    assert_eq!(e.outcome, "ok");
    assert_eq!((e.server.as_str(), e.tool.as_str()), ("fix", "echo"));
    assert_eq!(e.args_digest.len(), 12);
    assert!(e.project.as_deref().unwrap_or("").contains("proj"));
}
