// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! P3.8 — the content-pinning refusal leaves evidence.
//!
//! `Gateway::build` drops a server whose declared bytes do not verify against
//! the lockfile pin. That was already correct and already fail-closed; what it
//! was not was *findable*. It said its piece in a bare `eprintln!` and recorded
//! nothing — so the one refusal that fires when delivered bytes are not the
//! bytes the user reviewed was the one they could not look up afterwards.
//!
//! Phase 3 recorded the other two silent refusals and deliberately left this
//! one alone, because it sits ON the verification path rather than beside it.
//! Hence the shape of this file: the first two tests are about the evidence,
//! and the last two are about the thing that must NOT have changed.

use std::fs;
use std::sync::Mutex;

use agentstack::resolve::FrozenServer;

// These tests set the process-global HOME; serialize them.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Point HOME/AGENTSTACK_HOME at a sandbox so run logs land where we can read
/// them, and return the project dir.
fn sandbox(tmp: &std::path::Path) -> std::path::PathBuf {
    let home = tmp.join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));
    let proj = tmp.join("proj");
    fs::create_dir_all(proj.join(".agentstack")).unwrap();
    fs::write(
        proj.join(".agentstack/agentstack.toml"),
        "version = 1\n[servers.web-search]\ntype = \"stdio\"\ncommand = \"echo\"\n",
    )
    .unwrap();
    // The project has to be TRUSTED to reach the pin loop at all: an untrusted
    // bundle is refused wholesale, earlier and harder, and never gets as far as
    // per-server verification. That ordering is itself worth knowing — this
    // refusal only fires for content the user already said yes to, whose bytes
    // then moved, which is exactly the case they need to be told about.
    let digest = agentstack::trust::digest_for(&proj).unwrap();
    agentstack::commands::trust::grant_with_answers(&proj, true, Some(&digest), false, None)
        .expect("fixture must be trusted");
    proj
}

/// Every event line of a run, as parsed JSON.
fn run_events(run: &str) -> Vec<serde_json::Value> {
    let home = std::env::var("AGENTSTACK_HOME").unwrap();
    let mut out = Vec::new();
    for entry in walk(std::path::Path::new(&home)) {
        if entry.file_name().is_some_and(|n| n == "events.jsonl")
            && entry.to_string_lossy().contains(run)
        {
            for line in fs::read_to_string(&entry).unwrap_or_default().lines() {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                    out.push(v);
                }
            }
        }
    }
    out
}

fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(dir) else {
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

/// The drifted-pin frozen set: exactly what `verify_library_pin` hands
/// `Gateway::build` when a library server's live definition no longer matches
/// its lock entry.
fn drifted(name: &str) -> Vec<FrozenServer> {
    vec![(
        name.to_string(),
        Err(
            "library definition drifted from agentstack.lock (locked abc123, current def456)"
                .to_string(),
        ),
    )]
}

/// The property: a pin refusal inside a tracked run lands in that run's event
/// log, identified, with the reason the user was shown.
#[test]
fn a_pin_refusal_leaves_evidence_in_the_runs_event_log() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = sandbox(tmp.path());
    let run = "run-pin-0001";

    let gw = agentstack::gateway::Gateway::from_frozen(
        Some(&proj),
        agentstack_policy::CompiledRuleset::default(),
        drifted("web-search"),
        run,
    );

    // The refusal still refuses: nothing of that server is exposed.
    assert!(
        gw.is_empty(),
        "a server that failed pin verification must not be served"
    );
    assert!(
        !gw.proxied_servers().iter().any(|(n, _)| n == "web-search"),
        "the refused server must not appear among the proxied ones"
    );

    let events = run_events(run);
    let pin: Vec<_> = events
        .iter()
        .filter(|e| e["event"].as_str() == Some("pin_rejected"))
        .collect();
    assert_eq!(
        pin.len(),
        1,
        "exactly one pin_rejected event expected, got {events:#?}"
    );
    assert_eq!(pin[0]["server"].as_str(), Some("web-search"));
    assert!(
        pin[0]["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("agentstack.lock"),
        "the recorded reason must be the one the user read: {:?}",
        pin[0]["reason"]
    );
}

/// The event is identity-shaped: the server NAME and why, and nothing from the
/// unreviewed definition. Copying the bytes the gate just refused into the
/// evidence log would put them in front of exactly the reader the gate
/// protects.
#[test]
fn the_event_carries_identity_not_the_unreviewed_bytes() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = sandbox(tmp.path());
    let run = "run-pin-0002";

    agentstack::gateway::Gateway::from_frozen(
        Some(&proj),
        agentstack_policy::CompiledRuleset::default(),
        drifted("web-search"),
        run,
    );

    let events = run_events(run);
    let pin = events
        .iter()
        .find(|e| e["event"].as_str() == Some("pin_rejected"))
        .expect("the event must exist");
    let keys: Vec<&str> = pin
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    let mut sorted = keys.clone();
    sorted.sort_unstable();
    assert_eq!(
        sorted,
        vec!["event", "reason", "server", "ts"],
        "the event's shape is its contract — a new key here is a disclosure \
         decision, not a detail"
    );
}

