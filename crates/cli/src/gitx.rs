//! The one place AgentStack spawns git — hardened environment, transport
//! allowlist, LFS suppression, and a timeout. Remote operations use bounded,
//! argument-vector process calls. Like `sys` concentrates unsafe and `text`
//! concentrates hostile-string rules, `gitx` concentrates git-spawn policy:
//! no other module may call `Command::new("git")`.
//!
//! Two profiles, because not every git target is hostile:
//!
//! - [`Profile::Ingest`] — fetching content we are about to trust-gate
//!   (store clones/fetches, tag listing, gitpack). Full hardening including
//!   prompt suppression: ingestion must never wedge on interactive auth.
//! - [`Profile::Sync`] — the central library's first-party remote
//!   (`lib sync` clone/fetch/pull/push). The maintainer's own repo
//!   legitimately needs credentials, so prompts stay possible; the protocol
//!   allowlist, LFS suppression, and timeout still apply.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

/// Which trust posture the spawned git runs under. See module docs.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    Ingest,
    Sync,
}

/// Default timeout for any single git invocation, overridable via
/// `AGENTSTACK_GIT_TIMEOUT_MS` (same const-plus-env shape as the gateway's
/// `AGENTSTACK_STDIO_START_MS`). One knob for network and local ops alike: a
/// local `rev-parse` never gets near it, and a second tier isn't worth a
/// second knob.
const DEFAULT_TIMEOUT_MS: u64 = 300_000;
const POLL: Duration = Duration::from_millis(25);
const TERM_GRACE: Duration = Duration::from_millis(300);

/// LFS made inert at the flag level, for machines where `git-lfs` is NOT
/// installed — without these, checking out an LFS-attributed repo aborts
/// with "git-lfs: command not found". `GIT_LFS_SKIP_SMUDGE` (env, below)
/// covers the case where git-lfs IS installed. Skills are text; LFS content
/// is never wanted.
const LFS_FLAGS: &[&str] = &[
    "filter.lfs.smudge=",
    "filter.lfs.process=",
    "filter.lfs.required=false",
];

/// One git invocation's captured outcome. `Err` from [`run_raw`] means the
/// process could not be run to completion (spawn failure or timeout); a git
/// that ran and exited non-zero is `Ok` with `success == false`, so callers
/// that classify stderr (the `lib sync` pull) keep the raw text.
#[derive(Debug)]
pub struct GitOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Run git and bail on failure with the stderr in the message — the shape
/// `store::run_git`/`lib::git_out` always had.
pub fn run(profile: Profile, args: &[&str], cwd: Option<&Path>) -> Result<String> {
    let out = run_raw(profile, args, cwd)?;
    if !out.success {
        bail!("git {} failed: {}", args.join(" "), out.stderr.trim());
    }
    Ok(out.stdout.trim().to_string())
}

/// Run git and report the captured outcome without judging it.
pub fn run_raw(profile: Profile, args: &[&str], cwd: Option<&Path>) -> Result<GitOutput> {
    run_impl("git", profile, args, cwd, timeout_ms())
}

/// Whether a git invocation succeeds — for probing state (has-remote,
/// has-upstream). Spawn/timeout failures read as "no".
pub fn succeeds(profile: Profile, args: &[&str], cwd: Option<&Path>) -> bool {
    run_raw(profile, args, cwd)
        .map(|o| o.success)
        .unwrap_or(false)
}

/// Reject exotic git transports before any spawn. `GIT_ALLOW_PROTOCOL`
/// gates these on modern git, but older versions did not reliably route
/// `ext::` (arbitrary command execution) through the allowlist — and the
/// check costs one `starts_with` (design §B.1).
pub fn deny_weird_transport(url: &str) -> Result<()> {
    let l = url.trim_start().to_ascii_lowercase();
    if l.starts_with("ext::") || l.starts_with("fd::") {
        bail!(
            "unsupported git transport in '{}' — https, ssh, or file only",
            crate::text::sanitize_line(url)
        );
    }
    Ok(())
}

