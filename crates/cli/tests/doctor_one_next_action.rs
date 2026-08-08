// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Strategy v2 Phase 3, item 1 — "status as one next action", and the honest
//! reading of green.
//!
//! Two properties, both of which used to fail:
//!
//! 1. **Every report ends with exactly one recommended step.** Before this,
//!    the step was the first *repair* — so a healthy project, and a project
//!    whose only finding had a prose remedy, both ended with nothing.
//!    "0 errors, 0 warnings." and then silence reads as either "you're done"
//!    or "you forgot a step", and the reader cannot tell which.
//!
//!    That property lives on `next_step`, the human sentence. The machine
//!    field `next_action` answers a stricter question — "what may a program
//!    run verbatim?" — and is null wherever the honest human answer is a shape
//!    to fill in (`--server <server>`), a prose remedy, or a pointer at the
//!    report. A driver handed a placeholder loops on it forever.
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
/// is answered before any check runs, and it too ends with exactly one step —
/// so the property holds across the whole surface, not just the part this file
/// can drive.
///
/// The step is PROSE and the machine field is an explicit `null` (G33). It read
/// `agentstack init` on both, and `init` refuses without a terminal — which is
/// the one thing a driver, the only reader of `next_action`, always lacks. The
/// reasoning, including why every runnable spelling of `init` is worse than
/// nothing here, is on `overview::NO_MANIFEST_NEXT`, which `status` reads too so
/// the two surfaces describe one directory with one sentence.
#[test]
fn the_uninitialized_state_is_answered_before_checks_run() {
    let src = include_str!("../src/commands/doctor.rs");
    assert!(
        src.contains(r#""state": "needs_setup""#),
        "the no-manifest JSON state must still be answered before the checks run"
    );
    assert!(
        src.contains(r#""next_action": serde_json::Value::Null"#)
            && src.contains(r#""next_step": super::overview::NO_MANIFEST_NEXT.0"#),
        "the no-manifest JSON must answer a driver with an explicit null and a person with the \
         shared sentence — never with a command that refuses without a terminal"
    );
    let overview = include_str!("../src/commands/overview.rs");
    let body = overview
        .split("pub(crate) const NO_MANIFEST_NEXT: (&str, &str) = (")
        .nth(1)
        .expect("the shared sentence must exist");
    let sentence = body.split(");").next().unwrap_or_default();
    assert!(
        sentence.contains("agentstack init"),
        "…and that sentence must still tell the reader how to adopt the directory, or the null is \
         a dead end rather than a terminal answer"
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
        // `next_step` is the HUMAN field — the sentence the terminal prints —
        // and it is the one that must always be there. `next_action` is the
        // machine field and is checked below: runnable, or absent.
        let next = report["next_step"].as_str();

        let next = next.unwrap_or_else(|| {
            panic!("[{name}] next_step must never be null — a report with no path out is the bug this test exists for: {report}")
        });
        assert!(
            !next.trim().is_empty(),
            "[{name}] next_step must not be blank: {report}"
        );

        // The machine field carries a command a driver may exec verbatim, or
        // nothing at all. A placeholder (`--server <server>`) or a prose
        // remedy here is the loop this round closed: the driver runs it, it
        // refuses, it re-polls, forever.
        match report["next_action"].as_str() {
            None => assert!(
                report["next_action"].is_null(),
                "[{name}] next_action must be a string or null: {report}"
            ),
            Some(cmd) => {
                assert!(
                    cmd.starts_with("agentstack ") && !cmd.contains('<'),
                    "[{name}] next_action must be runnable verbatim, got {cmd:?}"
                );
            }
        }

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
            "[{name}] next_step is not an actionable step: {next:?}"
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
    let next = report["next_step"].as_str().unwrap_or_default();

    assert!(
        !next.is_empty(),
        "a clean report must still end with one action: {report}"
    );
    // It must be a step, not necessarily a COMMAND. This assertion used to
    // demand `agentstack …`, which was only ever satisfiable here by writing
    // the empty project's answer as `agentstack search <query>` — a shape
    // `machine_command` then dropped, so the report ended with a sentence that
    // looked runnable beside a machine field of `null` with nothing saying why
    // (G28). The query is the one argument only the person with the problem can
    // supply. So the terminal states this report already had — "nothing to
    // repair — this setup is verified", "review the errors above" — are joined
    // by one more, and the rule that actually matters is asserted directly
    // below instead: the sentence and the machine field must agree.
    assert_eq!(
        next, "find a server or skill to add — only you know what this project needs",
        "the empty project's step must say what only the human can supply: {report}"
    );
    // …and the MACHINE field is null, which does NOT contradict this file's
    // premise. This test is `every_report_ends_with_exactly_one_next_action`
    // and that property lives on `next_step`, the human sentence, asserted just
    // above — the machine field answers the stricter "what may a program run
    // verbatim?", and the module docs at the top of this file already say it is
    // null wherever the honest human answer is not executable.
    //
    // It briefly asserted `Some("agentstack search")` on the reasoning that a
    // runnable command exists here: `search` takes its query as an optional
    // positional, so the bare form is complete and exits 0. Complete, but not an
    // ACTION — it is a read-only browse that leaves every observable field of
    // the report identical, so a driver polling this field runs it and is handed
    // it again forever, which `guidance_is_executable.rs` reproduces as a loop.
    //
    // The second half of that reasoning — that nulling breaks `status-v1` — was
    // also wrong, and measurably: the Group and Verified rungs already answer
    // null from this field on far commoner projects, so consumers handle it.
    assert!(
        report["next_action"].is_null(),
        "the one missing input here is a human decision, so there is no command \
         for a driver — and a browse that changes nothing is not one: {report}"
    );
    // The shape a person substitutes into still ships — in the `why`, where no
    // driver is invited to run it. Losing it would trade one honesty problem
    // for a usability one.
    let why = report["next_step_why"]
        .as_str()
        .or_else(|| report["why"].as_str())
        .unwrap_or_default();
    assert!(
        why.contains("agentstack search <query>") || why.is_empty(),
        "the fillable shape should survive into the human explanation: {report}"
    );
}

/// The machine field never carries a placeholder, and the two summary surfaces
/// hand a program the same thing.
///
/// The reproduction this closes, on a project with nothing wrong:
/// `next_action: 'agentstack toolset create <name> --server <server>'`, run
/// verbatim → `no server '<server>' in the manifest or central library`, poll
/// again, same answer, forever. The human sentence is still right and still
/// printed; only what a driver is handed changes.
#[test]
fn neither_summary_surface_puts_an_unrunnable_value_in_the_machine_field() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    for (name, manifest) in shapes() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let proj = project(tmp.path(), manifest);

        let doc = doctor::collect(Some(&proj)).unwrap();
        let status = agentstack::commands::overview::status_body(Some(&proj)).unwrap();

        let d = doc["next_action"].clone();
        let s = status["next_action"]["command"].clone();
        for (surface, v) in [("doctor", &d), ("status", &s)] {
            if let Some(cmd) = v.as_str() {
                assert!(
                    cmd.starts_with("agentstack ") && !cmd.contains('<'),
                    "[{name}/{surface}] next_action must be runnable verbatim, got {cmd:?}"
                );
            } else {
                assert!(v.is_null(), "[{name}/{surface}] expected a string or null");
            }
        }
        // No cross-surface equality is asserted above consent on purpose:
        // `doctor` runs checks `status` never runs, and `status` ranks the
        // unregistered bridge where `doctor` holds it below consent, so the
        // two legitimately answer some states from different knowledge. What
        // binds them is the SHARED terminal — both end in
        // `overview::ladder_rung` and both filter through
        // `overview::machine_command`, which is why neither can emit a
        // placeholder here.
        // The human sentence stays, whatever the machine field says.
        assert!(
            doctor_step(&doc).len() > 3,
            "[{name}] the human next step must survive: {doc}"
        );
    }
}

fn doctor_step(report: &serde_json::Value) -> &str {
    report["next_step"].as_str().unwrap_or_default()
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

/// The Apply rung must never name a render that cannot happen here.
///
/// `apply` writes servers, instructions, settings, hooks and extensions —
/// never skills, which activate through `use`. A skills-only manifest standing
/// on the Apply rung was told `agentstack apply --write`; that reports
/// "already in sync", leaves the rung exactly where it was, and the ladder asks
/// for it again. A driver reading `next_action` and executing it verbatim
/// never leaves that state. The honest command for this rung is
/// `agentstack use --write`, which is valid with or without declared toolsets.
#[test]
fn a_skills_only_project_is_sent_to_use_not_to_a_render_that_does_nothing() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project(
        tmp.path(),
        Some("version = 1\n\n[skills.helper]\npath = \"./skills/helper\"\n"),
    );
    fs::create_dir_all(proj.join(".agentstack/skills/helper")).unwrap();
    fs::write(
        proj.join(".agentstack/skills/helper/SKILL.md"),
        "---\nname: helper\ndescription: helps\n---\n\nbody\n",
    )
    .unwrap();

    let report = doctor::collect(Some(&proj)).unwrap();
    let next = report["next_action"].as_str().unwrap_or_default();
    assert_ne!(
        next, "agentstack apply --write",
        "`apply` never renders skills — this rung would repeat forever: {report}"
    );

    // A manifest `apply` DOES render keeps the render. The correction is
    // narrow: only the rung whose command could not act is rewritten.
    let tmp2 = assert_fs::TempDir::new().unwrap();
    let served = project(
        tmp2.path(),
        Some("version = 1\n[servers.demo]\ntype = \"stdio\"\ncommand = \"echo\"\n"),
    );
    let served_report = doctor::collect(Some(&served)).unwrap();
    assert!(
        served_report["next_step"]
            .as_str()
            .unwrap_or_default()
            .starts_with("agentstack "),
        "{served_report}"
    );
}

/// A declared skill whose body is not on disk is an ERROR, not an advisory.
///
/// The source resolved — this is not a missing fetch, which `not installed`
/// covers with the command that repairs it. The body simply is not there, and
/// no command repairs it: `lock --write` resolves before it pins and exits
/// non-zero unchanged, `install` exits 1 for an inline body and does nothing
/// for a library one. Reporting it at warning level let `doctor` answer
/// `errors: 0` over a project that can never deliver what it declares
/// (invariant 8). The finding carries no `↳` fix, and `next_action` stays
/// null, precisely because naming a command here would name one that cannot
/// work.
#[test]
fn a_declared_skill_with_no_body_on_disk_is_an_error_that_names_no_false_fix() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project(
        tmp.path(),
        Some("version = 1\n\n[skills.ghost]\npath = \"./skills/ghost\"\n"),
    );
    // The directory exists; the body does not.
    fs::create_dir_all(proj.join(".agentstack/skills/ghost")).unwrap();

    let report = doctor::collect(Some(&proj)).unwrap();
    assert!(
        report["errors"].as_u64().unwrap_or(0) >= 1,
        "a declared skill with no body must not read as `errors: 0`: {report}"
    );
    let found = report["sections"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|s| s["lines"].as_array().unwrap())
        .any(|l| {
            l["level"] == "error"
                && l["msg"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("its body is not on disk")
        });
    assert!(found, "the error must name the condition: {report}");
    assert!(
        report["next_action"].is_null(),
        "no command repairs this state — the machine field must offer none: {report}"
    );
}

/// Round 7, finding 1. The per-item `fix` must survive to the machine field.
///
/// `ContentDrift::fix` is an `Option` precisely so a blocker no command
/// repairs can say NOTHING, and `trust --preview` already emits `fix: null`
/// here. `doctor` collapsed the whole never-pinned list to a bool, so it
/// answered `agentstack lock --write` over both states below — a command that
/// exits non-zero in the first and exits 0 with a green tick in the second,
/// leaving the blocking condition untouched either way. A driver reading one
/// field runs it forever.
#[test]
fn a_never_pinned_body_absent_from_disk_names_no_command_in_the_machine_field() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    // 1a — the only declared skill's body directory is missing outright.
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project(
        tmp.path(),
        Some("version = 1\n\n[skills.summarize]\npath = \"./skills/summarize\"\n"),
    );
    let report = doctor::collect(Some(&proj)).unwrap();
    assert!(
        report["next_action"].is_null(),
        "nothing repairs a body that is not on disk: {report}"
    );
    let sentence = report["next_step"].as_str().unwrap_or_default();
    assert!(
        sentence.contains("skills/summarize") && sentence.contains("not present on disk"),
        "the human sentence must still name the path and the condition: {report}"
    );

    // 1b — a skill declared outside every toolset, whose body is missing,
    // beside one that pins cleanly. `lock --write` exits 0 here, so exit code
    // gives a driver nothing; only a null machine field ends the loop.
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project(
        tmp.path(),
        Some(concat!(
            "version = 1\n\n[skills.summarize]\npath = \"./skills/summarize\"\n\n",
            "[skills.orphan]\npath = \"./skills/orphan\"\n\n",
            "[toolsets.dev]\nskills = [\"summarize\"]\n"
        )),
    );
    fs::create_dir_all(proj.join(".agentstack/skills/summarize")).unwrap();
    fs::write(
        proj.join(".agentstack/skills/summarize/SKILL.md"),
        "---\nname: summarize\ndescription: sums things up\n---\n\nbody\n",
    )
    .unwrap();
    let report = doctor::collect(Some(&proj)).unwrap();
    // Before anything is pinned the surface DOES carry a working fix, and
    // naming it stays correct — the loop must converge, not go silent early.
    assert_eq!(
        report["next_action"], "agentstack lock --write",
        "a genuinely pinnable item still names its command: {report}"
    );
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(["lock", "--write"])
        .current_dir(&proj)
        .env("HOME", tmp.path().join("home"))
        .env("AGENTSTACK_HOME", tmp.path().join("home/.agentstack"))
        .output()
        .unwrap();
    assert!(out.status.success(), "`lock --write` exits 0 in this state");
    let report = doctor::collect(Some(&proj)).unwrap();
    assert!(
        report["next_action"].is_null(),
        "the only blocker left carries no fix, so the loop must end: {report}"
    );
    let sentence = report["next_step"].as_str().unwrap_or_default();
    assert!(
        sentence.contains("skills/orphan") && sentence.contains("not present on disk"),
        "the human sentence must still name the path and the condition: {report}"
    );
}

// ------------------------------------------------------------------------
// G32(c) — the rung must never name the command that just refused.
// ------------------------------------------------------------------------

/// Run the binary in a sealed environment and return `(stdout+stderr, code)`.
///
/// Subprocess, not `doctor::collect`, and deliberately: the rung under test
/// branches on whether any harness on THIS machine can host the gateway, which
/// `detect.bin` answers by looking for `claude` on `PATH`. An in-process test
/// would inherit the developer's `PATH` and quietly measure a different rung on
/// a machine that happens to have Claude Code installed. `env_clear` makes the
/// state part of the fixture instead of part of the machine.
fn sealed(args: &[&str], home: &std::path::Path, proj: &std::path::Path) -> (String, i32) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(args)
        .current_dir(proj)
        .env_clear()
        .env("HOME", home)
        .env("AGENTSTACK_HOME", home.join(".agentstack"))
        .env("PATH", "/usr/bin:/bin")
        .env("NO_COLOR", "1")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("the binary must run");
    (
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
        out.status.code().expect("the process must exit normally"),
    )
}

