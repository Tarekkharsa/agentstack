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
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
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

/// Append one already-serialized JSON line to a machine-global audit stream,
/// creating the 0700 directory and the 0600 file as needed. Best-effort: every
/// failure is swallowed — a logging hiccup must never fail the thing it
/// describes. `rotate` opts into the size-capped current → `.1` rotation
/// (`trust.jsonl` deliberately keeps its full history and passes `false`).
///
/// Takes the line by value so the newline can be appended into the SAME buffer
/// and issued as one `write_all`: `writeln!` emits the payload and the `\n` as
/// separate `write()` syscalls, which two `O_APPEND` writers can interleave
/// into a torn, unparseable record. A single write of a newline-terminated
/// buffer under `O_APPEND` is atomic on local filesystems.
fn append_audit_line(path: &Path, mut line: String, rotate: bool) {
    let Some(dir) = path.parent() else { return };
    if fs::create_dir_all(dir).is_err() {
        return;
    }
    agentstack_core::util::restrict(dir, true);
    if rotate
        && fs::metadata(path)
            .map(|m| m.len() > MAX_BYTES)
            .unwrap_or(false)
    {
        let _ = fs::rename(path, path.with_extension("jsonl.1"));
    }
    line.push('\n');
    let mut opts = fs::OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    if let Ok(mut f) = opts.open(path) {
        // mode() applies only at creation — tighten a log that predates the
        // 0600 default (or survived a mode-preserving restore) too.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
        }
        let _ = f.write_all(line.as_bytes());
    }
}

/// Append one record. Best-effort: any failure is swallowed — a call-log
/// hiccup must never fail the tool call it describes.
pub fn record(rec: &CallRecord) {
    let Ok(line) = serde_json::to_string(rec) else {
        return;
    };
    append_audit_line(&log_path(), line, true);
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
    read_tail_of(&log_path(), n)
}

/// The tail reader every JSONL stream in this module shares. Generic over the
/// record type rather than duplicated per stream: the chunked backward walk is
/// the only subtle code here, and a second copy would be a second place to get
/// the fragment handling wrong.
fn read_tail_of<T: DeserializeOwned>(path: &Path, n: usize) -> Vec<T> {
    if n == 0 {
        return Vec::new();
    }
    let Ok(mut file) = fs::File::open(path) else {
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

// ─────────────────────────── trust-store mutation log ─────────────────────────
//
// Every mutation of the machine's trust store (`~/.agentstack/trust.json`)
// becomes one line in `~/.agentstack/audit/trust.jsonl` — the evidence stream
// the strategy's consent metrics are counted over (STRATEGY.md Phase 0:
// without recorded grant events, "no consent surprise" is unfalsifiable).
//
// What's recorded: timestamp, the action, the project key (the store's own
// canonical base-dir key), and the consent digest pinned or removed. What's
// never recorded: manifest bytes, the reviewed surface (its `identity` fields
// carry command lines), or anything else content-shaped — identity only.
//
// Deliberately NO rotation, unlike `calls.jsonl`: mutations are rare
// (human-paced), and the Phase 1 gate counts consent incidents over the FULL
// event history — rotating old grants away would corrupt the metric.
//
// Same honest scope as the call log: best-effort local evidence, appended by
// the trust crate only AFTER the store write succeeded — it adds events,
// never gates, and it is not tamper-evident.

/// What one trust-store mutation did — a closed 4-value set.
///
/// `Repin` is deliberately distinct from `Regrant`: a repin carries existing
/// trust across agentstack's OWN rewrite of the pinned bytes (no human in the
/// loop), so consent metrics counted over these events must be able to
/// exclude it; `Grant`/`Regrant` are the human consent moments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustAction {
    Grant,
    Regrant,
    Repin,
    Revoke,
}

/// One line in `audit/trust.jsonl`. Identity only — see the module note above
/// for what is never recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustMutation {
    pub ts: u64,
    pub action: TrustAction,
    /// Canonical project base dir — the trust store's own key for the entry.
    pub project: String,
    /// The `sha256:` consent digest pinned (grant/regrant/repin) or removed
    /// (revoke).
    pub digest: String,
}

