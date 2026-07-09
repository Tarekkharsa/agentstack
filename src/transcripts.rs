//! Best-effort readers for agent session transcripts — the cross-harness reach
//! `analyze` can't get from its own telemetry. Claude Code writes one JSONL
//! file per session under `~/.claude/projects/<project>/`; Codex writes
//! `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`.
//!
//! Both formats are undocumented and drift, so parsing is defensive by design:
//! a line that doesn't decode, or decodes to an unknown shape, is skipped and
//! counted — never an error. Only aggregates leave this module (counts, token
//! totals, tool names); prompt bodies are never extracted or stored.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::util::paths::expand_tilde;

/// Aggregate summary of one session transcript. Names and numbers only.
#[derive(Debug, Default, Clone)]
pub struct SessionSummary {
    /// `claude-code` or `codex`.
    pub harness: String,
    /// First/last event timestamp seen (ISO-8601; lexical order == time order).
    pub first_ts: Option<String>,
    pub last_ts: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Tool name -> invocation count within this session.
    pub tools: BTreeMap<String, u64>,
    /// Lines that failed to decode or matched no known shape.
    pub skipped_lines: u64,
}

/// Read every session under both default roots. Missing roots yield nothing.
pub fn read_default() -> Vec<SessionSummary> {
    let mut out = read_claude_root(&expand_tilde("~/.claude/projects"));
    out.extend(read_codex_root(&expand_tilde("~/.codex/sessions")));
    out
}

/// Claude Code: `<root>/<project-dir>/<session>.jsonl`.
pub fn read_claude_root(root: &Path) -> Vec<SessionSummary> {
    jsonl_files(root, 2)
        .iter()
        .filter_map(|p| read_session(p, Harness::Claude))
        .collect()
}

/// Codex: `<root>/YYYY/MM/DD/rollout-*.jsonl`.
pub fn read_codex_root(root: &Path) -> Vec<SessionSummary> {
    jsonl_files(root, 4)
        .iter()
        .filter_map(|p| read_session(p, Harness::Codex))
        .collect()
}

#[derive(Clone, Copy)]
enum Harness {
    Claude,
    Codex,
}

/// Collect `*.jsonl` files up to `depth` directory levels below `root`.
fn jsonl_files(root: &Path, depth: u32) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(root, depth, &mut out);
    out.sort();
    out
}

fn walk(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth > 0 {
                walk(&path, depth - 1, out);
            }
        } else if path.extension().is_some_and(|e| e == "jsonl") {
            out.push(path);
        }
    }
}

/// Parse one transcript. Returns `None` only if the file can't be opened or
/// holds no recognizable event at all.
fn read_session(path: &Path, harness: Harness) -> Option<SessionSummary> {
    let file = File::open(path).ok()?;
    let mut s = SessionSummary {
        harness: match harness {
            Harness::Claude => "claude-code".into(),
            Harness::Codex => "codex".into(),
        },
        ..Default::default()
    };
    let mut recognized = 0u64;
    // Codex reports cumulative totals per token_count event; keep the last.
    let (mut codex_in, mut codex_out) = (None::<u64>, None::<u64>);

    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            s.skipped_lines += 1;
            continue;
        };
        let known = match harness {
            Harness::Claude => claude_line(&v, &mut s),
            Harness::Codex => codex_line(&v, &mut s, &mut codex_in, &mut codex_out),
        };
        if known {
            recognized += 1;
            if let Some(ts) = v.get("timestamp").and_then(Value::as_str) {
                if s.first_ts.as_deref().map_or(true, |f| ts < f) {
                    s.first_ts = Some(ts.to_string());
                }
                if s.last_ts.as_deref().map_or(true, |l| ts > l) {
                    s.last_ts = Some(ts.to_string());
                }
            }
        } else {
            s.skipped_lines += 1;
        }
    }
    s.input_tokens += codex_in.unwrap_or(0);
    s.output_tokens += codex_out.unwrap_or(0);
    (recognized > 0).then_some(s)
}