/// A trusted, locked project carrying `manifest`, in its own sealed HOME.
fn sealed_project(
    root: &std::path::Path,
    name: &str,
    manifest: &str,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let home = root.join(format!("{name}-home"));
    let proj = root.join(name);
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(proj.join(".agentstack")).unwrap();
    fs::write(proj.join(".agentstack/agentstack.toml"), manifest).unwrap();
    let (_, code) = sealed(&["lock", "--write"], &home, &proj);
    assert_eq!(code, 0, "fixture: lock --write must succeed");
    let (preview, _) = sealed(&["trust", "--preview"], &home, &proj);
    let json: serde_json::Value =
        serde_json::from_str(&preview).expect("`trust --preview` must be JSON");
    let digest = json["surface_digest"].as_str().unwrap().to_string();
    let (_, code) = sealed(
        &["trust", "--yes", "--consented-digest", &digest],
        &home,
        &proj,
    );
    assert_eq!(code, 0, "fixture: trust must succeed");
    (home, proj)
}

fn next_action_of(json: &str) -> serde_json::Value {
    let report: serde_json::Value = serde_json::from_str(json).expect("doctor --json must be JSON");
    report["next_action"].clone()
}

/// Every capability routed live, and no CLI on this machine that can host a
/// gateway for it.
const UNDELIVERABLE: &str = "version = 1\n\
                             [targets]\n\
                             default = [\"claude-code\"]\n\
                             [servers.docs]\n\
                             type = \"stdio\"\n\
                             command = \"echo\"\n";

