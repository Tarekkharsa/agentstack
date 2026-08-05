// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]
#![cfg(unix)]

//! W4 — the runtime lease registry and the two failure semantics it owns
//! (`docs/design/automatic-delivery.md` §"Lease lifecycle", §"Failure
//! semantics" 1 and 3, and W4's acceptance paragraph).
//!
//! Four properties, five witnesses:
//!
//! 1. **A lease is externally visible.** It used to live only in the MCP
//!    subprocess's memory, so no other surface could see it at all.
//! 2. **A stale record never reads as live.** The decisive one. A record is
//!    not truth — liveness is derived from the recorded PID *and* that
//!    process's start time, because a crash leaves the record behind and the
//!    operating system is free to hand that PID to something else.
//! 3. **Toolset fencing.** Several toolsets declared and no lease open serves
//!    control-plane tools only; a lease exposes exactly the toolset it names.
//! 4. **Gateway unavailable.** No tools, a one-sentence explanation from both
//!    `status` and `doctor` naming the one recovery command, and — the part
//!    that is easy to lose — no file written in the gateway's place.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Mutex;

use serde_json::{json, Value};

// These tests mutate the process-global HOME/AGENTSTACK_HOME (children inherit
// them, and the in-process trust grant reads them directly); serialize them.
static ENV_LOCK: Mutex<()> = Mutex::new(());

const BIN: &str = env!("CARGO_BIN_EXE_agentstack");

/// A minimal MCP stdio server in POSIX sh exposing one named tool, so a test
/// can tell which upstream a discovery result came from.
fn fixture(tool: &str) -> String {
    format!(
        r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"protocolVersion":"2025-06-18","capabilities":{{}},"serverInfo":{{"name":"fix","version":"0"}}}}}}\n' "$id"
      ;;
    *'"method":"tools/list"'*)
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"tools":[{{"name":"{tool}","description":"Fixture tool {tool}.","inputSchema":{{"type":"object"}}}}]}}}}\n' "$id"
      ;;
  esac
done
"#
    )
}

