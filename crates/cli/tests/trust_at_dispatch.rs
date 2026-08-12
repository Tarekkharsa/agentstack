// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! W2 — trust is checked at dispatch, from the digest.
//!
//! `docs/design/automatic-delivery.md` §"Trust is checked at dispatch, from
//! the digest" states the property these tests exist to hold: revoking trust,
//! or drifting the consented bytes, must stop the **next** upstream call on an
//! already-established connection — not merely refuse the next lease, load, or
//! session call. Before W2 an already-spawned server stayed proxied until one
//! of those control-plane calls happened to re-check, so a withdrawn yes left
//! a live path open for as long as the agent kept using tools it already had.
//!
//! Every case below therefore has the same three beats, and the first is the
//! one that makes it a witness at all:
//!
//! 1. **Establish a live connection** — a real stdio child, spawned, spoken
//!    to, and answering.
//! 2. **Mutate trust the way the world actually mutates it** — outside
//!    AgentStack, so nothing in-process could have been notified.
//! 3. **Call again**, and require a refusal that is seatbelt-shaped and left
//!    evidence.
//!
//! The fixture "server" is a plain `sh` script, so the file has no runtime
//! dependency beyond a POSIX shell.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{json, Value};

use agentstack::gateway::Gateway;

// These tests mutate the process-global HOME/AGENTSTACK_HOME and the run-id
// env; serialize them.
static ENV_LOCK: Mutex<()> = Mutex::new(());

mod common;
use common::StdioServer;

/// A minimal MCP stdio server: answers `tools/list` with one `echo` tool and
/// `tools/call` by echoing the `msg` argument back.
fn fixture() -> String {
    StdioServer::new("fix")
        .tools(
            r#"{"name":"echo","description":"Echo the input back.","inputSchema":{"type":"object","properties":{"msg":{"type":"string"}},"required":["msg"]}}"#,
        )
        .on_call(
            r#"      msg=$(printf '%s' "$line" | sed -n 's/.*"msg":"\([^"]*\)".*/\1/p')
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"echo:%s"}]}}\n' "$id" "$msg""#,
        )
        .script()
}

/// A lock document that parses — the "before" bytes of the replacement case.
const LOCK_BEFORE: &str = "version = 2\n";

/// A different, equally valid lock document: what landing someone else's
/// re-lock (a `git pull`, a branch switch) actually puts on disk.
const LOCK_AFTER: &str = "version = 2\n\n[[instruction]]\nname = \"house-rules\"\npath = \"docs/house.md\"\nchecksum = \"1111111111111111111111111111111111111111111111111111111111111111\"\n";

