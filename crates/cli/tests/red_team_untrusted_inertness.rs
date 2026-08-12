// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! RED TEAM — a hostile repository is opened, and nobody has said yes yet.
//!
//! This is the threat the whole trust gate exists for: a checked-out project
//! whose manifest declares a server and a skill, connected to an agent by a
//! user who has done nothing more than open the directory. Invariant 3 says
//! that content is inert until the gate succeeds, and `docs/ENFORCEMENT.md`
//! scopes that claim precisely: on the **automatic** path — the auto-project
//! gateway — nothing spawns, nothing is contacted, no secret resolves, and no
//! declared content enters agent context.
//!
//! The fixture is built so that inertness is observable rather than asserted:
//! the declared server writes a marker file *the moment it starts*. If the
//! marker exists, something ran. The skill body carries a sentinel string; if
//! that string appears anywhere in the MCP traffic, untrusted content entered
//! the model's context.
//!
//! The project also ships a committed `agentstack.lock`, because pinning is
//! not consent — an attacker checks that file in like any other. Without it
//! the refusals below would fire on "unpinned" instead of "untrusted".
//!
//! The last test is the control. The same session, over the same project,
//! after a real grant — if the upstream call is not answered then, the
//! refusals above would prove nothing but a broken fixture.

#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// Written by the declared server as its first act. Its absence is the proof.
const SPAWN_MARKER: &str = "SERVER_STARTED";
/// Planted in the skill body; must never appear in MCP traffic before trust.
const SKILL_SENTINEL: &str = "INERT-BODY-SENTINEL-4c1f";
/// The value the manifest's `${RT_INERT_SECRET}` would resolve to.
const SECRET: &str = "sk-inert-DEADBEEF-neverresolved";

mod common;
use common::StdioServer;

/// A stdio "server" that announces its own start on the filesystem and then
/// speaks just enough MCP to be proxied. The marker file is the whole point:
/// if the trust gate leaks, this touches it.
fn announcer() -> String {
    StdioServer::new("probe")
        .prologue(r#"touch "$MARKER""#)
        .tools(r#"{"name":"ping","description":"Ping.","inputSchema":{"type":"object","properties":{}}}"#)
        .on_call(
            r#"      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"pong"}]}}\n' "$id""#,
        )
        .script()
}

struct Fixture {
    home: PathBuf,
    proj: PathBuf,
    marker: PathBuf,
}

fn fixture(tmp: &Path) -> Fixture {
    let home = tmp.join("home");
    let proj = tmp.join("proj");
    let marker = tmp.join(SPAWN_MARKER);
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(proj.join("skills/secretive")).unwrap();
    let script = proj.join("probe.sh");
    fs::write(&script, announcer()).unwrap();
    fs::write(
        proj.join("skills/secretive/SKILL.md"),
        format!("---\nname: secretive\ndescription: does things\n---\n\n{SKILL_SENTINEL}\n"),
    )
    .unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        format!(
            "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
             [skills.secretive]\npath = \"./skills/secretive\"\n\
             [servers.probe]\ntype = \"stdio\"\ncommand = \"/bin/sh\"\n\
             args = [\"{}\"]\n\
             env = {{ MARKER = \"{}\", TOKEN = \"${{RT_INERT_SECRET}}\" }}\n",
            script.display(),
            marker.display()
        ),
    )
    .unwrap();
    let fx = Fixture { home, proj, marker };
    // The attacker commits `agentstack.lock` too. Pinning is not consent — it
    // is just another repository file — so the fixture ships one. Without it
    // the load refusal below would fire on "unpinned", not on "untrusted",
    // and the test would pass even with the trust gate removed (it did, until
    // this line was added).
    cli(&fx, &["lock", "--write"]);
    fx
}

/// Run the real binary in the fixture's isolated environment.
fn cli(fx: &Fixture, args: &[&str]) -> (String, bool) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(args)
        .current_dir(&fx.proj)
        .env_clear()
        .env("HOME", &fx.home)
        .env("AGENTSTACK_HOME", fx.home.join(".agentstack"))
        .env("PATH", "/usr/bin:/bin")
        .env("RT_INERT_SECRET", SECRET)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("spawn agentstack");
    (
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
        out.status.success(),
    )
}

/// One live `agentstack mcp --auto-project` process. Adapted from
/// `yes_on_lease_path.rs` (test binaries cannot import each other) with a
/// fully controlled environment, so the secret is genuinely resolvable and
/// "did not resolve" cannot be an accident of a missing variable.
struct McpSession {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    stdout: std::io::BufReader<std::process::ChildStdout>,
}

impl McpSession {
    fn open(fx: &Fixture) -> McpSession {
        let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_agentstack"))
            .args(["mcp", "--auto-project"])
            .current_dir(&fx.proj)
            .env_clear()
            .env("HOME", &fx.home)
            .env("AGENTSTACK_HOME", fx.home.join(".agentstack"))
            .env("PATH", "/usr/bin:/bin")
            .env("AGENTSTACK_MANIFEST_DIR", &fx.proj)
            .env("RT_INERT_SECRET", SECRET)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = std::io::BufReader::new(child.stdout.take().unwrap());
        let mut s = McpSession {
            child,
            stdin,
            stdout,
        };
        s.request(1, "initialize", legacy_initialize_params());
        s
    }

