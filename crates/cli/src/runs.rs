//! Live harness runs: launch an agent CLI as a tracked child process, list the
//! ones currently alive, and kill them — from the terminal, without ever
//! opening Activity Monitor.
//!
//! A *run* is distinct from a [`crate::session`] (an ephemeral profile load keyed
//! by directory). A run is a real OS process that agentstack owns: we spawn the
//! harness binary in its own process group so we can tree-kill it later, and we
//! record it in `~/.agentstack/runs.json` so a *separate* agentstack process (the
//! read-only dashboard) can see it. When launched with a profile we reuse the
//! session machinery to apply it before launch and revert it on exit.
//!
//! Control split: launching is a terminal act (`agentstack run <harness>` runs the
//! TUI attached to your terminal, with agentstack as its parent); observing is
//! also possible from the dashboard, while killing is a terminal act
//! (`agentstack kill <id>`) that signals the recorded process group.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::scope::Scope;
use crate::util::paths;

/// A headless child exited nonzero. Carried as an error so it unwinds the
/// normal `Result` path (restoring scope guards, recording evidence), but
/// `main` recognizes it and exits with the child's own code instead of the
/// generic 1 — a CI consumer of `run --locked --prompt` must be able to tell
/// "the harness failed" from "agentstack refused", and must see *which* code.
///
/// Not an agentstack failure, so `main` prints no `error:` line for it: every
/// gate passed, the grant froze, and the launcher's stderr banners plus
/// `agentstack report run <id>` already carry the outcome.
///
/// (Rust note: this is the standard `anyhow` escape hatch — a concrete error
/// type that a caller recovers with `err.downcast_ref::<ChildExit>()`, the way
/// you'd check `instanceof` on a thrown error in TypeScript.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChildExit(pub i32);

impl std::fmt::Display for ChildExit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the harness exited with status {}", self.0)
    }
}

impl std::error::Error for ChildExit {}

/// One tracked harness process. Persisted to `runs.json`; the `pid` is also the
/// process-group id (the child is made a group leader at spawn).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    pub id: String,
    pub pid: i32,
    /// Adapter id, e.g. `claude-code`.
    pub harness: String,
    /// Adapter display name, for the UI.
    pub display: String,
    /// The binary we launched.
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub cwd: String,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    pub started_unix: u64,
    /// True if this run applied a profile (via `session::start`) that the
    /// foreground process will revert on exit.
    #[serde(default)]
    pub started_session: bool,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn registry_path() -> PathBuf {
    paths::agentstack_home().join("runs.json")
}

