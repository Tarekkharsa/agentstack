// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Phase 4 — `agentstack up` on a fresh machine.
//!
//! The scenario, literally: an isolated HOME that has never seen agentstack,
//! holding a checkout with a manifest, a lock, and a server whose credential is
//! a `${REF}` nothing on this machine can resolve. That is not an edge case for
//! `up` — it is the ordinary first run, and everything asserted here is about
//! it telling the truth about that state rather than reporting success.
//!
//! Four properties:
//!
//! 1. It reports the transcript shape: harnesses found, environment against the
//!    lock, render, secrets, one next step.
//! 2. An unresolved `${REF}` leaves its server out, fail-closed, and is named
//!    with the exact command that fixes it.
//! 3. The closing line comes from `doctor`'s next-action seam, and is TRUE in
//!    the state `up` actually leaves behind — which, on a fresh machine with an
//!    unreviewed checkout, is not "ready".
//! 4. It composes. There is no filesystem write in `up` itself: every byte it
//!    changes is changed by the command that already owned that write.

use std::fs;
use std::process::Command;

/// A checkout as it arrives on a new machine: a manifest with a server whose
/// token is a `${REF}`, and nothing trusted, locked, or rendered locally.
fn checkout(root: &std::path::Path) -> std::path::PathBuf {
    let proj = root.join("proj");
    fs::create_dir_all(proj.join(".agentstack")).unwrap();
    fs::write(
        proj.join(".agentstack/agentstack.toml"),
        r#"version = 1

[servers.web-search]
type = "stdio"
command = "echo"
env = { SEARCH_API_KEY = "${SEARCH_API_KEY}" }

[servers.notes]
type = "stdio"
command = "echo"

[profiles.research]
servers = ["web-search", "notes"]
"#,
    )
    .unwrap();
    proj
}

/// Run the real binary with a HOME that has nothing in it. Returns
/// (stdout+stderr, success) — `up` is allowed to finish non-zero, and what it
/// SAYS is the contract either way.
fn up(home: &std::path::Path, proj: &std::path::Path) -> (String, bool) {
    let exe = env!("CARGO_BIN_EXE_agentstack");
    let out = Command::new(exe)
        .args(["up", "--write", "--manifest-dir"])
        .arg(proj)
        .env("HOME", home)
        .env("AGENTSTACK_HOME", home.join(".agentstack"))
        // Isolate from the developer's real machine: no inherited secret can
        // resolve the ref this test is about.
        .env_remove("SEARCH_API_KEY")
        .env("NO_COLOR", "1")
        .output()
        .expect("the binary must run");
    let mut s = String::from_utf8_lossy(&out.stdout).to_string();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    (s, out.status.success())
}

fn fresh() -> (assert_fs::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let proj = checkout(tmp.path());
    (tmp, home, proj)
}

#[test]
fn library_flag_clones_the_personal_library_on_a_fresh_machine() {
    let (_tmp, home, proj) = fresh();
    let remote = proj.parent().unwrap().join("library-remote");
    fs::create_dir_all(&remote).unwrap();
    fs::write(remote.join("library.toml"), "version = 1\n").unwrap();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .arg("-C")
            .arg(&remote)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q"]);
    git(&["add", "library.toml"]);
    git(&[
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.com",
        "commit",
        "-qm",
        "library",
    ]);

    let exe = env!("CARGO_BIN_EXE_agentstack");
    let out = Command::new(exe)
        .args(["up", "--write", "--library"])
        .arg(format!("file://{}", remote.display()))
        .args(["--manifest-dir"])
        .arg(&proj)
        .env("HOME", &home)
        .env("AGENTSTACK_HOME", home.join(".agentstack"))
        .env_remove("SEARCH_API_KEY")
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    assert!(text.contains("cloned library"), "{text}");
    assert_eq!(
        fs::read_to_string(home.join(".agentstack/lib/library.toml")).unwrap(),
        "version = 1\n"
    );

    // `--library` records the URL before the pull proves it, so one typo
    // repoints the library and every later `agentstack up` fails here. The
    // refusal must therefore name a command that RUNS in this state: telling
    // the user to "re-run `agentstack up`" is naming the command that just
    // failed, with nothing in between that repoints the remote.
    let bad = Command::new(exe)
        .args(["up", "--write", "--library", "file:///nonexistent/nope.git"])
        .args(["--manifest-dir"])
        .arg(&proj)
        .env("HOME", &home)
        .env("AGENTSTACK_HOME", home.join(".agentstack"))
        .env_remove("SEARCH_API_KEY")
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&bad.stdout),
        String::from_utf8_lossy(&bad.stderr)
    );
    assert!(
        !bad.status.success(),
        "an unreachable remote must fail:\n{text}"
    );
    assert!(
        text.contains("agentstack up --library <url>"),
        "the refusal must name the way to repoint the remote:\n{text}"
    );
    assert!(
        text.contains("/nonexistent/nope.git"),
        "…and the remote it actually tried, so the typo is visible:\n{text}"
    );
}

