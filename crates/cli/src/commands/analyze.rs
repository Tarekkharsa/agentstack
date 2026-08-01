//! `agentstack report calls` — read-only usage analytics that complements `stats`.
//!
//! `stats` is the per-project inventory (activation counts + context-cost
//! footprint). `analyze` adds the two things it doesn't show: runtime **call
//! activity** from the audit log (`calllog`), and **library-wide dead weight** —
//! capabilities installed in the central library but never used anywhere. Local:
//! no network, no writes.
//!
//! `--tail N` additionally lists the last N individual calls; with `--json`
//! they land in an `events` array — the stable activity feed external UIs
//! consume (each entry is a raw [`CallRecord`]: argument digests only, never
//! values). The array is only present when `--tail` is asked for, so the
//! default JSON shape existing consumers parse is unchanged.
//!
//! `--include-loads` widens that feed to on-demand skill loads (`loads.jsonl`),
//! interleaved by timestamp and tagged with a `kind` discriminant on EVERY row.
//! It is opt-in for exactly that reason: without it the feed is byte-identical
//! to before, so a consumer whose decoder predates load rows never meets a row
//! shape it doesn't know. A load is never a call — it is absent from
//! `calls_summary`, from the human tables, and from every count here.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::Result;
use owo_colors::OwoColorize;
use serde_json::{json, Value};

use crate::calllog::{self, CallRecord, LoadRecord};
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
    if let Some(project) = &args.project {
        let want = crate::util::paths::expand_tilde(&project.display().to_string());
        calls.retain(|e| project_matches(e, &want));
    }
    let report = build_report(args, &calls);
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_human(&report);
        print_tool_table(&calls);
        if let Some(n) = args.tail {
            print_recent_calls(&calls, n);
        }
    }
    Ok(())
}

/// The JSON report for already-filtered calls. Separated from [`run`] so the
/// regression witness can compare two reports byte for byte.
fn build_report(args: &AnalyzeArgs, calls: &[CallRecord]) -> Value {
    let mut report = collect_with(calls);
    if args.include_loads {
        // Loads never touch `collect_with` — they are activity, not calls, and
        // every summary above counts calls. `--tail` still bounds the feed;
        // without it the whole (filtered) activity history is emitted, exactly
        // as `--tail` with an unreachable N would.
        let mut loads = calllog::read_loads_all();
        if let Some(days) = args.since {
            let cutoff = calllog::now_epoch().saturating_sub(days * 86_400);
            loads.retain(|l| l.ts >= cutoff);
        }
        if let Some(project) = &args.project {
            let want = crate::util::paths::expand_tilde(&project.display().to_string());
            loads.retain(|l| path_matches(l.project.as_deref(), &want));
        }
        report["events"] = merged_events(calls, &loads, args.tail.unwrap_or(usize::MAX));
    } else if let Some(n) = args.tail {
        report["events"] = tail_events(calls, n);
    }
    report
}

/// Component-wise path comparison, so `~/proj`, `/Users/x/proj`, and a
/// trailing-slash variant all name the same recorded project root. Shared by
/// both streams: a load records `project` in the same format a call does, so
/// `--project` must filter them identically.
fn path_matches(recorded: Option<&str>, want: &Path) -> bool {
    recorded.is_some_and(|p| Path::new(p) == want)
}

fn project_matches(rec: &CallRecord, want: &Path) -> bool {
    path_matches(rec.project.as_deref(), want)
}

/// The last `n` calls (input is already in append/chronological order),
/// serialized as raw records — the digest-only wire form of the log itself.
fn tail_events(calls: &[CallRecord], n: usize) -> Value {
    let start = calls.len().saturating_sub(n);
    serde_json::to_value(&calls[start..]).unwrap_or_else(|_| json!([]))
}

/// The `--include-loads` feed: calls and skill loads in one timestamp-ordered
/// array, every row carrying a `kind`. Call rows are the same records
/// `tail_events` emits with `kind: "call"` added; load rows are identity only
/// (name + agent-supplied reason), never the skill body.
///
/// `n` bounds the MERGED list, not either stream, so `--tail 10` means the ten
/// most recent activities of any kind.
fn merged_events(calls: &[CallRecord], loads: &[LoadRecord], n: usize) -> Value {
    let mut rows: Vec<(u64, Value)> = Vec::with_capacity(calls.len() + loads.len());
    for c in calls {
        let Ok(Value::Object(mut row)) = serde_json::to_value(c) else {
            continue;
        };
        row.insert("kind".into(), json!("call"));
        rows.push((c.ts, Value::Object(row)));
    }
    for l in loads {
        let mut row = json!({
            "kind": "skill_load",
            "ts": l.ts,
            "name": l.name,
            "reason": l.reason,
        });
        // Optional fields follow the record's own wire form: present only when
        // recorded, exactly as a call row omits an absent run/project.
        if let Some(run) = &l.run {
            row["run"] = json!(run);
        }
        if let Some(project) = &l.project {
            row["project"] = json!(project);
        }
        rows.push((l.ts, row));
    }
    // `sort_by_key` is stable, and calls were pushed first — so on an equal
    // timestamp a call sorts before a load, deterministically.
    rows.sort_by_key(|(ts, _)| *ts);
    let start = rows.len().saturating_sub(n);
    Value::Array(rows[start..].iter().map(|(_, row)| row.clone()).collect())
}

