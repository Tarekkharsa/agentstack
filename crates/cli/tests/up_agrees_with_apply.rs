// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! `up` and `apply --write` are one operation, so they may not disagree about
//! whether it succeeded.
//!
//! `up`'s render step IS `apply --write` — it calls
//! [`agentstack::commands::apply::write_quiet`] and owns no writing path of its
//! own. The two therefore make the same claim about the same project, and a
//! script that reads one exit code must not get a different verdict from the
//! other.
//!
//! The state pinned here is the one where they came apart. Every capability the
//! project has routes to the live lane, and no bridge is registered anywhere:
//! nothing reaches any tool, and nothing will until a second command runs.
//! `apply --write` calls that a refused delivery and exits 1 — the reasoning is
//! in its source: exit 0 would be the same false success its validation gate
//! already refuses to give. `up` ran the identical render, printed the identical
//! refusal, and exited 0 — and `up` is the documented new-machine command, so a
//! CI job that runs it and reads success believes an environment was set up when
//! nothing was delivered at all.
//!
//! Two witnesses, and the second is what keeps the first from being satisfied by
//! a command that simply always fails:
//!
//! 1. On the refused-delivery state the two exit codes are EQUAL and nonzero,
//!    and both name a way forward. A nonzero exit with no next step would be a
//!    worse product than the bug.
//! 2. On a project that delivers something — the same manifest plus one setting
//!    for the rendered lane — both still exit 0. Live routing on its own is not
//!    a failure; delivering nothing is.
//!
//! # The rest of `up`'s transcript is held to the same rule
//!
//! The render was not the only place `up` reported success it had not earned,
//! and the other two are here because they are the same defect on the same
//! screen — a surface claiming an outcome, or blaming the user for our failure:
//!
//! 3. **The lock verification.** `install --locked` is the step that makes the
//!    environment the reviewed one, and `up`'s own source calls it "the one step
//!    of `up` that must not be best-effort". It printed a yellow "could not
//!    verify against lock" and exited 0 anyway. Witness + control below.
//! 4. **The harness detection.** When the adapter registry failed to load, `up`
//!    printed the sentence it prints for a machine with no CLI installed —
//!    "none — install a supported CLI" — so our internal failure was reported as
//!    the user's missing software. Witness + control below.

use std::fs;
use std::path::Path;
use std::process::Command;

/// Everything this project has is an MCP server on an MCP-capable harness: the
/// planner routes it live, and with no bridge registered nothing is served.
/// There is no instruction, setting or hook, so the rendered lane has no content
/// of its own to carry either.
const LIVE_ONLY: &str = "version = 1\n\
                         [targets]\n\
                         default = [\"claude-code\"]\n\
                         [servers.docs]\n\
                         type = \"stdio\"\n\
                         command = \"echo\"\n";

/// The control: the same live-routed server, plus one declaration that really
/// does land on disk. The routing is identical — only the delivery differs.
const LIVE_PLUS_RENDERED: &str = "version = 1\n\
                                  [targets]\n\
                                  default = [\"claude-code\"]\n\
                                  [servers.docs]\n\
                                  type = \"stdio\"\n\
                                  command = \"echo\"\n\
                                  [settings.claude-code]\n\
                                  model = \"opus\"\n";

/// The one recovery command `apply` names for this state. Both surfaces must
/// carry it, or the exit code is all the user gets.
const CONNECT: &str = "agentstack more gateway connect --all --write";

struct Run {
    text: String,
    code: i32,
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
        text: format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
        // A signal death has no code and would otherwise silently read as a
        // pass through `!= 0`.
        code: out.status.code().expect("the process must exit normally"),
    }
}

