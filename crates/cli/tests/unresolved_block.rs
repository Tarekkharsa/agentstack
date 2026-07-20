// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Finding 1 (project review): an unresolved `${REF}` must never reach a live
//! config write. `apply --write` blocks the target by default and only writes
//! when `--allow-unresolved` is passed.

use std::fs;
use std::sync::Mutex;

use agentstack::cli::{ApplyArgs, DoctorArgs};
use agentstack::commands::{apply, doctor};
use agentstack::scope::Scope;

// Both tests mutate the process-global HOME; serialize them.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// A manifest with one server whose header needs a secret that does not resolve.
fn write_unresolved_manifest(proj: &std::path::Path) {
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [servers.kibana]\ntype = \"http\"\nurl = \"https://k/mcp\"\n\
         headers = { Authorization = \"Bearer ${NOPE_TOKEN}\" }\n",
    )
    .unwrap();
}

fn args(write: bool, allow_unresolved: bool) -> ApplyArgs {
    ApplyArgs {
        targets: vec!["claude-code".into()],
        profile: None,
        dry_run: false,
        write,
        scope: Some(Scope::Global),
        allow_unresolved,
        prune_foreign: false,
        no_gitignore: true,
    }
}

#[test]
fn unresolved_secret_blocks_write_unless_allowed() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    write_unresolved_manifest(&proj);

    let claude_cfg = home.join(".claude.json");

    // Default: write is blocked → the live config is never created, and the
    // apply itself errors (nonzero exit) so scripts see the blockage.
    let err = apply::run(&args(true, false), Some(&proj))
        .expect_err("a fully blocked apply --write must be an error");
    assert!(
        err.to_string().contains("blocked"),
        "error should name the blockage: {err}"
    );
    assert!(
        !claude_cfg.exists(),
        "unresolved secret must block the write — but {} was written",
        claude_cfg.display()
    );

    // Escape hatch: --allow-unresolved writes (leaving the ${REF} in place).
    apply::run(&args(true, true), Some(&proj)).unwrap();
    let written = fs::read_to_string(&claude_cfg).unwrap();
    assert!(written.contains("kibana"));
    assert!(
        written.contains("${NOPE_TOKEN}"),
        "the ref is left verbatim, never blanked"
    );
}

/// `doctor --fix` has no `--allow-unresolved`, so it must refuse to write a
/// drifted target whose secret doesn't resolve — never leak a `${REF}`.
#[test]
fn doctor_fix_refuses_unresolved_secret() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    write_unresolved_manifest(&proj);

    let claude_cfg = home.join(".claude.json");
    let dargs = DoctorArgs {
        ci: false,
        live: false,
        fix: true,
        deep: false,
        all: false,
        json: false,
        skip_drift: false,
    };
    doctor::run(&dargs, Some(&proj)).unwrap();
    assert!(
        !claude_cfg.exists(),
        "doctor --fix must not write an unresolved secret — but {} was written",
        claude_cfg.display()
    );
}

/// The `--write` summary must count targets actually written, not targets with
/// pending changes: an apply fully blocked by unresolved secrets must not end
/// with "Applied to N target(s)" while every write above it was refused.
/// Runs the real binary (own HOME) so the printed summary itself is asserted.
#[test]
fn blocked_write_summary_counts_written_targets() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\", \"cursor\"]\n\
         [servers.kibana]\ntype = \"http\"\nurl = \"https://k/mcp\"\n\
         headers = { Authorization = \"Bearer ${SOME_UNSET_SECRET}\" }\n",
    )
    .unwrap();

    let run = |args: &[&str]| {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_agentstack"))
            .args(args)
            .current_dir(&proj)
            .env("HOME", &home)
            .env("AGENTSTACK_HOME", home.join(".agentstack"))
            .env_remove("SOME_UNSET_SECRET")
            .output()
            .unwrap();
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
        )
    };

    let (ok, stdout) = run(&["apply", "--write", "--no-gitignore"]);
    assert!(
        !stdout.contains("Applied to"),
        "a fully blocked apply must not claim success:\n{stdout}"
    );
    assert!(
        stdout.contains("Wrote 0 of 2 target(s); 2 blocked"),
        "summary should count only targets actually written:\n{stdout}"
    );
    assert!(!ok, "a blocked apply --write must exit nonzero");
    assert!(
        !home.join(".claude.json").exists(),
        "blocked apply must not write the live config"
    );

    // Nothing was written, so a follow-up dry-run still shows both pending.
    let (dry_ok, dry) = run(&["apply", "--dry-run"]);
    assert!(
        dry.contains("2 target(s) would change"),
        "blocked targets must still show as pending:\n{dry}"
    );
    assert!(
        dry_ok,
        "a dry run never blocks a write, so it still exits 0"
    );
}

