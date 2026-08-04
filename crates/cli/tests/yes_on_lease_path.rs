// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! W1 — the yes on the lease path.
//!
//! `docs/design/automatic-delivery.md` §"Where the yes lives" states the three
//! properties these tests exist to hold:
//!
//! 1. a refusal **names what was refused and the one command that fixes it**;
//! 2. the card rendered from a refusal **discloses no less** than the card
//!    rendered from `agentstack trust`;
//! 3. **no MCP-invocable consent path exists** — the agent can relay the
//!    refusal and can never answer it.
//!
//! Before W1 the first two doors — `agentstack_lease_open` and
//! `agentstack_load` — refused with a bare sentence and recorded nothing. The
//! dispatch refusal (W2) was already evidenced; these two were the quiet ones,
//! which made "needs your yes" unanswerable from anything but a terminal
//! someone happened to be watching.
//!
//! Two modes are exercised, because the two refusals arrive by different
//! routes. `--auto-project` is where a never-trusted project is gated (the
//! `AutoProject` trust note); the eager `--manifest-dir` launch is where a
//! project that WAS trusted and then drifted is gated (the trust anchor). A
//! test that only used one would witness half the door.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{json, Value};

// These tests mutate the process-global HOME/AGENTSTACK_HOME (children inherit
// them, and the in-process card read below uses them directly); serialize them.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// A project with one stdio server and one inline skill: enough surface for the
/// review card to have something to disclose, and enough for a load to be a
/// real request rather than a name that was never going to resolve.
///
/// The base is canonicalized because two surfaces have to agree on it — the MCP
/// server records the project it refused for, and `status` matches its own
/// reading against that string. On macOS `/var` is a symlink to `/private/var`,
/// so an uncanonicalized temp path would make the two disagree for a reason
/// that has nothing to do with what is being tested.
fn project(tmp: &Path) -> PathBuf {
    let home = tmp.join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    let proj = tmp.join("proj");
    std::fs::create_dir_all(proj.join(".agentstack/skills/helper")).unwrap();
    std::fs::write(
        proj.join(".agentstack/skills/helper/SKILL.md"),
        "---\nname: helper\ndescription: Helps.\n---\nzzskillbody\n",
    )
    .unwrap();
    std::fs::write(
        proj.join(".agentstack/agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [profiles.backend]\nservers = [\"fix\"]\nskills = [\"helper\"]\n\
         [servers.fix]\ntype = \"stdio\"\ncommand = \"echo\"\n\
         [skills.helper]\npath = \"skills/helper\"\n",
    )
    .unwrap();
    proj.canonicalize().unwrap()
}

fn cleanup() {
    std::env::remove_var("AGENTSTACK_RUN_ID");
    std::env::remove_var("AGENTSTACK_HOME");
}

/// Every `calls.jsonl` row this machine's audit log holds.
fn audit_rows() -> Vec<Value> {
    let home = std::env::var("AGENTSTACK_HOME").unwrap();
    let path = Path::new(&home).join("audit").join("calls.jsonl");
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .collect()
}

/// The seatbelt trust-family denials, which is what a `tool: "trust"` row is.
fn trust_denials() -> Vec<Value> {
    audit_rows()
        .into_iter()
        .filter(|r| r["tool"].as_str() == Some("trust"))
        .collect()
}

/// Every event line of a run, as parsed JSON. (Same helper shape as
/// `trust_at_dispatch.rs` and `pin_refusal_is_recorded.rs`.)
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

/// One live `agentstack mcp` process, driven request by request. Copied from
/// `trust_at_dispatch.rs` rather than shared — test binaries cannot import each
/// other — with one addition: the auto-project launch, which is the mode a
/// never-trusted project is actually gated in.
struct McpSession {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    stdout: std::io::BufReader<std::process::ChildStdout>,
}

impl McpSession {
    /// Eager mode: one project, fixed at launch, trust anchored at build time.
    fn open_eager(proj: &Path, run: Option<&str>) -> McpSession {
        let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_agentstack"));
        cmd.args(["mcp", "--manifest-dir"]).arg(proj);
        McpSession::spawn(cmd, proj, run)
    }

    /// Auto-project mode: the project is discovered per connection and
    /// trust-gated by `AutoProject`. `$AGENTSTACK_MANIFEST_DIR` is the last rung
    /// of that discovery ladder and the only one a test can set deterministically
    /// (client roots need a roots-capable client; the cwd walk needs a chdir).
    fn open_auto(proj: &Path, run: Option<&str>) -> McpSession {
        let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_agentstack"));
        cmd.args(["mcp", "--auto-project"])
            .env("AGENTSTACK_MANIFEST_DIR", proj);
        McpSession::spawn(cmd, proj, run)
    }

    fn spawn(mut cmd: std::process::Command, proj: &Path, run: Option<&str>) -> McpSession {
        if let Some(run) = run {
            cmd.env("AGENTSTACK_RUN_ID", run);
        }
        let mut child = cmd
            .current_dir(proj)
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
        session.request(1, "initialize", json!({}));
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

/// The text of a `tools/call` result, whatever its outcome.
fn call_text(response: &Value) -> &str {
    response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
}

/// The acceptance sentence, asserted as a shape: what was refused, and the one
/// command that answers it.
fn assert_names_the_refusal_and_the_fix(text: &str, refused: &str) {
    assert!(text.contains("blocked:"), "not a seatbelt sentence: {text}");
    assert!(
        text.contains(refused),
        "the refusal must name WHAT was refused ({refused}): {text}"
    );
    assert!(text.contains("nothing ran"), "{text}");
    assert!(
        text.contains("agentstack trust"),
        "a refusal must name the ONE command that fixes it: {text}"
    );
}

/// A refused lease says what it refused and how to answer it — and leaves
/// exactly one row of evidence, in both destinations.
#[test]
fn a_refused_lease_names_the_refusal_and_the_fix_and_is_recorded() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let run = "run-yes-0001";
    let proj = project(tmp.path());

    let mut mcp = McpSession::open_auto(&proj, Some(run));
    let refused = mcp.call(2, "agentstack_lease_open", json!({ "profile": "backend" }));
    assert_eq!(
        refused["result"]["isError"],
        json!(true),
        "an untrusted project must not open a lease: {refused}"
    );
    assert_names_the_refusal_and_the_fix(call_text(&refused), "backend");
    mcp.close();

    // ONE row, not two: the same discipline the dispatch refusal keeps. A
    // second row would double-count a single refusal in every reader.
    let rows = trust_denials();
    assert_eq!(
        rows.len(),
        1,
        "expected exactly one trust denial: {rows:#?}"
    );
    assert_eq!(rows[0]["server"].as_str(), Some("backend"));
    assert_eq!(rows[0]["outcome"].as_str(), Some("denied"));
    assert_eq!(
        rows[0]["project"].as_str(),
        Some(proj.join(".agentstack").display().to_string().as_str()),
        "the row must name the project, or no status surface can find it"
    );

    // ...and the run-scoped mirror, with the control-plane verb in the `tool`
    // slot: nothing was dispatched, so there is no upstream tool to name.
    let events: Vec<Value> = run_events(run)
        .into_iter()
        .filter(|e| e["event"].as_str() == Some("trust_refused"))
        .collect();
    assert_eq!(events.len(), 1, "expected one mirror event: {events:#?}");
    assert_eq!(events[0]["tool"].as_str(), Some("agentstack_lease_open"));
    assert_eq!(events[0]["server"].as_str(), Some("backend"));
    assert_eq!(events[0]["state"].as_str(), Some("untrusted"));
    cleanup();
}

/// The same shape one door over: a refused load names the skill and the fix.
#[test]
fn a_refused_load_names_the_refusal_and_the_fix_and_is_recorded() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let run = "run-yes-0002";
    let proj = project(tmp.path());

    let mut mcp = McpSession::open_auto(&proj, Some(run));
    let refused = mcp.call(
        2,
        "agentstack_load",
        json!({ "name": "helper", "reason": "review the backend" }),
    );
    assert_eq!(
        refused["result"]["isError"],
        json!(true),
        "an untrusted project must not load a skill body: {refused}"
    );
    let text = call_text(&refused);
    assert_names_the_refusal_and_the_fix(text, "helper");
    // Inert means inert: the refusal may name the skill, never serve it.
    assert!(
        !text.contains("zzskillbody"),
        "the refused skill's content leaked into its refusal: {text}"
    );
    mcp.close();

    let rows = trust_denials();
    assert_eq!(
        rows.len(),
        1,
        "expected exactly one trust denial: {rows:#?}"
    );
    assert_eq!(rows[0]["server"].as_str(), Some("helper"));
    assert_eq!(rows[0]["outcome"].as_str(), Some("denied"));

    let events: Vec<Value> = run_events(run)
        .into_iter()
        .filter(|e| e["event"].as_str() == Some("trust_refused"))
        .collect();
    assert_eq!(events.len(), 1, "expected one mirror event: {events:#?}");
    assert_eq!(events[0]["tool"].as_str(), Some("agentstack_load"));
    assert_eq!(events[0]["server"].as_str(), Some("helper"));
    cleanup();
}

/// The disclosure clause, witnessed on a project that was trusted and then
/// drifted — the state the yes-card exists for.
///
/// Two halves, and the second is the one that keeps the contract honest. The
/// refusal leads to the card by NAMING the command; it does not carry a card,
/// or a smaller version of one, or anything the agent could answer. The card
/// itself is undiminished, because it is literally the same walk
/// `agentstack trust --preview` renders — there is exactly one.
#[test]
fn the_refusal_discloses_no_less_than_the_trust_card() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project(tmp.path());
    agentstack::trust::trust_unreviewed(&proj).expect("fixture must be trusted");

    // Eager mode anchors the connection to the digest it was trusted at...
    let mut mcp = McpSession::open_eager(&proj, None);
    // ...and then the bytes move, by nothing but a text editor.
    let manifest = proj.join(".agentstack/agentstack.toml");
    let mut body = std::fs::read_to_string(&manifest).unwrap();
    body.push_str("\n# appended by a text editor, not by agentstack\n");
    std::fs::write(&manifest, body).unwrap();

    let refused = mcp.call(
        2,
        "agentstack_load",
        json!({ "name": "helper", "reason": "review the backend" }),
    );
    assert_eq!(refused["result"]["isError"], json!(true), "{refused}");
    assert_names_the_refusal_and_the_fix(call_text(&refused), "helper");
    mcp.close();

    // The refusal carries NO card and NO way to answer one. The whole tool
    // result is checked, not just the sentence: a payload smuggled beside the
    // text would be exactly the second renderer this contract forbids.
    let whole = serde_json::to_string(&refused)
        .unwrap()
        .to_ascii_lowercase();
    for card_field in [
        "surface_digest",
        "\"items\"",
        "\"review\"",
        "may_read",
        "contacts",
        "prior_pin",
    ] {
        assert!(
            !whole.contains(card_field),
            "card payload ({card_field}) rode along with the refusal: {whole}"
        );
    }
    for answer in ["\"yes\"", "approve", "consent", "--consented"] {
        assert!(
            !whole.contains(answer),
            "the refusal offers an answer ({answer}) only a human may give: {whole}"
        );
    }

    // And the command the refusal names leads to the undiminished card: the
    // real `trust --preview` payload, per item, with every disclosure field the
    // panel contract promises.
    let card = agentstack::commands::trust::preview_value(&proj).expect("the card must render");
    let features: Vec<&str> = card["features"]
        .as_array()
        .expect("an enveloped payload")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(features.contains(&"trust-review-card-v1"), "{features:?}");
    assert!(features.contains(&"trust-card-diff-v1"), "{features:?}");
    assert_eq!(card["state"], "drifted", "the card must say what moved");
    let items = card["review"]["items"]
        .as_array()
        .expect("the review walk's items");
    assert!(
        !items.is_empty(),
        "a project with a server and a skill must disclose items: {card}"
    );
    for item in items {
        for field in [
            "kind", "name", "change", "identity", "runs", "contacts", "may_read",
        ] {
            assert!(
                item.get(field).is_some(),
                "the card item dropped '{field}' — the refusal would then lead \
                 somewhere that discloses less than the card it points at: {item}"
            );
        }
    }
    cleanup();
}

