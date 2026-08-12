// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! A preview may not promise a write that would refuse.
//!
//! `use <toolset>` and `apply` without `--write` exist to answer one question:
//! what will the next command do? A preview that closes with "Re-run with
//! `--write`" over a state where `--write` exits 1 and applies nothing has
//! answered it wrongly, and in the most expensive direction — the user acts on
//! it.
//!
//! Three instances, one family (P8-G2, P8-G3, P8-G4), and the family already had
//! six closed members (G22, G23, G24, G27, G29, G30, G32). The gates that refuse
//! the write are not re-derived here: both commands now record the refusal in
//! BOTH modes, from the same `blocked` / delivers-nothing readings the write
//! itself bails on, so the preview cannot drift from the write it is predicting.
//!
//! # What the dry run EXITS with, and why it is still 0
//!
//! Deliberately unchanged. A dry run's status reports whether the PREVIEW
//! succeeded, not whether the previewed write would; a preview that correctly
//! reports "this would refuse" has done its job. Three things settle it:
//!
//!   * `unresolved_block.rs` already pins the decision in words — "a dry run
//!     never blocks a write, so it still exits 0" — for the blocked-secret state
//!     that is this exact family.
//!   * `apply`'s own validation arm is the precedent inside the same function:
//!     on a manifest with structural errors the dry run exits 0 and simply
//!     refuses to name `--write`, pointing at the real next step instead. The
//!     two arms added here are that arm, applied to two more gates.
//!   * Exit 0 from a preview claims nothing about delivery. The false success
//!     the G-series keeps closing is `--write` exiting 0 over a project where
//!     nothing landed, and every one of those still exits 1 below.
//!
//! The harm was never the status; it was the sentence. So the sentence moved.
//!
//! Every witness has a control, because each fix is otherwise satisfiable by
//! deleting the line it corrects — and a preview that never names `--write` is a
//! worse product than the bug.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Run {
    text: String,
    code: i32,
}

/// Strip SGR escapes so an assertion reads the sentence, not its styling.
/// `--write` is printed bold, so "Re-run with `--write` to apply." carries an
/// escape in the middle of the phrase the user actually reads.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        // CSI … final byte in @-~. Every escape this binary emits is an SGR.
        if chars.next() != Some('[') {
            continue;
        }
        for f in chars.by_ref() {
            if ('@'..='~').contains(&f) {
                break;
            }
        }
    }
    out
}

fn run(args: &[&str], home: &Path, proj: &Path) -> Run {
    let out = Command::new(env!("CARGO_BIN_EXE_agentstack"))
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
    Run {
        text: strip_ansi(&format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )),
        // A signal death has no code and would otherwise silently read as a
        // pass through `!= 0`.
        code: out.status.code().expect("the process must exit normally"),
    }
}

/// A locked checkout carrying `manifest`, in its own HOME so no other machine
/// state can decide the outcome. NOT trusted: the trust gate is what several of
/// the witnesses below are about.
fn locked_project(root: &Path, name: &str, manifest: &str) -> (PathBuf, PathBuf) {
    let home = root.join(format!("{name}-home"));
    let proj = root.join(name);
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(proj.join(".agentstack")).unwrap();
    fs::write(proj.join(".agentstack/agentstack.toml"), manifest).unwrap();
    let locked = run(&["lock", "--write"], &home, &proj);
    assert_eq!(locked.code, 0, "fixture: lock failed:\n{}", locked.text);
    (home, proj)
}

/// Grant content-bound trust for the project as it now stands.
fn trust(home: &Path, proj: &Path) {
    let preview = run(&["trust", "--preview"], home, proj);
    let json: serde_json::Value =
        serde_json::from_str(&preview.text).expect("`trust --preview` must be JSON");
    let digest = json["surface_digest"]
        .as_str()
        .expect("the preview must carry a surface digest")
        .to_string();
    let granted = run(&["trust", "--yes", "--consented", &digest], home, proj);
    assert_eq!(granted.code, 0, "fixture: trust failed:\n{}", granted.text);
}

/// A toolset whose one skill is real content for a harness's skills dir, so an
/// untrusted project has something the trust gate can actually refuse. (With the
/// default routing the servers travel the live lane and `use` writes no config,
/// so a servers-only manifest would be refused nothing and prove nothing.)
fn skill_toolset(root: &Path, name: &str) -> (PathBuf, PathBuf) {
    let src = root.join(format!("{name}-skill"));
    fs::create_dir_all(&src).unwrap();
    fs::write(
        src.join("SKILL.md"),
        "---\nname: notes\ndescription: a skill\n---\nbody\n",
    )
    .unwrap();
    let manifest = format!(
        "version = 1\n\
         [delivery]\n\
         render_locally = true\n\
         [targets]\n\
         default = [\"claude-code\"]\n\
         [skills.notes]\n\
         path = \"{}\"\n\
         [profiles.backend]\n\
         skills = [\"notes\"]\n",
        src.display()
    );
    locked_project(root, name, &manifest)
}