/// A target can be written AND blocked in the same pass — its instructions
/// land while the server config is refused over an unresolved secret. The old
/// summary counted it in both columns ("1 of 1 written — 1 blocked"); the
/// split summary must count it once (blocked, partially written) and the
/// process must exit nonzero. Runs the real binary to assert both.
#[test]
fn partially_blocked_apply_counts_once_and_exits_nonzero() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let proj = tmp.path().join("proj");
    fs::create_dir_all(proj.join("instructions")).unwrap();
    fs::write(proj.join("instructions/house.md"), "House rule one.\n").unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [servers.demo]\ntype = \"http\"\nurl = \"https://d/mcp\"\n\
         headers = { Authorization = \"Bearer ${DEMO_TOKEN}\" }\n\
         [instructions.house]\npath = \"./instructions/house.md\"\n",
    )
    .unwrap();

    // --scope global: the assertions read ~/.claude/CLAUDE.md, and a repo
    // manifest defaults to project scope.
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(["apply", "--write", "--no-gitignore", "--scope", "global"])
        .current_dir(&proj)
        .env("HOME", &home)
        .env("AGENTSTACK_HOME", home.join(".agentstack"))
        .env_remove("DEMO_TOKEN")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    // The instructions half of the target landed; the server half was blocked.
    assert!(
        home.join(".claude/CLAUDE.md").exists(),
        "instructions should still be written:\n{stdout}"
    );
    assert!(
        !home.join(".claude.json").exists(),
        "the unresolved server config must not be written:\n{stdout}"
    );
    // One target, counted once: blocked (with a partial note), not "written".
    assert!(
        stdout.contains("Wrote 0 of 1 target(s); 1 blocked"),
        "a blocked target must not also count as written:\n{stdout}"
    );
    assert!(
        stdout.contains("(1 partially written)"),
        "the summary should note the partial write:\n{stdout}"
    );
    assert!(
        !out.status.success(),
        "an apply with blocked writes must exit nonzero"
    );
}

/// `use --write` with a blocked target must exit with an error (not a green
/// "activated on 0 target(s)") so scripts can't mistake a blocked activation
/// for success — and the live config must stay untouched.
#[test]
fn use_write_errors_when_all_targets_blocked() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [servers.kibana]\ntype = \"http\"\nurl = \"https://k/mcp\"\n\
         headers = { Authorization = \"Bearer ${NOPE_TOKEN}\" }\n\
         [profiles.p]\nservers = [\"kibana\"]\nskills = []\n",
    )
    .unwrap();

    let uargs = agentstack::cli::UseArgs {
        profile: Some("p".into()),
        targets: vec![],
        scope: Some(Scope::Global),
        write: true,
        allow_unresolved: false,
        prune_foreign: false,
        no_gitignore: true,
    };
    let err = agentstack::commands::use_profile::run(&uargs, Some(&proj))
        .expect_err("blocked activation must be an error");
    assert!(
        err.to_string().contains("blocked"),
        "error should name the blockage: {err}"
    );
    assert!(
        !home.join(".claude.json").exists(),
        "blocked use must not write the live config"
    );
    // A fully-blocked activation is a no-op on disk, so it must NOT leave a
    // lockfile behind: a lock alone flips the project's inferred delivery mode
    // to "clean-at-rest" (overview.rs P4), teaching the wrong workflow to a
    // static-mode user whose activation merely failed.
    assert!(
        !proj.join("agentstack.lock").exists(),
        "a fully-blocked use --write must not leave a phantom lockfile behind"
    );
}

/// A partially-blocked activation still writes its skills, so it genuinely
/// activated — the lockfile must be pinned even though a server target was
/// refused over an unresolved secret. (The other half of the atomicity rule:
/// only a *total* failure skips the lock.)
#[test]
fn partially_blocked_use_still_pins_the_lock() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    let proj = tmp.path().join("proj");
    fs::create_dir_all(proj.join("skills/demo")).unwrap();
    fs::write(proj.join("skills/demo/SKILL.md"), "# demo\n").unwrap();
    // The server is blocked (unresolved secret) but the profile also carries a
    // materializable skill — so at least one artifact lands and the activation
    // is a partial success, not a total failure.
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [servers.kibana]\ntype = \"http\"\nurl = \"https://k/mcp\"\n\
         headers = { Authorization = \"Bearer ${NOPE_TOKEN}\" }\n\
         [skills.demo]\npath = \"./skills/demo\"\n\
         [profiles.p]\nservers = [\"kibana\"]\nskills = [\"demo\"]\n",
    )
    .unwrap();

    let uargs = agentstack::cli::UseArgs {
        profile: Some("p".into()),
        targets: vec![],
        scope: Some(Scope::Global),
        write: true,
        allow_unresolved: false,
        prune_foreign: false,
        no_gitignore: true,
    };
    // The server target is still blocked, so the activation exits nonzero...
    let err = agentstack::commands::use_profile::run(&uargs, Some(&proj))
        .expect_err("a blocked server target must still error");
    assert!(err.to_string().contains("blocked"), "{err}");
    // ...but the skill genuinely materialized, so the lock IS pinned.
    assert!(
        proj.join("agentstack.lock").exists(),
        "a partially-successful activation must still pin the lockfile"
    );
}
