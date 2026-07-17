//! `agentstack report calls` — read-only usage analytics that complements `stats`.
//!
//! `stats` is the per-project inventory (activation counts + context-cost
//! footprint). `analyze` adds the two things it doesn't show: runtime **call
//! activity** from the audit log (`calllog`), and **library-wide dead weight** —
//! capabilities installed in the central library but never used anywhere. Local:
//! no network, no writes.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use owo_colors::OwoColorize;
use serde_json::{json, Value};

use crate::calllog::{self, CallRecord};
use crate::cli::AnalyzeArgs;
use crate::footprint::{fmt_tokens, Footprints};
use crate::library::Library;
use crate::usage::Usage;

pub fn run(args: &AnalyzeArgs) -> Result<()> {
    let mut report = collect();
    if args.transcripts {
        report["transcripts"] = transcripts_summary(&crate::transcripts::read_default());
    }
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_human(&report);
        if args.transcripts {
            print_transcripts(&report["transcripts"]);
        }
    }
    Ok(())
}

/// The analytics report as JSON — the shared shape the CLI renders and the
/// dashboard can consume. Every source is best-effort: a missing/corrupt file
/// degrades to empty rather than failing.
pub fn collect() -> Value {
    let calls = calllog::read_all();
    let usage = Usage::load().unwrap_or_default();
    let footprints = Footprints::load().unwrap_or_default();
    let library = Library::load_default().unwrap_or_default();

    json!({
        "calls": calls_summary(&calls),
        "dead_weight": dead_weight(&library, &usage, &footprints, &calls),
    })
}

fn calls_summary(calls: &[CallRecord]) -> Value {
    let (mut ok, mut error, mut denied) = (0u64, 0u64, 0u64);
    let mut per_server: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    let mut per_tool: BTreeMap<String, u64> = BTreeMap::new();
    let (mut min_ts, mut max_ts) = (u64::MAX, 0u64);

    for c in calls {
        match c.outcome.as_str() {
            "ok" => ok += 1,
            "denied" => denied += 1,
            _ => error += 1,
        }
        let entry = per_server.entry(c.server.clone()).or_insert((0, 0));
        entry.0 += 1;
        if c.outcome != agentstack_recorder::CallOutcome::Ok {
            entry.1 += 1;
        }
        *per_tool.entry(c.tool.clone()).or_insert(0) += 1;
        min_ts = min_ts.min(c.ts);
        max_ts = max_ts.max(c.ts);
    }

    let span_days = if calls.is_empty() {
        0
    } else {
        max_ts.saturating_sub(min_ts) / 86_400
    };

    let mut servers: Vec<_> = per_server.into_iter().collect();
    servers.sort_by(|a, b| b.1 .0.cmp(&a.1 .0).then(a.0.cmp(&b.0)));
    let by_server: Vec<Value> = servers
        .iter()
        .take(8)
        .map(|(s, (c, e))| json!({ "server": s, "calls": c, "errors": e }))
        .collect();

    let mut tools: Vec<_> = per_tool.into_iter().collect();
    tools.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let by_tool: Vec<Value> = tools
        .iter()
        .take(8)
        .map(|(t, c)| json!({ "tool": t, "calls": c }))
        .collect();

    json!({
        "total": calls.len(),
        "ok": ok,
        "error": error,
        "denied": denied,
        "span_days": span_days,
        "by_server": by_server,
        "by_tool": by_tool,
    })
}

fn dead_weight(lib: &Library, usage: &Usage, fp: &Footprints, calls: &[CallRecord]) -> Value {
    let called: BTreeSet<&str> = calls.iter().map(|c| c.server.as_str()).collect();

    // A library skill that no project has ever activated.
    let skills: Vec<Value> = lib
        .skills
        .iter()
        .filter(|s| usage.count(&s.name) == 0)
        .map(|s| json!({ "name": s.name }))
        .collect();

    // A library server never rendered into a config AND never called through
    // the gateway — pure overhead if it's live anywhere.
    let servers: Vec<Value> = lib
        .servers
        .iter()
        .filter(|s| usage.count(&s.name) == 0 && !called.contains(s.name.as_str()))
        .map(|s| json!({ "name": s.name, "est_tokens": fp.get(&s.name).map(|f| f.est_tokens) }))
        .collect();

    json!({ "skills": skills, "servers": servers })
}

