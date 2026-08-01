// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! P3.7 — does a machine-readable status surface say only what the evidence
//! supports?
//!
//! Three symptoms, one question. Each gets a witness here, plus the two
//! properties that make the fix a *contract revision* rather than an edit:
//!
//! 1. `state` said `ready` over an untrusted, never-activated project. Zero
//!    findings was true; "ready" was not, because nothing the project declares
//!    was live. Fixed additively — `readiness` is the honest field and `state`
//!    keeps its `status-v1` meaning, so the panel rendering "Ready" today does
//!    not silently change meaning under its users.
//! 2. `snapshot` emitted a plural `nextActions` where `doctor` and `status`
//!    both settle on one. A list where a decision belongs.
//! 3. `doctor` printed a green `✓ <REF> resolved from env` for a ref that
//!    `[policy.secrets]` refuses for every server referencing it — the same
//!    vacuous-green shape P3.1 removed, one section over, and directly
//!    contradicted by the Error the Policy section already raises.
//!
//! The two contract properties are the load-bearing ones: **`status-v1` is
//! untouched** and **the new name is advertised**. Without them this is three
//! bug fixes; with them it is a surface a consumer can migrate onto.

use std::fs;
use std::sync::Mutex;

use agentstack::commands::doctor;

// doctor mutates the process-global HOME; serialize these tests.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Point HOME/AGENTSTACK_HOME at the sandbox and write `manifest` into a fresh
/// project. Nothing is trusted and nothing is locked — which is exactly the
/// state symptom 1 is about.
fn project(tmp: &std::path::Path, manifest: &str) -> std::path::PathBuf {
    let home = tmp.join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    let proj = tmp.join("proj");
    fs::create_dir_all(proj.join(".agentstack")).unwrap();
    fs::write(proj.join(".agentstack/agentstack.toml"), manifest).unwrap();
    proj
}

// ---------------------------------------------------------------- symptom 1

/// The bug, stated as a property: a project nobody has reviewed and nothing
/// has activated is not ready, and the surface must not say it is.
#[test]
fn an_untrusted_never_activated_project_is_not_reported_ready() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project(
        tmp.path(),
        "version = 1\n[servers.demo]\ntype = \"stdio\"\ncommand = \"echo\"\n",
    );

    let report = doctor::collect(Some(&proj)).unwrap();
    let readiness = report["readiness"]
        .as_str()
        .unwrap_or_else(|| panic!("readiness must be present on every report: {report}"));

    assert_ne!(
        readiness, "ready",
        "an untrusted / never-activated project must not be reported ready — \
         this is the whole of P3.7 symptom 1: {report}"
    );
    // And it must say WHICH, so the reader knows what stands between here and
    // live. "not ready" with no reason is the dead end P3.1 removed.
    assert!(
        matches!(
            readiness,
            "untrusted" | "drifted" | "never_activated" | "needs_attention"
        ),
        "readiness must name what is missing, got {readiness:?}: {report}"
    );
}

/// The other half of symptom 1, and the reason this is a *revision*: `state`
/// still answers its own, narrower `status-v1` question. If this test ever
/// fails, an existing consumer's "Ready" chip changed meaning without them
/// opting in — the exact failure the versioned approach exists to prevent.
#[test]
fn status_v1_state_semantics_are_unchanged() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project(tmp.path(), "version = 1\n");

    let report = doctor::collect(Some(&proj)).unwrap();

    assert_eq!(
        report["state"].as_str(),
        Some("ready"),
        "`state` must keep meaning 'no check found anything to repair' — it is \
         status-v1 and has external consumers: {report}"
    );
    assert!(
        report["next_action"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "the P3.1 seam must survive this change untouched: {report}"
    );
}

/// `readiness` is a decision, not free text: every value a consumer may see is
/// one of the documented set. A typo'd or invented state is worse than a
/// missing one, because a UI will render it verbatim.
#[test]
fn readiness_is_drawn_from_the_documented_set() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    const KNOWN: &[&str] = &[
        "ready",
        "needs_attention",
        "untrusted",
        "drifted",
        "never_activated",
        "empty",
        "unknown",
        "needs_setup",
    ];

    for manifest in [
        "version = 1\n",
        "version = 1\n[servers.demo]\ntype = \"stdio\"\ncommand = \"echo\"\n",
        "version = 1\n\n[targets]\nclaude = true\n",
        "version = 1\n[servers.demo]\ntype = \"stdio\"\ncommand = \"echo\"\n\
         env = { TOKEN = \"${NOWHERE_TOKEN}\" }\n",
    ] {
        let tmp = assert_fs::TempDir::new().unwrap();
        let proj = project(tmp.path(), manifest);
        let report = doctor::collect(Some(&proj)).unwrap();
        let readiness = report["readiness"].as_str().unwrap_or_default();
        assert!(
            KNOWN.contains(&readiness),
            "undocumented readiness {readiness:?} for manifest {manifest:?}"
        );
    }

    // The pre-manifest payload is hand-written rather than built from
    // `Report::to_json`, so it is the one place the key can silently go
    // missing. Assert it is there, in the source, with a documented value.
    let src = include_str!("../src/commands/doctor.rs");
    assert!(
        src.contains(r#""readiness": "needs_setup""#),
        "the no-manifest JSON must carry readiness too — a key that vanishes on \
         the least-informed path is worse than one that never existed"
    );
}

// ---------------------------------------------------------------- symptom 2

