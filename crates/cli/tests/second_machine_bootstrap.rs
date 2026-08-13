// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The day-2 machine: what `up` leaves behind, and what the surfaces say about
//! it afterwards.
//!
//! A closing walkthrough ran the real second-machine path and found three
//! screens disagreeing with the machine they had just set up. Each one is a
//! sentence, not a mechanism — which is exactly why they need holding: nothing
//! fails, the user is simply told something untrue at the moment they are least
//! able to check it.

use std::path::Path;
use std::process::Command;

fn run(args: &[&str], home: &Path, cwd: &Path) -> (String, bool) {
    let out = Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("HOME", home)
        .env("AGENTSTACK_HOME", home.join(".agentstack"))
        .env("PATH", "/usr/bin:/bin")
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    let mut text = String::from_utf8_lossy(&out.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    (text, out.status.success())
}

/// S4: the machine bootstrap's happy path is OUTSIDE a project, and must end
/// cleanly.
///
/// `up --write` on a fresh machine syncs the library, detects the CLIs and
/// registers the bridge — then looked for a project manifest in the working
/// directory, did not find one, and exited 1 with `no agentstack manifest`.
/// Every piece of machine work had succeeded. A nonzero exit there tells a
/// script the bootstrap failed, and tells a person to distrust a setup that is
/// fine.
#[test]
fn up_outside_a_project_reports_the_machine_and_exits_clean() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let elsewhere = tmp.path().join("no-project-here");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&elsewhere).unwrap();

    let (out, ok) = run(&["up", "--write"], &home, &elsewhere);
    assert!(
        ok,
        "the machine bootstrap's own happy path must exit 0:\n{out}"
    );
    assert!(
        !out.contains("no agentstack manifest"),
        "a missing project is not a failure of the machine setup:\n{out}"
    );
    // Having done the machine work, it has to say where the user goes next —
    // the whole point of the command is that they are on a new machine.
    assert!(
        out.contains("Next:") && out.contains("clone a project"),
        "the bootstrap must name the next step:\n{out}"
    );
}

/// The other half of S4: naming a project and not having one IS an error.
///
/// `--manifest-dir` says "this project", so its absence is a real failure and
/// must keep failing. Without this the fix above would have turned a genuine
/// missing-manifest error into silence.
#[test]
fn up_still_fails_when_a_project_was_named_and_is_missing() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let named = tmp.path().join("named-but-empty");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&named).unwrap();

    let (out, ok) = run(
        &["up", "--write", "--manifest-dir", named.to_str().unwrap()],
        &home,
        tmp.path(),
    );
    assert!(
        !ok,
        "a named project that does not exist must still fail:\n{out}"
    );
}

/// S6: the bridge `up` itself wrote is not a server to import.
///
/// `gateway connect` registers AgentStack's own control plane in each harness's
/// GLOBAL config. On the second machine `status` read that entry back, found it
/// absent from the manifest, and reported "1 server configured here, not in
/// this setup" — offering to import the tool's own registration into the
/// project it serves.
///
/// `abandoned_render_is_named.rs` already holds this for the foreign-FILE
/// detector. This is the second reading of the same disk, the per-server count,
/// and it had no such exclusion. Driven through the registration path the
/// walkthrough used.
#[test]
fn the_bridge_up_registered_is_never_offered_for_import() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(proj.join(".agentstack")).unwrap();
    std::fs::write(
        proj.join(".agentstack/agentstack.toml"),
        "version = 1\n[toolsets.default]\nservers = []\n",
    )
    .unwrap();
    // A harness config file, so the harness is detected at all.
    std::fs::write(home.join(".claude.json"), "{}").unwrap();

    let (connect, ok) = run(
        &["more", "gateway", "connect", "claude-code", "--write"],
        &home,
        &proj,
    );
    assert!(
        ok && connect.contains("gateway registered"),
        "the fixture must actually register the bridge:\n{connect}"
    );

    let (out, ok) = run(&["status"], &home, &proj);
    assert!(ok, "{out}");
    assert!(
        !out.contains("not in this setup"),
        "our own bridge registration was offered for import:\n{out}"
    );
}

/// S5: after the library is linked, `lib sources` must stop saying nothing is.
///
/// The default `~/.agentstack/lib` source is implicit, and its row carried
/// "(the default — nothing linked yet)" unconditionally. On the second machine
/// — where `up --write` had just cloned and linked a library — that row sat
/// directly above the linked source it denied the existence of.
#[test]
fn lib_sources_stops_claiming_nothing_is_linked_once_something_is() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let proj = tmp.path().join("proj");
    let lib = tmp.path().join("my-library");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::create_dir_all(&lib).unwrap();

    // Before: nothing linked, and the note is true.
    let (before, ok) = run(&["more", "lib", "sources"], &home, &proj);
    assert!(ok, "{before}");
    assert!(
        before.contains("nothing linked yet"),
        "the premise: with nothing linked the note is correct:\n{before}"
    );

    let (linked, ok) = run(
        &[
            "more",
            "lib",
            "link",
            lib.to_str().unwrap(),
            "--name",
            "central",
            "--write",
        ],
        &home,
        &proj,
    );
    assert!(ok, "the fixture must link a source:\n{linked}");

    let (after, ok) = run(&["more", "lib", "sources"], &home, &proj);
    assert!(ok, "{after}");
    assert!(
        after.contains("central"),
        "the linked source must be listed:\n{after}"
    );
    assert!(
        !after.contains("nothing linked yet"),
        "a linked source is on the list, so the claim is false:\n{after}"
    );
}