fn load_all() -> BTreeMap<String, RunRecord> {
    fs::read_to_string(registry_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_all(map: &BTreeMap<String, RunRecord>) -> Result<()> {
    let path = registry_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut text = serde_json::to_string_pretty(map)?;
    text.push('\n');
    // Atomic (write-temp-then-rename) so a crash mid-write can't truncate the
    // registry, matching how the rest of agentstack persists state.
    crate::util::atomic::write(&path, &text).with_context(|| format!("writing {}", path.display()))
}

/// A short, unique-enough run id (`r-<hex>`). Derived from the wall clock and
/// pid via FNV-1a — same dependency-free trick as `state::hash` / the dashboard
/// token; not security-sensitive.
pub(crate) fn gen_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id() as u128;
    let mut h: u64 = 0xcbf29ce484222325;
    for b in (nanos ^ (pid << 32)).to_le_bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    let hex = format!("{h:016x}");
    format!("r-{}", &hex[..10])
}

/// Drop records whose process has exited. Returns the surviving map and whether
/// anything was removed (so the caller can persist the cleanup).
fn prune(mut map: BTreeMap<String, RunRecord>) -> (BTreeMap<String, RunRecord>, bool) {
    let before = map.len();
    map.retain(|_, r| crate::sys::pid_alive(r.pid));
    let changed = map.len() != before;
    (map, changed)
}

/// Every run currently alive on this machine. Self-healing: dead records (e.g.
/// from an `agentstack run` that was itself killed before it could clean up) are
/// pruned and the file rewritten.
pub fn list() -> Vec<RunRecord> {
    let (map, changed) = prune(load_all());
    if changed {
        let _ = save_all(&map);
    }
    map.into_values().collect()
}

/// Everything `launch` needs, resolved and validated up front — so the CLI
/// layer can run every check that might fail (manifest present, harness id
/// known, binary on PATH) BEFORE printing its "▶ launching…" banner. Split for
/// the same reason `sandbox.rs` checks the Docker daemon before its banner: a
/// user must never read "launching" above an error that means nothing launched.
pub struct LaunchPlan {
    ctx: crate::commands::Context,
    /// The id as the user typed it — recorded verbatim so `report runs` shows
    /// what was actually invoked.
    harness: String,
    bin: String,
    display: String,
}

impl LaunchPlan {
    pub fn default_scope(&self) -> Scope {
        Scope::default_for(&self.ctx.dir)
    }
}

/// Resolve and validate a launch without side effects: load the manifest, look
/// up `harness` in the adapter registry, and confirm its binary is on PATH.
pub fn prepare(manifest_dir: Option<&Path>, harness: &str) -> Result<LaunchPlan> {
    let ctx = crate::commands::load(manifest_dir)?;
    let desc = ctx.registry.get(harness).with_context(|| {
        // Name the valid ids right in the error: the full set is small (13),
        // and "see `agentstack adapters list`" alone costs a round-trip.
        let ids: Vec<&str> = ctx.registry.ids().collect();
        format!(
            "unknown CLI '{harness}' — valid ids: {} (details: `agentstack adapters list`)",
            ids.join(" · ")
        )
    })?;
    let bin = desc
        .detect
        .bin
        .clone()
        .with_context(|| format!("{} has no known launch binary to run", desc.display))?;
    if !crate::adapter::bin_on_path(&bin) {
        anyhow::bail!(
            "'{bin}' is not on your PATH — is {} installed?",
            desc.display
        );
    }
    let display = desc.display.clone();
    Ok(LaunchPlan {
        ctx,
        harness: harness.to_string(),
        bin,
        display,
    })
}

/// Launch a prepared harness as a tracked child, optionally applying `profile`
/// for the life of the run. Blocks until the harness exits (it's attached to
/// this terminal), then prunes the record and reverts the profile unless
/// `keep`. Build the plan with [`prepare`].
pub fn launch(
    plan: LaunchPlan,
    manifest_dir: Option<&Path>,
    profile: Option<&str>,
    scope: Scope,
    extra_args: &[String],
    keep: bool,
) -> Result<()> {
    let LaunchPlan {
        ctx,
        harness,
        bin,
        display,
    } = plan;
    let harness = harness.as_str();
    let dir = ctx.dir.clone();

    // Apply a profile for the lifetime of this run, if asked. Reuses the session
    // engine so we don't duplicate the snapshot/activate/revert logic.
    let mut started_session = false;
    if let Some(p) = profile {
        if crate::session::active(&dir).is_some() {
            anyhow::bail!(
                "a session is already active here — end it (`agentstack session end`) or run without --profile"
            );
        }
        crate::session::start(manifest_dir, p, scope)
            .with_context(|| format!("applying profile '{p}' for this run"))?;
        started_session = true;
    }

    // Spawn the harness in its own process group so a later kill takes the
    // tree. The run id rides along as an env var so tool calls the harness's
    // agent makes through `agentstack mcp` land in the audit log attributed
    // to this run.
    let id = gen_id();
    let started = Instant::now();
    let log = crate::calllog::RunLog::create(&id);
    if let Some(log) = &log {
        log.append(&crate::calllog::RunEvent::HostStarted {
            ts: crate::calllog::now_epoch(),
            harness: harness.to_string(),
            posture: "host".to_string(),
        });
    }
    // Spawn at the PROJECT root, not the manifest dir — under the preferred
    // layout `dir` is `.agentstack/`, and a harness session opened there sees
    // no source code. (With the legacy root manifest the two coincide.)
    let workdir = crate::manifest::project_root_of(&dir);
    let status = match launch_attached(
        &bin, extra_args, &workdir, &id, harness, &display, profile, scope,
    ) {
        Ok(s) => s,
        Err(e) => {
            if started_session && !keep {
                let _ = crate::session::end(manifest_dir);
            }
            if let Some(log) = &log {
                log.append(&crate::calllog::RunEvent::HostExited {
                    ts: crate::calllog::now_epoch(),
                    outcome: "launch-failed".to_string(),
                    code: None,
                    duration_ms: started.elapsed().as_millis() as u64,
                });
            }
            println!("\nSee what happened: `agentstack report run {id}`");
            return Err(e);
        }
    };
    if started_session && !keep {
        let _ = crate::session::end(manifest_dir);
    }
    if let Some(log) = &log {
        log.append(&crate::calllog::RunEvent::HostExited {
            ts: crate::calllog::now_epoch(),
            outcome: if status.code().is_some() {
                "exited".to_string()
            } else {
                "signaled".to_string()
            },
            code: status.code(),
            duration_ms: started.elapsed().as_millis() as u64,
        });
    }
    println!("\nSee what happened: `agentstack report run {id}`");
    // As before this was extracted: the harness's own exit code is its
    // business — only a wait/spawn failure is an error here.
    let _ = status;
    Ok(())
}

/// Spawn `bin` attached to this terminal under an EXISTING run id, track it in
/// the run registry, and block until it exits. The caller owns the id (the
/// locked flow creates it before its gates so refusals are recorded under the
/// same identity the child would have carried) and owns interpreting the exit
/// status. No profile logic here — `launch` layers that on top.
#[allow(clippy::too_many_arguments)]
pub(crate) fn launch_attached(
    bin: &str,
    extra_args: &[String],
    dir: &Path,
    id: &str,
    harness: &str,
    display: &str,
    profile: Option<&str>,
    scope: Scope,
) -> Result<std::process::ExitStatus> {
    let mut child =
        spawn_child(bin, extra_args, dir, id).with_context(|| format!("launching {display}"))?;
    let pid = child.id() as i32;
    let rec = RunRecord {
        id: id.to_string(),
        pid,
        harness: harness.to_string(),
        display: display.to_string(),
        command: bin.to_string(),
        args: extra_args.to_vec(),
        cwd: dir.to_string_lossy().into_owned(),
        profile: profile.map(String::from),
        scope: profile.map(|_| scope.as_str().to_string()),
        started_unix: now_secs(),
        started_session: false,
    };
    {
        let mut map = load_all();
        map.insert(id.to_string(), rec);
        let _ = save_all(&map);
    }

    // Block until the harness exits (or is killed — from here or the dashboard).
    let status = child.wait();

    // Clean up regardless of how it exited: drop the record.
    {
        let mut map = load_all();
        map.remove(id);
        let _ = save_all(&map);
    }
    Ok(status?)
}

/// Cap on the stdout a headless (`--locked --prompt`) run may hand back as its
/// captured result. Same figure as the executor's `MAX_RESULT_BYTES` (own
/// const on purpose — the crates don't share bounds across their boundary).
/// Output beyond the cap is drained but neither relayed nor hashed, and the
/// truncation is recorded honestly in the run's evidence.
pub(crate) const MAX_PROMPT_OUTPUT_BYTES: usize = 1024 * 1024;

/// What a bounded stdout capture observed: identity of the captured bytes,
/// never the bytes themselves (those were already relayed to our stdout).
#[derive(Debug, Clone)]
pub(crate) struct CapturedOutput {
    /// Bytes captured — the exact input to `sha256` (≤ the cap).
    pub bytes: u64,
    /// SHA-256 (hex) over the captured bytes.
    pub sha256: String,
    /// True when the recorded bytes may NOT be the child's complete output —
    /// either because it exceeded the cap OR because a read error cut the
    /// stream short before EOF. Both mean "do not read completeness into this
    /// digest"; the flag never claims completeness it cannot back.
    pub truncated: bool,
}

/// Relay `from` into `to` up to `cap` bytes, hashing exactly what was
/// captured; past the cap keep DRAINING (so the child never blocks on a full
/// pipe) but stop relaying and hashing, and report the truncation. A relay
/// write failure (e.g. our stdout is a pipe the reader closed) stops relaying
/// but not draining. A READ error mid-stream also sets `truncated` — the
/// stream ended abnormally, so the captured bytes cannot be attested as
/// complete; `bytes`/`sha256` still describe exactly what was captured.
fn relay_bounded(
    mut from: impl std::io::Read,
    mut to: impl std::io::Write,
    cap: usize,
) -> CapturedOutput {
    let mut captured: Vec<u8> = Vec::new();
    let mut truncated = false;
    let mut relay_ok = true;
    let mut buf = [0u8; 8192];
    loop {
        let n = match from.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => {
                // Read error before EOF: the capture is incomplete, and we
                // must not record it as a clean, complete output.
                truncated = true;
                break;
            }
        };
        let room = cap.saturating_sub(captured.len());
        let take = n.min(room);
        if take > 0 {
            captured.extend_from_slice(&buf[..take]);
            if relay_ok && to.write_all(&buf[..take]).is_err() {
                relay_ok = false;
            }
        }
        if take < n {
            truncated = true;
        }
    }
    if relay_ok {
        let _ = to.flush();
    }
    CapturedOutput {
        bytes: captured.len() as u64,
        sha256: agentstack_core::digest::sha256_hex(&captured),
        truncated,
    }
}

