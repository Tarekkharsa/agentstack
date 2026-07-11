//! Filesystem enforcement through the REAL `agentstack run --sandbox` binary —
//! Docker-gated.
//!
//! Two runs of the same container command (`sh -c 'echo pwned > /workspace/…'`)
//! against two projects:
//! - no `[policy.filesystem]` write scope → the workspace mounts READ-ONLY
//!   (deny-by-default), the write fails inside the container, and nothing
//!   appears in the host project dir;
//! - a bundle write scope covering the workspace (`./**`) → the mount is
//!   read-write and the file lands on the host.
//!
//! The kernel enforces the bind mode (`:ro`), not the harness — same container,
//! same command; only the policy differs. Compiles only with
//! `--features sandbox`; SKIPS when no Docker daemon or busybox image. Run it
//! where Docker exists:
//!   cargo test -p agentstack --features sandbox --test sandbox_fs -- --nocapture
#![cfg(feature = "sandbox")]

use std::fs;
use std::path::Path;
use std::process::Command;

const IMAGE: &str = "busybox:latest";

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

/// Strip ANSI escape sequences so the banner text is assertable.
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

/// One `run --sandbox` of `sh -c "echo pwned > /workspace/pwned.txt"` in
/// `proj`, returning (success, stripped stdout).
fn try_write_in_sandbox(proj: &Path, home: &Path, as_home: &Path) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args([
            "run",
            "--sandbox",
            "shtest",
            "--",
            "-c",
            "echo pwned > /workspace/pwned.txt",
        ])
        .current_dir(proj)
        .env("HOME", home)
        .env("AGENTSTACK_HOME", as_home)
        .env("AGENTSTACK_SANDBOX_IMAGE", IMAGE)
        .output()
        .unwrap();
    let stdout = strip_ansi(&String::from_utf8_lossy(&out.stdout));
    eprintln!("--- run --sandbox stdout ---\n{stdout}");
    eprintln!("--- stderr ---\n{}", String::from_utf8_lossy(&out.stderr));
    (out.status.success(), stdout)
}

#[test]
fn workspace_writes_blocked_read_only_and_allowed_with_a_write_scope() {
    if !docker_and_image() {
        return;
    }

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let as_home = home.join(".agentstack");
    fs::create_dir_all(as_home.join("adapters")).unwrap();
    // Empty machine policy: the decision below is purely the bundle's ask
    // (and the deny-by-default when it asks nothing).
    fs::write(as_home.join("agentstack.toml"), "version = 1\n").unwrap();
    // A throwaway harness whose launch binary is `sh`, so `run --sandbox`
    // builds the container command as `sh -c …`.
    fs::write(
        as_home.join("adapters/shtest.yaml"),
        "id: shtest\ndisplay: Sh Test\ndetect:\n  bin: sh\n",
    )
    .unwrap();

    eprintln!("\n=== AgentStack sandbox filesystem demo (real Docker) ===");
    eprintln!("The same container tries `echo pwned > /workspace/pwned.txt`;");
    eprintln!("only the project's [policy.filesystem] write scope differs.\n");

    // 1) No write scope → deny-by-default: the workspace mounts read-only,
    //    the kernel refuses the write, nothing lands on the host.
    eprintln!("[1] no [policy.filesystem] write scope (deny-by-default)");
    let ro_proj = tmp.path().join("ro-proj");
    fs::create_dir_all(&ro_proj).unwrap();
    fs::write(ro_proj.join("agentstack.toml"), "version = 1\n").unwrap();

    let (ok, stdout) = try_write_in_sandbox(&ro_proj, &home, &as_home);
    assert!(
        stdout.contains("read-only"),
        "the banner must say the workspace is read-only: {stdout}"
    );
    assert!(
        !ok,
        "the run must FAIL when the container can't write its workspace"
    );
    assert!(
        !ro_proj.join("pwned.txt").exists(),
        "nothing may land in the host project dir under a read-only mount"
    );
    eprintln!("    → kernel refused the write; host project untouched ✓\n");

    // 2) A bundle write scope covering the workspace → read-write mount, the
    //    same command succeeds and the file appears on the host.
    eprintln!("[2] bundle grants [policy.filesystem] write = [\"./**\"]");
    let rw_proj = tmp.path().join("rw-proj");
    fs::create_dir_all(&rw_proj).unwrap();
    fs::write(
        rw_proj.join("agentstack.toml"),
        "version = 1\n\n[policy.filesystem]\nwrite = [\"./**\"]\n",
    )
    .unwrap();

    let (ok, stdout) = try_write_in_sandbox(&rw_proj, &home, &as_home);
    assert!(
        stdout.contains("read-write"),
        "the banner must say the workspace is read-write: {stdout}"
    );
    assert!(ok, "the run must succeed when the write scope grants it");
    let written = fs::read_to_string(rw_proj.join("pwned.txt")).unwrap();
    assert_eq!(written.trim(), "pwned");
    eprintln!("    → write went through to the host project dir ✓\n");

    eprintln!("Result: same container, same command — the policy decided ro vs rw.");
}
