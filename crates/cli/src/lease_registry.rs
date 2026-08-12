//! The machine-level runtime lease registry (W4,
//! `docs/design/automatic-delivery.md` §"Lease lifecycle").
//!
//! A lease used to live only in the MCP subprocess's memory, so no other
//! surface — `status`, `doctor`, the panel — could see "a lease is open on
//! toolset X". This module is the shared view, and it is deliberately **not**
//! "a JSON state file treated as current truth":
//!
//! - a **record** is persisted when a lease opens and removed when it closes;
//! - **liveness is derived at read time**, never stored, from the recorded PID
//!   *and* that process's start time.
//!
//! The start time is what makes the derivation honest. A crashed MCP process
//! leaves its record behind, and the operating system is free to hand that PID
//! to something else — so "the PID exists" alone would read a dead lease as
//! live the moment a PID was reused. Comparing the recorded start time against
//! the live one distinguishes *this* process from *a* process with the same
//! number.
//!
//! Where a platform cannot supply a start time we report [`Liveness::Unknown`]
//! rather than guessing. Unknown is a real answer, and it is the fail-closed
//! one: nothing here may ever read as `live` without the start time agreeing.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::util::paths;

/// One open-lease record. Everything here is what was true *when the lease
/// opened*; nothing in it is a liveness claim — see [`liveness`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeaseRecord {
    /// Identifies this lease instance, so two leases from the same toolset (or
    /// the same PID over time) are never confused for one another.
    pub instance: String,
    /// The project the lease was opened over — the resolved manifest dir.
    pub project: String,
    /// The toolset the lease fenced to.
    pub toolset: String,
    /// The process that owns the lease.
    pub pid: i32,
    /// That process's start time, as an opaque platform token compared only
    /// for equality (see [`process_start_token`]). `None` when this platform
    /// could not supply one — which forces [`Liveness::Unknown`] forever after,
    /// deliberately.
    #[serde(default)]
    pub start_token: Option<String>,
    /// When the lease opened (unix seconds), for display only.
    pub started_unix: u64,
}

/// What a record's PID + start-time check says about it *right now*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    /// The recorded process exists and its start time matches: this lease is
    /// really open.
    Live,
    /// The recorded process is gone, or a different process now holds that PID
    /// (reuse). Either way the lease it described no longer exists.
    Stale,
    /// The PID exists, but no start time is available to rule out reuse — so
    /// whether the lease is open cannot be established. Never treat this as
    /// live.
    Unknown,
}

impl Liveness {
    pub fn as_str(self) -> &'static str {
        match self {
            Liveness::Live => "live",
            Liveness::Stale => "stale",
            Liveness::Unknown => "unknown",
        }
    }

    /// One sentence a surface can print verbatim, so every renderer says the
    /// same thing about the same state.
    pub fn why(self) -> &'static str {
        match self {
            Liveness::Live => {
                "the recorded process is running and its start time matches the record"
            }
            Liveness::Stale => {
                "the recorded process is gone, or its PID now belongs to a different process"
            }
            Liveness::Unknown => {
                "this platform supplied no process start time, so PID reuse cannot be ruled out — \
                 treat this as not established, never as live"
            }
        }
    }
}

/// Where records live. One file per lease rather than one shared map: two MCP
/// processes opening leases at the same moment then touch disjoint paths, so
/// neither can lose the other's write by saving its own stale copy of a map.
fn registry_dir() -> PathBuf {
    paths::agentstack_home().join("leases")
}

/// Serialize registry mutations across processes with the same discipline the
/// trust store uses (`crates/trust/src/lib.rs::with_store_lock`): `create_dir`
/// is the atomic primitive — it either creates the sentinel or fails because it
/// exists — so mutual exclusion needs nothing beyond the standard library. A
/// sentinel older than the stale bound is a crashed writer and gets broken.
///
/// Reads deliberately do NOT take this lock. A read never mutates, and a
/// per-file read that loses a race with a concurrent write simply skips that
/// file — which is the same answer it would have got a millisecond earlier.
fn with_registry_lock<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    const WAIT: std::time::Duration = std::time::Duration::from_secs(5);
    const STALE: std::time::Duration = std::time::Duration::from_secs(30);
    let lock_dir = paths::agentstack_home().join("leases.lock.d");
    let deadline = std::time::Instant::now() + WAIT;
    loop {
        match std::fs::create_dir(&lock_dir) {
            Ok(()) => break,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let stale = std::fs::metadata(&lock_dir)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .is_some_and(|age| age > STALE);
                if stale {
                    // Best-effort: losing this race only means retrying.
                    let _ = std::fs::remove_dir(&lock_dir);
                    continue;
                }
                anyhow::ensure!(
                    std::time::Instant::now() < deadline,
                    "the lease registry is locked by another agentstack process ({} exists) — \
                     retry, or remove it if no other process is running",
                    lock_dir.display()
                );
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // First write on this machine: the home dir itself is missing.
                std::fs::create_dir_all(paths::agentstack_home())
                    .context("creating the agentstack home directory")?;
            }
            Err(e) => return Err(e).context("locking the lease registry"),
        }
    }
    let out = f();
    let _ = std::fs::remove_dir(&lock_dir);
    out
}

