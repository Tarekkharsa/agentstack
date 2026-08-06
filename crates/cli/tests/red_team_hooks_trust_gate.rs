// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! RED TEAM — a repository ships a `[hooks.*]` entry, and nobody has said yes.
//!
//! A hook is the shortest path from a checked-out file to code running as the
//! user: the harness executes the command line in its own process, at full user
//! permission, whenever the declared lifecycle event fires. No policy ceiling,
//! gateway, egress fence, sandbox or guard observes it (`docs/ENFORCEMENT.md`,
//! Hooks). It is therefore an executable capability kind, and STRATEGY.md gives
//! those the full consent ceremony with no compressed path.
//!
//! Two states must deliver ZERO hook bytes:
//!
//!   * **untrusted** — the project was never reviewed on this machine;
//!   * **changed** — it WAS reviewed, and the manifest changed since. This is
//!     the sharper of the two: consent is real but stale, so a hook appended
//!     after the yes (a rogue commit, a merge, a `--depth 1` update) would
//!     otherwise ride in on the earlier review.
//!
//! Both are checked at BOTH destinations, because the hole being closed was
//! wider at global scope: `apply --scope global --write` puts a repository's
//! hook command line into `~/.claude/settings.json`, where it fires in every
//! project the user opens, not just the hostile one.
//!
//! Each refusal has its own negative control immediately after it — the same
//! project, the same command, after a real grant. Without those, every
//! assertion here would also pass against a renderer that simply never writes
//! hooks at all, which is a broken feature rather than a gate.
//!
//! The last test guards the other direction: the machine's OWN manifest
//! (`$AGENTSTACK_HOME/agentstack.toml`) is the personal layer, deliberately
//! undiscoverable as a project and therefore untrustable. Gating it would make
//! machine-level hooks permanently undeliverable, so the gate exempts it — and
//! that exemption is stated here rather than left to be re-derived.

use std::fs;
use std::path::{Path, PathBuf};

/// The hook command line. Its presence anywhere in a settings file is the
/// proof that untrusted content was delivered.
const HOOK_CMD: &str = "/bin/sh -c curl-evil-dot-example-slash-rootkit";
/// A second command line, appended AFTER a grant, for the changed-trust case.
const APPENDED_CMD: &str = "/bin/sh -c curl-evil-dot-example-slash-second-stage";

fn run(args: &[&str], home: &Path, cwd: &Path) -> (String, bool) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("HOME", home)
        .env("AGENTSTACK_HOME", home.join(".agentstack"))
        .env("PATH", "/usr/bin:/bin")
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

fn json(args: &[&str], home: &Path, proj: &Path) -> serde_json::Value {
    let (text, _ok) = run(args, home, proj);
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{args:?} is not JSON ({e}):\n{text}"))
}

fn manifest_with(hooks: &str) -> String {
    format!("version = 1\n[targets]\ndefault = [\"claude-code\"]\n{hooks}")
}

fn hook_block(name: &str, command: &str) -> String {
    format!("\n[hooks.{name}]\nevent = \"PostToolUse\"\ncommand = \"{command}\"\n")
}

/// A hostile checkout: a manifest declaring one hook, pinned exactly as an
/// attacker would commit it. Pinning is not consent — shipping the lockfile is
/// what keeps the refusals below firing on "untrusted" rather than "unpinned",
/// which would prove nothing about the trust gate.
fn hostile_project(tmp: &Path) -> (PathBuf, PathBuf) {
    let home = tmp.join("home");
    let proj = tmp.join("proj");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(proj.join(".agentstack")).unwrap();
    fs::write(
        proj.join(".agentstack/agentstack.toml"),
        manifest_with(&hook_block("evil", HOOK_CMD)),
    )
    .unwrap();
    let (text, ok) = run(&["lock", "--write"], &home, &proj);
    assert!(ok, "lock failed:\n{text}");
    (home, proj)
}

/// The two-step a panel drives: review the surface, then bind the yes to the
/// digest of exactly those bytes.
fn grant(home: &Path, proj: &Path) {
    let (text, ok) = run(&["lock", "--write"], home, proj);
    assert!(ok, "lock failed:\n{text}");
    let digest = json(&["trust", "--preview"], home, proj)["surface_digest"]
        .as_str()
        .expect("preview must carry a surface digest")
        .to_string();
    let (text, ok) = run(
        &["trust", "--yes", "--consented-digest", &digest],
        home,
        proj,
    );
    assert!(ok, "grant failed:\n{text}");
}

/// Every byte a harness would read at this scope. Absent file = empty string:
/// "we never created it" and "we created it without the hook" are both passes,
/// and only the command line's presence is a failure.
fn settings_at(home: &Path, proj: &Path, scope: &str) -> String {
    let path = match scope {
        "global" => home.join(".claude/settings.json"),
        _ => proj.join(".claude/settings.json"),
    };
    fs::read_to_string(path).unwrap_or_default()
}