    // `id` is already inside `frame`; it is kept in the signature so every
    // call site reads as "frame N" and the reply can be matched by eye.
    fn send(&mut self, _id: u64, frame: Value) -> Value {
        use std::io::{BufRead, Write};
        writeln!(self.stdin, "{frame}").unwrap();
        self.stdin.flush().unwrap();
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        serde_json::from_str(&line)
            .unwrap_or_else(|e| panic!("not a JSON-RPC frame ({e}): {line:?}"))
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

fn tool_names(list: &Value) -> Vec<String> {
    list["result"]["tools"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|t| t["name"].as_str().map(str::to_string))
        .collect()
}

/// Pin the project and grant consent — the same two-step a panel drives.
fn grant(fx: &Fixture) {
    let run = |args: &[&str]| cli(fx, args);
    let (text, ok) = run(&["lock", "--write"]);
    assert!(ok, "lock failed:\n{text}");
    let digest = serde_json::from_str::<Value>(&run(&["trust", "--preview"]).0).unwrap()
        ["surface_digest"]
        .as_str()
        .unwrap()
        .to_string();
    let (text, ok) = run(&["trust", "--yes", "--consented", &digest]);
    assert!(ok, "grant failed:\n{text}");
}

/// Nothing spawns, nothing is offered, nothing of the repo's content is said.
#[test]
fn an_untrusted_project_spawns_nothing_and_offers_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let fx = fixture(tmp.path());
    let mut mcp = McpSession::open(&fx);

    let list = mcp.request(2, "tools/list", json!({}));
    let names = tool_names(&list);
    // Weak on its own — upstream tools are absent from the flat list even when
    // trusted (see the control below). It is kept only as a cheap tripwire; the
    // load-bearing assertions are the refusal, the sentinel and the marker.
    assert!(
        !names.iter().any(|n| n.starts_with("probe__")),
        "an untrusted project's server tools were offered to the model: {names:?}"
    );
    assert!(
        !names.is_empty(),
        "the control plane must still be reachable — inert is not broken: {list}"
    );

    // Ask for the upstream tool anyway. It must be refused, and the refusal
    // must name the one command that answers it.
    let refused = mcp.call(3, "probe__ping", json!({}));
    let text = serde_json::to_string(&refused).unwrap();
    assert!(
        text.contains("agentstack trust") || refused["result"]["isError"] == json!(true),
        "an untrusted server call was not refused: {refused}"
    );

    // The whole session, checked for the two things that must never appear.
    let traffic = format!("{list}{refused}");
    assert!(
        !traffic.contains(SKILL_SENTINEL),
        "untrusted skill content entered agent context: {traffic}"
    );
    assert!(
        !traffic.contains(SECRET),
        "a secret resolved for an untrusted project: {traffic}"
    );

    mcp.close();
    assert!(
        !fx.marker.exists(),
        "the declared server was SPAWNED for an untrusted project"
    );
}

/// The load door specifically: a skill is the shortest path into the model's
/// context, so it gets its own refusal check.
#[test]
fn an_untrusted_skill_cannot_be_loaded_into_context() {
    let tmp = tempfile::tempdir().unwrap();
    let fx = fixture(tmp.path());
    let mut mcp = McpSession::open(&fx);

    let listed = mcp.call(2, "agentstack_list_loadable", json!({}));
    let loaded = mcp.call(
        3,
        "agentstack_load",
        json!({ "name": "secretive", "reason": "red team" }),
    );
    let traffic = format!("{listed}{loaded}");
    assert!(
        !traffic.contains(SKILL_SENTINEL),
        "an untrusted skill body was returned to the model: {traffic}"
    );
    mcp.close();
    assert!(!fx.marker.exists(), "listing/loading spawned the server");
}

/// The control. Same project, same session shape, after a real grant — the
/// upstream call is answered and the server does spawn. Without this, every
/// assertion above could be satisfied by a gateway that simply never works.
///
/// Note what is NOT asserted: that `probe__ping` appears in `tools/list`.
/// Upstream tools are deliberately not advertised in the flat tool list even
/// when trusted — they are reached through `tools_search` or a lease, which is
/// a context-budget decision, not a trust decision. So the honest witness of
/// "live" is that the dispatch answers and the process starts.
#[test]
fn the_same_project_becomes_live_once_consent_is_given() {
    let tmp = tempfile::tempdir().unwrap();
    let fx = fixture(tmp.path());
    grant(&fx);

    let mut mcp = McpSession::open(&fx);
    let called = mcp.call(3, "probe__ping", json!({}));
    assert!(
        serde_json::to_string(&called).unwrap().contains("pong"),
        "the trusted server did not answer: {called}"
    );
    mcp.close();
    assert!(
        fx.marker.exists(),
        "the fixture server never announced itself even when trusted — the \
         spawn marker is not a valid witness"
    );
}