/// A trusted, locked checkout carrying `manifest`, in its own HOME so no other
/// machine state can decide the outcome. Trust is granted because an untrusted
/// project fails for a different reason entirely, which would make the exit
/// codes below agree for reasons that have nothing to do with delivery.
fn project(root: &Path, name: &str, manifest: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let home = root.join(format!("{name}-home"));
    let proj = root.join(name);
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(proj.join(".agentstack")).unwrap();
    fs::write(proj.join(".agentstack/agentstack.toml"), manifest).unwrap();

    let locked = run(&["lock", "--write"], &home, &proj);
    assert_eq!(locked.code, 0, "fixture: lock failed:\n{}", locked.text);
    let preview = run(&["trust", "--preview"], &home, &proj);
    let json: serde_json::Value =
        serde_json::from_str(&preview.text).expect("`trust --preview` must be JSON");
    let digest = json["surface_digest"]
        .as_str()
        .expect("the preview must carry a surface digest")
        .to_string();
    let granted = run(&["trust", "--yes", "--consented", &digest], &home, &proj);
    assert_eq!(granted.code, 0, "fixture: trust failed:\n{}", granted.text);
    (home, proj)
}

// ------------------------------------------------------- 1. they must agree

/// The defect, stated as a property: on the identical project, in the identical
/// state, the two commands report the identical verdict — and it is a failure,
/// because nothing was delivered.
///
/// Each command gets its own copy of the fixture so neither can be reading a
/// state the other left behind.
#[test]
fn up_and_apply_agree_that_delivering_nothing_is_a_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let (a_home, a_proj) = project(tmp.path(), "for-apply", LIVE_ONLY);
    let (u_home, u_proj) = project(tmp.path(), "for-up", LIVE_ONLY);

    let applied = run(&["apply", "--write"], &a_home, &a_proj);
    let upped = run(&["up", "--write"], &u_home, &u_proj);

    // The premise: this is the refused-delivery state, not some other failure.
    assert!(
        applied.text.contains("nothing was delivered"),
        "fixture: apply must be refusing the delivery, or this test is measuring \
         something else:\n{}",
        applied.text
    );
    assert_eq!(
        applied.code, 1,
        "apply's own contract: a refused delivery exits 1:\n{}",
        applied.text
    );

    assert_eq!(
        upped.code, applied.code,
        "`up` runs apply's write and must not disagree with it about whether the \
         project was set up. apply exited {}, up exited {} — a CI job that runs \
         `up` on this project reads success over an environment where nothing \
         reached any tool.\n--- up ---\n{}\n--- apply ---\n{}",
        applied.code, upped.code, upped.text, applied.text
    );

    // An exit code alone strands the user. Both must name the way out — the
    // same one, since a single constant defines it.
    for (who, r) in [("apply", &applied), ("up", &upped)] {
        assert!(
            r.text.contains(CONNECT),
            "`{who}` failed without naming the command that answers it — a nonzero \
             exit with no next step is worse than the bug:\n{}",
            r.text
        );
        assert!(
            r.text
                .contains("agentstack more delivery render-locally --write"),
            "`{who}` must also name the override for a user who wants the files \
             anyway:\n{}",
            r.text
        );
    }
}

// ------------------------------------------------------------- 2. the control

/// The same live routing, one delivered setting: both exit 0.
///
/// Without this, the test above is satisfied by making `up` fail whenever a
/// capability goes live — which would fail every healthy MCP project on the
/// planet. The failure being agreed on is "nothing was delivered", never
/// "routing happened".
#[test]
fn a_project_that_delivers_something_still_exits_zero_from_both() {
    let tmp = tempfile::tempdir().unwrap();
    let (a_home, a_proj) = project(tmp.path(), "for-apply", LIVE_PLUS_RENDERED);
    let (u_home, u_proj) = project(tmp.path(), "for-up", LIVE_PLUS_RENDERED);

    let applied = run(&["apply", "--write"], &a_home, &a_proj);
    assert_eq!(
        applied.code, 0,
        "a project whose rendered lane has content delivered something:\n{}",
        applied.text
    );

    let upped = run(&["up", "--write"], &u_home, &u_proj);
    assert_eq!(
        upped.code, 0,
        "`up` must not have become a command that fails whenever anything routes \
         live:\n{}",
        upped.text
    );
    assert!(
        !upped.text.contains("nothing was delivered"),
        "fixture: this project delivers a setting, so the refusal must not \
         appear:\n{}",
        upped.text
    );
}

