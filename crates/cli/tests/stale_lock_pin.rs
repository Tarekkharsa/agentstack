// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The false-healthy lock: a hand-edited manifest, re-trusted, reported ready.
//!
//! The exact sequence a design review walked, and what it found:
//!
//! 1. `lock --write` then `trust .` — `status` reads `locked · trusted`.
//! 2. Edit `.agentstack/agentstack.toml` by hand (an `env` key the manifest
//!    did not declare before). Nothing that was pinned has moved, so the
//!    content-drift pass sees nothing.
//! 3. `trust .` again — the manifest bytes changed, so the digest moved, and
//!    the re-review restores `trusted`.
//! 4. `status` says `locked · trusted` and `doctor` reports a green `Drift`
//!    section — while `agentstack.lock` still pins the project as it was in
//!    step 1. The declaration the user just consented to is not in the lock.
//!
//! Nothing in that state is detectable from the pins alone: every pin matches
//! the body it pins, and the missing pin is missing, not wrong. Finding it
//! needed a resolver run, which the refusal path may not perform (ruling P23,
//! `commands/locked.rs`). So the lock now RECORDS the manifest bytes it was
//! computed from (`Lock::manifest_digest`), and both surfaces answer the
//! question with a digest comparison instead.
//!
//! The second test covers the wrong-order trap the same review found next:
//! `trust` on an unlocked project grants happily, and the next `lock --write`
//! voids the grant. Message only — what `trust` accepts is unchanged.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;

use agentstack::cli::{LockArgs, TrustArgs};
use agentstack::commands::{doctor, lock as lock_cmd, overview, trust as trust_cmd};
use agentstack::trust;

/// These tests mutate the process-global `HOME`/`AGENTSTACK_HOME`; serialize.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn lock_write() -> LockArgs {
    LockArgs {
        quiet: false,
        profile: None,
        update: None,
        upgrade: None,
        all: false,
        with_instructions: false,
        yes: false,
        write: true,
    }
}

/// A non-interactive grant, consenting to the surface digest as it stands at
/// THIS moment — recomputed per call, because every edit between calls moves
/// it (§7.2, the same two-step an external UI performs).
fn grant(proj: &Path) {
    let args = TrustArgs {
        path: Some(proj.to_path_buf()),
        list: false,
        revoke: false,
        yes: true,
        consented: trust::digest_for(proj),
        preview: false,
    };
    trust_cmd::run(&args, Some(proj)).unwrap();
}

/// A fake machine with an isolated home, plus an empty project dir.
fn machine(tmp: &Path) -> std::path::PathBuf {
    let home = tmp.join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));
    let proj = tmp.join("proj");
    fs::create_dir_all(&proj).unwrap();
    proj
}

/// One instruction fragment — something `lock` genuinely pins, so the project
/// reaches a real `locked · trusted` rather than a manifest with nothing to
/// pin (which never grows a lockfile at all).
const MANIFEST: &str = "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
                        [instructions.house]\npath = \"./instructions/house.md\"\n";

/// The hand edit: a `[settings.claude-code] env` key the manifest did not
/// declare before. Chosen because it is the review's own example AND the
/// hardest case for every other detector — it adds a declaration, so no
/// existing pin can mismatch, and the only evidence is a pin that is absent.
const HAND_EDIT: &str = "\n[settings.claude-code]\nenv = { FOO = \"bar\" }\n";