/// Everything this project has routes to the live lane, and no bridge is
/// registered: `apply --write` calls that a refused delivery and exits 1.
const LIVE_ONLY: &str = "version = 1\n\
                         [targets]\n\
                         default = [\"claude-code\"]\n\
                         [servers.docs]\n\
                         type = \"stdio\"\n\
                         command = \"echo\"\n";

/// The control: identical routing, plus one declaration that really does land.
const LIVE_PLUS_RENDERED: &str = "version = 1\n\
                                  [targets]\n\
                                  default = [\"claude-code\"]\n\
                                  [servers.docs]\n\
                                  type = \"stdio\"\n\
                                  command = \"echo\"\n\
                                  [settings.claude-code]\n\
                                  model = \"opus\"\n";

/// Servers forced onto the rendered lane, so the trust gate has a file to
/// refuse and `apply`'s blocked-write bail is the thing being predicted.
const RENDER_LOCALLY: &str = "version = 1\n\
                              [targets]\n\
                              default = [\"claude-code\"]\n\
                              [delivery]\n\
                              render_locally = true\n\
                              [servers.docs]\n\
                              type = \"stdio\"\n\
                              command = \"echo\"\n";

/// The sentence a preview may only print when `--write` would in fact apply.
const USE_PROMISE: &str = "Re-run with --write to apply.";
const APPLY_PROMISE: &str = "Re-run with --write to write.";

// ------------------------------------------------------------------- P8-G2

/// `use`'s dry run on an untrusted project told the user to run a command that
/// then refused.
///
/// The write is measured on the SAME state in the same test, so the preview's
/// claim is checked against the thing it was predicting rather than against a
/// remembered one.
#[test]
fn use_dry_run_does_not_promise_a_write_its_own_gate_would_refuse() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = skill_toolset(tmp.path(), "untrusted");

    let dry = run(&["use", "backend"], &home, &proj);
    let write = run(&["use", "backend", "--write"], &home, &proj);

    // The premise: this is the trust-refusal state, not some other failure.
    assert!(
        write.text.contains("has not been trusted for this content"),
        "fixture: the write must be refusing on trust, or this test measures \
         something else:\n{}",
        write.text
    );
    assert_eq!(
        write.code, 1,
        "the write refuses this state — that is what the preview must predict:\n{}",
        write.text
    );

    assert!(
        !dry.text.contains(USE_PROMISE),
        "the preview closed with `{USE_PROMISE}` over a state where `--write` \
         exits {} and applies nothing:\n{}",
        write.code,
        dry.text
    );
    assert!(
        dry.text.contains("would be BLOCKED"),
        "and it must say so — the ✗ lines above are per target; the closing line \
         is what a reader acts on:\n{}",
        dry.text
    );
    // The status is unchanged on purpose; see this file's header.
    assert_eq!(
        dry.code, 0,
        "a preview reports whether the PREVIEW succeeded — this one answered \
         correctly:\n{}",
        dry.text
    );
}

/// The control: a trusted project's preview keeps the ordinary line, and its
/// write keeps the ordinary exit.
///
/// Without this, the witness above is satisfied by never printing `--write`
/// again — which takes the next step away from every healthy project.
#[test]
fn a_trusted_project_still_gets_the_ordinary_dry_run_line() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = skill_toolset(tmp.path(), "trusted");
    trust(&home, &proj);

    let dry = run(&["use", "backend"], &home, &proj);
    assert_eq!(dry.code, 0, "a healthy preview exits 0:\n{}", dry.text);
    assert!(
        dry.text.contains(USE_PROMISE),
        "nothing blocks here, so the preview must still name the next \
         command:\n{}",
        dry.text
    );
    assert!(
        !dry.text.contains("would be BLOCKED"),
        "and it must not invent a blocker:\n{}",
        dry.text
    );

    // The promise is kept.
    let write = run(&["use", "backend", "--write"], &home, &proj);
    assert_eq!(
        write.code, 0,
        "the preview said `--write` would apply, so it must:\n{}",
        write.text
    );
}

// ------------------------------------------------------------------- P8-G4