// ------------------------------------------- 3. the lock verification's exit

/// A project whose pinned skill body has moved on disk since the lockfile was
/// written. `install --locked` refuses to re-pin it — that refusal is the whole
/// point of `--locked` — so this is the state the verification exists to catch.
///
/// The settings key is deliberate: it gives the rendered lane real content, so
/// the render SUCCEEDS and the only thing that can make `up` exit nonzero is the
/// lock verification itself. Without it this test would pass on the render's
/// exit code and say nothing about the lock at all.
fn skewed_from_its_lock(root: &Path, name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let src = root.join(format!("{name}-skill"));
    fs::create_dir_all(&src).unwrap();
    fs::write(
        src.join("SKILL.md"),
        "---\nname: mine\ndescription: a skill\n---\nbody one\n",
    )
    .unwrap();
    let manifest = format!(
        "version = 1\n\
         [targets]\n\
         default = [\"claude-code\"]\n\
         [skills.mine]\n\
         path = \"{}\"\n\
         [settings.claude-code]\n\
         model = \"opus\"\n",
        src.display()
    );
    let (home, proj) = project(root, name, &manifest);
    // Break the pin AFTER the lockfile and the grant exist: the bytes on disk
    // are no longer the bytes that were reviewed.
    fs::write(
        src.join("SKILL.md"),
        "---\nname: mine\ndescription: a skill\n---\nbody two, written after the pin\n",
    )
    .unwrap();
    (home, proj)
}

/// The defect: `up` verified nothing against the lock and reported success.
///
/// `--locked` is the difference between "you got what was reviewed" and "you got
/// whatever was on disk this morning". `up` printed a yellow line saying it
/// could not verify, four lines below its own comment calling this the one step
/// that must not be best-effort, and exited 0 — so a CI job reading that exit
/// code believed an environment had been checked against `agentstack.lock` when
/// nothing had checked it.
///
/// Continuing and succeeding are separable, and both halves are asserted:
/// `up` still renders and still prints its closing next step, and it still
/// exits nonzero.
#[test]
fn up_does_not_report_success_over_a_lock_it_could_not_verify() {
    let tmp = tempfile::tempdir().unwrap();
    let (i_home, i_proj) = skewed_from_its_lock(tmp.path(), "for-install");
    let (u_home, u_proj) = skewed_from_its_lock(tmp.path(), "for-up");

    // The premise: this is the unverifiable state, not some other failure.
    let installed = run(&["install", "--locked"], &i_home, &i_proj);
    assert_eq!(
        installed.code, 1,
        "fixture: `install --locked` must be refusing here, or this test is \
         measuring something else:\n{}",
        installed.text
    );

    let upped = run(&["up", "--write"], &u_home, &u_proj);
    assert_ne!(
        upped.code, 0,
        "`up` delegates the verification to `install --locked`, which exited {} — \
         reporting 0 here tells a CI job the environment was verified against the \
         lock when nothing verified it:\n{}",
        installed.code, upped.text
    );

    // The failure is not allowed to cost the user the rest of the command.
    assert!(
        upped.text.contains("next:"),
        "`up` must still finish its transcript — an exit code with no way forward \
         is worse than none:\n{}",
        upped.text
    );
    assert!(
        upped.text.contains("wrote 1 setting"),
        "`up` must still attempt the render: a user on a new machine with one \
         error and no configured CLI at all is worse off than before:\n{}",
        upped.text
    );
    assert!(
        !upped.text.contains("verified against lock —") || upped.text.contains("NOT verified"),
        "the screen must not soften an unverified lock into a passing claim:\n{}",
        upped.text
    );
}