/// `snapshot` now answers with one action as well as the list. The list stays
/// (its consumers are external and unversioned-in-this-repo); the singular is
/// what a panel should render.
#[test]
fn the_snapshot_offers_one_next_action_beside_the_list() {
    let src = include_str!("../src/snapshot.rs");
    assert!(
        src.contains(r#""nextActions": next_actions"#),
        "the plural array must survive — removing it is a breaking change to a \
         contract this item is explicitly not allowed to mutate"
    );
    assert!(
        src.contains(r#""nextAction": one_next_action(&next_actions)"#),
        "the singular decision must be emitted beside it"
    );
}

/// The singular is the *most severe* action, not merely the first — otherwise
/// it is a different arbitrary pick rather than a decision.
#[test]
fn the_one_next_action_prefers_severity_over_position() {
    let src = include_str!("../src/snapshot.rs");
    let body = src
        .split("fn one_next_action")
        .nth(1)
        .expect("one_next_action must exist");
    let head = &body[..body.len().min(600)];
    let error_at = head.find(r#"by_level("error")"#);
    let warn_at = head.find(r#"by_level("warn")"#);
    assert!(
        error_at.is_some() && warn_at.is_some() && error_at < warn_at,
        "errors must be preferred to warnings, and both to list position"
    );
}

// ---------------------------------------------------------------- symptom 3

/// A ref that resolves but that `[policy.secrets]` refuses for every server
/// referencing it must not be reported as a green pass. It resolves; nothing
/// can read it; saying "✓ resolved from env" tells the user the opposite of
/// what will happen at apply/gateway time.
#[test]
fn a_policy_refused_ref_is_not_a_green_resolved_line() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project(
        tmp.path(),
        "version = 1\n\
         [servers.demo]\n\
         type = \"stdio\"\n\
         command = \"echo\"\n\
         env = { TOKEN = \"${DEMO_TOKEN}\" }\n\
         \n\
         [policy.secrets]\n\
         demo = [\"SOMETHING_ELSE\"]\n",
    );
    // Make it genuinely resolvable, so the only reason not to print green is
    // the policy refusal — this is the exact state the bug reported as ✓.
    std::env::set_var("DEMO_TOKEN", "real-value");

    let report = doctor::collect(Some(&proj)).unwrap();
    std::env::remove_var("DEMO_TOKEN");

    let mut found = false;
    for section in report["sections"].as_array().into_iter().flatten() {
        if section["title"].as_str() != Some("Secrets") {
            continue;
        }
        for line in section["lines"].as_array().into_iter().flatten() {
            let msg = line["msg"].as_str().unwrap_or_default();
            if !msg.contains("DEMO_TOKEN") {
                continue;
            }
            found = true;
            assert_ne!(
                line["level"].as_str(),
                Some("ok"),
                "a ref refused by [policy.secrets] for every referencing server \
                 must not be a green pass: {msg}"
            );
            assert!(
                msg.contains("policy.secrets"),
                "the line must say WHY it is not a pass, in the reader's terms: {msg}"
            );
        }
    }
    assert!(
        found,
        "the Secrets section must still report the ref: {report}"
    );
}

/// The counterpart, so the fix is a discrimination rather than a blanket
/// downgrade: a ref the policy allows still reports the plain green line.
/// Without this, "never say green" would pass the test above and be useless.
#[test]
fn an_allowed_ref_still_reports_a_plain_green_line() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project(
        tmp.path(),
        "version = 1\n\
         [servers.demo]\n\
         type = \"stdio\"\n\
         command = \"echo\"\n\
         env = { TOKEN = \"${DEMO_TOKEN}\" }\n",
    );
    std::env::set_var("DEMO_TOKEN", "real-value");
    let report = doctor::collect(Some(&proj)).unwrap();
    std::env::remove_var("DEMO_TOKEN");

    let mut saw_green = false;
    for section in report["sections"].as_array().into_iter().flatten() {
        if section["title"].as_str() != Some("Secrets") {
            continue;
        }
        for line in section["lines"].as_array().into_iter().flatten() {
            let msg = line["msg"].as_str().unwrap_or_default();
            if msg.contains("DEMO_TOKEN") && line["level"].as_str() == Some("ok") {
                saw_green = true;
                assert!(
                    !msg.contains("policy.secrets"),
                    "an unrefused ref should get the plain line, not a caveat: {msg}"
                );
            }
        }
    }
    assert!(
        saw_green,
        "a resolvable ref no policy refuses must still read as a pass: {report}"
    );
}

// ------------------------------------------------------- the contract itself

/// The revision is only usable if a consumer can detect it. `status-honesty-v1`
/// must be advertised, and every name shipped before it must still be — a
/// feature list that loses a name breaks gating for every UI that reads it.
#[test]
fn the_new_contract_is_advertised_and_nothing_older_was_dropped() {
    let features = agentstack::ui_contract::FEATURES;
    assert!(
        features.contains(&"status-honesty-v1"),
        "the new contract must be advertised or no consumer can opt in"
    );
    assert!(
        features.contains(&"status-v1"),
        "status-v1 must remain advertised: it is untouched, and withdrawing it \
         would tell consumers a breaking change happened when none did"
    );
    assert_eq!(
        agentstack::ui_contract::SCHEMA_VERSION,
        1,
        "this change is additive — bumping the schema version would tell every \
         panel to disable itself over fields none of them read yet"
    );
}
