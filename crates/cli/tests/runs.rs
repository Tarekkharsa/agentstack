// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Tracked runs, end to end: launch a benign `sleep` as a run, see it listed,
//! kill it, and confirm a profile applied for the run is reverted when it ends.
//! Uses a throwaway adapter pointing at `sleep` so no real harness/TUI is needed.
//! Own test file so the HOME/AGENTSTACK_HOME overrides run isolated; the tests
//! within still share the process env, so an env lock serializes them.

#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use agentstack::scope::Scope;

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Point HOME + AGENTSTACK_HOME at a fresh temp tree, install a `sleep`-backed
/// adapter, and write `manifest` into a project dir. Returns (tempdir, proj).
fn setup(manifest: &str) -> (assert_fs::TempDir, PathBuf) {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let as_home = home.join(".agentstack");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", &as_home);

    let adapters = as_home.join("adapters");
    fs::create_dir_all(&adapters).unwrap();
    fs::write(
        adapters.join("sleeptest.yaml"),
        "id: sleeptest\ndisplay: Sleep Test\ndetect:\n  bin: sleep\n",
    )
    .unwrap();

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("agentstack.toml"), manifest).unwrap();
    (tmp, proj)
}

fn wait_until<F: Fn() -> bool>(f: F, max: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < max {
        if f() {
            return true;
        }
        thread::sleep(Duration::from_millis(50));
    }
    f()
}

/// Launch `sleep 30` as a run on a worker thread (launch blocks until the child
/// exits), confirm it is tracked, then kill it and confirm the registry drains
/// and the foreground wrapper returns.
#[test]
fn launch_lists_then_kill_removes_the_run() {
    let _guard = ENV_LOCK.lock().unwrap();
    let (_tmp, proj) = setup("version = 1\n[meta]\nname = \"t\"\n");

    let proj2 = proj.clone();
    let handle = thread::spawn(move || {
        let _ = agentstack::runs::launch(
            Some(&proj2),
            "sleeptest",
            None,
            Scope::Project,
            &["30".to_string()],
            false,
        );
    });

    assert!(
        wait_until(
            || !agentstack::runs::list().is_empty(),
            Duration::from_secs(5)
        ),
        "run never appeared in the registry"
    );
    let runs = agentstack::runs::list();
    assert_eq!(runs.len(), 1, "exactly one run");
    let run = runs[0].clone();
    assert_eq!(run.harness, "sleeptest");
    assert_eq!(run.command, "sleep");
    assert!(run.profile.is_none(), "no profile bound");
    assert!(run.pid > 0);

    agentstack::runs::kill(&run.id, false).unwrap();
    assert!(
        wait_until(
            || agentstack::runs::list().is_empty(),
            Duration::from_secs(5)
        ),
        "run still listed after kill"
    );
    handle.join().unwrap();
}

/// A run that exits on its own cleans up its own registry entry.
#[test]
fn run_cleans_up_its_record_on_normal_exit() {
    let _guard = ENV_LOCK.lock().unwrap();
    let (_tmp, proj) = setup("version = 1\n[meta]\nname = \"t\"\n");

    // Blocks ~1s, then launch() removes the record before returning.
    agentstack::runs::launch(
        Some(&proj),
        "sleeptest",
        None,
        Scope::Project,
        &["1".to_string()],
        false,
    )
    .unwrap();

    assert!(
        agentstack::runs::list().is_empty(),
        "record should be gone after the run exits"
    );
}

/// The harness starts at the PROJECT root, not the manifest dir. Under the
/// preferred `.agentstack/` layout the two differ, and a session opened
/// inside `.agentstack/` sees no source code — the legacy root layout (used
/// by the other tests here) masks the distinction.
#[test]
fn run_launches_the_harness_at_the_project_root() {
    let _guard = ENV_LOCK.lock().unwrap();
    let (tmp, proj) = setup("version = 1\n[meta]\nname = \"t\"\n");
    // Re-home the manifest into the preferred layout.
    fs::create_dir_all(proj.join(".agentstack")).unwrap();
    fs::rename(
        proj.join("agentstack.toml"),
        proj.join(".agentstack/agentstack.toml"),
    )
    .unwrap();
    // A shim "harness" that just records the directory it was launched from.
    fs::write(
        tmp.path().join("home/.agentstack/adapters/shtest.yaml"),
        "id: shtest\ndisplay: Sh Test\ndetect:\n  bin: sh\n",
    )
    .unwrap();

    agentstack::runs::launch(
        Some(&proj),
        "shtest",
        None,
        Scope::Project,
        &["-c".to_string(), "pwd > launched-from.txt".to_string()],
        false,
    )
    .unwrap();

    assert!(
        !proj.join(".agentstack/launched-from.txt").exists(),
        "harness must not launch inside the manifest dir"
    );
    let recorded = fs::read_to_string(proj.join("launched-from.txt")).unwrap();
    // Canonicalized: the kernel reports /private/var/… where the tempdir
    // spells /var/… on macOS.
    assert_eq!(
        PathBuf::from(recorded.trim()).canonicalize().unwrap(),
        proj.canonicalize().unwrap(),
        "harness cwd must be the project root"
    );
}