fn print_recent_calls(calls: &[CallRecord], n: usize) {
    let start = calls.len().saturating_sub(n);
    let recent = &calls[start..];
    if recent.is_empty() {
        return;
    }
    println!(
        "\n{}",
        format!("Last {}", super::count(recent.len(), "call")).bold()
    );
    for e in recent {
        let age_s = calllog::now_epoch().saturating_sub(e.ts);
        let age = match age_s {
            0..=59 => format!("{age_s}s ago"),
            60..=3_599 => format!("{}m ago", age_s / 60),
            3_600..=86_399 => format!("{}h ago", age_s / 3_600),
            _ => format!("{}d ago", age_s / 86_400),
        };
        // Pad BEFORE coloring — ANSI escapes would break the column width.
        let outcome = format!("{:<6}", e.outcome.as_str());
        let outcome = match e.outcome {
            calllog::CallOutcome::Ok => outcome.green().to_string(),
            calllog::CallOutcome::Denied => outcome.yellow().to_string(),
            calllog::CallOutcome::Error => outcome.red().to_string(),
        };
        let run = e.run.as_deref().unwrap_or("-");
        // Guard entries embed the whole command in `tool` — truncate for the
        // table (the JSON events keep the full string).
        let mut name = format!("{}__{}", e.server, e.tool);
        if name.chars().count() > 60 {
            name = format!("{}…", name.chars().take(59).collect::<String>());
        }
        println!("  {outcome} {name:<60} {:>6}ms  {:<10} {age}", e.ms, run);
    }
}

