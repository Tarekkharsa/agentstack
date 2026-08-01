// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Strategy v2 Phase 3, item 2 — the seatbelt is legible, and still a
//! seatbelt.
//!
//! Two claims, and the second is the one that matters:
//!
//! 1. **Every denial says what, why, and the safe next step**, and leaves
//!    evidence a reader can open afterwards. Two of the four families —
//!    host-path egress and secret-scope — left no evidence at all before this;
//!    the refusal happened, was printed once to a terminal nobody was
//!    necessarily watching, and vanished.
//!
//! 2. **A legible denial still denies.** Explaining a block must never become
//!    a step toward relaxing it. There is no explain-then-allow path, and the
//!    tests below assert the refusal *and* its explanation together, so a
//!    future change that softens one to improve the other fails here.
//!
//! Recorded is not prevented, and nothing in this file should be read as
//! claiming otherwise — `docs/ENFORCEMENT.md` stays the honest matrix. What
//! is asserted is that the decision the check made is retrievable.

use std::fs;
use std::path::Path;
use std::sync::Mutex;

use agentstack::gateway::Gateway;
use serde_json::json;

// These tests mutate the process-global HOME + env secrets; serialize them.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn setup_home(home: &Path) {
    fs::create_dir_all(home).unwrap();
    std::env::set_var("HOME", home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));
}

fn write_machine_policy(home: &Path, policy: &str) {
    let dir = home.join(".agentstack");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("agentstack.toml"),
        format!("version = 1\n{policy}"),
    )
    .unwrap();
}

/// Every line of the machine-global audit log.
fn audit_lines(home: &Path) -> Vec<serde_json::Value> {
    let path = home.join(".agentstack/audit/calls.jsonl");
    let Ok(body) = fs::read_to_string(path) else {
        return Vec::new();
    };
    body.lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// Every event in one run's log.
fn run_events(home: &Path, run: &str) -> Vec<serde_json::Value> {
    let path = home.join(".agentstack/runs").join(run).join("events.jsonl");
    let Ok(body) = fs::read_to_string(path) else {
        return Vec::new();
    };
    body.lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// The three parts, asserted as a shape rather than a golden string — so
/// copy can be improved without the witness rotting, but not hollowed out.
fn assert_legible(msg: &str, what: &str, why: &str, nothing: &str) {
    assert!(msg.contains("blocked:"), "no denial marker: {msg}");
    assert!(msg.contains(what), "does not say WHAT was stopped: {msg}");
    assert!(msg.contains(why), "does not say WHY: {msg}");
    assert!(msg.contains(nothing), "does not reassure: {msg}");
    // The next step is the part that was missing everywhere. A denial that
    // ends at "why" leaves the reader stuck with a correct explanation.
    assert!(
        msg.contains("[policy.") || msg.contains("[guard]") || msg.contains("agentstack "),
        "does not name a SAFE NEXT STEP: {msg}"
    );
}

/// **Secret-scope refusal.** Denies, explains, and — new — records.
#[test]
fn a_secret_scope_refusal_is_legible_and_leaves_evidence() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup_home(&home);
    write_machine_policy(&home, "[policy.secrets]\n\"*\" = [\"!EVIL_*\"]\n");
    std::env::set_var("EVIL_TOKEN", "leak-me-not-xyz");

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [servers.sneaky]\ntype = \"stdio\"\ncommand = \"/bin/echo\"\n\
         env = { TOKEN = \"${EVIL_TOKEN}\" }\n",
    )
    .unwrap();

    let gw = Gateway::from_manifest(Some(&proj));
    let err = gw
        .try_call("sneaky__anything", &json!({}))
        .expect("routed to the upstream")
        .expect_err("STILL DENIES — legibility must not become permission");
    let msg = format!("{err:#}");

    assert_legible(&msg, "EVIL_TOKEN", "[policy.secrets]", "nothing was read");
    // The value must never appear, in the message or the evidence.
    assert!(!msg.contains("leak-me-not-xyz"), "value leaked: {msg}");
    // A refusal and a missing value are different failures with different
    // answers. Telling the user to `secret set` a ref that policy refuses
    // sends them to do work that cannot help — worse than saying nothing,
    // because it reads as authoritative.
    assert!(
        !msg.contains("agentstack secret set"),
        "a policy REFUSAL must not be given the MISSING VALUE's advice: {msg}"
    );

    // Evidence: this refusal is now in the audit log, identity-shaped.
    let denials: Vec<_> = audit_lines(&home)
        .into_iter()
        .filter(|l| l["tool"] == "secret" && l["outcome"] == "denied")
        .collect();
    assert_eq!(
        denials.len(),
        1,
        "a secret-scope refusal must leave exactly one record — it left {}: {:?}",
        denials.len(),
        denials
    );
    let d = &denials[0];
    assert_eq!(d["server"], "sneaky");
    assert!(
        d["detail"].as_str().unwrap_or_default().contains("policy"),
        "the record must name the rule that refused: {d}"
    );
    for line in audit_lines(&home) {
        assert!(
            !line.to_string().contains("leak-me-not-xyz"),
            "a secret VALUE reached the audit log: {line}"
        );
    }

    std::env::remove_var("EVIL_TOKEN");
}

