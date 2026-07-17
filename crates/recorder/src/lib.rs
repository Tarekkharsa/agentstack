//! Append-only **diagnostic call log** of every tool call brokered by the
//! runtime gateway (`agentstack mcp` proxied calls and code-mode runtime
//! calls alike): `~/.agentstack/audit/calls.jsonl`, one JSON object per line.
//!
//! What's recorded: timestamp, run id (when the harness was launched by
//! `agentstack run`, via `AGENTSTACK_RUN_ID`), pid, project dir, server, tool,
//! a **keyed** SHA-256 digest of the arguments, outcome (`ok` / `error` /
//! `denied`), a short detail (the policy rule, or a fixed error class — never
//! upstream-authored text), and latency. What's never recorded: argument
//! values, results, resolved secrets, or anything an upstream server wrote —
//! a malicious server must not be able to inject content into this file.
//!
//! The digest key is a per-machine random secret (`audit/key`, mode 0600):
//! digests still correlate identical calls on this machine, but an exfiltrated
//! log alone can't confirm guessed argument values. The log and its directory
//! are created 0600/0700.
//!
//! Honest scope: this is best-effort local diagnostics (a logging hiccup must
//! never fail the call it describes — same contract as `usage::bump`), with
//! size-capped rotation of ~5 MB × two generations. It is **not** durable or
//! tamper-evident: any local process running as the user can edit it.

#![forbid(unsafe_code)]

use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use agentstack_core::util::paths;

const MAX_BYTES: u64 = 5 * 1024 * 1024;
const TAIL_CHUNK_BYTES: usize = 64 * 1024;

/// The env var `agentstack run` sets on the harness it launches, so calls made
/// by that run's agent can be attributed to the run.
pub const RUN_ID_ENV: &str = "AGENTSTACK_RUN_ID";

/// The outcome of one proxied tool call — a closed 3-value set. Serializes to
/// the same `"ok"` / `"error"` / `"denied"` the log has always used, so the
/// persisted wire form is byte-identical (a stale reader parses it unchanged).
/// Typed so the report/analyze consumers match variants instead of magic
/// strings, and a typo like `"Denied"` can't slip through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallOutcome {
    Ok,
    Error,
    Denied,
}

impl CallOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            CallOutcome::Ok => "ok",
            CallOutcome::Error => "error",
            CallOutcome::Denied => "denied",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallRecord {
    pub ts: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    pub pid: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub server: String,
    pub tool: String,
    /// First 12 hex chars of SHA-256 over the serialized arguments.
    pub args_digest: String,
    pub outcome: CallOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub ms: u64,
}

pub fn log_path() -> PathBuf {
    paths::agentstack_home().join("audit").join("calls.jsonl")
}

fn key_path() -> PathBuf {
    paths::agentstack_home().join("audit").join("key")
}

/// The per-machine digest key: 32 random bytes, created once with mode 0600.
/// Read fresh per call (tiny file; calls are network-scale) so tests and
/// relocated `AGENTSTACK_HOME`s behave. On a creation race the first writer
/// wins and everyone re-reads. `None` only when the key can neither be read
/// nor created — the caller falls back to an unkeyed digest rather than
/// dropping the record.
fn digest_key() -> Option<Vec<u8>> {
    let path = key_path();
    if let Ok(k) = fs::read(&path) {
        if k.len() >= 16 {
            return Some(k);
        }
    }
    let dir = path.parent()?;
    fs::create_dir_all(dir).ok()?;
    agentstack_core::util::restrict(dir, true);
    let key = agentstack_core::util::random_bytes();
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    match opts.open(&path) {
        Ok(mut f) => {
            f.write_all(&key).ok()?;
            Some(key)
        }
        // Lost the creation race — the other writer's key is the key (same
        // length floor as the primary read: a partial write is not a key).
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            fs::read(&path).ok().filter(|k| k.len() >= 16)
        }
        Err(_) => None,
    }
}

/// First 12 hex chars of SHA-256 over the per-machine key + the serialized
/// arguments. Keyed so identical calls still correlate locally, but the log
/// alone (without `audit/key`) can't confirm a guessed argument value.
pub fn digest_args(args: &Value) -> String {
    let mut h = Sha256::new();
    match digest_key() {
        Some(key) => h.update(&key),
        None => {
            // Unkeyed fallback: correlation across restarts degrades and the
            // guess-resistance property is lost for these records — say so
            // once instead of silently mixing digest kinds.
            static WARNED: std::sync::atomic::AtomicBool =
                std::sync::atomic::AtomicBool::new(false);
            if !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                eprintln!(
                    "warning: call-log digest key unavailable ({}); argument digests are unkeyed this session",
                    key_path().display()
                );
            }
        }
    }
    h.update(serde_json::to_string(args).unwrap_or_default().as_bytes());
    let hex = format!("{:x}", h.finalize());
    hex[..12].to_string()
}