fn write_executable(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Point HOME/AGENTSTACK_HOME at a sandbox and return the agentstack home.
fn sandbox_home(tmp: &Path) -> PathBuf {
    let home = tmp.join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    let as_home = home.join(".agentstack");
    std::env::set_var("AGENTSTACK_HOME", &as_home);
    as_home
}

fn cleanup() {
    std::env::remove_var("HOME");
    std::env::remove_var("AGENTSTACK_HOME");
}

/// One MCP stdio connection, driven request-by-request so a test can hold the
/// connection open (and therefore the lease) while querying another surface.
struct Connection {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Connection {
    fn open(args: &[&str], cwd: &Path, home: &Path) -> Self {
        Self::open_with(args, cwd, home, &[])
    }

    /// Open a connection with extra environment — used to give the MCP process
    /// a run id, so what it records is attributed to a tracked run exactly as
    /// `agentstack run` would attribute it.
    fn open_with(args: &[&str], cwd: &Path, home: &Path, env: &[(&str, &str)]) -> Self {
        let mut child = Command::new(BIN)
            .args(args)
            .current_dir(cwd)
            .env("AGENTSTACK_HOME", home)
            .envs(env.iter().copied())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Connection {
            child,
            stdin,
            stdout,
        }
    }

    /// Send one request and return the frame carrying its id, skipping any
    /// server-initiated notification that arrives first.
    fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        writeln!(
            self.stdin,
            "{}",
            json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
        )
        .unwrap();
        self.stdin.flush().unwrap();
        loop {
            let mut line = String::new();
            assert!(
                self.stdout.read_line(&mut line).unwrap() > 0,
                "the MCP process closed stdout before answering id {id}"
            );
            let Ok(frame) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if frame.get("id") == Some(&json!(id)) {
                return frame;
            }
        }
    }

    fn call(&mut self, id: u64, tool: &str, arguments: Value) -> String {
        let frame = self.request(
            id,
            "tools/call",
            json!({ "name": tool, "arguments": arguments }),
        );
        frame["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    fn tool_names(&mut self, id: u64) -> Vec<String> {
        let frame = self.request(id, "tools/list", json!({}));
        frame["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["name"].as_str().map(str::to_string))
            .collect()
    }

    fn close(mut self) {
        drop(self.stdin);
        let _ = self.child.wait();
    }
}

/// `agentstack lease status --json`, decoded.
fn lease_status(home: &Path) -> Value {
    let out = Command::new(BIN)
        .args(["lease", "status", "--json"])
        .env("AGENTSTACK_HOME", home)
        .output()
        .unwrap();
    assert!(out.status.success(), "lease status failed");
    serde_json::from_slice(&out.stdout).unwrap()
}

// ── 1. externally visible, with honest liveness ──────────────────────────────

#[test]
fn an_open_lease_is_visible_to_another_surface_with_honest_liveness() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = sandbox_home(tmp.path());
    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(proj.join(".agentstack")).unwrap();
    std::fs::write(
        proj.join(".agentstack/agentstack.toml"),
        "version = 1\n[profiles.backend]\nservers = []\nskills = []\n",
    )
    .unwrap();

    assert!(
        lease_status(&home)["leases"].as_array().unwrap().is_empty(),
        "no lease has been opened yet"
    );

    let mut conn = Connection::open(
        &["mcp", "--manifest-dir", proj.to_str().unwrap()],
        tmp.path(),
        &home,
    );
    conn.request(1, "initialize", json!({}));
    let opened = conn.call(2, "agentstack_lease_open", json!({ "profile": "backend" }));
    assert!(opened.contains("\"opened\": \"backend\""), "{opened}");

    // The other surface — a separate process — can see the lease the MCP
    // connection is holding. This is the whole point of the registry.
    let seen = lease_status(&home);
    let rows = seen["leases"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "exactly one lease record: {seen}");
    let row = &rows[0];
    assert_eq!(row["toolset"], "backend");
    assert!(row["project"].as_str().unwrap().contains("proj"));
    assert!(row["instance"].as_str().unwrap().len() > 1);
    assert!(row["pid"].as_i64().unwrap() > 0);
    // Liveness is DERIVED, and this platform (Linux or macOS) can supply a
    // process start time — so a genuinely open lease reads `live`. It may
    // never read `stale` while the owning process is running.
    assert_eq!(
        row["liveness"], "live",
        "an open lease on a start-time-capable platform reads live: {seen}"
    );
    assert!(row["why"].as_str().unwrap().contains("start time"));
    assert!(
        seen["note"].as_str().unwrap().contains("process-scoped"),
        "the read states the honest scope of a lease: {seen}"
    );
    assert!(
        seen["features"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f == "lease-status-v1"),
        "the contract is advertised: {seen}"
    );

    // Closing the lease removes the record; ending the process removes any
    // record a close did not.
    let closed = conn.call(3, "agentstack_lease_close", json!({}));
    assert!(closed.contains("\"closed\": \"backend\""), "{closed}");
    assert!(
        lease_status(&home)["leases"].as_array().unwrap().is_empty(),
        "a closed lease leaves no record"
    );
    conn.close();
    cleanup();
}

// ── 2. a stale record never reads as live ────────────────────────────────────

/// The decisive witness. Two fabricated records — one whose PID is dead, one
/// whose PID is very much alive but whose start time does not match (simulated
/// PID reuse) — must both read stale. A registry that answered from the file
/// alone would call both of them live, which is exactly the bug this design
/// exists to prevent.
#[test]
fn a_stale_record_never_reads_as_live() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = sandbox_home(tmp.path());
    let leases = home.join("leases");
    std::fs::create_dir_all(&leases).unwrap();

    // A genuinely dead PID: spawn something trivial and reap it, so the number
    // is one the OS really handed out and really took back.
    let mut corpse = Command::new("/bin/sh")
        .arg("-c")
        .arg("exit 0")
        .spawn()
        .unwrap();
    let dead_pid = corpse.id() as i32;
    corpse.wait().unwrap();

    std::fs::write(
        leases.join("dead.json"),
        json!({
            "instance": "dead",
            "project": "/tmp/proj",
            "toolset": "backend",
            "pid": dead_pid,
            "start_token": "whatever-it-was",
            "started_unix": 1_700_000_000u64,
        })
        .to_string(),
    )
    .unwrap();

    // PID reuse: this test process is alive, so the PID check alone would say
    // "live". Only the start-time comparison can tell that this record was
    // written by a different process that happened to hold the same number.
    let live_pid = std::process::id() as i32;
    std::fs::write(
        leases.join("reused.json"),
        json!({
            "instance": "reused",
            "project": "/tmp/proj",
            "toolset": "frontend",
            "pid": live_pid,
            "start_token": "a-start-time-this-process-does-not-have",
            "started_unix": 1_700_000_001u64,
        })
        .to_string(),
    )
    .unwrap();

    let seen = lease_status(&home);
    let rows = seen["leases"].as_array().unwrap();
    assert_eq!(
        rows.len(),
        2,
        "both fabricated records are reported: {seen}"
    );
    for row in rows {
        assert_eq!(
            row["liveness"], "stale",
            "record {} must not read as live: {seen}",
            row["instance"]
        );
    }
    // And the reused-PID row is stale *despite* its process existing — the
    // property the start token buys.
    let reused = rows
        .iter()
        .find(|r| r["instance"] == "reused")
        .expect("the reused-PID record is present");
    assert_eq!(reused["pid"].as_i64().unwrap(), live_pid as i64);
    assert_eq!(reused["liveness"], "stale");

    cleanup();
}

// ── 3 & 4. toolset fencing ───────────────────────────────────────────────────

/// A trusted project declaring two toolsets, each fencing its own stdio server.
fn two_toolset_project(tmp: &Path) -> PathBuf {
    let proj = tmp.join("proj");
    std::fs::create_dir_all(proj.join(".agentstack")).unwrap();
    let alpha = tmp.join("alpha.sh");
    let beta = tmp.join("beta.sh");
    write_executable(&alpha, &fixture("alpha_ping"));
    write_executable(&beta, &fixture("beta_pong"));
    std::fs::write(
        proj.join(".agentstack/agentstack.toml"),
        format!(
            "version = 1\n\
             [targets]\ndefault = [\"claude-code\"]\n\
             [servers.srva]\ntype = \"stdio\"\ncommand = \"{}\"\n\
             [servers.srvb]\ntype = \"stdio\"\ncommand = \"{}\"\n\
             [profiles.alpha]\nservers = [\"srva\"]\nskills = []\n\
             [profiles.beta]\nservers = [\"srvb\"]\nskills = []\n",
            alpha.display(),
            beta.display()
        ),
    )
    .unwrap();
    let proj = proj.canonicalize().unwrap();
    let digest = agentstack::trust::digest_for(&proj).expect("a manifest to consent to");
    agentstack::commands::trust::grant_with_answers(&proj, true, Some(&digest), false, None)
        .expect("the fixture must be trusted for the gateway to serve anything");
    proj
}

#[test]
fn no_lease_means_control_plane_tools_only_even_with_several_toolsets_declared() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = sandbox_home(tmp.path());
    let proj = two_toolset_project(tmp.path());

    let mut conn = Connection::open(&["mcp", "--auto-project"], &proj, &home);
    conn.request(1, "initialize", json!({}));

    // The advertised surface is agentstack's own control plane, and nothing
    // from either toolset.
    let names = conn.tool_names(2);
    assert!(names.iter().any(|n| n == "agentstack_lease_open"));
    assert!(
        !names.iter().any(|n| n.contains("__")),
        "no upstream tool is advertised without a lease: {names:?}"
    );

    // Nor does the discovery tool reach them: with several toolsets declared
    // and nothing selected, the implicit union is never served.
    let found = conn.call(3, "tools_search", json!({ "query": "fixture tool" }));
    assert!(
        !found.contains("alpha_ping") && !found.contains("beta_pong"),
        "no lease must expose no toolset's members, got: {found}"
    );

    conn.close();
    cleanup();
}

