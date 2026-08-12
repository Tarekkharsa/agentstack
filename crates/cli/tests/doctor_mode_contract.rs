// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! `doctor-mode-v1`: `doctor --json` carries `mode` and `activation`.
//!
//! `agentstack status` has printed both for a while; `doctor --json` did not,
//! so every JSON consumer was blind to them. That matters because a panel that
//! shows an on-disk path is making a claim only `static` makes true — and
//! because "never activated" is the difference between a project that is set
//! up and one that has never written anything, which nothing in the JSON said.
//!
//! …plus `doctor-liveness-v1`, the runtime reading BESIDE it. A build once
//! answered `live` / `not_live` in `activation` itself, under this same
//! contract name and the same `schema_version`, so every consumer gating on
//! the two documented words read a locked project as never activated. That is
//! the drift these tests pin: `activation` is lockfile-derived and keeps its
//! words, `live_state` is the lease-derived one, and a consumer of either gets
//! an explicit name to negotiate.

use std::fs;
use std::sync::Mutex;

use agentstack::commands::doctor;

// doctor mutates the process-global HOME; serialize these tests.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn project(tmp: &std::path::Path) -> std::path::PathBuf {
    let home = tmp.join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    let proj = tmp.join("proj");
    fs::create_dir_all(proj.join(".agentstack")).unwrap();
    fs::write(
        proj.join(".agentstack/agentstack.toml"),
        "version = 1\n[servers.demo]\ntype = \"stdio\"\ncommand = \"echo\"\n",
    )
    .unwrap();
    proj
}

#[test]
fn a_never_activated_project_says_so() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project(tmp.path());

    let report = doctor::collect(Some(&proj)).unwrap();

    // No lockfile and nothing rendered: the default mode, and the fact that
    // nothing has ever been written for this project.
    assert_eq!(report["mode"], "static", "{report}");
    assert_eq!(report["activation"], "never_activated", "{report}");
    assert_eq!(report["locked"], false, "{report}");
    // The runtime reading is its own field and answers its own question.
    assert_eq!(report["live_state"], "not_live", "{report}");
}

#[test]
fn a_lockfile_flips_activation() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project(tmp.path());
    fs::write(proj.join(".agentstack/agentstack.lock"), "version = 1\n").unwrap();

    let report = doctor::collect(Some(&proj)).unwrap();
    assert_eq!(report["activation"], "locked", "{report}");
    assert_eq!(report["locked"], true, "{report}");
    // A pin is not a connection: nothing is serving this project, and the
    // liveness field is the only one allowed to say so. Collapsing the two —
    // which one build did, by answering `not_live` in `activation` — is how a
    // consumer of the documented words reads a locked project as never
    // activated.
    assert_eq!(report["live_state"], "not_live", "{report}");
}

#[test]
fn a_discovered_project_always_reports_both() {
    // `collect(None)` does NOT mean "no project" — it discovers one from the
    // working directory, so it always has a mode to report. The genuine
    // no-project case is answered earlier, by the `needs_setup` payload in
    // `run`, which carries both keys as explicit nulls.
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project(tmp.path());

    let report = doctor::collect(Some(&proj)).unwrap();
    assert!(report["mode"].is_string(), "{report}");
    assert!(report["activation"].is_string(), "{report}");
    assert!(report["live_state"].is_string(), "{report}");
}

/// The values are a closed set per field, and the two sets are DISJOINT. A
/// field whose words changed under an unchanged contract name is the drift
/// this file exists to catch, and only naming both sets can catch it.
#[test]
fn each_field_keeps_its_own_documented_words() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project(tmp.path());

    for locked in [false, true] {
        if locked {
            fs::write(proj.join(".agentstack/agentstack.lock"), "version = 1\n").unwrap();
        }
        let report = doctor::collect(Some(&proj)).unwrap();
        let activation = report["activation"].as_str().unwrap_or_default();
        assert!(
            matches!(activation, "locked" | "never_activated"),
            "`activation` is `doctor-mode-v1` and keeps those two words: {report}"
        );
        let live = report["live_state"].as_str().unwrap_or_default();
        assert!(
            matches!(live, "live" | "not_live"),
            "`live_state` is `doctor-liveness-v1`: {report}"
        );
    }
}

/// The pre-manifest payload is hand-written rather than built from the report,
/// so it is the one path where a promised key can silently go missing.
#[test]
fn the_no_project_payload_carries_both_keys_as_nulls() {
    let src = include_str!("../src/commands/doctor.rs");
    for key in [
        "\"activation\": serde_json::Value::Null",
        "\"live_state\": serde_json::Value::Null",
    ] {
        assert!(
            src.contains(key),
            "the needs_setup payload must carry {key} — a key that vanishes on \
             the least-informed path is worse than one that never existed"
        );
    }
}

#[test]
fn the_contract_name_is_advertised() {
    // Without the name, a UI cannot distinguish an older binary's absent keys
    // from this binary's legitimate nulls, so it cannot use either.
    assert!(
        agentstack::ui_contract::FEATURES.contains(&"doctor-mode-v1"),
        "doctor-mode-v1 missing from FEATURES"
    );
    // The runtime reading is additive, so it gets its own name rather than new
    // words inside `activation`; without the name a panel reading `live_state`
    // would be sniffing a field.
    assert!(
        agentstack::ui_contract::FEATURES.contains(&"doctor-liveness-v1"),
        "doctor-liveness-v1 missing from FEATURES"
    );
    // Additive means additive: the envelope's version must not move, because a
    // bump tells every panel to disable itself.
    assert_eq!(agentstack::ui_contract::SCHEMA_VERSION, 1);
}