#[test]
fn bare_up_pulls_an_already_linked_library_without_pushing() {
    let (_tmp, home, proj) = fresh();
    let remote = proj.parent().unwrap().join("library-remote-pull");
    fs::create_dir_all(&remote).unwrap();
    fs::write(remote.join("library.toml"), "version = 1\n").unwrap();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .arg("-C")
            .arg(&remote)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q"]);
    git(&["add", "library.toml"]);
    git(&[
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.com",
        "commit",
        "-qm",
        "first",
    ]);

    let exe = env!("CARGO_BIN_EXE_agentstack");
    let run = |extra: &[&str]| {
        Command::new(exe)
            .arg("up")
            .arg("--write")
            .args(extra)
            .args(["--manifest-dir"])
            .arg(&proj)
            .env("HOME", &home)
            .env("AGENTSTACK_HOME", home.join(".agentstack"))
            .env_remove("SEARCH_API_KEY")
            .env("NO_COLOR", "1")
            .output()
            .unwrap()
    };
    let remote_url = format!("file://{}", remote.display());
    let _ = run(&["--library", &remote_url]);

    fs::create_dir_all(remote.join("skills/shared")).unwrap();
    fs::write(
        remote.join("skills/shared/SKILL.md"),
        "---\nname: shared\ndescription: shared\n---\nbody\n",
    )
    .unwrap();
    git(&["add", "skills/shared/SKILL.md"]);
    git(&[
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.com",
        "commit",
        "-qm",
        "second",
    ]);
    let remote_head_before = {
        let out = Command::new("git")
            .arg("-C")
            .arg(&remote)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };

    let out = run(&[]);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(text.contains("pulled shared library"), "{text}");
    assert!(home.join(".agentstack/lib/skills/shared/SKILL.md").exists());
    let remote_head_after = {
        let out = Command::new("git")
            .arg("-C")
            .arg(&remote)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    assert_eq!(remote_head_before, remote_head_after, "up must never push");
}

/// Property 1 — the Moment 9 shape. Not a snapshot of exact bytes (that would
/// break on every wording change and teach the next reader to update it
/// without thinking), but the sections in order: what was found, what the
/// environment is, the render, and one closing step.
#[test]
fn up_reports_what_it_found_what_it_verified_and_what_is_left() {
    let (_tmp, home, proj) = fresh();
    let (out, _ok) = up(&home, &proj);

    for marker in ["found harnesses", "your environment", "rendered", "next:"] {
        assert!(
            out.contains(marker),
            "the transcript is missing '{marker}':\n{out}"
        );
    }
    // The environment line must state the shape it verified, not just claim a
    // verification — "2 servers" is the evidence, "verified" alone is a claim.
    assert!(
        out.contains("2 servers"),
        "the environment line must say what was counted:\n{out}"
    );
    // Order matters: a user reads top to bottom and the closing step must be
    // last, after everything it is a conclusion about.
    let next_at = out.find("next:").unwrap();
    let render_at = out.find("rendered").unwrap();
    assert!(render_at < next_at, "the next step must come last:\n{out}");
}

#[test]
fn bare_up_is_a_safe_preview_and_names_write() {
    let (_tmp, home, proj) = fresh();
    let exe = env!("CARGO_BIN_EXE_agentstack");
    let out = Command::new(exe)
        .args(["up", "--manifest-dir"])
        .arg(&proj)
        .env("HOME", &home)
        .env("AGENTSTACK_HOME", home.join(".agentstack"))
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(text.contains("Nothing written"), "{text}");
    assert!(text.contains("--write"), "{text}");
    assert!(!home.join(".agentstack/changes").exists());
    assert!(!proj.join(".mcp.json").exists());
}

#[test]
fn json_mode_is_one_parseable_supervisor_readiness_document() {
    let (_tmp, home, proj) = fresh();
    let exe = env!("CARGO_BIN_EXE_agentstack");
    let out = Command::new(exe)
        .args(["up", "--json", "--write", "--manifest-dir"])
        .arg(&proj)
        .env("HOME", &home)
        .env("AGENTSTACK_HOME", home.join(".agentstack"))
        .env_remove("SEARCH_API_KEY")
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["command"], "up");
    assert!(json["success"].is_boolean());
    assert_eq!(json["applied"], true);
    assert!(json["state"]["readiness"].is_string());
    assert!(json["state"]["locked"].is_boolean());
    assert!(json["state"]["activation"].is_string());
    assert!(
        String::from_utf8_lossy(&out.stdout)
            .trim_start()
            .starts_with('{'),
        "stdout must contain JSON only"
    );
}