pub fn trust_log_path() -> PathBuf {
    paths::agentstack_home().join("audit").join("trust.jsonl")
}

/// Append one trust-store mutation. Best-effort: any failure is swallowed —
/// a recording hiccup must never fail the grant (or revoke) it describes.
/// Callers append AFTER their store write succeeds, so an event always
/// describes a mutation that actually happened.
pub fn record_trust(ev: &TrustMutation) {
    let Ok(line) = serde_json::to_string(ev) else {
        return;
    };
    // `rotate = false`: see the section note — the Phase 1 gate counts consent
    // incidents over the FULL history, so rotating old grants away would
    // corrupt the metric.
    append_audit_line(&trust_log_path(), line, false);
}

/// Read the full trust-mutation history, oldest first. Unparseable lines are
/// skipped (a torn write must not brick the log).
pub fn read_trust_all() -> Vec<TrustMutation> {
    let Ok(text) = fs::read_to_string(trust_log_path()) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

// ────────────────────────── on-demand skill-load stream ───────────────────────
//
// `agentstack_load` (the MCP loader) puts a skill's body into an agent's
// context on demand. That is activity a reviewer wants to see, and it is **not
// a call**: it opens no upstream connection, carries no arguments to digest and
// no outcome, and folding it into `calls.jsonl` would corrupt every count
// computed over that file. So it gets its own machine-global stream,
// `~/.agentstack/audit/loads.jsonl`, with the call log's discipline: 0600/0700,
// best-effort, size-capped rotation.
//
// What's recorded: timestamp, the skill NAME, the agent-supplied reason, the
// project the load was served from, and the run id when the harness was
// launched by `agentstack run`. What's never recorded: the skill BODY — the
// thing that was actually loaded. Identity only, like every stream here.
//
// Only SUCCESSFUL loads appear: a refusal (untrusted project, toolset fence,
// standing block, drifted bytes) fails the MCP call before any recording, so
// this stream contains no denials by construction. Recording is evidence, not
// enforcement — nothing reads this stream to make a decision.

/// Byte caps for the two agent-supplied strings on a [`LoadRecord`]. Nothing
/// upstream bounds either one — the MCP caller writes both — so the stream
/// bounds them itself (invariant 7: hostile input is bounded at the seam).
const LOAD_NAME_CAP: usize = 200;
const LOAD_REASON_CAP: usize = 500;

/// Strip control characters and truncate to `cap` bytes on a char boundary.
/// Control characters go because these strings are rendered in reports and
/// read by other tools; the truncation walks back to the last boundary because
/// `String::truncate` panics mid-codepoint.
fn bounded(s: &str, cap: usize) -> String {
    let mut out: String = s.chars().filter(|c| !c.is_control()).collect();
    if out.len() > cap {
        let mut end = cap;
        while end > 0 && !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
    }
    out
}

/// One line in `audit/loads.jsonl`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadRecord {
    pub ts: u64,
    pub name: String,
    pub reason: String,
    /// The project root the load was served from — the same value and format
    /// [`CallRecord::project`] carries, so one comparison filters both streams
    /// by project. `None` when the load had no resolvable manifest directory
    /// (the embedded manual serves without one).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// The run this load is attributed to (`AGENTSTACK_RUN_ID`), when set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
}

impl LoadRecord {
    /// Build a bounded record. Callers use this rather than a struct literal so
    /// the bounded `name`/`reason` can be reused for the run-log mirror —
    /// both sinks then carry byte-identical strings.
    pub fn new(
        ts: u64,
        name: &str,
        reason: &str,
        project: Option<String>,
        run: Option<String>,
    ) -> LoadRecord {
        LoadRecord {
            ts,
            name: bounded(name, LOAD_NAME_CAP),
            reason: bounded(reason, LOAD_REASON_CAP),
            project,
            run,
        }
    }
}

pub fn loads_log_path() -> PathBuf {
    paths::agentstack_home().join("audit").join("loads.jsonl")
}