/// Point HOME/AGENTSTACK_HOME at a sandbox, write a project whose one stdio
/// server is the fixture script, and trust it. Returns the project base.
///
/// `server_key` is the text placed inside the TOML quoted key, so a test can
/// declare a deliberately hostile name using TOML's own escapes.
fn trusted_project(tmp: &Path, server_key: &str) -> PathBuf {
    let home = tmp.join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    let proj = tmp.join("proj");
    std::fs::create_dir_all(proj.join(".agentstack")).unwrap();
    let script = proj.join("fix.sh");
    std::fs::write(&script, fixture()).unwrap();
    std::fs::write(
        proj.join(".agentstack/agentstack.toml"),
        format!(
            "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
             [servers.\"{server_key}\"]\ntype = \"stdio\"\ncommand = \"/bin/sh\"\nargs = [\"{}\"]\n",
            script.display()
        ),
    )
    .unwrap();
    std::fs::write(proj.join(".agentstack/agentstack.lock"), LOCK_BEFORE).unwrap();

    // `trust_unreviewed` is the test-fixture grant: it records trust at
    // whatever the project digests to right now, which is exactly the state a
    // human's `agentstack trust` would leave behind for these purposes.
    agentstack::trust::trust_unreviewed(&proj).expect("fixture must be trusted");
    proj
}

/// Build the gateway and prove the connection is LIVE: one successful round
/// trip through the real child. Everything after this is about an established
/// connection, which is the whole point.
fn live_gateway(proj: &Path, run: &str, server_name: &str) -> Gateway {
    // The gateway inherits run attribution from the environment at
    // construction, so the run id has to be set before it is built.
    std::env::set_var("AGENTSTACK_RUN_ID", run);
    let gw = Gateway::from_manifest(Some(proj));
    assert!(
        gw.trust_anchor().is_some(),
        "a trusted project must anchor its gateway — without that there is \
         nothing for the dispatch gate to compare against"
    );
    let first = gw
        .try_call(&format!("{server_name}__echo"), &json!({ "msg": "hello" }))
        .expect("the gateway owns this server")
        .expect("the first call must SUCCEED — the connection has to be live");
    assert!(
        first["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("echo:hello"),
        "the upstream really answered: {first}"
    );
    gw
}

/// Every event line of a run, as parsed JSON. (Same shape as
/// `pin_refusal_is_recorded.rs`'s helper.)
fn run_events(run: &str) -> Vec<Value> {
    let home = std::env::var("AGENTSTACK_HOME").unwrap();
    let mut out = Vec::new();
    for entry in walk(Path::new(&home)) {
        if entry.file_name().is_some_and(|n| n == "events.jsonl")
            && entry.to_string_lossy().contains(run)
        {
            for line in std::fs::read_to_string(&entry).unwrap_or_default().lines() {
                if let Ok(v) = serde_json::from_str::<Value>(line) {
                    out.push(v);
                }
            }
        }
    }
    out
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}

/// The one trust event of a run, or a readable failure.
fn sole_trust_event(run: &str) -> Value {
    let events = run_events(run);
    let refusals: Vec<&Value> = events
        .iter()
        .filter(|e| e["event"].as_str() == Some("trust_refused"))
        .collect();
    assert_eq!(
        refusals.len(),
        1,
        "exactly one trust_refused event expected, got {events:#?}"
    );
    refusals[0].clone()
}

/// Replace the trust store's entries — a revoke that touches nothing else, so
/// the consented bytes are provably unchanged and only the *yes* moved.
fn revoke_trust() {
    let path = agentstack::trust::store_path();
    let mut store: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    store["trusted"] = json!({});
    std::fs::write(&path, serde_json::to_string(&store).unwrap()).unwrap();
}

/// Assert a refusal reads as a trust refusal and names its fix.
fn assert_trust_shaped(err: &anyhow::Error, tool: &str) {
    let s = format!("{err}");
    assert!(s.starts_with("blocked:"), "not a seatbelt sentence: {s}");
    assert!(
        s.contains(tool),
        "the refusal must name what was tried: {s}"
    );
    assert!(s.contains("nothing ran"), "{s}");
    assert!(
        s.contains("agentstack trust"),
        "a refusal must name the ONE command that fixes it: {s}"
    );
}

fn cleanup() {
    std::env::remove_var("AGENTSTACK_RUN_ID");
    std::env::remove_var("AGENTSTACK_HOME");
}

/// One live `agentstack mcp` process, driven request by request.
///
/// The in-process tests above can only reach the gateway; the two below have
/// to reach the whole MCP surface, because the claim they witness is about
/// what *stays* reachable. Hence a real subprocess and a real JSON-RPC
/// conversation, `mcp_lease.rs`-style — except interleaved rather than
/// batched, since the mutation has to land between two calls on the same
/// connection.
struct McpSession {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    stdout: std::io::BufReader<std::process::ChildStdout>,
}

impl McpSession {
    fn open(proj: &Path) -> McpSession {
        let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_agentstack"))
            .args(["mcp", "--manifest-dir"])
            .arg(proj)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = std::io::BufReader::new(child.stdout.take().unwrap());
        let mut session = McpSession {
            child,
            stdin,
            stdout,
        };
        session.request(1, "initialize", legacy_initialize_params());
        session
    }

    /// Send one request and read its response. Requests are issued one at a
    /// time, so no id matching is needed.
    fn send(&mut self, id: u64, frame: Value) -> Value {
        use std::io::{BufRead, Write};
        writeln!(self.stdin, "{frame}").unwrap();
        self.stdin.flush().unwrap();
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        let v: Value = serde_json::from_str(&line)
            .unwrap_or_else(|e| panic!("not a JSON-RPC frame ({e}): {line:?}"));
        assert_eq!(v["id"], json!(id), "unexpected response: {v}");
        v
    }

    fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(
            id,
            json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
        )
    }

    fn call(&mut self, id: u64, name: &str, arguments: Value) -> Value {
        self.request(
            id,
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        )
    }

    fn close(mut self) {
        drop(self.stdin);
        let _ = self.child.wait();
    }
}

fn legacy_initialize_params() -> Value {
    json!({
        "protocolVersion": "2025-11-25",
        "capabilities": {},
        "clientInfo": { "name": "agentstack-test", "version": "1" }
    })
}

/// The text of a `tools/call` result, whatever its outcome.
fn call_text(response: &Value) -> &str {
    response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
}

/// The other half of the contract: what is emptied is the **upstream
/// capability surface**, not the whole connection. A user whose project just
/// went untrusted has to be able to see why and fix it — blinding them would
/// turn a fail-closed refusal into a dead end.
#[test]
fn control_plane_tools_survive_a_mid_connection_trust_refusal() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = trusted_project(tmp.path(), "fix");
    let mut mcp = McpSession::open(&proj);

    let ok = mcp.call(2, "fix__echo", json!({ "msg": "hello" }));
    assert!(
        call_text(&ok).contains("echo:hello"),
        "the connection must be live before it is broken: {ok}"
    );

    revoke_trust();

    let refused = mcp.call(3, "fix__echo", json!({ "msg": "again" }));
    assert_eq!(
        refused["result"]["isError"],
        json!(true),
        "the next upstream call must be refused: {refused}"
    );
    let text = call_text(&refused);
    assert!(text.contains("blocked:"), "{text}");
    assert!(text.contains("agentstack trust"), "{text}");

    // ...and the diagnosis path is still open on the SAME connection.
    for (id, tool) in [(4, "agentstack_doctor"), (5, "agentstack_list_loadable")] {
        let resp = mcp.call(id, tool, json!({}));
        assert_eq!(
            resp["result"]["isError"],
            json!(false),
            "{tool} must still answer after a trust refusal: {resp}"
        );
        assert!(
            !call_text(&resp).is_empty(),
            "{tool} answered emptily: {resp}"
        );
    }
    mcp.close();
    cleanup();
}