/// W1's third clause on the lease path: the agent can relay a refusal and can
/// never answer it. `trust_at_dispatch.rs` holds this at the dispatch door;
/// this holds it after the two doors W1 touched, because a refusal is exactly
/// the moment an agent has a reason to go looking for a way to say yes.
#[test]
fn no_mcp_invocable_consent_path_exists_on_the_lease_path() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project(tmp.path());
    // A second, trusted project so the store is a real file with real content —
    // a byte comparison against a missing file proves much less.
    let other = tmp.path().join("other");
    std::fs::create_dir_all(other.join(".agentstack")).unwrap();
    std::fs::write(other.join(".agentstack/agentstack.toml"), "version = 1\n").unwrap();
    agentstack::trust::trust_unreviewed(&other).unwrap();
    let store = agentstack::trust::store_path();
    let before = std::fs::read(&store).unwrap();

    let mut mcp = McpSession::open_auto(&proj, None);
    let lease = mcp.call(2, "agentstack_lease_open", json!({ "profile": "backend" }));
    assert_eq!(lease["result"]["isError"], json!(true), "{lease}");
    let load = mcp.call(
        3,
        "agentstack_load",
        json!({ "name": "helper", "reason": "x" }),
    );
    assert_eq!(load["result"]["isError"], json!(true), "{load}");

    // Nothing advertised can answer the refusal the agent just relayed.
    let listed = mcp.request(4, "tools/list", json!({}));
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
        for banned in ["trust", "consent", "approve", "grant", "yes"] {
            assert!(
                !name.contains(banned),
                "a tool named {name:?} offers the one answer only a human may give"
            );
        }
    }

    // ...and no plausible undeclared name quietly works.
    for (id, forged) in [
        (5, "agentstack_trust"),
        (6, "agentstack_consent"),
        (7, "agentstack_yes"),
        (8, "agentstack_approve"),
    ] {
        let resp = mcp.call(id, forged, json!({ "dir": ".", "yes": true }));
        assert_eq!(
            resp["result"]["isError"],
            json!(true),
            "an undeclared consent tool answered: {resp}"
        );
    }
    mcp.close();

    assert_eq!(
        before,
        std::fs::read(&store).unwrap(),
        "the trust store must be byte-identical — nothing over MCP may write consent"
    );
    cleanup();
}