/// The class guard, applied to `up`'s own output: no green line may claim a
/// pass over nothing examined.
///
/// `install --locked` verifies pinned SKILL sources. A manifest with no skills
/// gives it nothing to check — and the first draft of this command printed
/// "verified against lock" anyway, over an environment where the only thing
/// that had been verified was that there was nothing to verify. That is the
/// exact shape P3.1 removed from `doctor` and P3.7 removed from the status
/// contract, reintroduced by a new command a phase later, which is why it is
/// pinned here rather than trusted to review.
#[test]
fn up_does_not_claim_a_verification_it_did_not_perform() {
    let (_tmp, home, proj) = fresh();
    let (out, _ok) = up(&home, &proj);

    // The fixture declares no skills.
    assert!(
        !out.contains("verified against lock"),
        "nothing was verified — the claim must not appear:\n{out}"
    );
    assert!(
        out.contains("no pinned skill sources to verify"),
        "and the absence must be stated in words, not left blank:\n{out}"
    );
}

/// Property 2 — the unresolved `${REF}`. It is named, the exact command is
/// given, and the fail-closed consequence is stated in words. This is the one
/// piece of work that genuinely cannot be carried in a manifest (invariant 5),
/// so it is the one thing `up` must hand back rather than solve.
#[test]
fn an_unresolved_ref_is_named_with_its_command_and_its_consequence() {
    let (_tmp, home, proj) = fresh();
    let (out, _ok) = up(&home, &proj);

    assert!(
        out.contains("secrets"),
        "the secrets block must appear when a ref cannot resolve:\n{out}"
    );
    assert!(
        out.contains("SEARCH_API_KEY"),
        "the ref must be named:\n{out}"
    );
    assert!(
        out.contains("agentstack secret set SEARCH_API_KEY"),
        "the exact repair command must be given, not the verb alone:\n{out}"
    );
    assert!(
        out.contains("fail closed"),
        "the fail-closed consequence must be stated:\n{out}"
    );
    // And stated as it ACTUALLY behaves. The target's server config is held
    // back whole — a ref-less server in the same project is held back too,
    // while instructions and settings (which need no secret) still render.
    // `up` must not describe the per-server pausing the product does not do,
    // however much nicer it reads.
    assert!(
        out.contains("held back whole"),
        "the copy must describe the shipped whole-target rule, not the relaxed \
         behaviour the vision transcript implies:\n{out}"
    );
    assert!(
        !out.contains("stay paused"),
        "per-server pausing language would over-promise against the documented \
         fail-closed rule:\n{out}"
    );
    // The resolvable server must NOT be dragged down with it: a ref-less server
    // has nothing to wait for.
    assert!(
        !out.contains("${notes}"),
        "a server with no refs must not appear in the secrets block:\n{out}"
    );
}