/// A run that wrote nothing may not report an activation.
///
/// The failing write printed `⚠ activated 'backend' on N targets (wrote 0)`
/// ABOVE its own `error:` line. The counted targets are the ones with nothing to
/// do, so the sentence reads as a partial success where there was none.
///
/// G30's lesson holds here too: continuing and succeeding are separable. The
/// per-target transcript must survive — a user needs to see what was refused —
/// and only the claim of success is withdrawn.
#[test]
fn a_use_write_that_wrote_nothing_does_not_report_an_activation() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = skill_toolset(tmp.path(), "blocked-write");

    let write = run(&["use", "backend", "--write"], &home, &proj);

    assert_eq!(write.code, 1, "premise: the write failed:\n{}", write.text);
    assert!(
        !write.text.contains("activated 'backend' on"),
        "nothing was written, so there is no activation to report — this line \
         claims one over a run that exited {}:\n{}",
        write.code,
        write.text
    );
    assert!(
        write.text.contains("NOT activated"),
        "and the summary must still say what happened:\n{}",
        write.text
    );
    // Continuing and succeeding are separable: the transcript stays.
    assert!(
        write.text.contains("skills not materialized"),
        "the per-target refusal must still print — an exit code with no account \
         of what was refused is worse than the bug:\n{}",
        write.text
    );
    assert!(
        write.text.contains("agentstack trust ."),
        "and the way forward must still be named:\n{}",
        write.text
    );
}

/// The control: a write that DID activate still says so.
///
/// Without this, the witness above is satisfied by deleting the activation
/// summary — which would leave every successful `use --write` silent about what
/// it did.
#[test]
fn a_use_write_that_activated_still_says_so() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = skill_toolset(tmp.path(), "good-write");
    trust(&home, &proj);

    let write = run(&["use", "backend", "--write"], &home, &proj);
    assert_eq!(
        write.code, 0,
        "premise: the write succeeded:\n{}",
        write.text
    );
    assert!(
        write.text.contains("activated 'backend'"),
        "a real activation keeps its report:\n{}",
        write.text
    );
    assert!(
        !write.text.contains("NOT activated"),
        "and must not be reported as a failure:\n{}",
        write.text
    );
}

// ---------------------------------------- P8-G2, one gate deeper: lock drift

/// The fourth instance, found by reproducing the first three.
///
/// `use --write` runs a fail-closed drift gate BEFORE any target is touched: a
/// pinned skill whose bytes moved refuses the whole activation. That gate ran
/// under `--write` only, so the dry run neither mentioned the drift nor knew its
/// own write would refuse — and closed with "Re-run with `--write` to apply."
///
/// G24 recorded that its lane audit wrongly called its defect "the last of its
/// family". Reporting this one and leaving it would have repeated that. The gate
/// itself did not move: it is now ASKED in both modes and still ENFORCED in one.
#[test]
fn use_dry_run_reports_the_drift_gate_that_would_refuse_the_activation() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = skill_toolset(tmp.path(), "drifted");
    trust(&home, &proj);
    // Move the bytes AFTER the lockfile and the grant exist: what is on disk is
    // no longer what was pinned.
    fs::write(
        tmp.path().join("drifted-skill/SKILL.md"),
        "---\nname: notes\ndescription: a skill\n---\nbody two, written after the pin\n",
    )
    .unwrap();

    let dry = run(&["use", "backend"], &home, &proj);
    let write = run(&["use", "backend", "--write"], &home, &proj);

    assert!(
        write.text.contains("drifted from agentstack.lock"),
        "fixture: the write must be refusing on drift:\n{}",
        write.text
    );
    assert_eq!(write.code, 1, "the drift gate refuses:\n{}", write.text);

    assert!(
        !dry.text.contains(USE_PROMISE),
        "the preview promised a write the drift gate refuses:\n{}",
        dry.text
    );
    assert!(
        dry.text.contains("drifted from agentstack.lock"),
        "and it must name the drift at all — the preview was silent about the one \
         thing that stops the next command:\n{}",
        dry.text
    );
    assert!(
        dry.text.contains("agentstack lock --write"),
        "with the command that accepts it:\n{}",
        dry.text
    );
    // The gate did not move: a preview still writes nothing, and still exits 0.
    assert_eq!(dry.code, 0, "see this file's header:\n{}", dry.text);
    assert!(
        !proj.join(".claude/skills").exists(),
        "a dry run materialized nothing, then or now"
    );
}