/// Spawn `bin` headless under an EXISTING run id: stdin closed (codex hangs on
/// an open stdin in exec mode — no opt-out), stdout piped through a bounded
/// relay onto OUR stdout (so the command is pipeable), stderr inherited. Track
/// it in the run registry, block until it exits, and return the exit status
/// plus the captured-output identity. The prompt-bearing argv is used for the
/// spawn only — it is deliberately NOT persisted into `runs.json` (the grant
/// commits argv by keyed digest; a prompt is likelier than flags to carry
/// sensitive text).
#[allow(clippy::too_many_arguments)]
pub(crate) fn launch_captured(
    bin: &str,
    argv: &[String],
    dir: &Path,
    id: &str,
    harness: &str,
    display: &str,
    profile: Option<&str>,
    scope: Scope,
) -> Result<(std::process::ExitStatus, CapturedOutput)> {
    let mut child = spawn_child_captured(bin, argv, dir, id)
        .with_context(|| format!("launching {display} (headless)"))?;
    let pid = child.id() as i32;
    let rec = RunRecord {
        id: id.to_string(),
        pid,
        harness: harness.to_string(),
        display: display.to_string(),
        command: bin.to_string(),
        args: Vec::new(), // never the prompt-bearing argv — see docstring
        cwd: dir.to_string_lossy().into_owned(),
        profile: profile.map(String::from),
        scope: profile.map(|_| scope.as_str().to_string()),
        started_unix: now_secs(),
        started_session: false,
    };
    {
        let mut map = load_all();
        map.insert(id.to_string(), rec);
        let _ = save_all(&map);
    }

    // Read stdout on a thread WHILE waiting, or a chatty child fills the pipe
    // and deadlocks against our `wait()`. (Rust note: `take()` moves the pipe
    // handle out of `child` so the thread owns it — ownership transfer, like
    // handing the only reference across a worker boundary.)
    let stdout = child.stdout.take().expect("piped stdout");
    let reader = std::thread::spawn(move || {
        relay_bounded(stdout, std::io::stdout().lock(), MAX_PROMPT_OUTPUT_BYTES)
    });
    let status = child.wait();
    // A panicked relay thread means the output identity is UNKNOWN — error
    // out rather than fabricate a zero-byte digest (the recorder contract:
    // observed evidence or an explicit failure, never an invented value).
    let captured = reader.join();

    {
        let mut map = load_all();
        map.remove(id);
        let _ = save_all(&map);
    }
    let captured = captured.map_err(|_| {
        anyhow::anyhow!("the stdout capture thread failed — output evidence is unavailable")
    })?;
    Ok((status?, captured))
}

