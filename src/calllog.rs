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

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::util::paths;

const MAX_BYTES: u64 = 5 * 1024 * 1024;

/// The env var `agentstack run` sets on the harness it launches, so calls made
/// by that run's agent can be attributed to the run.
pub const RUN_ID_ENV: &str = "AGENTSTACK_RUN_ID";

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
    /// `ok` / `error` / `denied`.
    pub outcome: String,
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
    restrict_dir(dir);
    let key = random_bytes();
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

/// 32 bytes from the OS entropy pool, with a time/pid-mixed fallback where
/// /dev/urandom is unavailable. The key gates *guess confirmation*, not
/// encryption — the fallback's quality is acceptable for that.
fn random_bytes() -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::io::Read;
        if let Ok(mut f) = fs::File::open("/dev/urandom") {
            let mut buf = vec![0u8; 32];
            if f.read_exact(&mut buf).is_ok() {
                return buf;
            }
        }
    }
    let mut h = Sha256::new();
    h.update(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
            .to_le_bytes(),
    );
    h.update(std::process::id().to_le_bytes());
    h.finalize().to_vec()
}

fn restrict_dir(dir: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }
    #[cfg(not(unix))]
    let _ = dir;
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
    restrict_dir(dir);
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

pub fn now_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn with_home<T>(f: impl FnOnce() -> T) -> T {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", home.path());
        let out = f();
        std::env::remove_var("AGENTSTACK_HOME");
        out
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
                outcome: "ok".into(),
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
}