/// The other end of the loop: a refusal is not only recorded, it is *surfaced*.
/// A user who never saw the agent's terminal learns from `status` that
/// something is waiting on them, and is given the same one command.
#[test]
fn status_names_needs_your_yes_after_a_refusal() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project(tmp.path());
    agentstack::trust::trust_unreviewed(&proj).expect("fixture must be trusted");

    // A clean, trusted project says nothing about a pending yes — the key is
    // absent, not empty, so a consumer's "is anything waiting?" is one check.
    let clean = status_json(&proj);
    assert_eq!(clean["project"]["trust"], "trusted", "{clean}");
    assert!(
        clean["project"].get("needs_your_yes").is_none(),
        "a trusted project with nothing refused must carry no pending yes: {clean}"
    );

    // Now break trust the way the world does, and let a real refusal happen.
    let mut mcp = McpSession::open_eager(&proj, None);
    let manifest = proj.join(".agentstack/agentstack.toml");
    let mut body = std::fs::read_to_string(&manifest).unwrap();
    body.push_str("\n# appended by a text editor, not by agentstack\n");
    std::fs::write(&manifest, body).unwrap();
    let refused = mcp.call(
        2,
        "agentstack_load",
        json!({ "name": "helper", "reason": "review the backend" }),
    );
    assert_eq!(refused["result"]["isError"], json!(true), "{refused}");
    mcp.close();

    let out = status_json(&proj);
    assert_eq!(out["project"]["trust"], "drifted", "{out}");
    let pending = &out["project"]["needs_your_yes"];
    assert!(
        pending.get("refused").and_then(Value::as_u64).unwrap_or(0) >= 1,
        "status must count what was refused here: {out}"
    );
    assert!(
        pending["last_refused_ts"].as_u64().unwrap_or(0) > 0,
        "a refusal without a time is not evidence: {out}"
    );
    assert!(
        pending["fix"]
            .as_str()
            .unwrap_or_default()
            .starts_with("agentstack trust"),
        "the pending yes must name the one command: {out}"
    );
    // The headline must be the first step toward the pending yes that can
    // ACTUALLY RUN.
    //
    // This fixture never pins: it writes a manifest with an inline skill and
    // no lockfile. `agentstack trust` refuses outright on an unpinned loadable
    // surface ("its loadable surface isn't fully pinned"), so asserting that
    // the headline is `agentstack trust …` here asserted a dead end — a driver
    // executing the machine field gets a non-zero exit, an unchanged state,
    // and the same string on the next poll. The repair is two rungs, in order:
    // pin, then review. `needs_your_yes.fix` above still names the review, so
    // the pending yes is not hidden — only the headline is honest about which
    // rung comes first.
    //
    // Both halves are asserted unconditionally, and that is the point: reading
    // the expected headline out of the same payload being judged would let a
    // regression normalise itself (empty `surface_unpinned` + an `agentstack
    // trust` headline is exactly the old dead end, and would have passed).
    // This fixture writes `[skills.helper] path = "skills/helper"` — an INLINE
    // skill — with no lockfile, so an unpinned surface is a fact of the
    // fixture, not a variable.
    let headline = out["next_action"]["command"].as_str().unwrap_or_default();
    let unpinned = out["project"]["surface_unpinned"]
        .as_array()
        .expect("status must carry the unpinned surface as an array");
    assert!(
        !unpinned.is_empty(),
        "this fixture declares an inline skill with no lockfile, so the unpinned \
         surface must be reported: {out}"
    );
    let want = "agentstack lock --write";
    assert!(
        headline.starts_with(want),
        "next_action must name the first runnable rung toward the pending yes \
         (expected it to start with `{want}`): {out}"
    );
    // Still no card here: the fix names the command that renders it.
    assert!(
        pending.get("items").is_none() && pending.get("review").is_none(),
        "status must not carry card payload: {pending}"
    );
    // The advertised name a UI gates the field on.
    let features: Vec<&str> = out["features"]
        .as_array()
        .expect("an enveloped payload")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(features.contains(&"needs-your-yes-v1"), "{features:?}");
    cleanup();
}