/// A deliberate non-fix, recorded so it is not rediscovered as a bug.
///
/// **This test is written to fail the day someone fixes the thing it
/// describes.** That is the point: it passes today, documenting real
/// behaviour, and turns red exactly when the follow-up lands and needs
/// attention here.
///
/// Moment 9's transcript implies per-SERVER pausing: the servers whose refs are
/// missing stay paused, and the rest are rendered. `apply` blocks per TARGET —
/// one unresolved ref anywhere in the selection and the whole CLI config goes
/// unwritten, including servers that reference no secret at all. On a fresh
/// machine that is the difference between "three of four servers work while you
/// find your API key" and "nothing works".
///
/// `up` does not fix it, and neither should a passing change. Two reasons, the
/// second stronger than the first:
///
/// 1. It means changing which servers `render` writes — a writing-path change,
///    and `up`'s design constraint is that it composes and owns no writing path.
/// 2. **The whole-target block is a documented fail-closed rule**, not an
///    oversight: writes stay blocked while any `${REF}` in the selection is
///    unresolved (`docs/ARCHITECTURE.md`, render path). Per-server rendering is
///    therefore a deliberate RELAXATION of a fail-closed boundary.
///
/// **The bar for flipping this**, so whoever does knows it up front: a
/// line-by-line review with the original rule's rationale in front of the
/// reviewer, answering the consent question it encodes — is a
/// partially-rendered config a thing the user agreed to, and can they tell a
/// partial render from a complete one? Not a mid-phase edit, and not something
/// to infer permission for from this test going red.
///
/// Note that the vision's Moment 9 transcript implies the relaxed behaviour.
/// The vision bends; the shipped rule does not, until reviewed. `up`'s copy
/// describes what actually happens.
#[test]
fn a_refless_server_is_still_blocked_by_another_servers_missing_ref() {
    let (_tmp, home, proj) = fresh();
    let (_out, _ok) = up(&home, &proj);

    // `notes` references no secret. `web-search` does, and it is missing.
    let rendered = proj.join(".mcp.json");
    assert!(
        !rendered.exists(),
        "PER-SERVER RENDERING NOW WORKS — apply no longer blocks a whole target \
         over another server's missing ref. That is the improvement this test \
         was recording the absence of: delete this test, and update `up`'s docs \
         and Moment 9's acceptance note, which both currently say the coarse \
         behaviour is what ships."
    );
}

/// Property 3 — the closing line is the seam's, and it is true here.
///
/// This is the reason P3.7 had to land before `up`: on a fresh machine `up`
/// characteristically ends in an unfinished state, and a closing "✓ ready"
/// invented locally would be the false-ready bug reborn at the exact moment a
/// user is least able to catch it. The assertion is that the line agrees with
/// what `doctor` independently says — not that it says any particular thing.
#[test]
fn the_closing_line_is_the_doctors_next_action_not_a_verdict_of_its_own() {
    let (_tmp, home, proj) = fresh();
    let (out, _ok) = up(&home, &proj);

    let exe = env!("CARGO_BIN_EXE_agentstack");
    let doc = Command::new(exe)
        .args(["doctor", "--json", "--manifest-dir"])
        .arg(&proj)
        .env("HOME", &home)
        .env("AGENTSTACK_HOME", home.join(".agentstack"))
        .env_remove("SEARCH_API_KEY")
        .env("NO_COLOR", "1")
        .output()
        .expect("doctor must run");
    let json: serde_json::Value =
        serde_json::from_slice(&doc.stdout).expect("doctor --json must be JSON");
    let expected = json["next_action"].as_str().expect("the P3.1 seam");

    assert!(
        out.contains(expected),
        "up's closing line must BE the next-action seam's answer ({expected:?}), \
         not a summary of its own:\n{out}"
    );

    // And the state it left behind is honestly reported: nothing was reviewed
    // on this machine, so this is not a ready project, and neither surface
    // claims it is.
    assert_ne!(
        json["readiness"].as_str(),
        Some("ready"),
        "a fresh, unreviewed checkout is not ready — if this ever passes, `up` \
         is being allowed to declare victory over an untrusted project"
    );
    assert!(
        !out.contains("✓ ready"),
        "up must not mint its own readiness verdict:\n{out}"
    );
}