/// The control: a lock that DOES verify still exits 0.
///
/// Without this, the witness above is satisfied by making `up` fail whenever a
/// skill is declared — which would fail every healthy pinned project. What is
/// being reported is an unverifiable lock, not the presence of a lock.
#[test]
fn a_project_whose_lock_verifies_still_exits_zero() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("intact-skill");
    fs::create_dir_all(&src).unwrap();
    fs::write(
        src.join("SKILL.md"),
        "---\nname: mine\ndescription: a skill\n---\nbody one\n",
    )
    .unwrap();
    let manifest = format!(
        "version = 1\n\
         [targets]\n\
         default = [\"claude-code\"]\n\
         [skills.mine]\n\
         path = \"{}\"\n\
         [settings.claude-code]\n\
         model = \"opus\"\n",
        src.display()
    );
    let (home, proj) = project(tmp.path(), "intact", &manifest);

    let upped = run(&["up", "--write"], &home, &proj);
    assert_eq!(
        upped.code, 0,
        "the pinned body is untouched, so the verification passes and `up` has \
         nothing to report:\n{}",
        upped.text
    );
    assert!(
        upped.text.contains("verified against lock"),
        "and it must still make the green claim when something backed it:\n{}",
        upped.text
    );
}

// ------------------------------------------ 4. whose failure is "no harness"?

/// The defect: our failure, reported as the user's missing software.
///
/// `detected_harnesses` swallowed a failed `Registry::load()` into an empty
/// list, and the empty list prints "none — install a supported CLI". Measured
/// against a `~/.agentstack/adapters` this process cannot read, the line was
/// byte-identical to the one a clean machine gets — so a user with every
/// supported CLI installed was told to go and install one.
///
/// The environment guard is not a skip in disguise: it fires only where the
/// sandbox cannot express an unreadable directory at all (a process running as
/// root), and it says so rather than passing quietly.
#[test]
fn a_registry_that_will_not_load_is_not_reported_as_no_cli_installed() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = project(
        tmp.path(),
        "blamed",
        "version = 1\n[settings.claude-code]\nmodel = \"opus\"\n",
    );

    let adapters = home.join(".agentstack/adapters");
    fs::create_dir_all(&adapters).unwrap();
    let mut perms = fs::metadata(&adapters).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o000);
    fs::set_permissions(&adapters, perms).unwrap();
    let unreadable = fs::read_dir(&adapters).is_err();

    let upped = run(&["up", "--write"], &home, &proj);

    // Restore before any assertion, so a failure cannot leave the tempdir
    // undeletable.
    let mut back = fs::metadata(&adapters).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut back, 0o755);
    fs::set_permissions(&adapters, back).unwrap();

    if !unreadable {
        eprintln!(
            "skipped: this process can read a 0o000 directory (running as root?), \
             so the registry failure cannot be staged here"
        );
        return;
    }

    assert!(
        !upped.text.contains("install a supported CLI"),
        "the registry did not load, so nothing looked for a CLI — telling the user \
         to install one blames them for our failure:\n{}",
        upped.text
    );
    assert!(
        upped.text.contains("adapter registry did not load"),
        "`up` must say whose failure this was:\n{}",
        upped.text
    );
}

/// The control: a machine with genuinely no CLI installed still gets the
/// sentence written for it.
///
/// Without this, the witness above is satisfied by deleting the "install a
/// supported CLI" line outright — which would take away the one actionable
/// thing said to a user on a bare machine.
#[test]
fn a_machine_with_no_cli_installed_still_hears_that_and_only_that() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = project(
        tmp.path(),
        "bare",
        "version = 1\n[settings.claude-code]\nmodel = \"opus\"\n",
    );

    let upped = run(&["up", "--write"], &home, &proj);
    assert!(
        upped.text.contains("install a supported CLI"),
        "an empty `PATH` and an empty HOME is the no-CLI machine, and it keeps \
         its own sentence:\n{}",
        upped.text
    );
    assert!(
        !upped.text.contains("adapter registry did not load"),
        "and it must not be told our registry failed when it loaded fine:\n{}",
        upped.text
    );
}