/// A minimal MCP stdio server in POSIX sh: answers `initialize`, `tools/list`
/// (one `echo` tool), and `tools/call`. Copied from `trust_at_dispatch.rs`
/// rather than shared — test binaries cannot import each other, and a helper
/// crate for one shell script would cost more than it saves.
#[cfg(unix)]
const UPSTREAM_FIXTURE: &str = r#"#!/bin/sh
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
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"echo:%s"}]}}\n' "$id" "$msg"
      ;;
  esac
done
"#;

/// A trusted project whose one server is a REAL stdio child, so the connection
/// the W2 refusal interrupts is a live one. Same sandboxed-home shape as
/// [`project`], and canonicalized for the same reason.
#[cfg(unix)]
fn project_with_upstream(tmp: &Path) -> PathBuf {
    let home = tmp.join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    let proj = tmp.join("proj");
    std::fs::create_dir_all(proj.join(".agentstack")).unwrap();
    let script = proj.join("fix.sh");
    std::fs::write(&script, UPSTREAM_FIXTURE).unwrap();
    std::fs::write(
        proj.join(".agentstack/agentstack.toml"),
        format!(
            "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
             [servers.fix]\ntype = \"stdio\"\ncommand = \"/bin/sh\"\nargs = [\"{}\"]\n",
            script.display()
        ),
    )
    .unwrap();
    let proj = proj.canonicalize().unwrap();
    agentstack::trust::trust_unreviewed(&proj).expect("fixture must be trusted");
    proj
}

