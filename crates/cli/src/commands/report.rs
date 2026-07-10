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
    if !calls.is_empty() {
        o.push_str(&format!("  {}\n", "Tool calls".bold()));
        for c in &calls {
            let mark = match c.outcome.as_str() {
                "ok" => "✓".green().to_string(),
                "denied" => "✗".red().to_string(),
                _ => "⚠".yellow().to_string(),
            };
            let why = c
                .detail
                .as_deref()
                .map(|d| format!("  ({d})"))
                .unwrap_or_default();
            o.push_str(&format!(
                "    {mark} {}__{}  {}ms{why}\n",
                c.server, c.tool, c.ms
            ));
        }
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
    let calls = calls_for(run_id);
    Ok(serde_json::to_string_pretty(&serde_json::json!({
        "run": run_id,
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