/// Invariant 7 at the one denial whose `why` is not machine-authored policy
/// text: the reason is built from lockfile and manifest fragments, which are
/// repository content. It must not be able to rewrite the terminal around the
/// sentence saying the server was refused, nor forge a second log line.
#[test]
fn a_hostile_server_name_and_reason_cannot_forge_output_or_evidence() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = sandbox(tmp.path());
    let run = "run-pin-0003";

    let hostile_name = "evil\u{1b}[2J\nallowed";
    let hostile_reason = "drift\n{\"event\":\"pin_rejected\",\"server\":\"forged\"}";

    agentstack::gateway::Gateway::from_frozen(
        Some(&proj),
        agentstack_policy::CompiledRuleset::default(),
        vec![(hostile_name.to_string(), Err(hostile_reason.to_string()))],
        run,
    );

    let events = run_events(run);
    let pin: Vec<_> = events
        .iter()
        .filter(|e| e["event"].as_str() == Some("pin_rejected"))
        .collect();
    // One event, not two: an embedded newline must not split into a second
    // forged row. (JSON serialization escapes it; this asserts that, rather
    // than trusting it.)
    assert_eq!(
        pin.len(),
        1,
        "a newline in the reason forged a row: {events:#?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| e["server"].as_str() == Some("forged")),
        "a forged event reached the log: {events:#?}"
    );
    for field in ["server", "reason"] {
        let v = pin[0][field].as_str().unwrap_or_default();
        assert!(
            !v.contains('\u{1b}') && !v.contains('\n'),
            "{field} kept control characters: {v:?}"
        );
    }
}

/// The invariant the whole seam rests on, asserted at the source: making a
/// denial legible must not be a way to acquire permission. `refuse` returns
/// `()`, so the `continue` at the gateway's skip site is still the caller's to
/// perform — and this diff touched the verification path, which is exactly
/// where that has to stay true.
#[test]
fn making_the_refusal_legible_did_not_make_it_optional() {
    let src = include_str!("../src/gateway.rs");
    let arm = src
        .split("Err(reason) => {")
        .nth(1)
        .expect("the skip arm must exist");
    // Just the arm: up to its closing `};`, not an arbitrary window that would
    // sweep in unrelated code and make this assert about the wrong text.
    let arm = arm
        .split("\n                };")
        .next()
        .expect("a closed arm");
    assert!(
        arm.contains("continue;"),
        "the fail-closed drop must survive: {arm}"
    );
    // No early exit that could skip the `continue`, and nothing that turns the
    // refusal back into a served server.
    assert!(
        !arm.contains("return ") && !arm.contains("Ok(rs)"),
        "the skip arm must not have grown a path that serves the server anyway: {arm}"
    );
    let seat = include_str!("../src/seatbelt.rs");
    assert!(
        seat.contains("pub fn refuse(d: &Denial, project: Option<String>, run: Option<&str>) {"),
        "refuse must still return () — it is the structural reason a denial \
         cannot hand out permission"
    );
}