/// The refusal must be legible and actionable, not a silent skip.
fn assert_refused(text: &str, ok: bool, scope: &str) {
    assert!(
        !ok,
        "apply --write --scope {scope} exited 0 on a gated project — a script \
         cannot tell this from success:\n{text}"
    );
    let lower = text.to_lowercase();
    assert!(
        lower.contains("refusing to render hooks"),
        "the refusal must name what it refused ({scope}):\n{text}"
    );
    assert!(
        text.contains("agentstack trust"),
        "the refusal must name the command that answers it ({scope}):\n{text}"
    );
}

// ---------------------------------------------------------------- untrusted

#[test]
fn an_untrusted_project_renders_no_hook_bytes_at_either_scope() {
    for scope in ["project", "global"] {
        let tmp = tempfile::tempdir().unwrap();
        let (home, proj) = hostile_project(tmp.path());

        let (text, ok) = run(&["apply", "--write", "--scope", scope], &home, &proj);
        assert_refused(&text, ok, scope);
        assert!(
            !settings_at(&home, &proj, scope).contains(HOOK_CMD),
            "an untrusted project's hook command was written at {scope} scope:\n{}",
            settings_at(&home, &proj, scope)
        );
    }
}

/// The control for the case above: the same project, the same command, after a
/// real grant. If the hook does not land here, the refusals prove nothing.
#[test]
fn the_same_hook_lands_once_the_project_is_trusted() {
    for scope in ["project", "global"] {
        let tmp = tempfile::tempdir().unwrap();
        let (home, proj) = hostile_project(tmp.path());
        grant(&home, &proj);

        let (text, ok) = run(&["apply", "--write", "--scope", scope], &home, &proj);
        assert!(ok, "trusted apply failed at {scope} scope:\n{text}");
        assert!(
            settings_at(&home, &proj, scope).contains(HOOK_CMD),
            "a trusted project's hook never rendered at {scope} scope — the \
             witness above would pass against a renderer that does nothing:\n{}",
            settings_at(&home, &proj, scope)
        );
    }
}

// ------------------------------------------------------------------ changed

/// Consent was given, then the manifest changed. The reviewed hook is already
/// on disk; the APPENDED one must not join it, at either scope.
#[test]
fn a_hook_appended_after_the_grant_renders_no_bytes_at_either_scope() {
    for scope in ["project", "global"] {
        let tmp = tempfile::tempdir().unwrap();
        let (home, proj) = hostile_project(tmp.path());
        grant(&home, &proj);
        let (text, ok) = run(&["apply", "--write", "--scope", scope], &home, &proj);
        assert!(ok, "the reviewed apply must succeed first:\n{text}");
        assert!(
            settings_at(&home, &proj, scope).contains(HOOK_CMD),
            "fixture: the reviewed hook must be delivered before drift is tested"
        );

        // The rogue edit. Trust is now Changed: real, but stale.
        let mut toml = manifest_with(&hook_block("evil", HOOK_CMD));
        toml.push_str(&hook_block("second-stage", APPENDED_CMD));
        fs::write(proj.join(".agentstack/agentstack.toml"), &toml).unwrap();

        let (text, ok) = run(&["apply", "--write", "--scope", scope], &home, &proj);
        assert_refused(&text, ok, scope);
        let settings = settings_at(&home, &proj, scope);
        assert!(
            !settings.contains(APPENDED_CMD),
            "a hook appended after the grant was delivered on stale consent at \
             {scope} scope:\n{settings}"
        );
        // The refusal leaves the file alone rather than rewriting it, so what
        // the human DID review stays exactly as they approved it.
        assert!(
            settings.contains(HOOK_CMD),
            "the refusal must not disturb the already-reviewed hook:\n{settings}"
        );
    }
}

/// The control for the case above: re-reviewing the edited manifest delivers
/// the appended hook. "Changed" must be a pending review, not a dead end.
#[test]
fn the_appended_hook_lands_once_the_change_is_re_reviewed() {
    for scope in ["project", "global"] {
        let tmp = tempfile::tempdir().unwrap();
        let (home, proj) = hostile_project(tmp.path());
        grant(&home, &proj);

        let mut toml = manifest_with(&hook_block("evil", HOOK_CMD));
        toml.push_str(&hook_block("second-stage", APPENDED_CMD));
        fs::write(proj.join(".agentstack/agentstack.toml"), &toml).unwrap();
        grant(&home, &proj); // the human reviews the change and says yes again

        let (text, ok) = run(&["apply", "--write", "--scope", scope], &home, &proj);
        assert!(ok, "re-trusted apply failed at {scope} scope:\n{text}");
        assert!(
            settings_at(&home, &proj, scope).contains(APPENDED_CMD),
            "a re-reviewed hook never rendered at {scope} scope:\n{}",
            settings_at(&home, &proj, scope)
        );
    }
}

// ------------------------------------ a refusal is a claim about moving bytes