/// The same routing plus one declaration that lands on disk — so something IS
/// delivered and `apply --write` exits 0.
const DELIVERABLE: &str = "version = 1\n\
                           [targets]\n\
                           default = [\"claude-code\"]\n\
                           [servers.docs]\n\
                           type = \"stdio\"\n\
                           command = \"echo\"\n\
                           [settings.claude-code]\n\
                           model = \"opus\"\n";

/// The defect, and it is the G22/G24 class one rung further down: `doctor`
/// answered `agentstack apply --write` on the exact state where `apply --write`
/// refuses.
///
/// Measured before the fix: `apply --write` exits 1 with "nothing was
/// delivered", and `doctor` on the same project printed "0 errors, 0 warnings",
/// "ready: reviewed, activated, and verified", and `next: agentstack apply
/// --write`. A driver reading `next_action` runs the command it was just handed,
/// gets exit 1, re-reads the same field, and never leaves the state.
///
/// The command named instead is the one `apply`'s own refusal names, and it is
/// the only one of the two that can run here: `x gateway connect` exits 1 with
/// "no installed harness with MCP support detected", because the very condition
/// this rung fires on is that no harness here can host a bridge.
#[test]
fn the_rung_for_a_project_that_delivers_nothing_names_a_command_that_runs() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let (a_home, a_proj) = sealed_project(tmp.path(), "for-apply", UNDELIVERABLE);
    let (d_home, d_proj) = sealed_project(tmp.path(), "for-doctor", UNDELIVERABLE);

    // The premise: this really is the state `apply` refuses.
    let (applied, code) = sealed(&["apply", "--write"], &a_home, &a_proj);
    assert_eq!(
        code, 1,
        "fixture: apply must be refusing the delivery here:\n{applied}"
    );
    assert!(
        applied.contains("nothing was delivered"),
        "fixture: for the refusal this rung is about:\n{applied}"
    );

    let (json, _) = sealed(&["doctor", "--json"], &d_home, &d_proj);
    let next = next_action_of(&json);
    assert_ne!(
        next, "agentstack apply --write",
        "`doctor` may not hand a driver the command that just exited 1 on this \
         very project — that is a loop with no exit:\n{json}"
    );
    assert_eq!(
        next, "agentstack x delivery render-locally --write",
        "the rung must name the one way forward that runs here:\n{json}"
    );

    // Naming a runnable command is not enough — it has to end the state. The
    // rung converges: run it, render, and the finding is gone.
    let (_, code) = sealed(
        &["x", "delivery", "render-locally", "--write"],
        &d_home,
        &d_proj,
    );
    assert_eq!(
        code, 0,
        "the named command must run in the state it is named"
    );
    let (_, code) = sealed(&["apply", "--write"], &d_home, &d_proj);
    assert_eq!(code, 0, "and it must unblock the render it was blocking");
    let (json, _) = sealed(&["doctor", "--json"], &d_home, &d_proj);
    let report: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(
        report["errors"], 0,
        "the finding must clear once obeyed, or the loop simply moved:\n{json}"
    );
}