#[cfg(unix)]
fn spawn_child_captured(
    bin: &str,
    args: &[String],
    cwd: &Path,
    run_id: &str,
) -> Result<std::process::Child> {
    let mut cmd = std::process::Command::new(bin);
    cmd.args(args)
        .current_dir(cwd)
        .env(crate::calllog::RUN_ID_ENV, run_id)
        // ALWAYS closed: codex `exec` hangs forever on an open stdin, and a
        // headless run has no terminal conversation to inherit anyway.
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit());
    // Own process group so kill(-pgid) later reaps the child and its tree.
    crate::sys::spawn_in_new_process_group(&mut cmd);
    cmd.spawn().map_err(Into::into)
}

#[cfg(not(unix))]
fn spawn_child_captured(
    _bin: &str,
    _args: &[String],
    _cwd: &Path,
    _run_id: &str,
) -> Result<std::process::Child> {
    anyhow::bail!("`agentstack run` is not supported on this platform yet")
}

/// How long to wait for a graceful `SIGTERM` before escalating to `SIGKILL`.
const GRACE: Duration = Duration::from_millis(2000);

/// Kill a run by id: signal its whole process group, then drop the record. The
/// foreground `agentstack run` (a different process group) survives, observes its
/// child die, and reverts any profile it applied.
///
/// `force` sends `SIGKILL` immediately. Otherwise we ask politely with `SIGTERM`,
/// give the process [`GRACE`] to leave, then escalate to `SIGKILL` if it's still
/// alive — so a hung agent actually dies.
pub fn kill(id: &str, force: bool) -> Result<()> {
    let mut map = load_all();
    let rec = map
        .get(id)
        .cloned()
        .with_context(|| format!("no run '{id}' (it may have already exited)"))?;
    terminate(rec.pid, force)?;
    map.remove(id);
    save_all(&map)?;
    Ok(())
}