/// The control: an intact pin keeps the ordinary preview.
///
/// Without this, the witness above is satisfied by refusing every project that
/// declares a skill.
#[test]
fn a_pin_that_still_matches_keeps_the_ordinary_preview() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = skill_toolset(tmp.path(), "intact");
    trust(&home, &proj);

    let dry = run(&["use", "backend"], &home, &proj);
    assert_eq!(dry.code, 0, "a healthy preview exits 0:\n{}", dry.text);
    assert!(
        dry.text.contains(USE_PROMISE),
        "nothing drifted, so the preview keeps its next command:\n{}",
        dry.text
    );
    assert!(
        !dry.text.contains("drifted from agentstack.lock"),
        "and must not invent a drift:\n{}",
        dry.text
    );
}

// ------------------------------------------------------------------- P8-G3

/// `apply`'s dry run promised a write that would refuse the whole delivery.
///
/// "0 targets would change" is an honest count. "Re-run with `--write` to write"
/// was not: on this project `--write` exits 1 with "nothing was delivered".
#[test]
fn apply_dry_run_does_not_promise_a_write_that_would_deliver_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = locked_project(tmp.path(), "live-only", LIVE_ONLY);
    trust(&home, &proj);

    let dry = run(&["apply"], &home, &proj);
    let write = run(&["apply", "--write"], &home, &proj);

    // The premise: this is the refused-delivery state, not some other failure.
    assert!(
        write.text.contains("nothing was delivered"),
        "fixture: the write must be refusing the delivery:\n{}",
        write.text
    );
    assert_eq!(write.code, 1, "apply's own contract:\n{}", write.text);

    assert!(
        !dry.text.contains(APPLY_PROMISE),
        "the preview closed with `{APPLY_PROMISE}` over a state where `--write` \
         exits {}:\n{}",
        write.code,
        dry.text
    );
    assert!(
        dry.text.contains("would deliver nothing here"),
        "and it must say what the write would do instead:\n{}",
        dry.text
    );
    // A prediction with no way forward strands the reader exactly where the
    // write would have.
    assert!(
        dry.text
            .contains("agentstack x gateway connect --all --write"),
        "the preview must name the same recovery command the write names:\n{}",
        dry.text
    );
    assert_eq!(dry.code, 0, "see this file's header:\n{}", dry.text);
}

/// The same shape, one gate lower: a trust refusal `apply --write` bails on.
///
/// Found while closing P8-G3 and fixed by the same reading — the preview records
/// the blocked target the write would refuse, instead of recording it only when
/// writing.
#[test]
fn apply_dry_run_names_the_gate_that_would_block_the_write() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = locked_project(tmp.path(), "untrusted-render", RENDER_LOCALLY);

    let dry = run(&["apply"], &home, &proj);
    let write = run(&["apply", "--write"], &home, &proj);

    assert!(
        write.text.contains("has not been trusted for this content"),
        "fixture: the write must be refusing on trust:\n{}",
        write.text
    );
    assert_eq!(write.code, 1, "a blocked write exits 1:\n{}", write.text);

    assert!(
        !dry.text.contains(APPLY_PROMISE),
        "the preview promised a write that the trust gate refuses:\n{}",
        dry.text
    );
    assert!(
        dry.text.contains("would refuse on 1 target"),
        "and it must name the count the write bails on:\n{}",
        dry.text
    );
    // The withheld work is still described — going quiet would be worse (G24).
    assert!(
        dry.text.contains("1 server to apply"),
        "the preview must still state the delivery it is withholding:\n{}",
        dry.text
    );
}

/// The control: a project whose rendered lane really does carry something keeps
/// the ordinary preview line and the ordinary exits.
///
/// Without this, both `apply` witnesses are satisfied by never naming `--write`
/// again — and by a command that fails whenever anything routes live, which
/// would fail every healthy MCP project there is. The failure being predicted is
/// "nothing would be delivered", never "routing happened".
#[test]
fn an_apply_that_would_deliver_something_still_names_the_write() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = locked_project(tmp.path(), "delivers", LIVE_PLUS_RENDERED);
    trust(&home, &proj);

    let dry = run(&["apply"], &home, &proj);
    assert_eq!(dry.code, 0, "a healthy preview exits 0:\n{}", dry.text);
    assert!(
        dry.text.contains(APPLY_PROMISE),
        "this write would land a setting, so the preview must still name \
         it:\n{}",
        dry.text
    );
    assert!(
        !dry.text.contains("would deliver nothing here"),
        "and must not predict a refusal that is not coming:\n{}",
        dry.text
    );

    let write = run(&["apply", "--write"], &home, &proj);
    assert_eq!(
        write.code, 0,
        "the preview said `--write` would write, so it must:\n{}",
        write.text
    );
}
