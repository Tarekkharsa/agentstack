//! Live harness runs: launch an agent CLI as a tracked child process, list the
//! ones currently alive, and kill them — from the terminal or the dashboard,
//! without ever opening Activity Monitor.
//!
//! A *run* is distinct from a [`crate::session`] (an ephemeral profile load keyed
//! by directory). A run is a real OS process that agentstack owns: we spawn the
//! harness binary in its own process group so we can tree-kill it later, and we
//! record it in `~/.agentstack/runs.json` so a *separate* agentstack process (the
//! dashboard) can see and stop it. When launched with a profile we reuse the
//! session machinery to apply it before launch and revert it on exit.
//!
//! Control split: launching is a terminal act (`agentstack run <harness>` runs the
//! TUI attached to your terminal, with agentstack as its parent); observing and
//! killing also work from the dashboard, which only holds the PID and so signals
//! the process group rather than a `Child` handle.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::scope::Scope;
use crate::util::paths;

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
fn gen_id() -> String {
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

/// Whether a pid is still alive. `kill(pid, 0)` sends no signal — it just probes
/// existence/permission, returning 0 when the process is there.
#[cfg(unix)]
fn is_alive(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}

#[cfg(not(unix))]
fn is_alive(_pid: i32) -> bool {
    true
}

/// Drop records whose process has exited. Returns the surviving map and whether
/// anything was removed (so the caller can persist the cleanup).
fn prune(mut map: BTreeMap<String, RunRecord>) -> (BTreeMap<String, RunRecord>, bool) {
    let before = map.len();
    map.retain(|_, r| is_alive(r.pid));
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

/// Launch `harness` as a tracked child, optionally applying `profile` for the
/// life of the run. Blocks until the harness exits (it's attached to this
/// terminal), then prunes the record and reverts the profile unless `keep`.
pub fn launch(
    manifest_dir: Option<&Path>,
    harness: &str,
    profile: Option<&str>,
    scope: Scope,
    extra_args: &[String],
    keep: bool,
) -> Result<()> {
    let ctx = crate::commands::load(manifest_dir)?;
    let desc = ctx
        .registry
        .get(harness)
        .with_context(|| format!("unknown harness '{harness}' — see `agentstack adapters list`"))?;
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
        crate::session::start(manifest_dir, p, scope, None)
            .with_context(|| format!("applying profile '{p}' for this run"))?;
        started_session = true;
    }

    // Spawn the harness in its own process group so a later kill takes the
    // tree. The run id rides along as an env var so tool calls the harness's
    // agent makes through `agentstack mcp` land in the audit log attributed
    // to this run.
    let id = gen_id();
    let mut child = match spawn_child(&bin, extra_args, &dir, &id) {
        Ok(c) => c,
        Err(e) => {
            if started_session && !keep {
                let _ = crate::session::end(manifest_dir);
            }
            return Err(e).with_context(|| format!("launching {display}"));
        }
    };
    let pid = child.id() as i32;
    let rec = RunRecord {
        id: id.clone(),
        pid,
        harness: harness.to_string(),
        display,
        command: bin,
        args: extra_args.to_vec(),
        cwd: dir.to_string_lossy().into_owned(),
        profile: profile.map(String::from),
        scope: profile.map(|_| scope.as_str().to_string()),
        started_unix: now_secs(),
        started_session,
    };
    {
        let mut map = load_all();
        map.insert(id.clone(), rec);
        let _ = save_all(&map);
    }

    // Block until the harness exits (or is killed — from here or the dashboard).
    let status = child.wait();

    // Clean up regardless of how it exited: drop the record, revert the profile.
    {
        let mut map = load_all();
        map.remove(&id);
        let _ = save_all(&map);
    }
    if started_session && !keep {
        let _ = crate::session::end(manifest_dir);
    }
    status?;
    Ok(())
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
    use std::os::unix::process::CommandExt;
    let mut cmd = std::process::Command::new(bin);
    cmd.args(args)
        .current_dir(cwd)
        .env(crate::calllog::RUN_ID_ENV, run_id);
    // setpgid(0, 0): make the child its own process-group leader so kill(-pgid)
    // later reaps it and anything it spawned.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setpgid(0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
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

#[cfg(unix)]
fn terminate(pid: i32, force: bool) -> Result<()> {
    if force {
        return signal_pgid(pid, libc::SIGKILL);
    }
    signal_pgid(pid, libc::SIGTERM)?;
    // Poll until it's gone, then hard-kill anything that ignored SIGTERM.
    let deadline = Instant::now() + GRACE;
    while Instant::now() < deadline {
        if !is_alive(pid) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    if is_alive(pid) {
        signal_pgid(pid, libc::SIGKILL)?;
    }
    Ok(())
}

/// Send `sig` to the whole process group led by `pid` (negative pid → group).
#[cfg(unix)]
fn signal_pgid(pid: i32, sig: i32) -> Result<()> {
    let rc = unsafe { libc::kill(-pid, sig) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        // ESRCH = already gone; treat as success so the registry still gets cleaned.
        if err.raw_os_error() != Some(libc::ESRCH) {
            return Err(anyhow::anyhow!("signalling run: {err}"));
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn terminate(_pid: i32, _force: bool) -> Result<()> {
    anyhow::bail!("killing runs is not supported on this platform yet")
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