/// This process's start time as an opaque token, compared only for equality.
///
/// Two platforms, two routes, no new dependency and no new `unsafe` (CLAUDE.md
/// invariant 1 — nothing here goes near `sys.rs`):
///
/// - **Linux** reads `/proc/<pid>/stat` field 22, the process's start time in
///   clock ticks since boot. Pure filesystem read.
/// - **macOS** has no `/proc`, and the kernel route (`sysctl KERN_PROC_PID`)
///   is a raw FFI call this crate is not allowed to make. So it shells out to
///   `/bin/ps -o lstart=`, which reports an absolute start timestamp. The
///   argument is an integer PID we formatted ourselves and it is passed as an
///   argv entry, never through a shell (invariant 7).
///
/// Anywhere else: `None`, which makes every record read [`Liveness::Unknown`].
/// That is the honest direction to fail — a platform that cannot answer must
/// not have an answer invented for it.
#[cfg(target_os = "linux")]
pub fn process_start_token(pid: i32) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // Field 2 is the executable name in parentheses and may itself contain
    // spaces and ')', so the only safe split point is the LAST ')'. Fields
    // resume at 3 (state) after it, which puts field 22 (starttime) at index
    // 19 of what remains.
    let after_comm = stat.rsplit_once(')')?.1;
    after_comm
        .split_whitespace()
        .nth(19)
        .filter(|token| !token.is_empty())
        .map(str::to_string)
}

#[cfg(target_os = "macos")]
pub fn process_start_token(pid: i32) -> Option<String> {
    let out = std::process::Command::new("/bin/ps")
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    // `lstart` is padded and trailing-spaced; collapsing whitespace makes the
    // token stable across reads, which equality comparison depends on.
    let text = String::from_utf8(out.stdout).ok()?;
    let token = text.split_whitespace().collect::<Vec<_>>().join(" ");
    Some(token).filter(|t| !t.is_empty())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn process_start_token(_pid: i32) -> Option<String> {
    None
}

/// Derive whether `record` describes a lease that is really open, right now.
///
/// This is the whole point of the registry. The record is evidence of an
/// intent; only this function makes a liveness claim, and it makes it from the
/// operating system every time it is asked.
pub fn liveness(record: &LeaseRecord) -> Liveness {
    if !crate::sys::pid_alive(record.pid) {
        return Liveness::Stale;
    }
    match (
        record.start_token.as_deref(),
        process_start_token(record.pid),
    ) {
        // The decisive comparison: same PID, same start time — same process.
        (Some(recorded), Some(current)) if recorded == current => Liveness::Live,
        // Same PID, different start time: the OS reused the number. The lease
        // this record describes died with the process that opened it.
        (Some(_), Some(_)) => Liveness::Stale,
        // No start time on one side or the other. PID reuse cannot be ruled
        // out, so nothing may be claimed.
        _ => Liveness::Unknown,
    }
}

/// Write a record for a lease opening now, and return it so the caller can hold
/// the instance id for [`unregister`].
///
/// Stale records are pruned here rather than on read: this path already holds
/// the registry lock, and keeping the read side free of writes is what lets a
/// panel poll it without becoming a writer to machine state.
pub fn register(project: &Path, toolset: &str) -> Result<LeaseRecord> {
    let pid = std::process::id() as i32;
    let record = LeaseRecord {
        instance: new_instance_id(pid),
        project: project.display().to_string(),
        toolset: toolset.to_string(),
        pid,
        start_token: process_start_token(pid),
        started_unix: now_secs(),
    };
    let path = registry_dir().join(format!("{}.json", record.instance));
    let body = serde_json::to_string_pretty(&record)?;
    with_registry_lock(|| {
        std::fs::create_dir_all(registry_dir()).context("creating the lease registry directory")?;
        prune_stale_locked();
        crate::util::atomic::write(&path, &body).context("writing the lease record")?;
        Ok(())
    })?;
    Ok(record)
}

/// Remove one lease's record. Best-effort by contract: the caller is closing a
/// lease either way, and a record left behind is exactly the case read-time
/// validation already classifies as stale.
pub fn unregister(instance: &str) {
    let path = registry_dir().join(format!("{instance}.json"));
    let _ = with_registry_lock(|| {
        let _ = std::fs::remove_file(&path);
        Ok(())
    });
}

/// Every record on this machine with the liveness derived for each, newest
/// first. Read-only: stale records are reported as stale, not deleted, so this
/// is safe to call from any surface at any time.
pub fn open_leases() -> Vec<(LeaseRecord, Liveness)> {
    let mut out: Vec<(LeaseRecord, Liveness)> = read_records()
        .into_iter()
        .map(|record| {
            let state = liveness(&record);
            (record, state)
        })
        .collect();
    out.sort_by_key(|(record, _)| std::cmp::Reverse(record.started_unix));
    out
}

/// Live leases for one manifest directory, newest first. This is a reporting
/// helper only; authorization never consults the registry.
pub fn live_for_project(project: &Path) -> Vec<LeaseRecord> {
    let project = crate::manifest::resolve_manifest_dir(project);
    open_leases()
        .into_iter()
        .filter(|(record, state)| *state == Liveness::Live && Path::new(&record.project) == project)
        .map(|(record, _)| record)
        .collect()
}

fn read_records() -> Vec<LeaseRecord> {
    let Ok(entries) = std::fs::read_dir(registry_dir()) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|body| serde_json::from_str::<LeaseRecord>(&body).ok())
        .collect()
}