fn drift_lines(report: &serde_json::Value) -> Vec<String> {
    report["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["title"] == "Drift")
        .map(|s| {
            s["lines"]
                .as_array()
                .unwrap()
                .iter()
                .map(|l| l["msg"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn stale_drift_line(report: &serde_json::Value) -> Option<String> {
    drift_lines(report)
        .into_iter()
        .find(|l| l.contains("older agentstack.toml"))
}

#[test]
fn a_hand_edited_manifest_leaves_the_lock_stale_and_both_surfaces_say_so() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = machine(tmp.path());
    fs::write(proj.join("agentstack.toml"), MANIFEST).unwrap();
    fs::create_dir_all(proj.join("instructions")).unwrap();
    fs::write(proj.join("instructions/house.md"), "# house\n").unwrap();

    // ── 1. locked · trusted, honestly ──────────────────────────────────────
    lock_cmd::run(&lock_write(), Some(&proj)).unwrap();
    grant(&proj);

    let body = overview::status_body(Some(&proj)).unwrap();
    let project = &body["project"];
    assert_eq!(project["locked"], true);
    assert_eq!(project["trust"], "trusted");
    assert_eq!(
        project["lock_stale"], false,
        "a lock written from THIS manifest is not stale: {project}"
    );
    let report = doctor::collect(Some(&proj)).unwrap();
    assert_eq!(
        stale_drift_line(&report),
        None,
        "nothing to report before the edit: {:?}",
        drift_lines(&report)
    );

    // ── 2. the hand edit, and 3. the re-review that hides it ───────────────
    let mut edited = fs::read_to_string(proj.join("agentstack.toml")).unwrap();
    edited.push_str(HAND_EDIT);
    fs::write(proj.join("agentstack.toml"), edited).unwrap();
    grant(&proj);

    // ── 4. …and this is where the old surfaces said "ready" ────────────────
    let body = overview::status_body(Some(&proj)).unwrap();
    let project = &body["project"];
    // The two readings that made the state look healthy are still exactly what
    // they were — this is not a test that trust or drift changed meaning.
    assert_eq!(
        project["trust"], "trusted",
        "the re-review did restore trust"
    );
    assert_eq!(
        project["content_drift"].as_array().unwrap().len(),
        0,
        "no pinned body moved — which is why nothing else could see this"
    );
    // The new reading, and the one that stops the false ready.
    assert_eq!(
        project["lock_stale"], true,
        "the lock still pins the pre-edit manifest: {project}"
    );

    // The Next line names both steps, in the order they must happen: the
    // machine field carries the runnable first one, its `why` the second.
    let next = &body["next_action"];
    assert_eq!(next["command"], "agentstack lock --write");
    let why = next["why"].as_str().unwrap();
    assert!(
        why.contains("agentstack trust ."),
        "the Next line must name the re-review as the second step: {why}"
    );

    // …and `doctor` reports it as drift instead of a green section.
    let report = doctor::collect(Some(&proj)).unwrap();
    let line = stale_drift_line(&report).unwrap_or_else(|| {
        panic!(
            "doctor must not report a clean Drift section here: {:?}",
            drift_lines(&report)
        )
    });
    assert!(
        line.contains("agentstack lock --write") && line.contains("agentstack trust ."),
        "doctor names both steps too: {line}"
    );

    // ── The repair converges: re-pin, re-review, and both surfaces clear ───
    lock_cmd::run(&lock_write(), Some(&proj)).unwrap();
    grant(&proj);
    let body = overview::status_body(Some(&proj)).unwrap();
    assert_eq!(body["project"]["lock_stale"], false);
    assert_eq!(body["project"]["trust"], "trusted");
    let report = doctor::collect(Some(&proj)).unwrap();
    assert_eq!(stale_drift_line(&report), None);
}

/// L3, the wrong-order trap: a grant on an unlocked project binds to a
/// lockfile that does not exist yet, and the next `lock --write` voids it.
/// `trust` now says so — and says nothing of the kind once the project is
/// pinned. Behaviour is untouched: both invocations still grant.
///
/// Driven through the real binary because the warning is a stderr line, and
/// stderr is where it must be: `trust --preview` writes JSON to stdout, and a
/// warning printed there would break every parser reading it.
#[test]
fn trust_on_an_unlocked_project_says_lock_first() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = machine(tmp.path());
    // A server, not an instruction fragment: the trap needs a project the
    // grant ACCEPTS while unlocked, and a declared body with no pin is refused
    // outright (`surface_unpinned`). A server definition is pinned from the
    // manifest itself, so an unlocked project declaring one is `grantable` —
    // which is precisely the state where the warning has something to say.
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [servers.echo]\ntype = \"stdio\"\ncommand = \"/bin/echo\"\n",
    )
    .unwrap();
    let home = tmp.path().join("home");

    let run = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_agentstack"))
            .args(args)
            .current_dir(&proj)
            .env("HOME", &home)
            .env("AGENTSTACK_HOME", home.join(".agentstack"))
            .env("NO_COLOR", "1")
            .output()
            .unwrap();
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).to_string(),
            String::from_utf8_lossy(&out.stderr).to_string(),
        )
    };

    // Unlocked: the preview raises the flag IN its JSON — and prints nothing
    // beside it, because a caller that merges stdout and stderr must still be
    // handed a parseable document.
    let (ok, stdout, stderr) = run(&["trust", "--preview", "."]);
    assert!(ok, "the preview still runs: {stderr}");
    let preview: serde_json::Value = serde_json::from_str(&format!("{stdout}{stderr}")).unwrap();
    assert_eq!(
        preview["lock_first"], true,
        "an unlocked preview must raise the flag: {preview}"
    );
    assert_eq!(
        preview["grantable"], true,
        "message only — the verdict is unchanged: {preview}"
    );

    // Unlocked: the grant warns too, and still grants.
    let digest = trust::digest_for(&proj).unwrap();
    let (ok, _, stderr) = run(&["trust", ".", "--yes", "--consented", &digest]);
    assert!(ok, "the grant still succeeds: {stderr}");
    assert!(
        stderr.contains("lock first"),
        "an unlocked grant must warn: {stderr}"
    );

    // Pinned: nothing to warn about, on either path.
    let (ok, _, stderr) = run(&["lock", "--write"]);
    assert!(ok, "{stderr}");
    let (ok, stdout, stderr) = run(&["trust", "--preview", "."]);
    assert!(ok, "{stderr}");
    let preview: serde_json::Value = serde_json::from_str(&format!("{stdout}{stderr}")).unwrap();
    assert_eq!(
        preview["lock_first"], false,
        "a locked project has no order to get wrong: {preview}"
    );
    let digest = trust::digest_for(&proj).unwrap();
    let (ok, _, stderr) = run(&["trust", ".", "--yes", "--consented", &digest]);
    assert!(ok, "{stderr}");
    assert!(
        !stderr.contains("lock first"),
        "a locked grant is silent: {stderr}"
    );
}
