// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Strategy v2 Phase 3, item 1 — "status as one next action", and the honest
//! reading of green.
//!
//! Two properties, both of which used to fail:
//!
//! 1. **Every report ends with exactly one recommended command.** Before this,
//!    `next_action` was the first *repair* — so a healthy project, and a
//!    project whose only finding had a prose remedy, both ended with nothing.
//!    "0 errors, 0 warnings." and then silence reads as either "you're done"
//!    or "you forgot a step", and the reader cannot tell which.
//!
//! 2. **Green means verified.** The reproducibility check reported a green
//!    pass on a project that declares no toolset — it had examined zero items.
//!    That is the false-ready shape v0.17.1 removed from `status`, still alive
//!    one surface over. A check that read nothing now says so, in words.
//!
//! The fixtures below are deliberately the *shapes doctor can be in*, not one
//! happy path: empty manifest, no toolset, a declared server, a toolset that
//! pulls no library skill, and an unresolvable secret reference — plus the
//! uninitialized directory, which `collect` cannot reach and which is checked
//! separately. Property 1 is asserted over all of them, so a future check that
//! returns a fixless finding cannot quietly reintroduce the dead end.

use std::fs;
use std::sync::Mutex;

use agentstack::commands::doctor;

// doctor mutates the process-global HOME; serialize these tests.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// The one exit state `collect` cannot reach: an uninitialized directory. It
/// is answered before any check runs, and it too ends with exactly one
/// command — so the property holds across the whole surface, not just the
/// part this file can drive.
#[test]
fn the_uninitialized_state_is_answered_before_checks_run() {
    let src = include_str!("../src/commands/doctor.rs");
    assert!(
        src.contains(r#""state": "needs_setup""#) && src.contains(r#""agentstack init""#),
        "the no-manifest JSON state must still name init as its one action"
    );
}

/// Point HOME/AGENTSTACK_HOME at the sandbox and write `manifest` into a fresh
/// project.
fn project(tmp: &std::path::Path, manifest: Option<&str>) -> std::path::PathBuf {
    let home = tmp.join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    let proj = tmp.join("proj");
    fs::create_dir_all(proj.join(".agentstack")).unwrap();
    if let Some(body) = manifest {
        fs::write(proj.join(".agentstack/agentstack.toml"), body).unwrap();
    }
    proj
}

/// Every project shape these fixtures can produce, as (name, manifest body).
fn shapes() -> Vec<(&'static str, Option<&'static str>)> {
    // The no-manifest case is deliberately absent: `collect` is the
    // post-load seam and returns a load *error* there. That state is answered
    // earlier, by `run`, as `state: needs_setup` + `next_action: agentstack
    // init` — see `the_uninitialized_state_is_answered_before_checks_run`.
    vec![
        ("empty manifest", Some("version = 1\n")),
        (
            "no toolset, one server",
            Some("version = 1\n[servers.demo]\ntype = \"stdio\"\ncommand = \"echo\"\n"),
        ),
        (
            "no toolset, targets only",
            Some("version = 1\n\n[targets]\nclaude = true\n"),
        ),
        (
            "a toolset with no library skill",
            Some(
                "version = 1\n[servers.demo]\ntype = \"stdio\"\ncommand = \"echo\"\n\
                 \n[profiles.work]\nservers = [\"demo\"]\n",
            ),
        ),
        (
            "a secret reference nothing resolves",
            Some(
                "version = 1\n[servers.demo]\ntype = \"stdio\"\ncommand = \"echo\"\n\
                 env = { TOKEN = \"${NOWHERE_TOKEN}\" }\n",
            ),
        ),
    ]
}

/// Property 1, over every shape: exactly one next action, and it is a command
/// the reader can actually run.
#[test]
fn every_report_ends_with_exactly_one_next_action() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    for (name, manifest) in shapes() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let proj = project(tmp.path(), manifest);

        let report = doctor::collect(Some(&proj)).unwrap();
        let next = report["next_action"].as_str();

        let next = next.unwrap_or_else(|| {
            panic!("[{name}] next_action must never be null — a report with no path out is the bug this test exists for: {report}")
        });
        assert!(
            !next.trim().is_empty(),
            "[{name}] next_action must not be blank: {report}"
        );

        // "Exactly one" is a claim about shape, not just count: a newline or a
        // "then" would be a list wearing a string's clothes.
        assert!(
            !next.contains('\n'),
            "[{name}] next_action must be ONE command, got a multi-line value: {next:?}"
        );

        // It must be something to *do*. Either an agentstack command, or a
        // prose remedy carried over from a finding that has no command — but
        // never an empty gesture.
        assert!(
            next.len() > 3,
            "[{name}] next_action is not an actionable step: {next:?}"
        );
    }
}