#[test]
fn opening_a_lease_exposes_exactly_that_toolset() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = sandbox_home(tmp.path());
    let proj = two_toolset_project(tmp.path());

    let mut conn = Connection::open(&["mcp", "--auto-project"], &proj, &home);
    conn.request(1, "initialize", json!({}));
    let opened = conn.call(2, "agentstack_lease_open", json!({ "profile": "alpha" }));
    assert!(opened.contains("\"opened\": \"alpha\""), "{opened}");

    let found = conn.call(3, "tools_search", json!({ "query": "fixture tool" }));
    assert!(
        found.contains("alpha_ping"),
        "the leased toolset's member is exposed: {found}"
    );
    assert!(
        !found.contains("beta_pong"),
        "and nothing more — the other toolset stays fenced out: {found}"
    );

    // Closing returns the connection to control-plane-only, rather than
    // widening it back to the union.
    conn.call(4, "agentstack_lease_close", json!({}));
    let after = conn.call(5, "tools_search", json!({ "query": "fixture tool" }));
    assert!(
        !after.contains("alpha_ping") && !after.contains("beta_pong"),
        "closing a lease must not leave a wider surface behind: {after}"
    );

    conn.close();
    cleanup();
}

/// Every `calls.jsonl` row this sandbox's audit log holds.
fn audit_rows(home: &Path) -> Vec<Value> {
    std::fs::read_to_string(home.join("audit/calls.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .collect()
}

/// The fence is a refusal, and a refusal must leave evidence.
///
/// Closing the false precondition made the fenced gateway genuinely empty,
/// which is the stronger answer — but it also routed the call past the policy
/// firewall's denial record and into a bare `unknown tool` error. That made
/// the fence the one refusal shape in the product that blocked something and
/// wrote nothing down: no audit row for a reviewer, and a sentence that told
/// the caller a declared capability did not exist rather than that it was not
/// selected yet.
#[test]
fn a_fenced_call_to_a_declared_server_is_refused_and_recorded() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = sandbox_home(tmp.path());
    let proj = two_toolset_project(tmp.path());

    let mut conn = Connection::open(&["mcp", "--auto-project"], &proj, &home);
    conn.request(1, "initialize", json!({}));

    // `srva` IS declared, and toolset `alpha` selects it — it is fenced, not
    // absent. That distinction is the whole point of this witness.
    let frame = conn.request(
        2,
        "tools/call",
        json!({ "name": "srva__alpha_ping", "arguments": {} }),
    );
    assert_eq!(
        frame["result"]["isError"],
        json!(true),
        "the fenced call must be refused: {frame}"
    );
    let said = frame["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // (1) a refusal, in the seatbelt's shape.
    assert!(said.contains("blocked:"), "not a seatbelt sentence: {said}");
    assert!(
        said.contains("srva") && said.contains("alpha_ping"),
        "the refusal names what was refused: {said}"
    );
    assert!(said.contains("nothing ran"), "{said}");

    // (3) and the one command that fixes it, naming the toolset that selects
    // the server — not `[policy.tools]`, which denied nothing here.
    assert!(
        said.contains("agentstack_lease_open"),
        "the fix is opening a lease: {said}"
    );
    assert!(
        said.contains("profile=alpha"),
        "and it names the toolset that selects this server: {said}"
    );

    conn.close();

    // (2) the evidence. One row, denied, filed under the fence's own tag so a
    // reader cannot mistake it for a `[policy.tools]` denial.
    let fenced: Vec<Value> = audit_rows(&home)
        .into_iter()
        .filter(|r| r["tool"] == json!("fence"))
        .collect();
    assert_eq!(
        fenced.len(),
        1,
        "exactly one fence record expected, got {:#?}",
        audit_rows(&home)
    );
    assert_eq!(fenced[0]["outcome"], json!("denied"), "{:#?}", fenced[0]);
    assert_eq!(fenced[0]["server"], json!("srva"), "{:#?}", fenced[0]);

    cleanup();
}

/// `agentstack report run <id>`, as a reviewer reads it.
fn run_report(home: &Path, run: &str) -> String {
    let out = Command::new(BIN)
        .args(["report", "run", run])
        .env("AGENTSTACK_HOME", home)
        .output()
        .unwrap();
    assert!(out.status.success(), "report run failed");
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// Every event line of one run's `events.jsonl`, parsed.
fn run_events(home: &Path, run: &str) -> Vec<Value> {
    std::fs::read_to_string(home.join("runs").join(run).join("events.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .collect()
}

/// G17 — the fence refusal reaches a RUN REPORT, not only the machine-global
/// audit log.
///
/// The row in `calls.jsonl` proved the refusal happened; it did not make it
/// findable from the one place a reviewer reads one run's story. The report's
/// Tool-calls section is built from `ToolCall` events, and a fence refusal is
/// deliberately not one of those (it is not a call the run made) — so without
/// its own event and its own rendered section the refusal was invisible to
/// `agentstack report run <id>`. Both destinations, in one witness, because
/// either one alone is the bug.
#[test]
fn a_fenced_call_inside_a_run_reaches_both_the_audit_log_and_the_run_report() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = sandbox_home(tmp.path());
    let proj = two_toolset_project(tmp.path());
    let run = "run-fence-0001";

    let mut conn = Connection::open_with(
        &["mcp", "--auto-project"],
        &proj,
        &home,
        &[("AGENTSTACK_RUN_ID", run)],
    );
    conn.request(1, "initialize", json!({}));
    let frame = conn.request(
        2,
        "tools/call",
        json!({ "name": "srva__alpha_ping", "arguments": {} }),
    );
    assert_eq!(
        frame["result"]["isError"],
        json!(true),
        "the fenced call must still be refused: {frame}"
    );
    conn.close();

    // (1) the machine-global audit log, unchanged by this work.
    let fenced: Vec<Value> = audit_rows(&home)
        .into_iter()
        .filter(|r| r["tool"] == json!("fence"))
        .collect();
    assert_eq!(
        fenced.len(),
        1,
        "exactly one fence record expected, got {:#?}",
        audit_rows(&home)
    );
    assert_eq!(fenced[0]["run"], json!(run), "{:#?}", fenced[0]);

    // (2) the run's own event log — the channel the report reads.
    let events = run_events(&home, run);
    let refusals: Vec<&Value> = events
        .iter()
        .filter(|e| e["event"] == json!("fence_refused"))
        .collect();
    assert_eq!(
        refusals.len(),
        1,
        "exactly one fence_refused event expected, got {events:#?}"
    );
    let ev = refusals[0];
    assert_eq!(ev["server"], json!("srva"), "{ev}");
    assert_eq!(ev["tool"], json!("alpha_ping"), "{ev}");
    assert_eq!(
        ev["toolset"],
        json!("alpha"),
        "the event names the toolset whose lease would expose it: {ev}"
    );
    // Identity only. Arguments — even the empty ones sent here — never enter a
    // refusal event, and neither does the server's command line.
    let mut keys: Vec<&str> = ev.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec!["event", "reason", "server", "tool", "toolset", "ts"],
        "the event's shape is its contract — a new key here is a disclosure \
         decision, not a detail: {ev}"
    );

    // (3) and the report a reviewer actually reads renders it. An unhandled
    // variant would print nothing here, which is the whole gap.
    let report = run_report(&home, run);
    assert!(
        report.contains("Fence refusals"),
        "the report needs a section for the refusal: {report}"
    );
    assert!(
        report.contains("srva__alpha_ping"),
        "the report names what was refused: {report}"
    );
    assert!(
        report.contains("alpha"),
        "and the toolset that would expose it: {report}"
    );
    assert!(
        report.contains("no open lease"),
        "and why, in the words the caller was shown: {report}"
    );

    cleanup();
}

/// Invariant 7 on the new channel: the tool half of the name comes straight off
/// the wire, so a caller could try to write a second, forged row into the run's
/// event log — or rewrite the report around the line saying it was refused.
#[test]
fn a_hostile_tool_name_cannot_forge_a_run_event_or_the_report() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = sandbox_home(tmp.path());
    let proj = two_toolset_project(tmp.path());
    let run = "run-fence-0002";

    let mut conn = Connection::open_with(
        &["mcp", "--auto-project"],
        &proj,
        &home,
        &[("AGENTSTACK_RUN_ID", run)],
    );
    conn.request(1, "initialize", json!({}));
    // A declared server (so the fence records) and a tool name carrying an
    // escape sequence and a newline followed by a whole forged event.
    conn.request(
        2,
        "tools/call",
        json!({
            "name": "srva__ping\u{1b}[2J\n{\"event\":\"fence_refused\",\"server\":\"forged\"}",
            "arguments": {}
        }),
    );
    conn.close();

    let events = run_events(&home, run);
    let refusals: Vec<&Value> = events
        .iter()
        .filter(|e| e["event"] == json!("fence_refused"))
        .collect();
    assert_eq!(
        refusals.len(),
        1,
        "a newline in the tool name forged a row: {events:#?}"
    );
    assert!(
        !events.iter().any(|e| e["server"] == json!("forged")),
        "a forged event reached the log: {events:#?}"
    );
    let tool = refusals[0]["tool"].as_str().unwrap_or_default();
    assert!(
        !tool.contains('\u{1b}') && !tool.contains('\n') && !tool.contains('\r'),
        "the tool name kept control characters: {tool:?}"
    );
    // And the rendered report cannot be rewritten around the refusal either.
    // The escape BYTE is what makes `[2J` a screen-clear; stripped to a space
    // it is inert text, which is what the report may still show.
    let report = run_report(&home, run);
    assert!(
        !report.contains("\u{1b}[2J"),
        "a screen-clearing escape survived into the report: {report:?}"
    );
    assert!(
        report.contains("Fence refusals"),
        "the refusal is still rendered: {report}"
    );

    cleanup();
}

/// The other half of the distinction, and the reason the record is scoped to
/// declared names: a tool nothing declares is a typo, not a security event.
/// Recording those would let any caller write unbounded rows into the shared
/// audit log simply by inventing names.
#[test]
fn an_undeclared_tool_name_is_refused_but_records_nothing() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = sandbox_home(tmp.path());
    let proj = two_toolset_project(tmp.path());

    let mut conn = Connection::open(&["mcp", "--auto-project"], &proj, &home);
    conn.request(1, "initialize", json!({}));
    let frame = conn.request(
        2,
        "tools/call",
        json!({ "name": "nosuch__whatever", "arguments": {} }),
    );
    assert_eq!(
        frame["result"]["isError"],
        json!(true),
        "an unknown name is still an error: {frame}"
    );
    let said = frame["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        said.contains("unknown tool"),
        "an undeclared name keeps the plain error: {said}"
    );

    conn.close();
    assert!(
        audit_rows(&home)
            .iter()
            .all(|r| r["tool"] != json!("fence")),
        "an undeclared name must not write to the audit log: {:#?}",
        audit_rows(&home)
    );

    cleanup();
}

// ── 5. gateway unavailable ───────────────────────────────────────────────────

/// Every path under `root`, relative and sorted — the "was anything written?"
/// fingerprint.
fn tree(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            out.push(
                path.strip_prefix(root)
                    .unwrap_or(&path)
                    .display()
                    .to_string(),
            );
            if path.is_dir() {
                stack.push(path);
            }
        }
    }
    out.sort();
    out
}

