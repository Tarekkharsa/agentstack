//! `agentstack report <run>` — the flight-recorder viewer (ROADMAP Phase 3).
//!
//! Reads one run's append-only event log (sandbox lifecycle + egress
//! decisions, from `~/.agentstack/runs/<id>/events.jsonl`) and the tool calls
//! that run's agent made (from the machine-global audit log, filtered by run
//! id), and renders a report a security reviewer can read. Read-only; scope is
//! deliberately a log + viewer, not a dashboard.

use anyhow::Result;
use owo_colors::OwoColorize;

use agentstack_recorder::{read_all, CallRecord, RunEvent, RunLog};

use crate::cli::ReportArgs;

pub fn run(args: &ReportArgs) -> Result<()> {
    if args.json {
        println!("{}", report_json(&args.run)?);
    } else {
        print!("{}", report_text(&args.run));
    }
    Ok(())
}

/// The tool calls attributed to this run (audit log filtered by run id).
fn calls_for(run_id: &str) -> Vec<CallRecord> {
    read_all()
        .into_iter()
        .filter(|c| c.run.as_deref() == Some(run_id))
        .collect()
}

/// One tool call, normalized from either source (the run's own `ToolCall`
/// events or a fallback `CallRecord` from the audit log) so the renderer treats
/// them identically.
struct ToolRow {
    server: String,
    tool: String,
    outcome: String,
    detail: Option<String>,
    ms: u64,
}

/// The run's tool calls, preferring its self-contained `events.jsonl` and
/// falling back to the cross-project audit log for older runs. The two sources
/// carry the same non-sensitive fields (server, tool, decision, digest-backed
/// timing) — never argument values.
fn tool_rows(events: &[RunEvent], calls: &[CallRecord]) -> Vec<ToolRow> {
    let from_events: Vec<ToolRow> = events
        .iter()
        .filter_map(|e| match e {
            RunEvent::ToolCall {
                server,
                tool,
                outcome,
                detail,
                ms,
                ..
            } => Some(ToolRow {
                server: server.clone(),
                tool: tool.clone(),
                outcome: outcome.clone(),
                detail: detail.clone(),
                ms: *ms,
            }),
            _ => None,
        })
        .collect();
    if !from_events.is_empty() {
        return from_events;
    }
    calls
        .iter()
        .map(|c| ToolRow {
            server: c.server.clone(),
            tool: c.tool.clone(),
            outcome: c.outcome.clone(),
            detail: c.detail.clone(),
            ms: c.ms,
        })
        .collect()
}

/// Distinct `(server, ref)` secret references this run resolved, in first-seen
/// order. Ref NAMES only — a value never enters the event log.
fn secret_refs(events: &[RunEvent]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for e in events {
        if let RunEvent::SecretAccess {
            server, reference, ..
        } = e
        {
            let pair = (server.clone(), reference.clone());
            if !out.contains(&pair) {
                out.push(pair);
            }
        }
    }
    out
}

/// A one-line wall-time summary, or `None` when there's nothing to report. The
/// sandbox lifetime needs both a `SandboxStarted` and a `SandboxExited` to be
/// known; the in-tool total is the sum of the run's tool-call durations.
fn wall_time_summary(events: &[RunEvent], rows: &[ToolRow]) -> Option<String> {
    let started = events.iter().find_map(|e| match e {
        RunEvent::SandboxStarted { ts, .. } => Some(*ts),
        _ => None,
    });
    let exited = events.iter().find_map(|e| match e {
        RunEvent::SandboxExited { ts, .. } => Some(*ts),
        _ => None,
    });
    let mut parts: Vec<String> = Vec::new();
    // `saturating_sub` guards against a clock that went backwards between the
    // two timestamps (epoch seconds are coarse and not monotonic).
    if let (Some(a), Some(b)) = (started, exited) {
        parts.push(format!("{}s sandbox", b.saturating_sub(a)));
    }
    if !rows.is_empty() {
        let in_tool: u64 = rows.iter().map(|r| r.ms).sum();
        parts.push(format!(
            "{} tool call{}, {in_tool}ms in-tool",
            rows.len(),
            if rows.len() == 1 { "" } else { "s" }
        ));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("; "))
    }
}