/// Property 1 again, at the other end of the range: a report with nothing
/// wrong still names a step. This is the case the old code answered with
/// `null`, and the reason `state` and `next_action` are now independent —
/// `state` says whether anything is broken, `next_action` says what to do.
#[test]
fn a_healthy_report_still_names_a_step() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project(tmp.path(), Some("version = 1\n"));

    let report = doctor::collect(Some(&proj)).unwrap();
    let next = report["next_action"].as_str().unwrap_or_default();

    assert!(
        !next.is_empty(),
        "a clean report must still end with one action: {report}"
    );
    assert!(
        next.starts_with("agentstack "),
        "with nothing to repair the step should be a command we own, got {next:?}"
    );
}

/// Property 2: the no-toolset fixture's report *names its non-coverage*.
///
/// The old line was `✓ no library-backed toolset skills to verify` — a green
/// tick over zero examined items. The assertion is deliberately two-sided:
/// the honest words must be present AND the line must not be levelled `ok`,
/// because rewording a green line leaves the lie in the marker.
#[test]
fn reproducibility_over_nothing_is_not_reported_as_a_pass() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    // A project with targets and no toolset at all: the exact shape that
    // produced the false green.
    let proj = project(
        tmp.path(),
        Some("version = 1\n\n[targets]\nclaude = true\n"),
    );

    let report = doctor::collect(Some(&proj)).unwrap();

    let mut found = None;
    for section in report["sections"].as_array().unwrap() {
        for line in section["lines"].as_array().unwrap() {
            let msg = line["msg"].as_str().unwrap_or_default();
            if msg.contains("reproducibility") {
                found = Some((
                    line["level"].as_str().unwrap_or_default().to_string(),
                    msg.to_string(),
                ));
            }
        }
    }

    let (level, msg) = found.unwrap_or_else(|| {
        panic!(
            "the reproducibility check must still report on a no-toolset project — \
             staying silent is the same failure as a false pass: {report}"
        )
    });

    assert_ne!(
        level, "ok",
        "a check that examined nothing reported a pass: {msg:?}"
    );
    assert_eq!(
        level, "unchecked",
        "non-coverage has its own level so a UI can render it honestly: {msg:?}"
    );
    assert!(
        msg.contains("nothing declared to check"),
        "the report must say what it did not check, in words: {msg:?}"
    );
}

/// The same doctrine, as a *class* rather than one line: no line anywhere in a
/// report may claim a green pass while saying it had nothing to examine.
/// This is the tamper guard — it fails if a future check adds another
/// `Level::Ok, "no X defined"` sibling, which is exactly how this bug arrived.
#[test]
fn no_green_line_anywhere_claims_a_pass_over_nothing() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    for (name, manifest) in shapes() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let proj = project(tmp.path(), manifest);
        let report = doctor::collect(Some(&proj)).unwrap();

        for section in report["sections"].as_array().unwrap() {
            for line in section["lines"].as_array().unwrap() {
                if line["level"].as_str() != Some("ok") {
                    continue;
                }
                let msg = line["msg"].as_str().unwrap_or_default().to_lowercase();
                // The tell of a vacuous pass: the check reports that the thing
                // it checks does not exist here. A verified negative reads the
                // other way round ("no unsupported syntax **for any target**"
                // — it looked at the targets), so the patterns below are the
                // emptiness phrasings only.
                for vacuous in [
                    "nothing to check",
                    "nothing to probe",
                    "nothing declared",
                    "to verify",
                ] {
                    assert!(
                        !msg.contains(vacuous),
                        "[{name}] a green line claims a pass over nothing examined: {msg:?}"
                    );
                }
            }
        }
    }
}