/// One Claude Code event: assistant turns carry `message.usage` (per-turn, so
/// summed) and `message.content[]` tool_use blocks.
fn claude_line(v: &Value, s: &mut SessionSummary) -> bool {
    let Some(msg) = v.get("message") else {
        // Non-message events (summaries, hooks) still anchor timestamps.
        return v.get("timestamp").is_some();
    };
    if let Some(usage) = msg.get("usage") {
        s.input_tokens += usage
            .get("input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        s.output_tokens += usage
            .get("output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
    }
    if let Some(content) = msg.get("content").and_then(Value::as_array) {
        for block in content {
            if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                if let Some(name) = block.get("name").and_then(Value::as_str) {
                    *s.tools.entry(name.to_string()).or_insert(0) += 1;
                }
            }
        }
    }
    true
}

/// One Codex event: `payload.type` discriminates; `function_call` /
/// `custom_tool_call` name tools, `token_count.info.total_token_usage` is a
/// cumulative counter (last one wins).
fn codex_line(
    v: &Value,
    s: &mut SessionSummary,
    total_in: &mut Option<u64>,
    total_out: &mut Option<u64>,
) -> bool {
    let Some(payload) = v.get("payload") else {
        return false;
    };
    match payload.get("type").and_then(Value::as_str) {
        Some("function_call") | Some("custom_tool_call") => {
            if let Some(name) = payload.get("name").and_then(Value::as_str) {
                *s.tools.entry(name.to_string()).or_insert(0) += 1;
            }
        }
        Some("token_count") => {
            if let Some(total) = payload.get("info").and_then(|i| i.get("total_token_usage")) {
                if let Some(n) = total.get("input_tokens").and_then(Value::as_u64) {
                    *total_in = Some(n);
                }
                if let Some(n) = total.get("output_tokens").and_then(Value::as_u64) {
                    *total_out = Some(n);
                }
            }
        }
        Some(_) => {}
        None => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_lines(path: &Path, lines: &[&str]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut f = File::create(path).unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
    }

    #[test]
    fn claude_sessions_sum_usage_and_count_tools() {
        let tmp = tempfile::tempdir().unwrap();
        write_lines(
            &tmp.path().join("proj-a/s1.jsonl"),
            &[
                r#"{"type":"assistant","timestamp":"2026-07-01T10:00:00Z","message":{"usage":{"input_tokens":100,"output_tokens":20},"content":[{"type":"tool_use","name":"Bash"},{"type":"text","text":"hi"}]}}"#,
                r#"{"type":"assistant","timestamp":"2026-07-01T10:05:00Z","message":{"usage":{"input_tokens":50,"output_tokens":10},"content":[{"type":"tool_use","name":"Bash"},{"type":"tool_use","name":"Read"}]}}"#,
                "not json at all",
            ],
        );
        let sessions = read_claude_root(tmp.path());
        assert_eq!(sessions.len(), 1);
        let s = &sessions[0];
        assert_eq!(s.harness, "claude-code");
        assert_eq!(s.input_tokens, 150);
        assert_eq!(s.output_tokens, 30);
        assert_eq!(s.tools.get("Bash"), Some(&2));
        assert_eq!(s.tools.get("Read"), Some(&1));
        assert_eq!(s.skipped_lines, 1);
        assert_eq!(s.first_ts.as_deref(), Some("2026-07-01T10:00:00Z"));
        assert_eq!(s.last_ts.as_deref(), Some("2026-07-01T10:05:00Z"));
    }

    #[test]
    fn codex_sessions_take_last_cumulative_total() {
        let tmp = tempfile::tempdir().unwrap();
        write_lines(
            &tmp.path().join("2026/07/01/rollout-a.jsonl"),
            &[
                r#"{"timestamp":"2026-07-01T09:00:00Z","type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{}"}}"#,
                r#"{"timestamp":"2026-07-01T09:01:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1000,"output_tokens":40}}}}"#,
                r#"{"timestamp":"2026-07-01T09:02:00Z","type":"event_msg","payload":{"type":"token_count","info":null}}"#,
                r#"{"timestamp":"2026-07-01T09:03:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":2500,"output_tokens":90}}}}"#,
            ],
        );
        let sessions = read_codex_root(tmp.path());
        assert_eq!(sessions.len(), 1);
        let s = &sessions[0];
        assert_eq!(s.harness, "codex");
        // Cumulative counter: last total wins, not the sum.
        assert_eq!(s.input_tokens, 2500);
        assert_eq!(s.output_tokens, 90);
        assert_eq!(s.tools.get("exec_command"), Some(&1));
    }

    #[test]
    fn unknown_shapes_are_skipped_not_fatal() {
        let tmp = tempfile::tempdir().unwrap();
        write_lines(
            &tmp.path().join("2026/07/02/rollout-b.jsonl"),
            &[
                r#"{"timestamp":"2026-07-02T09:00:00Z","no_payload_here":true}"#,
                r#"{"timestamp":"2026-07-02T09:01:00Z","payload":{"type":"some_future_event"}}"#,
            ],
        );
        let sessions = read_codex_root(tmp.path());
        // One recognized event (the future-typed payload) keeps the session.
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].skipped_lines, 1);
    }

    #[test]
    fn missing_roots_and_empty_files_yield_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(read_claude_root(&tmp.path().join("nope")).is_empty());
        write_lines(&tmp.path().join("proj/empty.jsonl"), &[]);
        assert!(read_claude_root(tmp.path()).is_empty());
    }
}