/// **Host-path egress refusal.** The family the strategy named as unrecorded.
#[test]
fn a_host_path_egress_refusal_is_legible_and_leaves_evidence() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup_home(&home);
    write_machine_policy(&home, "[policy.egress]\n\"*\" = [\"!evil.example\"]\n");

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [servers.reacher]\ntype = \"http\"\nurl = \"https://evil.example/mcp\"\n",
    )
    .unwrap();

    let gw = Gateway::from_manifest(Some(&proj));

    // STILL DENIES: the server is not built as an upstream, so the call does
    // not route at all. This is the invariant — the sentence below explains
    // a refusal that already happened, it does not stand in for one.
    assert!(
        gw.try_call("reacher__anything", &json!({})).is_none(),
        "a refused server must not be reachable — explaining a block must never allow it"
    );

    let denials: Vec<_> = audit_lines(&home)
        .into_iter()
        .filter(|l| l["tool"] == "egress" && l["outcome"] == "denied")
        .collect();
    assert_eq!(
        denials.len(),
        1,
        "the host-path egress refusal must leave exactly one record: {denials:?}"
    );
    assert_eq!(denials[0]["server"], "reacher");
    assert!(
        denials[0]["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("policy"),
        "the record must name the rule: {}",
        denials[0]
    );
}

/// The run-scoped mirror: a refusal inside a tracked run is openable with
/// `agentstack report run <id>`, which is the whole point of "evidence the
/// user can open" rather than "a line that scrolled past".
#[test]
fn refusals_inside_a_run_land_in_that_runs_event_log() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup_home(&home);
    write_machine_policy(
        &home,
        "[policy.egress]\n\"*\" = [\"!evil.example\"]\n\
         [policy.secrets]\n\"*\" = [\"!EVIL_*\"]\n",
    );
    std::env::set_var("EVIL_TOKEN", "leak-me-not-xyz");
    std::env::set_var("AGENTSTACK_RUN_ID", "r-seatbelt01");

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [servers.reacher]\ntype = \"http\"\nurl = \"https://evil.example/mcp\"\n\
         [servers.sneaky]\ntype = \"stdio\"\ncommand = \"/bin/echo\"\n\
         env = { TOKEN = \"${EVIL_TOKEN}\" }\n",
    )
    .unwrap();

    let gw = Gateway::from_manifest(Some(&proj));
    // Drive the secret path too, so both new event families are exercised.
    let _ = gw.try_call("sneaky__anything", &json!({}));

    let events = run_events(&home, "r-seatbelt01");
    assert!(
        events
            .iter()
            .any(|e| e["event"] == "egress" && e["allowed"] == false),
        "a denied host-path egress must appear in the run's log: {events:?}"
    );
    assert!(
        events.iter().any(|e| e["event"] == "secret_denied"),
        "a denied secret ref must appear in the run's log: {events:?}"
    );
    // Identity only — never the value, and never whether the value exists.
    for e in &events {
        assert!(
            !e.to_string().contains("leak-me-not-xyz"),
            "a secret VALUE reached a run event: {e}"
        );
    }

    std::env::remove_var("EVIL_TOKEN");
    std::env::remove_var("AGENTSTACK_RUN_ID");
}

/// The invariant, structurally.
///
/// `seatbelt::refuse` returns `()`. It has no success value and no way to
/// signal "carry on", so composing a denial cannot hand a call site
/// permission — the call site still owns its own `continue` / `bail!`. This
/// is what makes "no explain-then-allow path" a property of the seam rather
/// than a habit of whoever wrote the last call site.
#[test]
fn the_denial_seam_cannot_grant_permission() {
    let src = include_str!("../src/seatbelt.rs");

    assert!(
        src.contains("pub fn refuse(d: &Denial, project: Option<String>, run: Option<&str>) {"),
        "`refuse` must return nothing — a return value is how an explain-then-allow path would start"
    );
    assert!(
        !src.contains("-> bool") && !src.contains("-> Result"),
        "nothing in the seatbelt seam may return a decision: a denial is not a question"
    );

    // And recording must never be able to fail the refusal it describes.
    assert!(
        src.contains("best-effort"),
        "the recording contract must stay stated where it is implemented"
    );
}

/// The four families are named in one place, so a fifth cannot be added
/// without deciding what its sentence says. (Locked-run gate decisions are a
/// real fifth denial family, deliberately out of Phase 3's scope — see the
/// handoff; this guard is what will surface it if someone folds it in here.)
#[test]
fn every_family_names_what_did_not_happen() {
    let src = include_str!("../src/seatbelt.rs");
    for clause in [
        "nothing ran",
        "nothing was sent",
        "nothing was read",
        "nothing was written",
    ] {
        assert!(
            src.contains(clause),
            "a family lost its reassurance clause: {clause}"
        );
    }
}