/// The analytics report as JSON — the shared shape the CLI renders and the
/// external UIs can consume. Every source is best-effort: a missing/corrupt file
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
        "Prune with `agentstack lib remove <name>` (or drop it from a toolset).".dimmed()
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
    fn project_filter_and_tail_keep_only_matching_recent_events() {
        let mut a1 = rec("figma", "figma__get", "ok", 10);
        a1.project = Some("/tmp/proj-a".into());
        let mut a2 = rec("github", "github__list", "denied", 20);
        a2.project = Some("/tmp/proj-a/".into()); // trailing slash — same root
        let mut b = rec("figma", "figma__get", "ok", 15);
        b.project = Some("/tmp/proj-b".into());
        let none = rec("figma", "figma__get", "ok", 30); // no project recorded

        let mut calls = vec![a1, b, a2, none];
        let want = std::path::PathBuf::from("/tmp/proj-a");
        calls.retain(|e| project_matches(e, &want));
        assert_eq!(
            calls.len(),
            2,
            "component-wise match, record without project dropped"
        );

        // tail keeps the LAST n in log order and serializes digests only.
        let events = tail_events(&calls, 1);
        let events = events.as_array().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["tool"], "github__list");
        assert_eq!(events[0]["outcome"], "denied");
        assert_eq!(events[0]["args_digest"], "0");
        assert!(events[0].get("args").is_none(), "raw args never serialize");
        // Larger n than available degrades to everything, no panic.
        assert_eq!(tail_events(&calls, 99).as_array().unwrap().len(), 2);
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

    fn analyze_args(tail: Option<usize>, include_loads: bool) -> AnalyzeArgs {
        AnalyzeArgs {
            json: true,
            since: None,
            tail,
            include_loads,
            project: None,
        }
    }

    fn load(ts: u64, name: &str, project: Option<&str>, run: Option<&str>) -> LoadRecord {
        LoadRecord::new(
            ts,
            name,
            "because the task needs it",
            project.map(str::to_owned),
            run.map(str::to_owned),
        )
    }

    /// The regression witness for the flag's whole reason to exist: with loads
    /// sitting in `loads.jsonl`, the report WITHOUT `--include-loads` is byte
    /// for byte the report produced when the stream is empty. An older
    /// consumer's strict decoder never meets a row shape it predates.
    #[test]
    fn loads_on_disk_never_change_the_default_feed() {
        with_home(|| {
            let calls = vec![rec("figma", "figma__get", "ok", 10)];
            let args = analyze_args(Some(10), false);
            let empty_stream = serde_json::to_string_pretty(&build_report(&args, &calls)).unwrap();

            for l in [
                load(5, "rust-review", None, None),
                load(11, "docs", None, None),
            ] {
                calllog::record_skill_load(&l);
            }
            assert_eq!(calllog::read_loads_all().len(), 2, "loads are on disk");

            let with_stream = serde_json::to_string_pretty(&build_report(&args, &calls)).unwrap();
            assert_eq!(with_stream, empty_stream, "default feed is byte-identical");
            assert!(!with_stream.contains("skill_load"), "{with_stream}");
            assert!(!with_stream.contains("kind"), "{with_stream}");

            // Asking for them changes the feed and nothing else: the counts
            // above the feed are identical either way.
            let opted_in = build_report(&analyze_args(Some(10), true), &calls);
            let plain: Value = serde_json::from_str(&with_stream).unwrap();
            assert_eq!(opted_in["calls"], plain["calls"], "counts untouched");
            assert_eq!(opted_in["dead_weight"], plain["dead_weight"]);
            let events = opted_in["events"].as_array().unwrap();
            assert_eq!(events.len(), 3, "{events:?}");
            assert_eq!(events[0]["kind"], "skill_load");
            assert_eq!(events[0]["name"], "rust-review");
            assert_eq!(events[1]["kind"], "call");
            assert_eq!(events[2]["kind"], "skill_load");
            // Identity only: a load row never carries a body or a call's shape.
            assert!(events[0].get("args_digest").is_none());
            assert!(events[0].get("outcome").is_none());
        });
    }

    /// Merge order (ts ascending, calls first on a tie), `--tail` applied to
    /// the merged list, and `--project` filtering loads the same way it
    /// filters calls.
    #[test]
    fn merged_feed_orders_by_timestamp_and_filters_like_calls() {
        let mut c1 = rec("figma", "figma__get", "ok", 20);
        c1.project = Some("/tmp/proj-a".into());
        let mut c2 = rec("github", "github__list", "ok", 40);
        c2.project = Some("/tmp/proj-a".into());
        let calls = vec![c1, c2];
        let loads = vec![
            load(20, "same-ts", Some("/tmp/proj-a"), None),
            load(30, "middle", Some("/tmp/proj-a"), Some("r-1")),
        ];

        let merged = merged_events(&calls, &loads, usize::MAX);
        let rows = merged.as_array().unwrap();
        let order: Vec<(&str, u64)> = rows
            .iter()
            .map(|r| (r["kind"].as_str().unwrap(), r["ts"].as_u64().unwrap()))
            .collect();
        assert_eq!(
            order,
            vec![
                ("call", 20),
                ("skill_load", 20),
                ("skill_load", 30),
                ("call", 40)
            ],
            "ts ascending, and a call before a load on an equal ts"
        );
        assert_eq!(rows[2]["run"], "r-1", "run attribution rides along");
        // `run` is omitted, not null, when the load carried no attribution.
        let merged = merged_events(&[], &loads[..1], usize::MAX);
        assert!(merged[0].get("run").is_none(), "{merged}");

        // --tail bounds the MERGED list: the two most recent activities.
        let merged = merged_events(&calls, &loads, 2);
        let rows = merged.as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["kind"], "skill_load");
        assert_eq!(rows[1]["ts"], 40);

        // --project filters loads with the same comparison calls use.
        let want = std::path::PathBuf::from("/tmp/proj-b");
        assert!(!path_matches(loads[0].project.as_deref(), &want));
        let want = std::path::PathBuf::from("/tmp/proj-a");
        assert!(path_matches(loads[0].project.as_deref(), &want));
        assert!(
            !path_matches(None, &want),
            "an unattributed load is dropped"
        );
    }

    /// Counting discipline: a load is never a call. Whatever is in
    /// `loads.jsonl`, the summary totals are computed over calls alone.
    #[test]
    fn loads_never_enter_the_call_summary() {
        with_home(|| {
            for i in 0..5 {
                calllog::record_skill_load(&load(i, "noisy", None, None));
            }
            let calls = vec![
                rec("figma", "figma__get", "ok", 0),
                rec("figma", "figma__get", "error", 86_400),
                rec("github", "github__list", "denied", 0),
            ];
            let s = calls_summary(&calls);
            assert_eq!(s["total"], 3, "loads must not inflate the total");
            assert_eq!(s["ok"], 1);
            assert_eq!(s["error"], 1);
            assert_eq!(s["denied"], 1);
            // …and no load leaks into the per-server/per-tool breakdowns.
            let text = serde_json::to_string(&s).unwrap();
            assert!(
                !text.contains("noisy") && !text.contains("skill_load"),
                "{text}"
            );
        });
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