/// Property 4 — it composes; it does not write.
///
/// Stated structurally rather than by observation, because "I did not see a
/// write" is a fact about one run and this needs to be a property. A second
/// writing path is where the consent, undo, and gitignore work of the last
/// three phases would get quietly re-implemented wrong.
#[test]
fn up_owns_no_writing_path_of_its_own() {
    let src = include_str!("../src/commands/up.rs");
    for forbidden in [
        "fs::write",
        "fs::create_dir",
        "fs::remove",
        "File::create",
        "OpenOptions",
        "write_atomic",
    ] {
        assert!(
            !src.contains(forbidden),
            "`up` must compose existing commands, never write directly — found \
             {forbidden:?}"
        );
    }
    // And the two commands that DO own its writes are the ones it calls.
    assert!(
        src.contains("super::install::run") && src.contains("super::apply::write_quiet"),
        "`up` must drive the existing install and render paths"
    );
    // The lock verification is the step that must not be best-effort: `--locked`
    // refuses to re-pin rather than silently accepting whatever upstream is now.
    assert!(
        src.contains("locked: true"),
        "`up` must verify against the lock, not reconcile it"
    );
}

// ------------------------------------------- the bootstrap cannot brick itself

/// A library repo whose committed `.gitignore` predates the trash — the state
/// of every library initialized before `.trash/` existed.
fn legacy_library_remote(root: &std::path::Path, name: &str) -> String {
    let remote = root.join(name);
    fs::create_dir_all(&remote).unwrap();
    fs::write(remote.join("library.toml"), "version = 1\n").unwrap();
    // No `.trash/` line: the whole point of the fixture.
    fs::write(remote.join(".gitignore"), "*.log\n").unwrap();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .arg("-C")
            .arg(&remote)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q"]);
    git(&["add", "-A"]);
    git(&[
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.com",
        "commit",
        "-qm",
        "library",
    ]);
    format!("file://{}", remote.display())
}