/// W1's acceptance clause, held at W2's boundary: the agent can relay a
/// refusal and can never answer it. Nothing added here may expose a
/// trust-granting operation over MCP — not as a tool, not as a description
/// that invites one, and not as an undeclared name that happens to work.
#[test]
fn no_mcp_invocable_consent_path_exists() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = trusted_project(tmp.path(), "fix");
    let store = agentstack::trust::store_path();
    let before = std::fs::read(&store).unwrap();

    let mut mcp = McpSession::open(&proj);
    let listed = mcp.request(2, "tools/list", json!({}));
    let tools = listed["result"]["tools"]
        .as_array()
        .expect("a tool list")
        .clone();
    assert!(!tools.is_empty(), "the control plane must advertise tools");

    for tool in &tools {
        let name = tool["name"]
            .as_str()
            .unwrap_or_default()
            .to_ascii_lowercase();
        for banned in ["trust", "consent", "approve", "grant"] {
            assert!(
                !name.contains(banned),
                "a tool named {name:?} offers the one answer only a human may give"
            );
        }
        // Descriptions may *mention* trust (`agentstack_explain` tells the
        // agent to read a capability before a human trusts it). What none may
        // do is offer to perform the granting.
        let desc = tool["description"]
            .as_str()
            .unwrap_or_default()
            .to_ascii_lowercase();
        for banned in [
            "grant trust",
            "trusts the project",
            "trust the project",
            "record trust",
            "mark as trusted",
            "consent to",
            "approve the manifest",
        ] {
            assert!(
                !desc.contains(banned),
                "{name}'s description offers to grant consent: {desc}"
            );
        }
    }

    // A plausible forged name must not quietly work.
    let forged = mcp.call(3, "agentstack_trust", json!({ "dir": "." }));
    assert_eq!(
        forged["result"]["isError"],
        json!(true),
        "an undeclared consent tool must not answer: {forged}"
    );
    mcp.close();

    assert_eq!(
        before,
        std::fs::read(&store).unwrap(),
        "the trust store must be byte-identical — nothing over MCP may write consent"
    );
    cleanup();
}

/// Case 1 of the contract: **trust revoked** mid-connection. The bytes never
/// move; only the human's yes does.
#[test]
fn revoked_trust_stops_the_next_upstream_call_on_a_live_connection() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let run = "run-trust-0001";
    let proj = trusted_project(tmp.path(), "fix");
    let gw = live_gateway(&proj, run, "fix");

    revoke_trust();

    let err = gw
        .try_call("fix__echo", &json!({ "msg": "again" }))
        .expect("the gateway still owns the server — it refuses, it does not disown")
        .expect_err("the NEXT call on the live connection must be refused");
    assert_trust_shaped(&err, "echo");
    assert!(
        format!("{err}").contains("revoked"),
        "a withdrawn yes must not be reported as drifted bytes: {err}"
    );

    let event = sole_trust_event(run);
    assert_eq!(event["server"].as_str(), Some("fix"));
    assert_eq!(event["tool"].as_str(), Some("echo"));
    assert_eq!(event["state"].as_str(), Some("revoked"));
    cleanup();
}

/// Case 2: an **out-of-band manifest modification** — bytes appended by
/// nothing but a text editor, so no AgentStack code path could have observed
/// the change.
#[test]
fn out_of_band_manifest_edit_stops_the_next_upstream_call() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let run = "run-trust-0002";
    let proj = trusted_project(tmp.path(), "fix");
    let gw = live_gateway(&proj, run, "fix");

    let manifest = proj.join(".agentstack/agentstack.toml");
    let mut body = std::fs::read_to_string(&manifest).unwrap();
    body.push_str("\n# appended by a text editor, not by agentstack\n");
    std::fs::write(&manifest, body).unwrap();

    let err = gw
        .try_call("fix__echo", &json!({ "msg": "again" }))
        .expect("the gateway still owns the server")
        .expect_err("an out-of-band manifest edit must stop the next call");
    assert_trust_shaped(&err, "echo");

    let event = sole_trust_event(run);
    assert_eq!(event["state"].as_str(), Some("changed"));
    cleanup();
}

