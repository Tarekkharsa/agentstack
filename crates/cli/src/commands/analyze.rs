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
    let mut calls = calllog::read_all();
    if let Some(days) = args.since {
        let cutoff = calllog::now_epoch().saturating_sub(days * 86_400);
        calls.retain(|e| e.ts >= cutoff);
    }
    let report = collect_with(&calls);
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_human(&report);
        print_tool_table(&calls);
    }
    Ok(())
}

/// The analytics report as JSON — the shared shape the CLI renders and the
/// dashboard can consume. Every source is best-effort: a missing/corrupt file
/// degrades to empty rather than failing.
pub fn collect() -> Value {
    collect_with(&calllog::read_all())
}

fn collect_with(calls: &[CallRecord]) -> Value {
    let usage = Usage::load().unwrap_or_default();
    let footprints = Footprints::load().unwrap_or_default();
    let library = Library::load_default().unwrap_or_default();

    json!({
        "calls": calls_summary(calls),
        "dead_weight": dead_weight(&library, &usage, &footprints, calls),
    })
}

/// The full per-tool table (every `server__tool`, ok/err/denied/last-seen) —
/// the detail view the retired `audit --calls` used to print, kept here so
/// `report calls` is a strict superset of it.
fn print_tool_table(calls: &[CallRecord]) {
    if calls.is_empty() {
        return;
    }
    struct Row {
        ok: u64,
        err: u64,
        denied: u64,
        last: u64,
    }
    let mut rows: BTreeMap<String, Row> = BTreeMap::new();
    for e in calls {
        let r = rows
            .entry(format!("{}__{}", e.server, e.tool))
            .or_insert(Row {
                ok: 0,
                err: 0,
                denied: 0,
                last: 0,
            });
        match e.outcome.as_str() {
            "ok" => r.ok += 1,
            "denied" => r.denied += 1,
            _ => r.err += 1,
        }
        r.last = r.last.max(e.ts);
    }
    println!(
        "\n{:<40} {:>6} {:>6} {:>7}  {}",
        "tool".bold(),
        "ok".bold(),
        "err".bold(),
        "denied".bold(),
        "last".bold()
    );
    for (name, r) in &rows {
        let age_d = calllog::now_epoch().saturating_sub(r.last) / 86_400;
        let last = if age_d == 0 {
            "today".to_string()
        } else {
            format!("{age_d}d ago")
        };
        // Pad BEFORE coloring — ANSI escapes would break the column width.
        let denied = format!("{:>7}", r.denied);
        let denied = if r.denied > 0 {
            denied.red().to_string()
        } else {
            denied
        };
        println!("{name:<40} {:>6} {:>6} {denied}  {last}", r.ok, r.err);
    }
    println!(
        "\nLog: {} (argument digests only — never values)",
        calllog::log_path().display()
    );
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