/// The seam between W1 and W2: a refusal at **dispatch** is counted by the same
/// "needs your yes" the two lease-path refusals feed.
///
/// The three surfaces each derive the recorded/queried project string
/// themselves — `gateway.rs` from its `commands::load(dir).dir`, the lease and
/// load refusals from `resolve_manifest_dir(project_root_of(dir))`, and
/// `overview::needs_your_yes` from its own `commands::load(...).dir`. Reading
/// the code says they agree; nothing *holds* them to it, and the failure mode
/// is silent in exactly the wrong direction: the loudest refusal — the one that
/// stopped a live connection — would simply never be counted, and `status`
/// would under-report when it matters most. So the agreement is witnessed
/// end-to-end, on a live child, rather than asserted from a reading.
#[cfg(unix)]
#[test]
fn a_dispatch_refusal_is_counted_by_needs_your_yes() {
    use agentstack::gateway::Gateway;

    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project_with_upstream(tmp.path());
    // The gateway inherits run attribution at construction, so it is set first.
    std::env::set_var("AGENTSTACK_RUN_ID", "run-yes-0003");

    // A live connection: a real child, spawned and answering.
    let gw = Gateway::from_manifest(Some(&proj));
    assert!(
        gw.trust_anchor().is_some(),
        "a trusted project must anchor its gateway, or there is no dispatch \
         gate to refuse at"
    );
    let first = gw
        .try_call("fix__echo", &json!({ "msg": "hello" }))
        .expect("the gateway owns this server")
        .expect("the first call must SUCCEED — the connection has to be live");
    assert!(
        first["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("echo:hello"),
        "the upstream really answered: {first}"
    );

    // Trust breaks the way the world breaks it: bytes appended by a text
    // editor, with nothing in-process notified.
    let manifest = proj.join(".agentstack/agentstack.toml");
    let mut body = std::fs::read_to_string(&manifest).unwrap();
    body.push_str("\n# appended by a text editor, not by agentstack\n");
    std::fs::write(&manifest, body).unwrap();

    let err = gw
        .try_call("fix__echo", &json!({ "msg": "again" }))
        .expect("the gateway still owns the server — it refuses, it does not disown")
        .expect_err("the next call on the live connection must be refused");
    assert!(
        format!("{err}").starts_with("blocked:"),
        "not a seatbelt sentence: {err}"
    );

    // The recording side of the seam, named exactly: the dispatch refusal files
    // itself under the manifest dir, which is the string the reader wants.
    let rows = trust_denials();
    assert_eq!(rows.len(), 1, "expected one dispatch denial: {rows:#?}");
    assert_eq!(
        rows[0]["project"].as_str(),
        Some(proj.join(".agentstack").display().to_string().as_str()),
        "the dispatch refusal must file under the same project string the \
         status reader matches on"
    );

    // ...and the reading side, through the real `status` read path.
    let out = status_json(&proj);
    assert_eq!(out["project"]["trust"], "drifted", "{out}");
    let pending = &out["project"]["needs_your_yes"];
    assert!(
        pending.get("refused").and_then(Value::as_u64).unwrap_or(0) >= 1,
        "a refusal that stopped a LIVE connection must be counted: {out}"
    );
    assert!(
        pending["fix"]
            .as_str()
            .unwrap_or_default()
            .starts_with("agentstack trust"),
        "the pending yes must name the one command: {out}"
    );
    cleanup();
}

/// `agentstack status --json` for one project, as a real command run.
fn status_json(proj: &Path) -> Value {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(["status", "--json", "--manifest-dir"])
        .arg(proj)
        .current_dir(proj)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "status failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("status --json was not JSON ({e}): {:?}", out.stdout))
}