/// Case 3: the **lock replaced wholesale**, which is what a `git pull` or a
/// branch switch actually does to a project.
#[test]
fn wholesale_lock_replacement_stops_the_next_upstream_call() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let run = "run-trust-0003";
    let proj = trusted_project(tmp.path(), "fix");
    let gw = live_gateway(&proj, run, "fix");

    // Overwritten, not edited: the whole file arrives from somewhere else.
    std::fs::write(proj.join(".agentstack/agentstack.lock"), LOCK_AFTER).unwrap();

    let err = gw
        .try_call("fix__echo", &json!({ "msg": "again" }))
        .expect("the gateway still owns the server")
        .expect_err("a swapped lock must stop the next call");
    assert_trust_shaped(&err, "echo");

    let event = sole_trust_event(run);
    assert_eq!(event["state"].as_str(), Some("changed"));
    cleanup();
}

/// The emptied surface: after a violation the gateway advertises no upstream
/// tools at all, cache or no cache. A refusal that still *listed* the tools
/// would invite the agent to keep trying, and would leak the shape of a
/// project the connection is no longer authorized for.
#[test]
fn emptied_surface_hides_upstream_tools_from_tools_list() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let run = "run-trust-0004";
    let proj = trusted_project(tmp.path(), "fix");
    let gw = live_gateway(&proj, run, "fix");

    // Populate the discovery cache first: the cache is exactly what would
    // otherwise keep serving a list the project no longer has a yes for.
    assert!(
        gw.namespaced_tools()
            .iter()
            .any(|t| { t.get("name").and_then(Value::as_str) == Some("fix__echo") }),
        "the trusted surface must list the upstream tool first"
    );

    revoke_trust();

    assert!(
        gw.namespaced_tools().is_empty(),
        "the upstream capability surface must EMPTY on a trust violation"
    );
    cleanup();
}

/// The refusal is seatbelt-shaped and evidenced, and repository-authored
/// identifiers cannot forge either. Mirrors the hostile-name case in
/// `pin_refusal_is_recorded.rs`: the server name is manifest content, which is
/// hostile input (invariant 7).
#[test]
fn the_refusal_is_seatbelt_shaped_and_recorded() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let run = "run-trust-0005";
    // A name that tries to clear the screen and forge an "allowed" line, plus
    // a newline that would split one JSON row into two. Declared with TOML's
    // own escapes on the left and the decoded bytes on the right — the two
    // must describe the same name, or the test is asserting nothing.
    let hostile = "evil\u{1b}[2J\nallowed";
    let proj = trusted_project(tmp.path(), "evil\\u001B[2J\\nallowed");
    let gw = live_gateway(&proj, run, hostile);

    revoke_trust();

    let err = gw
        .try_call(&format!("{hostile}__echo"), &json!({ "msg": "x" }))
        .expect("the gateway owns the hostile-named server")
        .expect_err("the next call must be refused");
    let sentence = format!("{err}");

    // All four parts of a seatbelt denial, in the order a reader needs them.
    assert!(sentence.starts_with("blocked:"), "{sentence}");
    assert!(sentence.contains("call echo"), "attempted: {sentence}");
    assert!(
        sentence.contains("revoked"),
        "why, named as itself: {sentence}"
    );
    assert!(sentence.contains("nothing ran"), "{sentence}");
    assert!(
        sentence.contains("agentstack trust"),
        "next step: {sentence}"
    );
    // The hostile name cannot rewrite the terminal around the sentence that
    // says the call was stopped.
    assert!(
        !sentence.contains('\u{1b}'),
        "an escape survived into the denial: {sentence:?}"
    );

    // Evidence: identity, not bytes — and not a forged second row.
    let event = sole_trust_event(run);
    let mut keys: Vec<&str> = event
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec!["event", "reason", "server", "state", "tool", "ts"],
        "the event's shape is its contract — a new key here is a disclosure \
         decision, not a detail"
    );
    assert!(
        !run_events(run)
            .iter()
            .any(|e| e["server"].as_str() == Some("allowed")),
        "a newline in the server name forged a row"
    );
    for field in ["server", "tool", "reason"] {
        let v = event[field].as_str().unwrap_or_default();
        assert!(
            !v.contains('\u{1b}') && !v.contains('\n'),
            "{field} kept control characters: {v:?}"
        );
    }
    // The call arguments are NOT in the evidence: a call refused for lack of a
    // valid yes is precisely one whose payload must not be copied anywhere.
    assert!(
        !serde_json::to_string(&event).unwrap().contains("args"),
        "the trust event must stay identity-shaped: {event}"
    );
    cleanup();
}