/// Render the human-readable report.
pub fn report_text(run_id: &str) -> String {
    let events = RunLog::read(run_id);
    let calls = calls_for(run_id);

    if events.is_empty() && calls.is_empty() {
        return format!(
            "No record for run '{run_id}'. Sandboxed runs record under \
             ~/.agentstack/runs/<id>/; tool calls appear once the run's agent \
             makes them.\n"
        );
    }

    let mut o = String::new();
    o.push_str(&format!("{}\n", format!("Run {run_id}").bold()));

    // Enforcement posture: the honest label for how strongly this run's policy
    // was enforced, recorded by the CLI when the sandbox started. Omitted for
    // runs that predate posture recording (the field is additive).
    if let Some(p) = crate::commands::sandbox::read_recorded_posture(run_id) {
        o.push_str(&format!("  {:<9} {}\n", "Posture", p));
    }

    // Lifecycle: the sandbox start line.
    for e in &events {
        if let RunEvent::SandboxStarted {
            image, workspace, ..
        } = e
        {
            o.push_str(&format!(
                "  {:<9} {}   workspace {}\n",
                "Sandbox", image, workspace
            ));
        }
    }

    // Egress decisions — allow and block both shown; a report is what the
    // sandbox reached, not only what it was denied.
    let egress: Vec<&RunEvent> = events
        .iter()
        .filter(|e| matches!(e, RunEvent::Egress { .. }))
        .collect();
    if !egress.is_empty() {
        o.push_str(&format!("  {}\n", "Egress".bold()));
        for e in egress {
            if let RunEvent::Egress {
                server,
                host,
                allowed,
                rule,
                ..
            } = e
            {
                let mark = if *allowed {
                    "✓".green().to_string()
                } else {
                    "✗".red().to_string()
                };
                let why = rule
                    .as_deref()
                    .map(|r| format!("  ({r})"))
                    .unwrap_or_default();
                o.push_str(&format!("    {mark} {server} → {host}{why}\n"));
            }
        }
    }

    // Tool calls the run's agent made (digest only, never argument values).
    // Sourced from the run's OWN events.jsonl when the gateway mirrored them
    // there; older runs fall back to the cross-project audit log.
    let rows = tool_rows(&events, &calls);
    if !rows.is_empty() {
        o.push_str(&format!("  {}\n", "Tool calls".bold()));
        for r in &rows {
            let mark = match r.outcome.as_str() {
                "ok" => "✓".green().to_string(),
                "denied" => "✗".red().to_string(),
                _ => "⚠".yellow().to_string(),
            };
            let why = r
                .detail
                .as_deref()
                .map(|d| format!("  ({d})"))
                .unwrap_or_default();
            o.push_str(&format!(
                "    {mark} {}__{}  {}ms{why}\n",
                r.server, r.tool, r.ms
            ));
        }
    }

    // Secret refs this run resolved — NAMES only, never values. New event kind;
    // omitted cleanly for runs recorded before the gateway emitted it.
    let secrets = secret_refs(&events);
    if !secrets.is_empty() {
        o.push_str(&format!("  {}\n", "Secret refs".bold()));
        for (server, reference) in &secrets {
            o.push_str(&format!("    {server} → {reference}\n"));
        }
    }

    // Wall-time summary: the sandbox's lifetime (when both start and exit were
    // recorded) and the time the agent spent inside tool calls. Omitted when
    // there's nothing to summarize.
    if let Some(line) = wall_time_summary(&events, &rows) {
        o.push_str(&format!("  {:<9} {}\n", "Wall time", line));
    }

    // Exit.
    for e in &events {
        if let RunEvent::SandboxExited { code, .. } = e {
            let shown = code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "killed by signal".to_string());
            o.push_str(&format!("  {:<9} {}\n", "Exit", shown));
        }
    }

    o
}