#[cfg(unix)]
fn spawn_child(
    bin: &str,
    args: &[String],
    cwd: &Path,
    run_id: &str,
) -> Result<std::process::Child> {
    let mut cmd = std::process::Command::new(bin);
    cmd.args(args)
        .current_dir(cwd)
        .env(crate::calllog::RUN_ID_ENV, run_id);
    // Own process group so kill(-pgid) later reaps the child and its tree.
    crate::sys::spawn_in_new_process_group(&mut cmd);
    cmd.spawn().map_err(Into::into)
}

#[cfg(not(unix))]
fn spawn_child(
    _bin: &str,
    _args: &[String],
    _cwd: &Path,
    _run_id: &str,
) -> Result<std::process::Child> {
    anyhow::bail!("`agentstack run` is not supported on this platform yet")
}

fn terminate(pid: i32, force: bool) -> Result<()> {
    if force {
        return signal_run_group(pid, crate::sys::Signal::Kill);
    }
    signal_run_group(pid, crate::sys::Signal::Term)?;
    // Poll until it's gone, then hard-kill anything that ignored SIGTERM.
    let deadline = Instant::now() + GRACE;
    while Instant::now() < deadline {
        if !crate::sys::pid_alive(pid) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    if crate::sys::pid_alive(pid) {
        signal_run_group(pid, crate::sys::Signal::Kill)?;
    }
    Ok(())
}

/// Signal the run's whole process group, treating "already gone" (ESRCH) as
/// success so a race with the process exiting still cleans the registry.
fn signal_run_group(pid: i32, sig: crate::sys::Signal) -> Result<()> {
    match crate::sys::signal_group(pid, sig) {
        Ok(()) => Ok(()),
        Err(e) if crate::sys::is_already_gone(&e) => Ok(()),
        Err(e) => Err(anyhow::anyhow!("signalling run: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(pid: i32) -> RunRecord {
        RunRecord {
            id: format!("r-{pid}"),
            pid,
            harness: "claude-code".into(),
            display: "Claude Code".into(),
            command: "claude".into(),
            args: vec![],
            cwd: "/tmp".into(),
            profile: None,
            scope: None,
            started_unix: 0,
            started_session: false,
        }
    }

    #[test]
    fn gen_id_is_well_formed() {
        let id = gen_id();
        assert!(id.starts_with("r-"));
        assert_eq!(id.len(), 12);
    }

    /// W2 witness: output past the cap truncates honestly — the sink and the
    /// hash cover exactly the first `cap` bytes, `bytes` says so, and
    /// `truncated` is true; under the cap nothing is cut and `truncated` is
    /// false. The evidence never claims more (or less) than was captured.
    #[test]
    fn relay_bounded_truncates_at_cap_and_records_it() {
        let input = vec![b'x'; 20];
        let mut sink = Vec::new();
        let out = relay_bounded(&input[..], &mut sink, 8);
        assert_eq!(sink, vec![b'x'; 8], "relay stops at the cap");
        assert_eq!(out.bytes, 8);
        assert_eq!(out.sha256, agentstack_core::digest::sha256_hex(&input[..8]));
        assert!(out.truncated);

        let mut sink = Vec::new();
        let out = relay_bounded(&input[..], &mut sink, 64);
        assert_eq!(sink, input, "under the cap nothing is cut");
        assert_eq!(out.bytes, 20);
        assert!(!out.truncated);
    }

    #[cfg(unix)]
    #[test]
    fn prune_drops_dead_pids_keeps_live() {
        let mut map = BTreeMap::new();
        // Our own process is certainly alive.
        map.insert("live".to_string(), rec(std::process::id() as i32));
        // An implausibly high pid is certainly not.
        map.insert("dead".to_string(), rec(2_000_000_000));
        let (pruned, changed) = prune(map);
        assert!(changed);
        assert!(pruned.contains_key("live"));
        assert!(!pruned.contains_key("dead"));
    }
}