fn timeout_ms() -> u64 {
    std::env::var("AGENTSTACK_GIT_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&ms| ms > 0)
        .unwrap_or(DEFAULT_TIMEOUT_MS)
}

/// The environment additions per profile, as data — pure so the test can
/// assert the policy without spawning anything. `parent` is how the caller's
/// environment is consulted: an explicitly user-set `GIT_ALLOW_PROTOCOL` or
/// `GIT_SSH_COMMAND` always wins (documented escape hatches); prompt
/// suppression under `Ingest` does not yield.
fn hardened_env(
    profile: Profile,
    parent: &dyn Fn(&str) -> Option<String>,
) -> Vec<(&'static str, String)> {
    let mut env: Vec<(&'static str, String)> = Vec::new();
    if parent("GIT_ALLOW_PROTOCOL").is_none() {
        // Narrower than vercel's list on purpose: no cleartext http/git
        // protocols for a security tool. file: stays for tests and local
        // flows, ssh: for private repos.
        env.push(("GIT_ALLOW_PROTOCOL", "https:ssh:file".to_string()));
    }
    env.push(("GIT_LFS_SKIP_SMUDGE", "1".to_string()));
    if profile == Profile::Ingest {
        env.push(("GIT_TERMINAL_PROMPT", "0".to_string()));
        if parent("GIT_SSH_COMMAND").is_none() {
            // SSH prompts via /dev/tty, bypassing the nulled stdin;
            // BatchMode makes it fail fast instead.
            env.push(("GIT_SSH_COMMAND", "ssh -oBatchMode=yes".to_string()));
        }
    }
    env
}

/// The actual spawn: process group + piped stdio + concurrent drain +
/// deadline poll. `program` is a parameter only so the timeout test can
/// substitute a stub; production always passes "git".
///
/// The reader threads are load-bearing, not style: a child that fills the OS
/// pipe buffer blocks writing until someone reads, so draining must happen
/// WHILE we wait — otherwise a chatty clone deadlocks and the poll loop
/// misreads it as a hang. Moving each pipe into its thread is an ownership
/// transfer (like handing a stream to a worker, but compiler-enforced):
/// after `thread::spawn(move …)` only that thread can touch it.
fn run_impl(
    program: &str,
    profile: Profile,
    args: &[&str],
    cwd: Option<&Path>,
    timeout_ms: u64,
) -> Result<GitOutput> {
    let mut cmd = Command::new(program);
    if let Some(dir) = cwd {
        cmd.arg("-C").arg(dir);
    }
    for flag in LFS_FLAGS {
        cmd.arg("-c").arg(flag);
    }
    cmd.args(args);
    for (k, v) in hardened_env(profile, &|k| std::env::var(k).ok()) {
        cmd.env(k, v);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Its own process group, so a timeout can reap git AND whatever git
    // spawned (ssh, credential helpers) in one signal.
    crate::sys::spawn_in_new_process_group(&mut cmd);
    let mut child = cmd.spawn().context("running git (is it installed?)")?;

    let drain = |pipe: Option<Box<dyn std::io::Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut buf);
            }
            buf
        })
    };
    let out_thread = drain(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
    );
    let err_thread = drain(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
    );

    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let status = loop {
        if let Some(status) = child.try_wait().context("waiting for git")? {
            break status;
        }
        if Instant::now() >= deadline {
            kill_ladder(&mut child);
            let _ = out_thread.join();
            let _ = err_thread.join();
            bail!(
                "git {} timed out after {}s — set AGENTSTACK_GIT_TIMEOUT_MS to raise the \
                 limit, or clone manually and point agentstack at the local path",
                args.join(" "),
                timeout_ms.div_ceil(1000)
            );
        }
        std::thread::sleep(POLL);
    };

    let stdout = String::from_utf8_lossy(&out_thread.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&err_thread.join().unwrap_or_default()).into_owned();
    Ok(GitOutput {
        success: status.success(),
        stdout,
        stderr,
    })
}