/// Render the report as JSON.
pub fn report_json(run_id: &str) -> Result<String> {
    let events = RunLog::read(run_id);
    // A gateway-routed run mirrors each tool call into BOTH events.jsonl (a
    // `ToolCall`) and the cross-project audit log, so emitting both raw would
    // double-count. But the mirror is best-effort and independent of the audit
    // write, so the two can diverge — a mirror that failed for one call leaves
    // that call ONLY in the audit log. So instead of dropping `calls` wholesale
    // when any ToolCall exists, keep only the audit records NOT already present
    // as a ToolCall event (matched on server+tool+digest+timestamp). That
    // de-dupes the common case and still surfaces a call the mirror missed.
    let mut event_keys = std::collections::HashSet::new();
    for e in &events {
        if let RunEvent::ToolCall {
            server,
            tool,
            args_digest,
            ts,
            ..
        } = e
        {
            event_keys.insert((server.clone(), tool.clone(), args_digest.clone(), *ts));
        }
    }
    let calls: Vec<CallRecord> = calls_for(run_id)
        .into_iter()
        .filter(|c| {
            !event_keys.contains(&(
                c.server.clone(),
                c.tool.clone(),
                c.args_digest.clone(),
                c.ts,
            ))
        })
        .collect();
    Ok(serde_json::to_string_pretty(&serde_json::json!({
        "run": run_id,
        // Additive field: the recorded enforcement posture slug, or null for a
        // run that predates posture recording.
        "posture": crate::commands::sandbox::read_recorded_posture(run_id).map(|p| p.slug()),
        "events": events,
        "calls": calls,
    }))?)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn renders_lifecycle_egress_and_calls() {
        with_home(|| {
            let log = RunLog::create("r-report").unwrap();
            log.append(&RunEvent::SandboxStarted {
                ts: 1,
                image: "agentstack/sandbox".into(),
                workspace: "/proj".into(),
            });
            log.append(&RunEvent::Egress {
                ts: 2,
                server: "web-search".into(),
                host: "api.search.example".into(),
                allowed: true,
                rule: None,
            });
            log.append(&RunEvent::Egress {
                ts: 3,
                server: "web-search".into(),
                host: "evil.example".into(),
                allowed: false,
                rule: Some("[policy.egress] denied".into()),
            });
            log.append(&RunEvent::SandboxExited {
                ts: 4,
                code: Some(0),
            });
            agentstack_recorder::record(&CallRecord {
                ts: 2,
                run: Some("r-report".into()),
                pid: 1,
                project: None,
                server: "web-search".into(),
                tool: "search".into(),
                args_digest: "abc".into(),
                outcome: "ok".into(),
                detail: None,
                ms: 12,
            });

            let text = report_text("r-report");
            assert!(text.contains("Run r-report"), "{text}");
            assert!(text.contains("agentstack/sandbox") && text.contains("/proj"));
            assert!(text.contains("api.search.example"));
            assert!(text.contains("evil.example") && text.contains("[policy.egress] denied"));
            assert!(text.contains("web-search__search") && text.contains("12ms"));
            assert!(text.contains("Exit") && text.contains('0'));
        });
    }

    /// A run whose gateway mirrored its calls into `events.jsonl` (every
    /// sandboxed run now) must not ALSO list them from the audit log in JSON —
    /// that would double-count. `calls` is the fallback, superseded by events.
    #[test]
    fn report_json_does_not_double_count_tool_calls() {
        with_home(|| {
            let log = RunLog::create("r-dedup").unwrap();
            // The same call, in BOTH logs (as the gateway writes it).
            log.append(&RunEvent::ToolCall {
                ts: 5,
                server: "figma".into(),
                tool: "get_file".into(),
                outcome: "ok".into(),
                args_digest: "abc".into(),
                detail: None,
                ms: 9,
            });
            agentstack_recorder::record(&CallRecord {
                ts: 5,
                run: Some("r-dedup".into()),
                pid: 1,
                project: None,
                server: "figma".into(),
                tool: "get_file".into(),
                args_digest: "abc".into(),
                outcome: "ok".into(),
                detail: None,
                ms: 9,
            });

            let v: serde_json::Value =
                serde_json::from_str(&report_json("r-dedup").unwrap()).unwrap();
            // The tool call appears once (as an event); the redundant audit-log
            // fallback is omitted.
            assert_eq!(v["calls"].as_array().unwrap().len(), 0, "{v}");
            let tool_events = v["events"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|e| e["event"] == "tool_call")
                .count();
            assert_eq!(tool_events, 1, "{v}");
        });
    }

    /// An OLDER run with no `ToolCall` events still surfaces its audit-log
    /// calls in JSON (the fallback path).
    #[test]
    fn report_json_keeps_calls_for_event_less_runs() {
        with_home(|| {
            let log = RunLog::create("r-old").unwrap();
            log.append(&RunEvent::SandboxExited {
                ts: 2,
                code: Some(0),
            });
            agentstack_recorder::record(&CallRecord {
                ts: 1,
                run: Some("r-old".into()),
                pid: 1,
                project: None,
                server: "figma".into(),
                tool: "get_file".into(),
                args_digest: "abc".into(),
                outcome: "ok".into(),
                detail: None,
                ms: 9,
            });
            let v: serde_json::Value =
                serde_json::from_str(&report_json("r-old").unwrap()).unwrap();
            assert_eq!(v["calls"].as_array().unwrap().len(), 1, "{v}");
        });
    }

    #[test]
    fn posture_line_shown_when_recorded() {
        with_home(|| {
            let log = RunLog::create("r-post").unwrap();
            log.append(&RunEvent::SandboxExited {
                ts: 1,
                code: Some(0),
            });
            // The CLI records posture beside events.jsonl; emulate that here.
            std::fs::write(log.path().with_file_name("posture"), "lockdown").unwrap();
            let text = report_text("r-post");
            assert!(text.contains("Posture"), "{text}");
            assert!(text.contains("LOCKDOWN / ENFORCED"), "{text}");
            // JSON carries the slug.
            let v: serde_json::Value =
                serde_json::from_str(&report_json("r-post").unwrap()).unwrap();
            assert_eq!(v["posture"], "lockdown");
        });
    }

    #[test]
    fn renders_event_sourced_tool_calls_secrets_and_wall_time() {
        with_home(|| {
            let log = RunLog::create("r-actions").unwrap();
            log.append(&RunEvent::SandboxStarted {
                ts: 100,
                image: "agentstack/sandbox".into(),
                workspace: "/proj".into(),
            });
            log.append(&RunEvent::ToolCall {
                ts: 101,
                server: "figma".into(),
                tool: "get_file".into(),
                outcome: "ok".into(),
                args_digest: "abc123".into(),
                detail: None,
                ms: 30,
            });
            log.append(&RunEvent::ToolCall {
                ts: 102,
                server: "figma".into(),
                tool: "delete_file".into(),
                outcome: "denied".into(),
                args_digest: "def456".into(),
                detail: Some("machine policy denies delete_*".into()),
                ms: 0,
            });
            log.append(&RunEvent::SecretAccess {
                ts: 103,
                server: "figma".into(),
                reference: "FIGMA_TOKEN".into(),
            });
            // A duplicate ref must collapse to one line.
            log.append(&RunEvent::SecretAccess {
                ts: 104,
                server: "figma".into(),
                reference: "FIGMA_TOKEN".into(),
            });
            log.append(&RunEvent::SandboxExited {
                ts: 110,
                code: Some(0),
            });

            let text = report_text("r-actions");
            // Tool calls sourced from the run's OWN events (no audit record).
            assert!(
                text.contains("figma__get_file") && text.contains("30ms"),
                "{text}"
            );
            assert!(
                text.contains("figma__delete_file")
                    && text.contains("machine policy denies delete_*"),
                "{text}"
            );
            // Secret refs section, names only, deduped to a single line.
            assert!(text.contains("Secret refs"), "{text}");
            assert_eq!(text.matches("FIGMA_TOKEN").count(), 1, "{text}");
            // Wall-time summary: 10s sandbox span, 2 calls, 30ms in-tool.
            assert!(text.contains("Wall time"), "{text}");
            assert!(text.contains("10s sandbox"), "{text}");
            assert!(
                text.contains("2 tool calls") && text.contains("30ms in-tool"),
                "{text}"
            );
            // JSON carries the new event kinds too.
            let v: serde_json::Value =
                serde_json::from_str(&report_json("r-actions").unwrap()).unwrap();
            assert_eq!(v["events"][1]["event"], "tool_call");
            assert_eq!(v["events"][3]["event"], "secret_access");
            assert_eq!(v["events"][3]["ref"], "FIGMA_TOKEN");
        });
    }

    #[test]
    fn unknown_run_reports_no_record() {
        with_home(|| {
            let text = report_text("r-nope");
            assert!(text.contains("No record for run 'r-nope'"), "{text}");
        });
    }

    #[test]
    fn json_carries_events_and_calls() {
        with_home(|| {
            let log = RunLog::create("r-json").unwrap();
            log.append(&RunEvent::SandboxExited {
                ts: 1,
                code: Some(2),
            });
            let json = report_json("r-json").unwrap();
            let v: serde_json::Value = serde_json::from_str(&json).unwrap();
            assert_eq!(v["run"], "r-json");
            assert_eq!(v["events"][0]["event"], "sandbox_exited");
            assert_eq!(v["events"][0]["code"], 2);
        });
    }
}