/// Append one record. Best-effort: any failure is swallowed — a call-log
/// hiccup must never fail the tool call it describes.
pub fn record(rec: &CallRecord) {
    let path = log_path();
    let Some(dir) = path.parent() else { return };
    if fs::create_dir_all(dir).is_err() {
        return;
    }
    agentstack_core::util::restrict(dir, true);
    // Size-capped rotation: current → .1 (previous generation dropped).
    if fs::metadata(&path)
        .map(|m| m.len() > MAX_BYTES)
        .unwrap_or(false)
    {
        let _ = fs::rename(&path, path.with_extension("jsonl.1"));
    }
    let Ok(line) = serde_json::to_string(rec) else {
        return;
    };
    let mut opts = fs::OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    if let Ok(mut f) = opts.open(&path) {
        // mode() applies only at creation — tighten a log that predates the
        // 0600 default (or survived a mode-preserving restore) too.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
        }
        let _ = writeln!(f, "{line}");
    }
}

/// Read the log, newest last. Unparseable lines are skipped (a torn write
/// from a crash must not brick the whole log).
pub fn read_all() -> Vec<CallRecord> {
    let Ok(text) = fs::read_to_string(log_path()) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// Read at most the last `n` parseable records, newest last, without reading
/// the whole log when the requested tail fits near the end of the file.
/// Malformed lines and a leading fragment caused by a backward seek are
/// skipped.
pub fn read_tail(n: usize) -> Vec<CallRecord> {
    if n == 0 {
        return Vec::new();
    }
    let Ok(mut file) = fs::File::open(log_path()) else {
        return Vec::new();
    };
    let Ok(mut start) = file.seek(SeekFrom::End(0)) else {
        return Vec::new();
    };
    let mut window = Vec::new();

    loop {
        let chunk_len = start.min(TAIL_CHUNK_BYTES as u64) as usize;
        start -= chunk_len as u64;
        if file.seek(SeekFrom::Start(start)).is_err() {
            return Vec::new();
        }
        let mut chunk = vec![0; chunk_len];
        if file.read_exact(&mut chunk).is_err() {
            return Vec::new();
        }
        chunk.extend_from_slice(&window);
        window = chunk;

        let complete = if start == 0 {
            window.as_slice()
        } else {
            match window.iter().position(|byte| *byte == b'\n') {
                Some(boundary) => &window[boundary + 1..],
                None => &[],
            }
        };
        let records: Vec<_> = complete
            .split(|byte| *byte == b'\n')
            .filter_map(|line| serde_json::from_slice(line).ok())
            .collect();
        if records.len() >= n || start == 0 {
            return records.into_iter().rev().take(n).rev().collect();
        }
    }
}

pub fn now_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ─────────────────────────── run-scoped flight recorder ───────────────────────
//
// The machine-global `calls.jsonl` above is diagnostics across every project.
// A *sandboxed run* (Phase 2 `agentstack run --sandbox`) also gets its OWN
// append-only event log under `~/.agentstack/runs/<run-id>/events.jsonl`, so a
// Phase 3 `agentstack report run <id>` can read exactly one run's lifecycle and
// the egress proxy's per-decision output — separate from the cross-project
// diagnostic log. Synchronous, best-effort, and `core`-only by design: the
// async runtime/egress crates own a channel and drain it into these plain
// appends, so the recorder itself never pulls in an async runtime.
//
// The event set is a seed — only the variants the runtime (container
// lifecycle) and egress (per-host decisions) crates emit today. More land as
// those crates grow; the report viewer waits until Phase 3.

/// One line in a run's `events.jsonl`. `#[serde(tag = "event")]` makes each
/// row self-describing (`{"event":"egress",…}`) so the future report reader
/// needs no schema out of band.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum RunEvent {
    /// A governed ephemeral-code execution started. Digests identify the
    /// source, input, runtime, and frozen authority without recording their
    /// sensitive contents.
    ExecutionStarted {
        ts: u64,
        execution_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent_run_id: Option<String>,
        source_digest: String,
        input_digest: String,
        authority_digest: String,
        runtime_digest: String,
        granted_tools: Vec<String>,
        limits: Value,
    },
    /// Terminal evidence for one governed execution. The result is represented
    /// by digest only; raw source, input, output, and secrets are never events.
    ExecutionFinished {
        ts: u64,
        execution_id: String,
        outcome: String,
        duration_ms: u64,
        calls: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        result_digest: Option<String>,
        stdout_bytes: usize,
        stderr_bytes: usize,
    },
    /// One hard executor limit ended or rejected an execution.
    ExecutionLimitHit {
        ts: u64,
        execution_id: String,
        limit: String,
        observed: u64,
    },
    /// The sandbox container was created and started.
    SandboxStarted {
        ts: u64,
        image: String,
        /// Host path mounted as the container's workspace.
        workspace: String,
    },
    /// The egress proxy allowed or blocked one outbound connection, attributed
    /// to the MCP server that opened it. `rule` names the matching policy line
    /// on a block.
    Egress {
        ts: u64,
        server: String,
        host: String,
        allowed: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        rule: Option<String>,
    },
    /// One tool call the run's agent made through the gateway, mirrored into
    /// this run's log so a report reads the run's ACTIONS without cross-
    /// referencing the machine-global `calls.jsonl`. Sensitive fields follow
    /// that audit record exactly: only the keyed argument DIGEST is stored,
    /// never values or resolved secrets; `outcome` is `ok` / `error` /
    /// `denied`; `detail` is the policy rule on a block or a fixed error class
    /// on a failure — never upstream-authored text.
    ToolCall {
        ts: u64,
        /// Governed execution that caused this call, when the gateway call
        /// came from the ephemeral executor rather than the ambient agent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        execution_id: Option<String>,
        server: String,
        tool: String,
        outcome: CallOutcome,
        /// Keyed SHA-256 digest prefix over the arguments (see
        /// [`digest_args`]) — the same value `calls.jsonl` stores, never the
        /// argument values themselves.
        args_digest: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
        ms: u64,
    },
    /// A secret reference this run resolved, by ref NAME only — never the
    /// value. Attributed to the server the ref was resolved for, so a reviewer
    /// can see a run's secret surface without any value ever touching the log.
    SecretAccess {
        ts: u64,
        server: String,
        /// The `${REF}` name (e.g. `OPENAI_API_KEY`). `ref` is a Rust keyword,
        /// so the field is `reference` in code but `"ref"` on the wire.
        #[serde(rename = "ref")]
        reference: String,
    },
    /// The sandbox container exited. `code` is absent when it was killed by a
    /// signal (e.g. teardown).
    SandboxExited {
        ts: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        code: Option<i32>,
    },
    /// A locked host-run attempt opened — emitted BEFORE any gate (locked-run
    /// contract §3 step 2), so a refusal is itself recorded evidence. Carries
    /// invocation identity only: never argv (caller-supplied, possibly
    /// secret-bearing; §4) and no grant digest (the grant is not frozen yet).
    AttemptStarted {
        ts: u64,
        harness: String,
        posture: String,
    },
    /// One pre-launch gate's decision (trust / locked-verify /
    /// policy-admission). Emitted before the grant freeze, so it carries no
    /// grant digest by construction (§9). `detail` is the observed state or
    /// the refusal text — never secret values, never raw argv.
    GateDecision {
        ts: u64,
        gate: String,
        passed: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    /// The `AuthorityGrant` froze (§3 step 6). Every material event from here
    /// on can carry this digest.
    GrantFrozen { ts: u64, grant_digest: String },
    /// Terminal outcome of a locked run attempt: a pre-launch refusal (no
    /// grant digest), a launch failure, or the harness exit. `usage` carries
    /// observed token/cost evidence or the literal `"unavailable"` — never a
    /// fabricated value or a zero standing in for unknown (§9).
    LockedOutcome {
        ts: u64,
        outcome: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
        duration_ms: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        grant_digest: Option<String>,
        usage: String,
    },
}

/// The append-only event log for one sandboxed run.
///
/// Construct once per run with [`RunLog::create`] (which prepares the run's
/// private directory), hold it for the run's lifetime, and [`append`] events
/// as they happen. Reading back for a report is [`RunLog::read`].
///
/// [`append`]: RunLog::append
pub struct RunLog {
    dir: PathBuf,
}

/// A run id is safe to use as a single directory segment: non-empty, and only
/// the characters `agentstack run`'s `gen_id` produces (`r-<hex>`) plus the
/// conservative superset a user-set `AGENTSTACK_RUN_ID` might carry. Rejects
/// anything with a path separator, `..`, or other surprises — defensive even
/// though ids are agentstack-generated, so a stray env value can never escape
/// the runs directory.
fn safe_run_segment(run_id: &str) -> bool {
    !run_id.is_empty()
        && run_id.len() <= 128
        && run_id != "."
        && run_id != ".."
        && run_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
}

fn run_dir(run_id: &str) -> Option<PathBuf> {
    if !safe_run_segment(run_id) {
        return None;
    }
    Some(paths::agentstack_home().join("runs").join(run_id))
}

impl RunLog {
    /// Prepare a run's private event directory (0700). `None` when `run_id`
    /// isn't a safe path segment (see [`safe_run_segment`]).
    pub fn create(run_id: &str) -> Option<RunLog> {
        let dir = run_dir(run_id)?;
        fs::create_dir_all(&dir).ok()?;
        agentstack_core::util::restrict(&dir, true);
        Some(RunLog { dir })
    }

    /// The events file path for this run.
    pub fn path(&self) -> PathBuf {
        self.dir.join("events.jsonl")
    }

    /// Append one event. Best-effort: any failure is swallowed — a recorder
    /// hiccup must never fail the run it describes (same contract as
    /// [`record`]).
    pub fn append(&self, ev: &RunEvent) {
        let Ok(mut line) = serde_json::to_string(ev) else {
            return;
        };
        // Include the newline in ONE buffer and issue a single `write_all`.
        // Two concurrent appenders (the egress proxy thread and the sandbox
        // lifecycle thread) each hold their own `O_APPEND` handle; `writeln!`
        // emits the line and the `\n` as separate `write()` syscalls, so their
        // outputs could interleave into a torn, unparseable line — a silently
        // dropped audit record. A single write of a NUL-free, newline-terminated
        // buffer under `O_APPEND` is atomic on local filesystems, so records
        // stay whole even under concurrent writers.
        line.push('\n');
        let path = self.path();
        let mut opts = fs::OpenOptions::new();
        opts.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        if let Ok(mut f) = opts.open(&path) {
            let _ = f.write_all(line.as_bytes());
        }
    }

    /// Append one event with a **checked** write: any serialization, open, or
    /// write failure is returned to the caller instead of swallowed.
    ///
    /// This is what the locked-run contract's material events (attempt, gate
    /// decisions, `GrantFrozen`, terminal outcome) require: "successfully
    /// appended" means the write returned without error — NOT crash-durable
    /// `fsync` — and a run must refuse to proceed when a material event cannot
    /// be recorded (§3 step 2, §9). Best-effort telemetry keeps [`append`].
    pub fn append_checked(&self, ev: &RunEvent) -> std::io::Result<()> {
        let mut line = serde_json::to_string(ev)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        // Same single-buffer O_APPEND discipline as `append` (torn-line
        // avoidance under concurrent writers).
        line.push('\n');
        let path = self.path();
        let mut opts = fs::OpenOptions::new();
        opts.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        opts.open(&path)?.write_all(line.as_bytes())
    }

    /// Read a run's events, oldest first. Unparseable lines are skipped (a
    /// torn write must not brick the log). Empty when the run has none.
    pub fn read(run_id: &str) -> Vec<RunEvent> {
        let Some(dir) = run_dir(run_id) else {
            return Vec::new();
        };
        let Ok(text) = fs::read_to_string(dir.join("events.jsonl")) else {
            return Vec::new();
        };
        text.lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn with_home<T>(f: impl FnOnce() -> T) -> T {
        let _guard = agentstack_core::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", home.path());
        let out = f();
        std::env::remove_var("AGENTSTACK_HOME");
        out
    }

    fn call_record(ts: u64) -> CallRecord {
        CallRecord {
            ts,
            run: None,
            pid: 1,
            project: None,
            server: "server".into(),
            tool: format!("tool-{ts}-{}", "x".repeat(256)),
            args_digest: format!("{ts:012x}"),
            outcome: CallOutcome::Ok,
            detail: None,
            ms: ts,
        }
    }

    fn write_calls(records: impl IntoIterator<Item = CallRecord>) {
        let path = log_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let text = records
            .into_iter()
            .map(|record| serde_json::to_string(&record).unwrap() + "\n")
            .collect::<String>();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn read_tail_handles_empty_exact_and_truncated_logs() {
        with_home(|| {
            write_calls([]);
            assert!(read_tail(3).is_empty());

            write_calls((1..=3).map(call_record));
            assert_eq!(
                read_tail(3).iter().map(|r| r.ts).collect::<Vec<_>>(),
                [1, 2, 3]
            );

            let mut file = fs::OpenOptions::new()
                .append(true)
                .open(log_path())
                .unwrap();
            file.write_all(b"{\"ts\":4,\"server\":\"cut-off").unwrap();
            assert_eq!(
                read_tail(4).iter().map(|r| r.ts).collect::<Vec<_>>(),
                [1, 2, 3]
            );
        });
    }

    #[test]
    fn read_tail_returns_only_latest_records_across_chunks() {
        with_home(|| {
            write_calls((0..600).map(call_record));
            let tail = read_tail(25);
            assert_eq!(tail.len(), 25);
            assert_eq!(tail.first().unwrap().ts, 575);
            assert_eq!(tail.last().unwrap().ts, 599);
            assert!(!tail.iter().any(|record| record.ts < 575));
            assert!(fs::metadata(log_path()).unwrap().len() > TAIL_CHUNK_BYTES as u64);
        });
    }

    #[test]
    fn digest_is_stable_keyed_and_value_free() {
        with_home(|| {
            let a = digest_args(&json!({ "msg": "s3cr3t-value" }));
            let b = digest_args(&json!({ "msg": "s3cr3t-value" }));
            let c = digest_args(&json!({ "msg": "other" }));
            assert_eq!(a, b, "same args on the same machine correlate");
            assert_ne!(a, c);
            assert_eq!(a.len(), 12);
            assert!(!a.contains("s3cr3t"));
            // Keyed: the digest is not the bare hash of the arguments, so a
            // log without audit/key can't confirm a guessed value.
            let mut h = Sha256::new();
            h.update(
                serde_json::to_string(&json!({ "msg": "s3cr3t-value" }))
                    .unwrap()
                    .as_bytes(),
            );
            let unkeyed = format!("{:x}", h.finalize())[..12].to_string();
            assert_ne!(a, unkeyed, "digest must be keyed");
        });
    }

    #[cfg(unix)]
    #[test]
    fn key_and_log_are_created_private() {
        use std::os::unix::fs::PermissionsExt;
        with_home(|| {
            digest_args(&json!({}));
            record(&CallRecord {
                ts: 0,
                run: None,
                pid: 0,
                project: None,
                server: "s".into(),
                tool: "t".into(),
                args_digest: "0".into(),
                outcome: CallOutcome::Ok,
                detail: None,
                ms: 0,
            });
            let mode = |p: &std::path::Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&key_path()), 0o600, "digest key must be private");
            assert_eq!(mode(&log_path()), 0o600, "call log must be private");
            assert_eq!(
                mode(log_path().parent().unwrap()),
                0o700,
                "audit dir must be private"
            );
        });
    }

    #[test]
    fn run_events_roundtrip_in_order() {
        with_home(|| {
            let log = RunLog::create("r-abc123").expect("safe id");
            let events = vec![
                RunEvent::SandboxStarted {
                    ts: 1,
                    image: "agentstack/sandbox".into(),
                    workspace: "/proj".into(),
                },
                RunEvent::Egress {
                    ts: 2,
                    server: "web-search".into(),
                    host: "api.search.example".into(),
                    allowed: true,
                    rule: None,
                },
                RunEvent::Egress {
                    ts: 3,
                    server: "web-search".into(),
                    host: "evil.example".into(),
                    allowed: false,
                    rule: Some("[policy.egress] \"*\" = \"!evil.example\"".into()),
                },
                RunEvent::SandboxExited {
                    ts: 4,
                    code: Some(0),
                },
            ];
            for e in &events {
                log.append(e);
            }
            assert_eq!(
                RunLog::read("r-abc123"),
                events,
                "read back in append order"
            );
            // Self-describing rows: the discriminant is on the wire.
            let raw = fs::read_to_string(log.path()).unwrap();
            assert!(raw.contains("\"event\":\"egress\""), "{raw}");
            // A blocked decision carries its rule; an allowed one omits it.
            assert!(raw.contains("evil.example") && raw.contains("[policy.egress]"));
        });
    }

    #[test]
    fn tool_call_and_secret_access_roundtrip() {
        with_home(|| {
            let log = RunLog::create("r-actions").expect("safe id");
            let events = vec![
                RunEvent::ToolCall {
                    ts: 10,
                    execution_id: None,
                    server: "figma".into(),
                    tool: "get_file".into(),
                    outcome: CallOutcome::Ok,
                    args_digest: "0123456789ab".into(),
                    detail: None,
                    ms: 42,
                },
                RunEvent::ToolCall {
                    ts: 11,
                    execution_id: Some("exec-1".into()),
                    server: "figma".into(),
                    tool: "delete_file".into(),
                    outcome: CallOutcome::Denied,
                    args_digest: "beefbeefbeef".into(),
                    detail: Some("machine policy denies delete_*".into()),
                    ms: 0,
                },
                RunEvent::SecretAccess {
                    ts: 12,
                    server: "figma".into(),
                    reference: "FIGMA_TOKEN".into(),
                },
            ];
            for e in &events {
                log.append(e);
            }
            assert_eq!(RunLog::read("r-actions"), events, "round-trip in order");
            let raw = fs::read_to_string(log.path()).unwrap();
            // Self-describing rows, and the wire uses the short `"ref"` key.
            assert!(raw.contains("\"event\":\"tool_call\""), "{raw}");
            assert!(raw.contains("\"event\":\"secret_access\""), "{raw}");
            assert!(raw.contains("\"ref\":\"FIGMA_TOKEN\""), "{raw}");
            // A denied call keeps its rule; a plain ok omits the detail field.
            assert!(raw.contains("machine policy denies delete_*"));
            // The digest is on the wire but no argument value ever is.
            assert!(raw.contains("0123456789ab"));
        });
    }

    #[test]
    fn call_outcome_wire_form_is_the_legacy_lowercase_string() {
        // The typed CallOutcome must serialize to exactly the strings the log
        // has always used, so a record written today is byte-identical to one
        // from before the enum existed, and old logs parse unchanged.
        for (variant, text) in [
            (CallOutcome::Ok, "\"ok\""),
            (CallOutcome::Error, "\"error\""),
            (CallOutcome::Denied, "\"denied\""),
        ] {
            assert_eq!(serde_json::to_string(&variant).unwrap(), text);
            assert_eq!(serde_json::from_str::<CallOutcome>(text).unwrap(), variant);
            assert_eq!(variant.as_str(), text.trim_matches('"'));
        }
    }

    #[test]
    fn old_logs_without_new_variants_still_parse() {
        with_home(|| {
            // A log written before the ToolCall/SecretAccess variants existed:
            // only the original three event kinds. Adding variants is additive,
            // so these rows must still parse against the current enum.
            let log = RunLog::create("r-old").unwrap();
            let legacy = "\
{\"event\":\"sandbox_started\",\"ts\":1,\"image\":\"img\",\"workspace\":\"/w\"}
{\"event\":\"egress\",\"ts\":2,\"server\":\"s\",\"host\":\"h\",\"allowed\":true}
{\"event\":\"tool_call\",\"ts\":2,\"server\":\"s\",\"tool\":\"t\",\"outcome\":\"ok\",\"args_digest\":\"abc\",\"ms\":1}
{\"event\":\"sandbox_exited\",\"ts\":3,\"code\":0}
";
            fs::write(log.path(), legacy).unwrap();
            let events = RunLog::read("r-old");
            assert_eq!(events.len(), 4, "all legacy rows parse");
            assert!(matches!(events[0], RunEvent::SandboxStarted { .. }));
            assert!(matches!(
                events[2],
                RunEvent::ToolCall {
                    execution_id: None,
                    ..
                }
            ));
            assert!(matches!(
                events[3],
                RunEvent::SandboxExited { code: Some(0), .. }
            ));
        });
    }

    #[test]
    fn read_of_unknown_run_is_empty_not_error() {
        with_home(|| {
            assert!(RunLog::read("r-nope").is_empty());
        });
    }

    #[test]
    fn unsafe_run_ids_cannot_escape_the_runs_dir() {
        with_home(|| {
            for bad in ["", ".", "..", "../evil", "a/b", "x\0y"] {
                assert!(RunLog::create(bad).is_none(), "must reject {bad:?}");
                assert!(RunLog::read(bad).is_empty(), "no read for {bad:?}");
            }
        });
    }

    #[cfg(unix)]
    #[test]
    fn run_event_log_is_private() {
        use std::os::unix::fs::PermissionsExt;
        with_home(|| {
            let log = RunLog::create("r-priv").unwrap();
            log.append(&RunEvent::SandboxExited { ts: 0, code: None });
            let mode = |p: &std::path::Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&log.path()), 0o600, "run events must be private");
            assert_eq!(mode(&log.dir), 0o700, "run dir must be private");
        });
    }
}