/// Aggregate per-session transcript summaries into a per-harness report.
/// Coverage is uneven by nature (rich for Claude Code, thinner for Codex), so
/// each harness block names how much it could actually see.
fn transcripts_summary(sessions: &[crate::transcripts::SessionSummary]) -> Value {
    /// sessions, input tokens, output tokens, tool -> calls.
    type HarnessAgg = (u64, u64, u64, BTreeMap<String, u64>);
    let mut by_harness: BTreeMap<&str, HarnessAgg> = BTreeMap::new();
    for s in sessions {
        let e = by_harness.entry(s.harness.as_str()).or_default();
        e.0 += 1;
        e.1 += s.input_tokens;
        e.2 += s.output_tokens;
        for (tool, n) in &s.tools {
            *e.3.entry(tool.clone()).or_insert(0) += n;
        }
    }
    let harnesses: Vec<Value> = by_harness
        .into_iter()
        .map(|(harness, (count, input, output, tools))| {
            let mut top: Vec<_> = tools.into_iter().collect();
            top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            let top_tools: Vec<Value> = top
                .iter()
                .take(8)
                .map(|(t, n)| json!({ "tool": t, "calls": n }))
                .collect();
            json!({
                "harness": harness,
                "sessions": count,
                "input_tokens": input,
                "output_tokens": output,
                "top_tools": top_tools,
            })
        })
        .collect();
    json!({ "harnesses": harnesses })
}

fn print_transcripts(t: &Value) {
    println!("\n{}", "Transcripts".bold());
    let Some(harnesses) = t["harnesses"].as_array().filter(|a| !a.is_empty()) else {
        println!(
            "  {}",
            "No local session transcripts found (~/.claude/projects, ~/.codex/sessions).".dimmed()
        );
        return;
    };
    for h in harnesses {
        println!(
            "  {:<12} {} session(s) · {} in (uncached) / {} out",
            h["harness"].as_str().unwrap_or("?"),
            h["sessions"].as_u64().unwrap_or(0),
            fmt_tokens(h["input_tokens"].as_u64().unwrap_or(0)),
            fmt_tokens(h["output_tokens"].as_u64().unwrap_or(0)),
        );
        if let Some(tools) = h["top_tools"].as_array().filter(|a| !a.is_empty()) {
            for t in tools.iter().take(5) {
                println!(
                    "    {:<24} {:>6}",
                    t["tool"].as_str().unwrap_or("?"),
                    t["calls"].as_u64().unwrap_or(0),
                );
            }
        }
    }
    println!(
        "\n  {}",
        "Aggregates only — transcript content is never read into the report.".dimmed()
    );
}