fn library_status(home: &std::path::Path) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(home.join(".agentstack/lib"))
        .args(["status", "--short"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// The defect: bootstrap ensured its own `.trash/` line in the library's
/// tracked `.gitignore` BEFORE the clean-tree gate that refuses to pull over
/// local changes. On a library that predates the trash, the first `up --write`
/// made the dirt and every later one aborted on it — permanently, until the
/// user hand-committed AgentStack's own edit.
///
/// The property is the one a user can check: the same command, twice, works
/// twice, and leaves the library clean enough to keep working a third time.
#[test]
fn a_library_that_predates_the_trash_bootstraps_twice() {
    let (_tmp, home, proj) = fresh();
    let url = legacy_library_remote(proj.parent().unwrap(), "legacy-library-remote");

    let exe = env!("CARGO_BIN_EXE_agentstack");
    let run = |extra: &[&str]| {
        let out = Command::new(exe)
            .arg("up")
            .arg("--write")
            .args(extra)
            .args(["--manifest-dir"])
            .arg(&proj)
            .env("HOME", &home)
            .env("AGENTSTACK_HOME", home.join(".agentstack"))
            .env_remove("SEARCH_API_KEY")
            .env("NO_COLOR", "1")
            .output()
            .unwrap();
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    };

    let first = run(&["--library", &url]);
    assert!(first.contains("cloned library"), "fixture:\n{first}");

    for pass in ["second", "third"] {
        let text = run(&[]);
        assert!(
            !text.contains("library has local changes"),
            "the {pass} `up --write` refused to pull over dirt AgentStack made itself \
             — the bootstrap bricked the machine it was setting up:\n{text}"
        );
        assert!(
            text.contains("shared library already current"),
            "the {pass} `up --write` never reached the pull:\n{text}"
        );
    }

    assert_eq!(
        library_status(&home),
        "",
        "bootstrap only receives: it must leave the library's working tree as clean \
         as it found it"
    );
}

/// The other half: some other command's uncommitted `.trash/` line must not
/// block the pull either (`lib sync --status` writes it and returns without
/// committing), while a real local edit still does. Without the control, the
/// tolerance above could be "the gate was deleted".
#[test]
fn the_managed_ignore_line_is_forgiven_and_the_users_own_edit_is_not() {
    let (_tmp, home, proj) = fresh();
    let url = legacy_library_remote(proj.parent().unwrap(), "tolerance-library-remote");

    let exe = env!("CARGO_BIN_EXE_agentstack");
    let run = || {
        let out = Command::new(exe)
            .args(["up", "--write", "--manifest-dir"])
            .arg(&proj)
            .env("HOME", &home)
            .env("AGENTSTACK_HOME", home.join(".agentstack"))
            .env_remove("SEARCH_API_KEY")
            .env("NO_COLOR", "1")
            .output()
            .unwrap();
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    };
    let clone = Command::new(exe)
        .args(["up", "--write", "--library", &url, "--manifest-dir"])
        .arg(&proj)
        .env("HOME", &home)
        .env("AGENTSTACK_HOME", home.join(".agentstack"))
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    assert!(clone.status.code().is_some(), "the binary must run");

    let ignore = home.join(".agentstack/lib/.gitignore");
    fs::write(&ignore, "*.log\n.trash/\n").unwrap();
    let tolerated = run();
    assert!(
        !tolerated.contains("library has local changes"),
        "AgentStack's own managed ignore line is not the user's local work:\n{tolerated}"
    );
    assert!(
        tolerated.contains("shared library already current"),
        "the pull must still have run:\n{tolerated}"
    );

    // The control: the user's own edit to the same file still stops the pull.
    fs::write(&ignore, "*.log\n.trash/\nsecrets/\n").unwrap();
    let refused = run();
    assert!(
        refused.contains("library has local changes"),
        "a real local change must still block the pull — the gate is narrowed, not \
         removed:\n{refused}"
    );
}

// ------------------------------------- a file-only machine still gets set up

/// `up` used to abort the whole bootstrap when the gateway had nowhere to go.
/// The guard asked "is any CLI installed?", `connect` asks "is any CLI able to
/// host an MCP server?", and on a machine whose only harness is file-only (Pi
/// manages skills, settings and instructions and has no MCP config at all) the
/// two disagree: `connect` bailed, `?` propagated, and the lock verification
/// and the render never ran. Having nowhere to register the bridge is a fact
/// about the machine, not a failure of `up`.
#[test]
fn a_machine_whose_only_cli_is_file_only_still_renders() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let bin = tmp.path().join("bin");
    let proj = tmp.path().join("proj");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&bin).unwrap();
    fs::create_dir_all(proj.join(".agentstack")).unwrap();

    // The only CLI on this machine, and it takes no MCP server.
    fs::write(bin.join("pi"), "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(bin.join("pi"), fs::Permissions::from_mode(0o755)).unwrap();
    }

    fs::write(proj.join(".agentstack/HOUSE.md"), "house rules\n").unwrap();
    fs::write(
        proj.join(".agentstack/agentstack.toml"),
        "version = 1\n\
         [targets]\n\
         default = [\"pi\"]\n\
         [instructions.house]\n\
         path = \"HOUSE.md\"\n",
    )
    .unwrap();

    let exe = env!("CARGO_BIN_EXE_agentstack");
    let run = |args: &[&str]| {
        let out = Command::new(exe)
            .args(args)
            .current_dir(&proj)
            .env_clear()
            .env("HOME", &home)
            .env("AGENTSTACK_HOME", home.join(".agentstack"))
            // The fake CLI first: nothing the developer's machine has installed
            // may decide this outcome.
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
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
    };

    let (locked, code) = run(&["lock", "--write"]);
    assert_eq!(code, 0, "fixture: lock failed:\n{locked}");
    let (preview, _) = run(&["trust", "--preview"]);
    let json: serde_json::Value =
        serde_json::from_str(&preview).expect("`trust --preview` must be JSON");
    let digest = json["surface_digest"]
        .as_str()
        .expect("the preview must carry a surface digest")
        .to_string();
    let (granted, code) = run(&["trust", "--yes", "--consented-digest", &digest]);
    assert_eq!(code, 0, "fixture: trust failed:\n{granted}");

    let (text, code) = run(&["up", "--write"]);
    assert_eq!(
        code, 0,
        "a machine with no MCP-capable CLI must still be set up — the gateway simply \
         has nowhere to register:\n{text}"
    );
    assert!(
        text.contains("skipped"),
        "the skipped gateway step must be stated, not silent:\n{text}"
    );
    assert!(
        proj.join("AGENTS.md").exists(),
        "the render must have run: Pi's instructions file is what this project \
         delivers:\n{text}"
    );

    // The dry run predicts the same skip rather than promising a registration.
    let (plan, code) = run(&["up"]);
    assert_eq!(code, 0, "the preview must not fail:\n{plan}");
    assert!(
        plan.contains("gateway   skipped"),
        "the plan must predict the write it will perform:\n{plan}"
    );
}