/// Append one skill load. Best-effort: any failure is swallowed — a recording
/// hiccup must never fail the load it describes.
///
/// Re-bounds `name`/`reason` even though [`LoadRecord::new`] already did: this
/// function is the choke point every write to the stream passes through, and a
/// struct literal built elsewhere must not be able to put an unbounded string
/// on disk. `bounded` is idempotent, so the extra pass changes nothing.
pub fn record_skill_load(rec: &LoadRecord) {
    let bounded_rec = LoadRecord::new(
        rec.ts,
        &rec.name,
        &rec.reason,
        rec.project.clone(),
        rec.run.clone(),
    );
    let Ok(line) = serde_json::to_string(&bounded_rec) else {
        return;
    };
    append_audit_line(&loads_log_path(), line, true);
}

/// Read the load stream, oldest first. Unparseable lines are skipped (a torn
/// write must not brick the log).
pub fn read_loads_all() -> Vec<LoadRecord> {
    let Ok(text) = fs::read_to_string(loads_log_path()) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// The load stream's tail, newest last — the [`read_tail`] reader over
/// `loads.jsonl`.
pub fn read_loads_tail(n: usize) -> Vec<LoadRecord> {
    read_tail_of(&loads_log_path(), n)
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
    /// A secret reference `[policy.secrets]` refused to resolve for this
    /// server. Deliberately a separate variant rather than an outcome field on
    /// [`RunEvent::SecretAccess`]: that variant means "this run *read* this
    /// secret", and it is what a reviewer counts to see a run's secret
    /// surface. A refused ref was never read — folding the two together would
    /// make the existing reading wrong.
    ///
    /// Identity-shaped like every other event here: the server, the ref NAME,
    /// and the policy rule that refused it. Never the value, never whether the
    /// value exists — a denied ref does not reach any backing store, so this
    /// event cannot leak the one bit that scoping exists to withhold.
    SecretDenied {
        ts: u64,
        server: String,
        /// The `${REF}` name. `ref` is a Rust keyword, so the field is
        /// `reference` in code but `"ref"` on the wire — matching
        /// [`RunEvent::SecretAccess`].
        #[serde(rename = "ref")]
        reference: String,
        /// The matching `[policy.secrets]` line. Policy-authored text from
        /// this machine's own configuration, never upstream-authored.
        rule: String,
    },
    /// The gateway refused to serve a server because its declared bytes did
    /// not verify against the lockfile pin — the content-pinning refusal, the
    /// one that fires when what would be delivered is not what was reviewed.
    ///
    /// A separate variant for the same reason [`RunEvent::SecretDenied`] is
    /// one: no existing variant means "a server was withheld". Filing it under
    /// [`RunEvent::ToolCall`] would corrupt a run's tool-call count, and there
    /// is no generic denial variant to overload — nor should there be, since a
    /// reviewer counting pin refusals is asking a different question from one
    /// counting anything else.
    ///
    /// Identity-shaped: the server NAME and the reason it failed to verify.
    /// Never the server's command line, environment, or the bytes themselves —
    /// the whole point of this refusal is that those bytes are unreviewed, and
    /// copying unreviewed content into the evidence log would put it in front
    /// of exactly the reader the gate exists to protect.
    ///
    /// Best-effort and never gating: like every event here it is appended
    /// through [`RunLog::append`], which returns `()`. The refusal has already
    /// happened by the time this is written, and a recorder failure can only
    /// lose the evidence — never restore the server.
    PinRejected {
        ts: u64,
        server: String,
        /// Why verification failed, in the words the user sees. Bounded and
        /// control-character-stripped by the caller before it arrives here:
        /// its inputs include lockfile- and manifest-derived fragments, which
        /// are repository content and therefore hostile input (invariant 7).
        reason: String,
    },
    /// The gateway refused to dispatch to an already-connected upstream
    /// because the project's consent digest no longer matched the one the
    /// connection was authorized against — trust revoked, the manifest edited
    /// out of band, or the lock replaced wholesale (W2, "trust is checked at
    /// dispatch, from the digest").
    ///
    /// Its own variant for the same reason [`RunEvent::PinRejected`] is one:
    /// no existing variant means "a live connection stopped being authorized".
    /// Filing it under [`RunEvent::ToolCall`] with `outcome: denied` would put
    /// it in the same bucket as a `[policy.tools]` block — a reviewer counting
    /// policy denials is asking a different question from one asking whether a
    /// consent boundary moved underneath a running session, and a call that
    /// was refused for *lack of a valid yes* is not a call the run made.
    ///
    /// Identity-shaped: the server and tool NAMES, a closed-set `state` tag,
    /// and the reason in the words the user was shown. Never the arguments —
    /// a call refused because trust no longer holds is precisely one whose
    /// payload should not be copied anywhere.
    ///
    /// Best-effort and never gating, like every event here: the refusal has
    /// already happened by the time this is written, and a recorder failure
    /// can only lose the evidence — never restore the call.
    ///
    /// The same variant also carries the two refusals that happen *before* any
    /// dispatch (W1, "the yes on the lease path"): a lease the gateway would
    /// not open and a skill it would not load, for a project whose yes does not
    /// hold. They are the same fact — "the review no longer covers this
    /// project" — met at an earlier door, so they get the same event rather
    /// than a second one a reader would have to learn about separately.
    TrustRefused {
        ts: u64,
        /// The name of the capability the refusal was about: the upstream
        /// server a dispatch was addressed to, or — when the refusal happened
        /// at lease-open or load — the toolset or skill that was refused.
        /// Manifest- or caller-authored, therefore hostile input — bounded and
        /// control-character stripped by the caller (invariant 7).
        server: String,
        /// The bare upstream tool name, bounded by the caller for the same
        /// reason. For a refusal that happened at lease-open or load rather
        /// than at dispatch, this is the **control-plane verb** that was
        /// refused (`agentstack_lease_open`, `agentstack_load`) — machine
        /// authored, because no upstream tool was ever named.
        tool: String,
        /// Which way trust failed: `"revoked"`, `"changed"`, `"untrusted"`, or
        /// `"unreadable"`. A closed, machine-authored set. `"revoked"` is
        /// reachable only from the dispatch path, which holds the anchor the
        /// connection was authorized against; a lease/load refusal reads a
        /// withdrawn yes back as `"untrusted"`, because the store keeps no
        /// trace of an entry that was removed.
        state: String,
        /// Why, in the words the user saw. Machine-authored text; bounded by
        /// the caller so the log line and the terminal line are identical.
        reason: String,
    },
    /// A skill body entered this run's agent context on demand
    /// (`agentstack_load`) — the run-scoped mirror of one `loads.jsonl` line.
    ///
    /// Its own variant for the same reason [`RunEvent::PinRejected`] is one: a
    /// load is not a call. Filing it under [`RunEvent::ToolCall`] would corrupt
    /// this run's tool-call count — the number a reviewer reads as "actions the
    /// agent took through the gateway" — and a load has no server, no
    /// arguments to digest, and no outcome to record.
    ///
    /// Only SUCCESSFUL loads appear. A refused load fails the MCP call itself
    /// before any recording, so there are no denials here by construction. If
    /// denied-load evidence is ever wanted it must be a NEW variant, never an
    /// outcome field on this one — for the reason spelled out on
    /// [`RunEvent::SecretDenied`]: this variant means "the agent read this
    /// skill", and that reading has to stay true.
    ///
    /// Identity only: the skill NAME and the agent-supplied reason, both
    /// bounded and control-character-stripped by [`LoadRecord::new`] at the
    /// record site. The skill BODY is never an event.
    ///
    /// The wire tag is `skill_load` (the event), not the variant's own
    /// snake_case — renamed explicitly so the row names what happened.
    #[serde(rename = "skill_load")]
    SkillLoaded {
        ts: u64,
        name: String,
        reason: String,
    },
    /// The sandbox container exited. `code` is absent when it was killed by a
    /// signal (e.g. teardown).
    SandboxExited {
        ts: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        code: Option<i32>,
    },
    /// An ordinary tracked host run started. Unlike a locked attempt this has
    /// no pre-launch gates or frozen grant; the posture is advisory host mode.
    HostStarted {
        ts: u64,
        harness: String,
        posture: String,
    },
    /// Terminal lifecycle record for an ordinary tracked host run.
    HostExited {
        ts: u64,
        outcome: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        code: Option<i32>,
        duration_ms: u64,
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
    /// Bounded-stdout evidence for a headless (`--locked --prompt`) run:
    /// content identity only — digest, byte count, and whether the capture
    /// cap truncated it. The output TEXT is never an event (it is the run's
    /// product, relayed to the caller's stdout, not evidence).
    HeadlessOutput {
        ts: u64,
        /// Bytes captured (≤ the launcher's cap), the exact input to `sha256`.
        bytes: u64,
        sha256: String,
        truncated: bool,
    },
    /// A governed workflow run began — the envelope event of the
    /// workflow-level evidence tree (design doc §6 step 3, §12.4 Stage E):
    /// the workflow log is the JOIN TABLE over per-child run logs; each
    /// child's own events stay in its own log. Identity only: the pinned
    /// script digest admission verified, the deterministic digest of the
    /// EFFECTIVE grant (machine ∩ manifest ∩ script meta), and the
    /// invoker-args identity — Stage F's divergence refusals consume these.
    /// This event stream doubles as the resume journal, but it is written as
    /// EVIDENCE: `ts` is CLI-stamped wall clock and must never become replay
    /// input.
    WorkflowStarted {
        ts: u64,
        workflow: String,
        workflow_digest: String,
        grant_digest: String,
        /// Digest over the RAW `--args-json` bytes (length-framed, the
        /// no-args case pinned distinctly) — the byte-identical resume
        /// precondition, recorded proactively for Stage F.
        args_digest: String,
        max_agents: u32,
        max_wall_seconds: u64,
    },
    /// One `agent()` call became a locked child run. Appended BEFORE the
    /// child spawns (fail-closed: an unrecordable spawn does not launch).
    ///
    /// Deliberate deviation from the §12.4 sketch, which lists
    /// `child_grant_digest` here: the child's grant exists only after its
    /// OWN freeze, so a pre-spawn event cannot carry it — and duplicating it
    /// post-hoc would break the join-table principle the same section
    /// states. The report joins the child's `GrantFrozen`/`LockedOutcome`
    /// via `child_run_id` instead; the digest lives in exactly one place.
    StepSpawned {
        ts: u64,
        /// The engine's request id — the step's stable identity.
        step: u64,
        role: String,
        child_run_id: String,
        /// Length-framed digest over canonical prompt + opts — the per-step
        /// replay-alignment identity (Stage F), recorded proactively. The
        /// prompt TEXT itself never enters an event.
        request_digest: String,
        /// Script-authored display label: byte-bounded at append, sanitized
        /// at report render (rule 7 at the terminal seam).
        #[serde(skip_serializing_if = "Option::is_none", default)]
        label: Option<String>,
        /// Prior step ids whose RESULT text appears in this step's prompt
        /// (§11 ruling 3: report-only metadata, no blocking semantics).
        /// Bounded substring detection — false negatives are accepted and
        /// stated at the detector; a reviewability aid, not DLP.
        #[serde(skip_serializing_if = "Vec::is_empty", default)]
        taint: Vec<u64>,
        /// §12.1 serial (config-swap) fallback scheduling — the one
        /// deliberate degrade, recorded rather than stderr-only.
        #[serde(skip_serializing_if = "skip_false", default)]
        serial: bool,
        /// The codex connector-layer residual applies to this child
        /// (§12.1 gate condition 1) — recorded per child.
        #[serde(skip_serializing_if = "skip_false", default)]
        codex_residual: bool,
    },
    /// A step's child run completed and its result resolved the `agent()`
    /// promise. Result identity (digest/bytes/truncated) lives in the
    /// child's own `HeadlessOutput` — joined, never duplicated.
    StepCompleted { ts: u64, step: u64 },
    /// A resumed session took over this run's log (Stage F). Appended AFTER
    /// the journaled steps replayed and BEFORE the first live event of the
    /// resumed session — so a refused resume leaves the journal
    /// byte-untouched and re-attemptable, and everything after the LAST
    /// marker is the newest session's live tail. Identity is NOT restated:
    /// the original `WorkflowStarted` was verified byte-identical (script,
    /// grant, args digests) before this could append.
    WorkflowResumed { ts: u64, replayed_steps: u64 },
    /// A step failed closed (the script saw `null`). `reason` is a
    /// launcher-authored category — never upstream or script-authored text.
    StepFailed { ts: u64, step: u64, reason: String },
    /// Terminal outcome of a workflow run. `outcome` names every Stage B–D
    /// terminal path distinctly: `done`, `failed:<kind>` (the engine's
    /// error kinds, e.g. `failed:agents_exhausted`), `wall_deadline` (the
    /// cooperative in-band refusal), `engine_invariant_breach` (the CLI's
    /// defense-in-depth assertion), `watchdog_kill` (appended best-effort by
    /// the dying watchdog before exit 124). `exhausted` is independent of
    /// `outcome` because a run can exhaust its agent ceiling and still
    /// complete (`done`) — the non-forgeable engine flag, recorded honestly
    /// either way.
    WorkflowCompleted {
        ts: u64,
        outcome: String,
        exhausted: bool,
        duration_ms: u64,
    },
}

/// serde helper: omit a `false` bool field entirely (the event stays lean;
/// `default` restores it on read).
#[allow(clippy::trivially_copy_pass_by_ref)]
fn skip_false(b: &bool) -> bool {
    !*b
}

/// The append-only event log for one tracked run.
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

    /// Stage E: the workflow step event serializes lean (empty taint, false
    /// serial/codex flags, absent label produce NO keys) and round-trips —
    /// the journal shape stays minimal without losing read-back fidelity.
    #[test]
    fn workflow_step_event_is_lean_and_round_trips() {
        let ev = RunEvent::StepSpawned {
            ts: 1,
            step: 0,
            role: "reader".into(),
            child_run_id: "r-abc".into(),
            request_digest: "d".into(),
            label: None,
            taint: Vec::new(),
            serial: false,
            codex_residual: false,
        };
        let line = serde_json::to_string(&ev).unwrap();
        for absent in ["label", "taint", "serial", "codex_residual"] {
            assert!(!line.contains(absent), "{absent} should be omitted: {line}");
        }
        let back: RunEvent = serde_json::from_str(&line).unwrap();
        assert_eq!(back, ev);

        let marked = RunEvent::StepSpawned {
            ts: 1,
            step: 2,
            role: "reader".into(),
            child_run_id: "r-def".into(),
            request_digest: "d".into(),
            label: Some("map:a".into()),
            taint: vec![0, 1],
            serial: true,
            codex_residual: true,
        };
        let line = serde_json::to_string(&marked).unwrap();
        let back: RunEvent = serde_json::from_str(&line).unwrap();
        assert_eq!(back, marked);

        // Stage F: the resume marker round-trips and is self-describing.
        let resumed = RunEvent::WorkflowResumed {
            ts: 2,
            replayed_steps: 3,
        };
        let line = serde_json::to_string(&resumed).unwrap();
        assert!(line.contains("\"event\":\"workflow_resumed\""), "{line}");
        let back: RunEvent = serde_json::from_str(&line).unwrap();
        assert_eq!(back, resumed);
    }

    /// P0.2 witness: the trust-mutation event is self-describing on the wire
    /// (snake_case action), round-trips, and `record_trust` appends exactly
    /// one parseable line per call.
    #[test]
    fn trust_mutation_round_trips_and_appends_one_line() {
        let ev = TrustMutation {
            ts: 7,
            action: TrustAction::Regrant,
            project: "/tmp/proj".into(),
            digest: "sha256:abc".into(),
        };
        let line = serde_json::to_string(&ev).unwrap();
        assert!(line.contains("\"action\":\"regrant\""), "{line}");
        let back: TrustMutation = serde_json::from_str(&line).unwrap();
        assert_eq!(back, ev);

        with_home(|| {
            record_trust(&ev);
            assert_eq!(read_trust_all(), vec![ev.clone()]);
            record_trust(&ev);
            assert_eq!(read_trust_all().len(), 2);
        });
    }

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

    /// A skill load is its own event kind: it round-trips, and its wire tag is
    /// the literal `skill_load` (not the variant's snake_case) — the tag the
    /// report reader and every external consumer match on.
    #[test]
    fn skill_load_event_round_trips_under_its_own_wire_tag() {
        with_home(|| {
            let log = RunLog::create("r-loads").expect("safe id");
            let ev = RunEvent::SkillLoaded {
                ts: 20,
                name: "rust-review".into(),
                reason: "reviewing a Rust diff".into(),
            };
            log.append(&ev);
            assert_eq!(RunLog::read("r-loads"), vec![ev]);
            let raw = fs::read_to_string(log.path()).unwrap();
            assert!(raw.contains("\"event\":\"skill_load\""), "{raw}");
            assert!(!raw.contains("skill_loaded"), "{raw}");
            // A load is never a tool call: nothing about the row can be read as
            // one, so a tool-call count over this log is unaffected.
            assert!(!raw.contains("tool_call"), "{raw}");
        });
    }

    /// The loads stream round-trips through both readers, and bounds its two
    /// agent-supplied strings at the record site: control characters are
    /// stripped and over-long text is cut on a char boundary.
    #[test]
    fn load_records_round_trip_and_are_bounded_at_the_record_site() {
        with_home(|| {
            let rec = LoadRecord::new(
                7,
                "rust-review",
                "reviewing a diff",
                Some("/tmp/proj".into()),
                Some("r-1".into()),
            );
            record_skill_load(&rec);
            assert_eq!(read_loads_all(), vec![rec.clone()]);
            assert_eq!(read_loads_tail(1), vec![rec]);

            // Hostile input: control characters (including a newline that would
            // otherwise be read as a row boundary) and a multi-byte string far
            // over the cap.
            let long = "é".repeat(LOAD_REASON_CAP);
            record_skill_load(&LoadRecord::new(
                8,
                "na\u{7}me\nwith\rcontrols",
                &long,
                None,
                None,
            ));
            let all = read_loads_all();
            assert_eq!(all.len(), 2, "one line per load: {all:?}");
            let hostile = &all[1];
            assert_eq!(hostile.name, "namewithcontrols");
            assert!(
                hostile.reason.len() <= LOAD_REASON_CAP,
                "reason bounded: {}",
                hostile.reason.len()
            );
            assert!(
                hostile.reason.chars().all(|c| c == 'é'),
                "truncated on a char boundary, not mid-codepoint"
            );
            // The bound is the stream's, not the constructor's: a struct
            // literal that skipped `LoadRecord::new` is bounded too.
            record_skill_load(&LoadRecord {
                ts: 9,
                name: "x\u{1}y".into(),
                reason: long.clone(),
                project: None,
                run: None,
            });
            let literal = read_loads_all().pop().unwrap();
            assert_eq!(literal.name, "xy");
            assert!(literal.reason.len() <= LOAD_REASON_CAP);
            // Absent project/run leave no keys on the wire (same shape rule as
            // `CallRecord`).
            let raw = fs::read_to_string(loads_log_path()).unwrap();
            let last = raw.lines().last().unwrap();
            assert!(
                !last.contains("project") && !last.contains("\"run\""),
                "{last}"
            );
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

            // The other direction: a log written by THIS binary carries rows an
            // older one predates. A `skill_load` row parses here…
            let log = RunLog::create("r-mixed").unwrap();
            fs::write(
                log.path(),
                "{\"event\":\"sandbox_started\",\"ts\":1,\"image\":\"img\",\"workspace\":\"/w\"}\n\
                 {\"event\":\"skill_load\",\"ts\":2,\"name\":\"rust-review\",\"reason\":\"why\"}\n",
            )
            .unwrap();
            let events = RunLog::read("r-mixed");
            assert_eq!(events.len(), 2);
            assert!(matches!(events[1], RunEvent::SkillLoaded { .. }));
            // …and an older binary drops exactly that row and keeps the rest:
            // `RunLog::read`'s `filter_map(ok)` skips a variant it doesn't know
            // rather than failing the whole log. Adding variants is additive in
            // both directions, and a dropped row can only lose evidence.
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
