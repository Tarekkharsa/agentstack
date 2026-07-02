//! Append-only audit log of every tool call brokered by the runtime gateway
//! (`agentstack mcp` proxied calls and code-mode runtime calls alike):
//! `~/.agentstack/audit/calls.jsonl`, one JSON object per line.
//!
//! What's recorded: timestamp, run id (when the harness was launched by
//! `agentstack run`, via `AGENTSTACK_RUN_ID`), pid, project dir, server, tool,
//! a SHA-256 **digest** of the arguments, outcome (`ok` / `error` / `denied`),
//! a short detail (the policy rule or error class), and latency. What's never
//! recorded: argument values, results, or resolved secrets — the digest lets
//! two calls be compared without storing what they said.
//!
//! Logging is best-effort and must never fail a tool call (same contract as
//! `usage::bump`). Rotation keeps at most two generations of ~5 MB.

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

pub fn digest_args(args: &Value) -> String {
    let mut h = Sha256::new();
    h.update(serde_json::to_string(args).unwrap_or_default().as_bytes());
    let hex = format!("{:x}", h.finalize());
    hex[..12].to_string()
}

/// Append one record. Best-effort: any failure is swallowed — an audit-log
/// hiccup must never fail the tool call it describes.
pub fn record(rec: &CallRecord) {
    let path = log_path();
    let Some(dir) = path.parent() else { return };
    if fs::create_dir_all(dir).is_err() {
        return;
    }
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
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&path) {
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

    #[test]
    fn digest_is_stable_and_value_free() {
        let a = digest_args(&json!({ "msg": "s3cr3t-value" }));
        let b = digest_args(&json!({ "msg": "s3cr3t-value" }));
        let c = digest_args(&json!({ "msg": "other" }));
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 12);
        assert!(!a.contains("s3cr3t"));
    }
}