/// A run launched with a profile applies it before the harness starts and
/// reverts it when the run ends (here: when killed).
#[test]
fn profile_run_applies_then_reverts_on_exit() {
    let _guard = ENV_LOCK.lock().unwrap();
    let (_tmp, proj) = setup(
        "version = 1\n[meta]\nname = \"t\"\n\
         [targets]\ndefault = [\"claude-code\"]\n\
         [servers.demo]\ntype = \"http\"\nurl = \"https://demo.example/mcp\"\n\
         [profiles.p1]\nservers = [\"demo\"]\nskills = []\n",
    );
    // Claude Code's project MCP config lives at .mcp.json in the repo root.
    let cfg = proj.join(".mcp.json");

    let proj2 = proj.clone();
    let handle = thread::spawn(move || {
        let _ = agentstack::runs::launch(
            Some(&proj2),
            "sleeptest",
            Some("p1"),
            Scope::Project,
            &["30".to_string()],
            false,
        );
    });

    // Once the run is tracked, the profile's server must be written to config.
    assert!(
        wait_until(
            || !agentstack::runs::list().is_empty(),
            Duration::from_secs(5)
        ),
        "profile run never appeared"
    );
    assert!(
        wait_until(|| reads_contains(&cfg, "demo"), Duration::from_secs(5)),
        "profile server was not applied to .mcp.json"
    );
    let run = agentstack::runs::list()[0].clone();
    assert_eq!(run.profile.as_deref(), Some("p1"));

    // Kill the run; the wrapper reverts the profile as it tears down.
    agentstack::runs::kill(&run.id, false).unwrap();
    handle.join().unwrap(); // wait for launch() to fully finish, including revert

    assert!(
        !reads_contains(&cfg, "demo"),
        "profile server should be reverted after the run ends"
    );
    assert!(agentstack::runs::list().is_empty());
}

/// A process that ignores SIGTERM is still taken down — `kill` escalates to
/// SIGKILL after the grace period.
#[test]
fn kill_escalates_to_sigkill_when_sigterm_is_ignored() {
    let _guard = ENV_LOCK.lock().unwrap();
    let (_tmp, proj) = setup("version = 1\n[meta]\nname = \"t\"\n");
    // An adapter whose binary (sh) we make ignore SIGTERM and keep running.
    let as_home = std::env::var("AGENTSTACK_HOME").unwrap();
    fs::write(
        Path::new(&as_home).join("adapters/stubborn.yaml"),
        "id: stubborn\ndisplay: Stubborn\ndetect:\n  bin: sh\n",
    )
    .unwrap();

    let proj2 = proj.clone();
    let handle = thread::spawn(move || {
        let _ = agentstack::runs::launch(
            Some(&proj2),
            "stubborn",
            None,
            Scope::Project,
            &[
                "-c".to_string(),
                "trap '' TERM; while :; do sleep 1; done".to_string(),
            ],
            false,
        );
    });

    assert!(
        wait_until(
            || !agentstack::runs::list().is_empty(),
            Duration::from_secs(5)
        ),
        "stubborn run never appeared"
    );
    let run = agentstack::runs::list()[0].clone();

    // Graceful kill: SIGTERM is trapped, so it must escalate to SIGKILL.
    agentstack::runs::kill(&run.id, false).unwrap();
    assert!(
        wait_until(
            || agentstack::runs::list().is_empty(),
            Duration::from_secs(6)
        ),
        "stubborn run survived kill escalation"
    );
    handle.join().unwrap();
}

fn reads_contains(path: &Path, needle: &str) -> bool {
    fs::read_to_string(path)
        .map(|s| s.contains(needle))
        .unwrap_or(false)
}