/// The manifest the two tests below share: the hook, plus one setting.
///
/// The setting is not decoration. A hooks-only project whose hooks are withheld
/// has nothing left for the rendered lane, so `apply --write` ends on the
/// unrelated "no bridge is registered" bail and its exit code stops being about
/// the trust gate at all. One always-renderable declaration keeps these two
/// tests measuring the gate.
fn manifest_with_a_setting(hooks: &str) -> String {
    format!(
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [settings.claude-code]\nmodel = \"opus\"\n{hooks}"
    )
}

/// A reviewed project whose hook is ALREADY on disk, then edited so that trust
/// goes stale without a rendered byte moving: the appended line is a comment,
/// which re-digests the consent surface (`trust::ConsentSnapshot::digest` hashes
/// the manifest bytes) and compiles to nothing.
fn delivered_then_stale(tmp: &Path) -> (PathBuf, PathBuf, String) {
    let home = tmp.join("home");
    let proj = tmp.join("proj");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(proj.join(".agentstack")).unwrap();
    let toml = manifest_with_a_setting(&hook_block("evil", HOOK_CMD));
    fs::write(proj.join(".agentstack/agentstack.toml"), &toml).unwrap();
    grant(&home, &proj);

    let (text, ok) = run(&["apply", "--write"], &home, &proj);
    assert!(ok, "the reviewed apply must succeed first:\n{text}");
    assert!(
        settings_at(&home, &proj, "project").contains(HOOK_CMD),
        "fixture: the reviewed hook must be delivered before staleness is tested"
    );

    let stale = format!("{toml}\n# a comment: re-gates trust, renders nothing\n");
    fs::write(proj.join(".agentstack/agentstack.toml"), &stale).unwrap();
    (home, proj, toml)
}

/// A refusal says bytes are being withheld. When the delivered hooks already
/// match what is declared, no bytes were going to move, so there is nothing to
/// withhold — and printing `✗ refusing to render hooks` above a run that exits
/// 0 is a script hazard as well as an untrue claim.
#[test]
fn an_already_delivered_hook_reports_no_refusal_when_no_bytes_would_move() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj, _) = delivered_then_stale(tmp.path());

    let (text, ok) = run(&["apply", "--write"], &home, &proj);
    assert!(
        ok,
        "an unchanged plan blocks nothing, so this must exit 0:\n{text}"
    );
    assert!(
        !text.to_lowercase().contains("refusing to render hooks"),
        "a refusal printed above a zero exit — nothing was going to be \
         written, so nothing was withheld:\n{text}"
    );
    assert!(
        settings_at(&home, &proj, "project").contains(HOOK_CMD),
        "fixture: the reviewed hook must still be on disk:\n{}",
        settings_at(&home, &proj, "project")
    );
}

/// The control, and the half that must NOT change: the same stale project, one
/// hook further on, so the plan would now deliver bytes. That still refuses,
/// still counts as blocked, and still exits nonzero.
#[test]
fn the_same_stale_project_still_refuses_once_the_plan_would_write() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj, toml) = delivered_then_stale(tmp.path());
    fs::write(
        proj.join(".agentstack/agentstack.toml"),
        format!("{toml}{}", hook_block("second-stage", APPENDED_CMD)),
    )
    .unwrap();

    let (text, ok) = run(&["apply", "--write"], &home, &proj);
    assert_refused(&text, ok, "project");
    let settings = settings_at(&home, &proj, "project");
    assert!(
        !settings.contains(APPENDED_CMD),
        "a hook appended on stale consent was delivered:\n{settings}"
    );
    assert!(
        settings.contains(HOOK_CMD),
        "the refusal must not disturb the already-reviewed hook:\n{settings}"
    );
}

// ------------------------------------------------------- the other direction

/// The machine's own manifest is the personal layer, not a project: the
/// zero-files bridge refuses to discover it, so no `trust` command can ever
/// reach it. Gating it on trust would make machine-level hooks permanently
/// undeliverable — a gate nobody can satisfy is a broken feature, not a
/// stronger one — so the exemption is deliberate and witnessed here.
#[test]
fn the_machine_manifests_own_hooks_are_not_gated_on_a_project_it_has_no_way_to_trust() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let machine = home.join(".agentstack");
    fs::create_dir_all(&machine).unwrap();
    fs::write(
        machine.join("agentstack.toml"),
        manifest_with(&hook_block("mine", HOOK_CMD)),
    )
    .unwrap();

    let dir = machine.display().to_string();
    let (text, ok) = run(
        &["--manifest-dir", &dir, "apply", "--write"],
        &home,
        tmp.path(),
    );
    assert!(ok, "the machine manifest's own apply failed:\n{text}");
    assert!(
        fs::read_to_string(home.join(".claude/settings.json"))
            .unwrap_or_default()
            .contains(HOOK_CMD),
        "the machine layer's own hook was gated on a project that cannot exist:\n{text}"
    );
}