/// Drop records that no longer describe a live lease. Callers must already hold
/// the registry lock.
fn prune_stale_locked() {
    let Ok(entries) = std::fs::read_dir(registry_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let drop = match std::fs::read_to_string(&path)
            .ok()
            .and_then(|body| serde_json::from_str::<LeaseRecord>(&body).ok())
        {
            // Unparseable leftovers are not records of anything.
            None => true,
            Some(record) => liveness(&record) == Liveness::Stale,
        };
        if drop {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// A per-lease id that is unique on this machine without a new dependency: the
/// owning PID plus the nanosecond the lease opened. Two leases in the same
/// process are ordered in time, and two processes differ by PID.
fn new_instance_id(pid: i32) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{pid:x}-{nanos:x}")
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The decisive property, at the unit level: a record whose PID is dead and
    /// a record whose PID exists under a DIFFERENT start time both read stale.
    /// The second is PID reuse, which is the entire reason the start token is
    /// part of the record.
    #[test]
    fn stale_and_reused_pids_never_read_live() {
        let me = std::process::id() as i32;
        let live = LeaseRecord {
            instance: "t-live".into(),
            project: "/tmp/proj".into(),
            toolset: "backend".into(),
            pid: me,
            start_token: process_start_token(me),
            started_unix: 0,
        };
        // Only assert `Live` where the platform can actually supply the token;
        // elsewhere `Unknown` is the correct, honest answer.
        if live.start_token.is_some() {
            assert_eq!(liveness(&live), Liveness::Live);
        } else {
            assert_eq!(liveness(&live), Liveness::Unknown);
        }

        let reused = LeaseRecord {
            start_token: Some("not-the-start-time-of-this-process".into()),
            ..live.clone()
        };
        assert_eq!(
            liveness(&reused),
            Liveness::Stale,
            "a PID whose start time disagrees is a reused PID, not a live lease"
        );

        // A pid well above any live process; if it happens to exist we only
        // lose the negative assertion, never soundness.
        let dead = LeaseRecord {
            pid: 2_000_000_000,
            ..live.clone()
        };
        assert_eq!(liveness(&dead), Liveness::Stale);

        // No recorded token (a platform that cannot supply one) can never read
        // live, however alive the PID is.
        let tokenless = LeaseRecord {
            start_token: None,
            ..live
        };
        assert_eq!(liveness(&tokenless), Liveness::Unknown);
    }

    #[test]
    fn instance_ids_do_not_collide_within_one_process() {
        let a = new_instance_id(7);
        let b = new_instance_id(7);
        assert_ne!(a, b);
    }
}