/// The first control: a project that DOES deliver something keeps its ordinary
/// rung and gains no finding.
///
/// Without this, the witness above is satisfied by routing every live-lane
/// project to `render-locally` — which would push a working zero-files setup
/// into writing files it deliberately does not write.
#[test]
fn a_project_that_delivers_something_gains_no_undeliverable_finding() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let (home, proj) = sealed_project(tmp.path(), "delivers", DELIVERABLE);

    let (applied, code) = sealed(&["apply", "--write"], &home, &proj);
    assert_eq!(
        code, 0,
        "fixture: the rendered lane carries a setting, so this delivers:\n{applied}"
    );

    let (json, _) = sealed(&["doctor", "--json"], &home, &proj);
    assert!(
        !json.contains("nothing here can be delivered"),
        "a project that delivered something must not be told it delivers \
         nothing:\n{json}"
    );
    assert_ne!(
        next_action_of(&json),
        "agentstack x delivery render-locally --write",
        "and it must not be pushed into writing files it already writes:\n{json}"
    );
}

/// The second control: the finding stays BELOW the consent rung.
///
/// `render-locally` records an override; it renders nothing while the gate is
/// up, so naming it over an untrusted project would be step 2 named while step
/// 1 is outstanding — the same dead end the unconnected-bridge finding is
/// already held back for. The same project, only untrusted, must still be sent
/// to the review.
#[test]
fn the_undeliverable_finding_does_not_outrank_the_review() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("untrusted-home");
    let proj = tmp.path().join("untrusted");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(proj.join(".agentstack")).unwrap();
    fs::write(proj.join(".agentstack/agentstack.toml"), UNDELIVERABLE).unwrap();
    let (_, code) = sealed(&["lock", "--write"], &home, &proj);
    assert_eq!(code, 0, "fixture: lock --write must succeed");

    let (json, _) = sealed(&["doctor", "--json"], &home, &proj);
    assert_eq!(
        next_action_of(&json),
        "agentstack trust .",
        "nothing is delivered here either, but the review comes first:\n{json}"
    );
}