/// SIGTERM the group, give it a short grace, SIGKILL what remains — the same
/// escalation the gateway uses on its stdio children. On platforms without
/// process groups, fall back to killing the direct child.
fn kill_ladder(child: &mut std::process::Child) {
    let pgid = child.id() as i32;
    if crate::sys::signal_group(pgid, crate::sys::Signal::Term).is_ok() {
        let grace = Instant::now() + TERM_GRACE;
        while Instant::now() < grace {
            if matches!(child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = crate::sys::signal_group(pgid, crate::sys::Signal::Kill);
    } else {
        let _ = child.kill();
    }
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_policy_per_profile() {
        let empty = |_: &str| None;
        let ingest = hardened_env(Profile::Ingest, &empty);
        let get = |env: &[(&str, String)], k: &str| {
            env.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone())
        };
        assert_eq!(
            get(&ingest, "GIT_ALLOW_PROTOCOL").as_deref(),
            Some("https:ssh:file")
        );
        assert_eq!(get(&ingest, "GIT_LFS_SKIP_SMUDGE").as_deref(), Some("1"));
        assert_eq!(get(&ingest, "GIT_TERMINAL_PROMPT").as_deref(), Some("0"));
        assert_eq!(
            get(&ingest, "GIT_SSH_COMMAND").as_deref(),
            Some("ssh -oBatchMode=yes")
        );

        // Sync: first-party remote — no prompt suppression, no SSH override.
        let sync = hardened_env(Profile::Sync, &empty);
        assert_eq!(get(&sync, "GIT_TERMINAL_PROMPT"), None);
        assert_eq!(get(&sync, "GIT_SSH_COMMAND"), None);
        assert_eq!(
            get(&sync, "GIT_ALLOW_PROTOCOL").as_deref(),
            Some("https:ssh:file")
        );
        assert_eq!(get(&sync, "GIT_LFS_SKIP_SMUDGE").as_deref(), Some("1"));

        // Caller-set values win for the documented escape hatches.
        let preset = |k: &str| match k {
            "GIT_ALLOW_PROTOCOL" => Some("https".to_string()),
            "GIT_SSH_COMMAND" => Some("ssh -i ~/.ssh/work".to_string()),
            _ => None,
        };
        let overridden = hardened_env(Profile::Ingest, &preset);
        assert_eq!(get(&overridden, "GIT_ALLOW_PROTOCOL"), None);
        assert_eq!(get(&overridden, "GIT_SSH_COMMAND"), None);
        // …but Ingest prompt suppression never yields.
        assert_eq!(
            get(&overridden, "GIT_TERMINAL_PROMPT").as_deref(),
            Some("0")
        );
    }

    #[test]
    fn weird_transports_rejected() {
        assert!(deny_weird_transport("ext::sh -c whoami").is_err());
        assert!(deny_weird_transport("EXT::sh").is_err());
        assert!(deny_weird_transport("fd::17").is_err());
        assert!(deny_weird_transport("https://github.com/a/b").is_ok());
        assert!(deny_weird_transport("git@github.com:a/b.git").is_ok());
        assert!(deny_weird_transport("file:///tmp/repo").is_ok());
    }

    /// The timeout must fire, reap the child (and its group), and name the
    /// override knob — using a stub program so the test needs no network and
    /// no real hang.
    #[cfg(unix)]
    #[test]
    fn timeout_kills_a_hung_git() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = assert_fs::TempDir::new().unwrap();
        let stub = tmp.path().join("fake-git");
        std::fs::write(&stub, "#!/bin/sh\nsleep 30\n").unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();

        let started = Instant::now();
        let err = run_impl(
            &stub.to_string_lossy(),
            Profile::Ingest,
            &["clone", "https://example.invalid/x"],
            None,
            400,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("timed out"),
            "expected timeout error, got: {err:#}"
        );
        assert!(
            err.to_string().contains("AGENTSTACK_GIT_TIMEOUT_MS"),
            "timeout error must name the override knob"
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "timeout must fire promptly, not wait out the child"
        );
    }
}