fn run_cli(args: &[&str], cwd: &Path, home: &Path) -> String {
    let out = Command::new(BIN)
        .args(args)
        .current_dir(cwd)
        .env("AGENTSTACK_HOME", home)
        .env("HOME", home.parent().unwrap())
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn an_unavailable_gateway_yields_no_tools_and_writes_no_file() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = sandbox_home(tmp.path());
    let user_home = tmp.path().join("home");

    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(proj.join(".agentstack")).unwrap();
    std::fs::write(
        proj.join(".agentstack/agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [servers.srv]\ntype = \"stdio\"\ncommand = \"echo\"\n",
    )
    .unwrap();

    // A harness with the gateway registered — pointing at a binary that is not
    // there. This is what a moved or uninstalled agentstack looks like from
    // inside the harness config, and it is the concrete form of "the gateway
    // is unreachable".
    let missing = tmp.path().join("nowhere/agentstack");
    std::fs::write(
        user_home.join(".claude.json"),
        json!({
            "mcpServers": {
                "agentstack": {
                    "command": missing.display().to_string(),
                    "args": ["mcp", "--auto-project"],
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    // (a) The harness receives no tools, because the process it was told to
    // launch cannot be launched. Nothing subtler is going on: there is no
    // bridge, so there is no tool list.
    assert!(
        Command::new(&missing).arg("mcp").spawn().is_err(),
        "the registered gateway command must not be launchable"
    );

    let before = tree(&proj);

    // (b) Both surfaces explain the outage in one sentence and name the one
    // recovery command.
    let status = run_cli(&["status"], &proj, &home);
    assert!(
        status.contains("Gateway unavailable"),
        "status explains the outage: {status}"
    );
    assert!(
        status.contains("receives no tools"),
        "status says what the harness gets: {status}"
    );
    assert!(
        // The namespaced spelling is the discoverable one — `gateway` is hidden,
        // so `agentstack x gateway …` is the form every surface prints
        // (`connect::GATEWAY_RECOVERY`). The bare spelling still parses, but the
        // surfaces must name what `agentstack --help` can reach in one hop.
        status.contains("agentstack x gateway connect --all"),
        "status names the one recovery command: {status}"
    );

    let doctor = run_cli(&["doctor", "--all"], &proj, &home);
    assert!(
        doctor.contains("Gateway unavailable"),
        "doctor explains the outage: {doctor}"
    );
    assert!(
        // Same namespaced spelling as the status assertion above.
        doctor.contains("agentstack x gateway connect --all"),
        "doctor names the one recovery command: {doctor}"
    );

    // (c) And nothing was written in the gateway's place. No silent fallback
    // into the rendered lane: a static render is always an explicit user
    // action, so an outage leaves the project exactly as it was.
    assert_eq!(
        tree(&proj),
        before,
        "an outage must not write anything into the project"
    );
    assert!(!proj.join(".mcp.json").exists());
    assert!(!proj.join(".claude").exists());

    cleanup();
}