fn print_human(report: &Value) {
    let calls = &report["calls"];
    let total = calls["total"].as_u64().unwrap_or(0);

    println!("{}", "Call activity".bold());
    if total == 0 {
        println!(
            "  {}",
            "No brokered calls recorded yet — the runtime gateway logs them when \
             you use `agentstack run` / `agentstack mcp`."
                .dimmed()
        );
    } else {
        let span = calls["span_days"].as_u64().unwrap_or(0);
        let span_str = if span == 0 {
            "today".to_string()
        } else {
            format!("{span}d")
        };
        println!("  {total} calls over {span_str}");
        println!(
            "  {} {}   {} {}   {} {}",
            "ok".green(),
            calls["ok"].as_u64().unwrap_or(0),
            "error".red(),
            calls["error"].as_u64().unwrap_or(0),
            "denied".yellow(),
            calls["denied"].as_u64().unwrap_or(0),
        );
        if let Some(servers) = calls["by_server"].as_array().filter(|a| !a.is_empty()) {
            println!("\n  {}", "top servers".dimmed());
            for s in servers {
                let er = s["errors"].as_u64().unwrap_or(0);
                let etag = if er > 0 {
                    format!("  ({er} error/denied)").red().to_string()
                } else {
                    String::new()
                };
                println!(
                    "    {:<24} {:>5} calls{etag}",
                    s["server"].as_str().unwrap_or("?"),
                    s["calls"].as_u64().unwrap_or(0),
                );
            }
        }
        if let Some(tools) = calls["by_tool"].as_array().filter(|a| !a.is_empty()) {
            println!("\n  {}", "top tools".dimmed());
            for t in tools {
                println!(
                    "    {:<24} {:>5}",
                    t["tool"].as_str().unwrap_or("?"),
                    t["calls"].as_u64().unwrap_or(0),
                );
            }
        }
    }

    let dw = &report["dead_weight"];
    let skills = dw["skills"].as_array().cloned().unwrap_or_default();
    let servers = dw["servers"].as_array().cloned().unwrap_or_default();
    println!("\n{}", "Library dead weight".bold());
    if skills.is_empty() && servers.is_empty() {
        println!(
            "  {}",
            "Nothing unused — or nothing installed in the central library yet.".dimmed()
        );
        return;
    }
    if !skills.is_empty() {
        println!("  {} never activated:", "skills".dimmed());
        for s in &skills {
            println!("    - {}", s["name"].as_str().unwrap_or("?"));
        }
    }
    if !servers.is_empty() {
        println!("  {} installed but never called:", "servers".dimmed());
        for s in &servers {
            let cost = s["est_tokens"]
                .as_u64()
                .map(|t| format!(" (~{}/session)", fmt_tokens(t)))
                .unwrap_or_default();
            println!("    - {}{cost}", s["name"].as_str().unwrap_or("?"));
        }
    }
    println!(
        "\n  {}",
        "Prune with `agentstack lib remove <name>` (or drop it from a profile).".dimmed()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calllog::CallRecord;

    fn rec(server: &str, tool: &str, outcome: &str, ts: u64) -> CallRecord {
        CallRecord {
            ts,
            run: None,
            pid: 1,
            project: None,
            server: server.into(),
            tool: tool.into(),
            args_digest: "0".into(),
            outcome: match outcome {
                "denied" => agentstack_recorder::CallOutcome::Denied,
                "error" => agentstack_recorder::CallOutcome::Error,
                _ => agentstack_recorder::CallOutcome::Ok,
            },
            detail: None,
            ms: 1,
        }
    }

    #[test]
    fn summarizes_calls_by_outcome_and_server() {
        let calls = vec![
            rec("figma", "figma__get", "ok", 0),
            rec("figma", "figma__get", "error", 86_400),
            rec("github", "github__list", "denied", 0),
        ];
        let s = calls_summary(&calls);
        assert_eq!(s["total"], 3);
        assert_eq!(s["ok"], 1);
        assert_eq!(s["error"], 1);
        assert_eq!(s["denied"], 1);
        assert_eq!(s["span_days"], 1);
        // figma has the most calls → first, with one non-ok counted as error.
        assert_eq!(s["by_server"][0]["server"], "figma");
        assert_eq!(s["by_server"][0]["calls"], 2);
        assert_eq!(s["by_server"][0]["errors"], 1);
    }

    #[test]
    fn transcripts_summary_groups_by_harness_and_ranks_tools() {
        use crate::transcripts::SessionSummary;
        let mut a = SessionSummary {
            harness: "claude-code".into(),
            input_tokens: 100,
            output_tokens: 10,
            ..Default::default()
        };
        a.tools.insert("Bash".into(), 3);
        a.tools.insert("Read".into(), 1);
        let mut b = SessionSummary {
            harness: "claude-code".into(),
            input_tokens: 50,
            output_tokens: 5,
            ..Default::default()
        };
        b.tools.insert("Bash".into(), 2);
        let c = SessionSummary {
            harness: "codex".into(),
            input_tokens: 2500,
            output_tokens: 90,
            ..Default::default()
        };

        let t = transcripts_summary(&[a, b, c]);
        let hs = t["harnesses"].as_array().unwrap();
        assert_eq!(hs.len(), 2);
        // BTreeMap order: claude-code before codex.
        assert_eq!(hs[0]["harness"], "claude-code");
        assert_eq!(hs[0]["sessions"], 2);
        assert_eq!(hs[0]["input_tokens"], 150);
        assert_eq!(hs[0]["top_tools"][0]["tool"], "Bash");
        assert_eq!(hs[0]["top_tools"][0]["calls"], 5);
        assert_eq!(hs[1]["harness"], "codex");
        assert_eq!(hs[1]["output_tokens"], 90);
    }

    #[test]
    fn dead_weight_flags_uncalled_unactivated_capabilities() {
        use crate::library::{Library, LibrarySkill};
        let mut lib = Library::default();
        lib.skills.push(LibrarySkill {
            name: "used".into(),
            source: "path".into(),
            path: Some("used".into()),
            git: None,
            rev: None,
            subpath: None,
            checksum: None,
            version: None,
            provenance: None,
        });
        lib.skills.push(LibrarySkill {
            name: "unused".into(),
            source: "path".into(),
            path: Some("unused".into()),
            git: None,
            rev: None,
            subpath: None,
            checksum: None,
            version: None,
            provenance: None,
        });
        let mut usage = Usage::default();
        usage.activations.insert("used".into(), 3);

        let dw = dead_weight(&lib, &usage, &Footprints::default(), &[]);
        let names: Vec<&str> = dw["skills"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["unused"], "only the never-activated skill");
    }
}
