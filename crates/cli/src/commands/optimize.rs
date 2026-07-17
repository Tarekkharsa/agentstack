//! `agentstack optimize` — turn the signals agentstack already collects
//! (activation counts, the gateway call audit log, per-server context cost,
//! the trust ledger, managed-state) into concrete recommendations.
//!
//! Read-only by default; `--json` for machines; `--write` applies only the
//! recommendations marked safe — provably-dead manifest entries and
//! trust-ledger hygiene, never anything that changes live behavior.
//!
//! Honesty rules, enforced in the shape of [`Recommendation`]:
//! - every recommendation carries its **evidence** (numbers + window + source),
//! - the **exact command or TOML** to act on it, and
//! - **why it is safe** to auto-apply or why it needs a human.
//!
//! Visibility limit, stated wherever it applies: the audit log records only
//! gateway-brokered calls (`agentstack mcp` / code mode). A server rendered
//! into a native config is called directly by the harness and leaves no trace
//! here — so "0 gateway calls" alone never justifies an automatic removal; a
//! server must also be absent from every rendered config (state) and every
//! profile before `--write` may touch it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result};
use owo_colors::OwoColorize;
use serde::Serialize;
use serde_json::{json, Value};

use crate::calllog::CallRecord;
use crate::cli::OptimizeArgs;
use crate::footprint::{fmt_age, fmt_tokens, Footprints};
use crate::manifest::Manifest;
use crate::usage::Usage;

/// Minimum days of audit-log history before "never called" counts as evidence
/// rather than "we only just started watching".
const MIN_HISTORY_DAYS: u64 = 14;

#[derive(Debug, Serialize)]
pub struct Recommendation {
    /// Stable machine id: `unused-server`, `firewall-narrow`, `denied-calls`,
    /// `error-noisy`, `stale-trust`, `unused-skill`, `measure`.
    pub kind: &'static str,
    /// The capability / path the recommendation is about.
    pub target: String,
    /// `high` / `medium` / `low`.
    pub impact: &'static str,
    pub title: String,
    /// Concrete numbers, each naming its data source and window.
    pub evidence: Vec<String>,
    /// The exact command to run or TOML to add.
    pub action: String,
    /// Whether `--write` may apply this without a human.
    pub safe_auto: bool,
    /// Why it's safe — or exactly what a human must weigh.
    pub safety: String,
}

/// Everything `analyze` reads, gathered once so the analysis itself is pure
/// and unit-testable.
pub struct Inputs<'a> {
    pub manifest: &'a Manifest,
    pub usage: &'a Usage,
    pub footprints: &'a Footprints,
    /// Audit-log records, already filtered to the requested window.
    pub calls: &'a [CallRecord],
    /// Server names currently rendered into ANY target config (from state).
    pub managed_anywhere: BTreeSet<String>,
    /// Trust-ledger rows: (path, still exists on disk, current trust state).
    pub trust: Vec<(String, bool, crate::trust::TrustState)>,
    pub now: u64,
}

/// Per-server aggregation of the call log.
#[derive(Default)]
struct CallStats {
    total: u64,
    denied: u64,
    errors: u64,
    /// tool → call count (any outcome).
    tools: BTreeMap<String, u64>,
    /// tool → successful call count. Firewall proposals draw from THIS map
    /// only — a tool that was only ever denied must never end up in a
    /// recommended allowlist (that would loosen policy, not narrow it).
    ok_tools: BTreeMap<String, u64>,
    /// tool → most recent denial detail (the policy rule).
    denied_tools: BTreeMap<String, (u64, String)>,
}

/// Every on-disk signal `analyze` reads, owned so the borrow in [`Inputs`] is
/// trivial to construct. Loaded once and shared by the CLI `run`, `--json`, and
/// the dashboard snapshot so the gathering logic lives in exactly one place.
struct Signals {
    usage: Usage,
    footprints: Footprints,
    calls: Vec<CallRecord>,
    managed_anywhere: BTreeSet<String>,
    trust: Vec<(String, bool, crate::trust::TrustState)>,
    now: u64,
}

impl Signals {
    /// Load and window-filter every signal. `since` mirrors `--since` (days);
    /// `None` keeps the whole audit log. Every source is best-effort.
    fn load(since: Option<u64>) -> Self {
        let usage = Usage::load().unwrap_or_default();
        let footprints = Footprints::load().unwrap_or_default();
        let now = crate::calllog::now_epoch();

        let mut calls = crate::calllog::read_all();
        if let Some(days) = since {
            let cutoff = now.saturating_sub(days * 86_400);
            calls.retain(|c| c.ts >= cutoff);
        }

        let state = crate::state::State::load().unwrap_or_default();
        let managed_anywhere: BTreeSet<String> = state
            .targets
            .values()
            .flat_map(|t| t.managed_servers.iter().cloned())
            .collect();

        let trust: Vec<(String, bool, crate::trust::TrustState)> = crate::trust::TrustStore::load()
            .trusted
            .keys()
            .map(|p| {
                let path = Path::new(p);
                (p.clone(), path.exists(), crate::trust::check(path))
            })
            .collect();

        Signals {
            usage,
            footprints,
            calls,
            managed_anywhere,
            trust,
            now,
        }
    }

    /// Borrow these signals as the pure [`Inputs`] `analyze` consumes.
    fn inputs<'a>(&'a self, manifest: &'a Manifest) -> Inputs<'a> {
        Inputs {
            manifest,
            usage: &self.usage,
            footprints: &self.footprints,
            calls: &self.calls,
            managed_anywhere: self.managed_anywhere.clone(),
            trust: self.trust.clone(),
            now: self.now,
        }
    }
}

/// Assemble the machine-readable report (project + window + recommendations)
/// from gathered inputs. Pure over `inputs` so `--json`, the dashboard, and the
/// unit tests all produce the identical shape.
fn report_json(project: &str, inputs: &Inputs, since: Option<u64>) -> Value {
    json!({
        "project": project,
        "windowDays": span_days(inputs.calls, inputs.now),
        "gatewayCalls": inputs.calls.len(),
        "sinceDays": since,
        "recommendations": analyze(inputs),
    })
}

/// The optimize report as JSON — the shape the dashboard's Insights panel
/// embeds. Resilient: a manifest that won't load degrades to an empty report
/// rather than failing the whole snapshot.
pub fn collect(manifest_dir: Option<&Path>) -> Value {
    let Ok(ctx) = super::load(manifest_dir) else {
        return json!({
            "project": Value::Null,
            "windowDays": 0,
            "gatewayCalls": 0,
            "sinceDays": Value::Null,
            "recommendations": [],
        });
    };
    let signals = Signals::load(None);
    let inputs = signals.inputs(&ctx.loaded.manifest);
    report_json(&ctx.dir.display().to_string(), &inputs, None)
}

pub fn run(args: &OptimizeArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let ctx = super::load(manifest_dir)?;
    let signals = Signals::load(args.since);
    let inputs = signals.inputs(&ctx.loaded.manifest);

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report_json(
                &ctx.dir.display().to_string(),
                &inputs,
                args.since
            ))?
        );
        return Ok(());
    }

    let recs = analyze(&inputs);
    let span = span_days(&signals.calls, signals.now);
    print_report(&ctx, &recs, &signals.calls, span);

    if args.write {
        apply_safe(&ctx, &recs)?;
    } else if recs.iter().any(|r| r.safe_auto) {
        println!(
            "\nRe-run with {} to apply the {} recommendation(s) marked safe.",
            "--write".bold(),
            recs.iter().filter(|r| r.safe_auto).count()
        );
    }
    Ok(())
}

/// Days between the oldest record in the (filtered) log and now.
fn span_days(calls: &[CallRecord], now: u64) -> u64 {
    calls
        .iter()
        .map(|c| c.ts)
        .min()
        .map(|oldest| now.saturating_sub(oldest) / 86_400)
        .unwrap_or(0)
}

pub fn analyze(inp: &Inputs) -> Vec<Recommendation> {
    let mut recs = Vec::new();
    let span = span_days(inp.calls, inp.now);
    let enough_history = span >= MIN_HISTORY_DAYS;

    // Aggregate the call log per server once.
    let mut stats: BTreeMap<String, CallStats> = BTreeMap::new();
    for c in inp.calls {
        let s = stats.entry(c.server.clone()).or_default();
        s.total += 1;
        *s.tools.entry(c.tool.clone()).or_insert(0) += 1;
        match c.outcome.as_str() {
            "denied" => {
                s.denied += 1;
                let e = s
                    .denied_tools
                    .entry(c.tool.clone())
                    .or_insert((0, String::new()));
                e.0 += 1;
                if let Some(d) = &c.detail {
                    e.1 = d.clone();
                }
            }
            "error" => s.errors += 1,
            _ => {
                *s.ok_tools.entry(c.tool.clone()).or_insert(0) += 1;
            }
        }
    }

    // Which profiles reference each server.
    let mut in_profiles: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (pname, p) in &inp.manifest.profiles {
        for s in &p.servers {
            in_profiles.entry(s.as_str()).or_default().push(pname);
        }
    }

    let window_note = if inp.calls.is_empty() {
        "audit log is empty".to_string()
    } else {
        format!(
            "{} gateway call(s) over {span}d in the audit log",
            inp.calls.len()
        )
    };

    for (name, _server) in &inp.manifest.servers {
        let s = stats.get(name.as_str());
        let calls_n = s.map(|s| s.total).unwrap_or(0);
        let activations = inp.usage.count(name);
        let managed = inp.managed_anywhere.contains(name);
        let profiles = in_profiles.get(name.as_str()).cloned().unwrap_or_default();
        let fp = inp.footprints.get(name);

        let mut evidence = vec![
            format!("{calls_n} gateway call(s) for this server ({window_note})"),
            format!("{activations} activation(s) — times agentstack rendered it into a config (usage.json)"),
        ];
        if let Some(f) = fp {
            evidence.push(format!(
                "context cost ~{} per session across {} tool(s) ({}) (footprint.json)",
                fmt_tokens(f.est_tokens),
                f.tools,
                fmt_age(f.measured_at)
            ));
        }
        if managed {
            evidence.push(
                "currently rendered into at least one native config (state.json) — direct harness calls there are invisible to the audit log".into(),
            );
        }
        if !profiles.is_empty() {
            evidence.push(format!("referenced by profile(s): {}", profiles.join(", ")));
        }

        // Unused server: no gateway calls AND never rendered by agentstack.
        if calls_n == 0 && activations == 0 {
            let safe = !managed && profiles.is_empty() && enough_history;
            let impact = match fp.map(|f| f.est_tokens) {
                Some(t) if t >= 1500 => "high",
                Some(t) if t >= 500 => "medium",
                _ => {
                    if managed {
                        "medium"
                    } else {
                        "low"
                    }
                }
            };
            recs.push(Recommendation {
                kind: "unused-server",
                target: name.clone(),
                impact,
                title: format!("'{name}' is declared but shows no use at all"),
                evidence: evidence.clone(),
                action: format!("agentstack remove {name} --write"),
                safe_auto: safe,
                safety: if safe {
                    format!(
                        "safe: not rendered in any config, in no profile, never activated, 0 gateway calls over {span}d — provably inert; removal is a commit-safe manifest edit"
                    )
                } else if !enough_history {
                    format!(
                        "manual: only {span}d of audit history (need {MIN_HISTORY_DAYS}d) — too early to call it unused"
                    )
                } else if managed {
                    "manual: it is live in a native config, where the harness calls it directly — the audit log can't see those calls".into()
                } else {
                    format!(
                        "manual: profile(s) {} reference it — removing it changes what those profiles load",
                        profiles.join(", ")
                    )
                },
            });
            continue;
        }

        // Rendered natively, zero gateway calls, real context cost: worth a
        // human look, but never auto (native calls are invisible to us).
        if calls_n == 0
            && activations > 0
            && enough_history
            && fp.map(|f| f.est_tokens >= 500).unwrap_or(false)
        {
            recs.push(Recommendation {
                kind: "unused-server",
                target: name.clone(),
                impact: if fp.map(|f| f.est_tokens).unwrap_or(0) >= 1500 {
                    "high"
                } else {
                    "medium"
                },
                title: format!("'{name}' costs context every session but shows no gateway use"),
                evidence: evidence.clone(),
                action: format!("agentstack remove {name} --write"),
                safety: "manual: it has been rendered into native configs — the harness may call it directly there, which this log cannot see. Remove only if you know you don't use it.".into(),
                safe_auto: false,
            });
        }

        // Firewall narrowing: many tools exposed, few successfully used.
        if let (Some(f), Some(s)) = (fp, s) {
            let distinct = s.ok_tools.len();
            let already_ruled = inp.manifest.policy.tools.contains_key(name);
            if f.tools >= 10
                && s.total >= 10
                && distinct >= 1
                && distinct <= (f.tools / 5).max(3)
                && enough_history
                && !already_ruled
            {
                let used: Vec<String> = s.ok_tools.keys().cloned().collect();
                let mut ev = vec![
                    format!(
                        "exposes {} tool(s) costing ~{} per session (footprint.json, {})",
                        f.tools,
                        fmt_tokens(f.est_tokens),
                        fmt_age(f.measured_at)
                    ),
                    format!(
                        "only {distinct} distinct tool(s) called successfully over {span}d: {}",
                        s.ok_tools
                            .iter()
                            .map(|(t, n)| format!("{t} ×{n}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ];
                if inp.managed_anywhere.contains(name) {
                    ev.push(
                        "also rendered natively — direct calls there are invisible to this count"
                            .into(),
                    );
                }
                recs.push(Recommendation {
                    kind: "firewall-narrow",
                    target: name.clone(),
                    impact: if f.est_tokens >= 2000 { "high" } else { "medium" },
                    title: format!(
                        "'{name}': allowlist the {distinct} tools you actually use"
                    ),
                    evidence: ev,
                    action: format!(
                        "add to agentstack.toml:\n[policy.tools]\n{name} = [{}]",
                        used.iter()
                            .map(|t| format!("\"{t}\""))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    safe_auto: false,
                    safety: "manual: an allowlist makes every other tool invisible to agents at the gateway — future workflows may legitimately need more. Denied tools show up in `agentstack report calls` if you cut too deep.".into(),
                });
            }
        }
    }

    // Denied calls: the firewall firing is information either way.
    for (server, s) in &stats {
        if s.denied == 0 {
            continue;
        }
        let tools: Vec<String> = s
            .denied_tools
            .iter()
            .map(|(t, (n, rule))| format!("{t} ×{n} (rule: {rule})"))
            .collect();
        recs.push(Recommendation {
            kind: "denied-calls",
            target: server.clone(),
            impact: "medium",
            title: format!("'{server}': {} call(s) denied by the tool firewall", s.denied),
            evidence: vec![
                format!("denied over {span}d: {}", tools.join("; ")),
                format!("({} total call(s) to this server in the window)", s.total),
            ],
            action: format!(
                "review `[policy.tools]` for '{server}' (agentstack report calls for the full log) — loosen the rule if these were legitimate, keep it if not"
            ),
            safe_auto: false,
            safety: "manual: a denial is the firewall working as configured — only a human knows whether the agent should have been allowed".into(),
        });
    }

    // Error-noisy servers: mostly failing at runtime.
    for (server, s) in &stats {
        if s.total >= 5 && s.errors * 2 >= s.total {
            recs.push(Recommendation {
                kind: "error-noisy",
                target: server.clone(),
                impact: "medium",
                title: format!(
                    "'{server}': {}/{} call(s) errored — likely misconfigured",
                    s.errors, s.total
                ),
                evidence: vec![format!(
                    "{} error(s) out of {} call(s) over {span}d (audit log)",
                    s.errors, s.total
                )],
                action: format!(
                    "agentstack explain {server}   # check its secrets, then: agentstack doctor --live"
                ),
                safe_auto: false,
                safety: "manual: the fix depends on the failure (secret, URL, upstream outage) — diagnose before changing anything".into(),
            });
        }
    }

    // Trust-ledger hygiene.
    for (path, exists, state) in &inp.trust {
        if !exists {
            recs.push(Recommendation {
                kind: "stale-trust",
                target: path.clone(),
                impact: "low",
                title: format!("trusted project no longer exists: {path}"),
                evidence: vec!["directory not found on disk; its trust grant is dead weight (trust.json)".into()],
                action: format!("agentstack trust --revoke {path}"),
                safe_auto: true,
                safety: "safe: revoking trust for a nonexistent path only tightens — nothing can lose access".into(),
            });
        } else if *state == crate::trust::TrustState::Changed {
            recs.push(Recommendation {
                kind: "stale-trust",
                target: path.clone(),
                impact: "medium",
                title: format!("manifest changed since trusted: {path}"),
                evidence: vec![
                    "content digest no longer matches the trust grant — the bridge already dropped it to control-plane only (trust.json)".into(),
                ],
                action: format!("review the manifest, then: agentstack trust {path}"),
                safe_auto: false,
                safety: "manual: re-trusting is exactly the review the gate exists to force".into(),
            });
        }
    }

    // Skills that agentstack never materialized anywhere. With no profiles
    // declared every inline skill is the implicit default — always referenced.
    for (name, _) in &inp.manifest.skills {
        let referenced = inp.manifest.profiles.is_empty()
            || inp
                .manifest
                .profiles
                .values()
                .any(|p| p.loads_all_skills() || p.skills.iter().any(|s| s == name));
        if inp.usage.count(name) == 0 && !referenced && enough_history {
            recs.push(Recommendation {
                kind: "unused-skill",
                target: name.clone(),
                impact: "low",
                title: format!("skill '{name}' was never materialized and no profile loads it"),
                evidence: vec![
                    format!("0 activation(s) since tracking began (usage.json, {span}d of runtime history)"),
                    "in no profile's skill list".into(),
                ],
                action: format!("agentstack remove {name} --write   # or keep it in the central library only"),
                safe_auto: false,
                safety: "manual: skill invocations inside a harness aren't logged — activation count only proves agentstack never rendered it, not that you never used a copy".into(),
            });
        }
    }

    // No cost data at all → everything above is running half-blind.
    let unmeasured = inp
        .manifest
        .servers
        .keys()
        .filter(|n| inp.footprints.get(n).is_none())
        .count();
    if unmeasured > 0 && unmeasured * 2 > inp.manifest.servers.len() {
        recs.push(Recommendation {
            kind: "measure",
            target: "footprints".into(),
            impact: "low",
            title: format!(
                "{unmeasured}/{} server(s) have no measured context cost",
                inp.manifest.servers.len()
            ),
            evidence: vec!["cost-based recommendations above are incomplete without it (footprint.json)".into()],
            action: "agentstack report usage --live".into(),
            safe_auto: false,
            safety: "manual only because it spawns/contacts the manifest's servers once to measure them; the measurement itself is read-only".into(),
        });
    }

    let rank = |r: &Recommendation| match r.impact {
        "high" => 0,
        "medium" => 1,
        _ => 2,
    };
    recs.sort_by(|a, b| {
        rank(a)
            .cmp(&rank(b))
            .then(a.kind.cmp(b.kind))
            .then(a.target.cmp(&b.target))
    });
    recs
}

fn print_report(ctx: &super::Context, recs: &[Recommendation], calls: &[CallRecord], span: u64) {
    println!("{} — {}", "Optimize".bold(), ctx.dir.display());
    println!(
        "Data: {} gateway call(s) over {span}d · activations since first apply · context costs from `stats --live`\n",
        calls.len()
    );
    if calls.is_empty() {
        println!(
            "{} the audit log is empty — recommendations are limited to static signals. Use the gateway (zero-files bridge or `agentstack run`) to collect runtime evidence.\n",
            "⚠".yellow()
        );
    }
    if recs.is_empty() {
        println!(
            "{} nothing to recommend — the stack looks lean.",
            "✓".green()
        );
        return;
    }
    for r in recs {
        let tag = match r.impact {
            "high" => "HIGH".red().to_string(),
            "medium" => "MED ".yellow().to_string(),
            _ => "LOW ".dimmed().to_string(),
        };
        println!("{tag} {} · {}", r.kind.bold(), r.title);
        for e in &r.evidence {
            println!("      - {e}");
        }
        // Multi-line actions (TOML snippets) indent under the label.
        let mut lines = r.action.lines();
        println!("      {} {}", "action:".bold(), lines.next().unwrap_or(""));
        for l in lines {
            println!("              {l}");
        }
        let mark = if r.safe_auto {
            "safe with --write".green().to_string()
        } else {
            "needs review".yellow().to_string()
        };
        println!("      {mark} — {}\n", r.safety);
    }
    let safe = recs.iter().filter(|r| r.safe_auto).count();
    println!(
        "{} recommendation(s): {safe} safe to auto-apply, {} need review.",
        recs.len(),
        recs.len() - safe
    );
}

/// Apply only the `safe_auto` recommendations: dead manifest entries are
/// removed (one atomic write, diff shown), dead trust grants revoked.
fn apply_safe(ctx: &super::Context, recs: &[Recommendation]) -> Result<()> {
    let safe: Vec<&Recommendation> = recs.iter().filter(|r| r.safe_auto).collect();
    if safe.is_empty() {
        println!("\nNothing marked safe to auto-apply.");
        return Ok(());
    }
    println!();

    let removals: Vec<&str> = safe
        .iter()
        .filter(|r| r.kind == "unused-server")
        .map(|r| r.target.as_str())
        .collect();
    if !removals.is_empty() {
        let original = std::fs::read_to_string(&ctx.loaded.manifest_path)
            .with_context(|| format!("reading {}", ctx.loaded.manifest_path.display()))?;
        let mut text = original.clone();
        for name in &removals {
            text = super::remove::remove_entry(&text, "servers", name)?;
        }
        print!(
            "{}",
            crate::util::diff::render(&original, &text)
                .lines()
                .map(|l| format!("  {l}\n"))
                .collect::<String>()
        );
        crate::util::atomic::write(&ctx.loaded.manifest_path, &text)
            .with_context(|| format!("writing {}", ctx.loaded.manifest_path.display()))?;
        println!(
            "{} removed {} inert server(s): {}",
            "✓".green(),
            removals.len(),
            removals.join(", ")
        );
    }

    for r in safe.iter().filter(|r| r.kind == "stale-trust") {
        if crate::trust::revoke(Path::new(&r.target))? {
            println!("{} revoked dead trust grant: {}", "✓".green(), r.target);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::footprint::ServerFootprint;

    fn manifest(toml_str: &str) -> Manifest {
        toml::from_str(toml_str).unwrap()
    }

    fn call(server: &str, tool: &str, outcome: &str, ts: u64) -> CallRecord {
        CallRecord {
            ts,
            run: None,
            pid: 1,
            project: None,
            server: server.into(),
            tool: tool.into(),
            args_digest: "x".into(),
            outcome: match outcome {
                "denied" => agentstack_recorder::CallOutcome::Denied,
                "error" => agentstack_recorder::CallOutcome::Error,
                _ => agentstack_recorder::CallOutcome::Ok,
            },
            detail: if outcome == "denied" {
                Some("!*".into())
            } else {
                None
            },
            ms: 1,
        }
    }

    const NOW: u64 = 100 * 86_400;
    /// A call old enough to give the log ≥ MIN_HISTORY_DAYS of span.
    fn old_call() -> CallRecord {
        call("anchor", "ping", "ok", NOW - 30 * 86_400)
    }

    fn base_inputs<'a>(
        m: &'a Manifest,
        usage: &'a Usage,
        fps: &'a Footprints,
        calls: &'a [CallRecord],
    ) -> Inputs<'a> {
        Inputs {
            manifest: m,
            usage,
            footprints: fps,
            calls,
            managed_anywhere: BTreeSet::new(),
            trust: Vec::new(),
            now: NOW,
        }
    }

    #[test]
    fn inert_server_is_safe_only_when_provably_dead() {
        let m = manifest(
            "version = 1\n[servers.dead]\ntype = \"http\"\nurl = \"https://x\"\n\
             [servers.inprofile]\ntype = \"http\"\nurl = \"https://y\"\n\
             [profiles.p]\nservers = [\"inprofile\"]\n",
        );
        let usage = Usage::default();
        let fps = Footprints::default();
        let calls = vec![old_call()];
        let recs = analyze(&base_inputs(&m, &usage, &fps, &calls));

        let dead = recs
            .iter()
            .find(|r| r.kind == "unused-server" && r.target == "dead")
            .expect("dead server flagged");
        assert!(dead.safe_auto, "not managed, no profile, history ok → safe");
        assert!(dead.safety.contains("provably inert"));

        let inp = recs
            .iter()
            .find(|r| r.kind == "unused-server" && r.target == "inprofile")
            .expect("profile-referenced server flagged");
        assert!(!inp.safe_auto, "profile membership blocks auto-removal");
        assert!(inp.safety.contains("profile"));
    }

    #[test]
    fn short_history_blocks_auto_removal() {
        let m = manifest("version = 1\n[servers.new]\ntype = \"http\"\nurl = \"https://x\"\n");
        let usage = Usage::default();
        let fps = Footprints::default();
        let calls = vec![call("anchor", "ping", "ok", NOW - 2 * 86_400)]; // 2d span
        let recs = analyze(&base_inputs(&m, &usage, &fps, &calls));
        let r = recs.iter().find(|r| r.target == "new").unwrap();
        assert!(!r.safe_auto);
        assert!(r.safety.contains("too early"));
    }

    #[test]
    fn natively_rendered_server_never_auto_removes_and_states_visibility_limit() {
        let m = manifest("version = 1\n[servers.figma]\ntype = \"http\"\nurl = \"https://x\"\n");
        let mut usage = Usage::default();
        usage.activations.insert("figma".into(), 4);
        let mut fps = Footprints::default();
        fps.servers.insert(
            "figma".into(),
            ServerFootprint {
                tools: 20,
                est_tokens: 3000,
                measured_at: NOW,
            },
        );
        let calls = vec![old_call()];
        let mut inputs = base_inputs(&m, &usage, &fps, &calls);
        inputs.managed_anywhere.insert("figma".into());
        let recs = analyze(&inputs);
        let r = recs
            .iter()
            .find(|r| r.kind == "unused-server" && r.target == "figma")
            .expect("high-cost, gateway-silent server flagged");
        assert_eq!(r.impact, "high");
        assert!(!r.safe_auto);
        assert!(r.safety.contains("native configs"));
        assert!(r.evidence.iter().any(|e| e.contains("invisible")));
    }

    #[test]
    fn firewall_narrow_proposes_exact_allowlist_and_respects_existing_rules() {
        let m = manifest("version = 1\n[servers.big]\ntype = \"http\"\nurl = \"https://x\"\n");
        let usage = Usage::default();
        let mut fps = Footprints::default();
        fps.servers.insert(
            "big".into(),
            ServerFootprint {
                tools: 40,
                est_tokens: 5000,
                measured_at: NOW,
            },
        );
        let mut calls = vec![old_call()];
        for i in 0..12 {
            calls.push(call(
                "big",
                if i % 2 == 0 { "search" } else { "get" },
                "ok",
                NOW - 86_400,
            ));
        }
        // A denied-only tool must NOT make it into the proposed allowlist —
        // that would loosen policy, not narrow it.
        calls.push(call("big", "delete_all", "denied", NOW - 86_400));
        let recs = analyze(&base_inputs(&m, &usage, &fps, &calls));
        let r = recs
            .iter()
            .find(|r| r.kind == "firewall-narrow")
            .expect("narrowing proposed");
        assert!(r.action.contains("[policy.tools]"));
        assert!(r.action.contains("big = [\"get\", \"search\"]"));
        assert!(!r.action.contains("delete_all"));
        assert!(!r.safe_auto);

        // With a rule already present, don't second-guess it.
        let m2 = manifest(
            "version = 1\n[servers.big]\ntype = \"http\"\nurl = \"https://x\"\n\
             [policy.tools]\nbig = [\"search\"]\n",
        );
        let recs2 = analyze(&base_inputs(&m2, &usage, &fps, &calls));
        assert!(!recs2.iter().any(|r| r.kind == "firewall-narrow"));
    }

    #[test]
    fn denied_and_error_calls_surface_with_evidence() {
        let m = manifest("version = 1\n[servers.s]\ntype = \"http\"\nurl = \"https://x\"\n");
        let usage = Usage::default();
        let fps = Footprints::default();
        let mut calls = vec![old_call()];
        for _ in 0..3 {
            calls.push(call("s", "rm_rf", "denied", NOW - 86_400));
        }
        for _ in 0..4 {
            calls.push(call("s", "flaky", "error", NOW - 86_400));
        }
        calls.push(call("s", "ok_tool", "ok", NOW - 86_400));
        let recs = analyze(&base_inputs(&m, &usage, &fps, &calls));

        let denied = recs.iter().find(|r| r.kind == "denied-calls").unwrap();
        assert!(denied.evidence.iter().any(|e| e.contains("rm_rf ×3")));
        assert!(!denied.safe_auto);

        let noisy = recs.iter().find(|r| r.kind == "error-noisy").unwrap();
        assert!(noisy.title.contains("4/8"));
    }

    #[test]
    fn trust_hygiene_dead_path_is_safe_changed_needs_review() {
        let m = manifest("version = 1\n");
        let usage = Usage::default();
        let fps = Footprints::default();
        let calls = vec![];
        let mut inputs = base_inputs(&m, &usage, &fps, &calls);
        inputs.trust = vec![
            (
                "/gone/project".into(),
                false,
                crate::trust::TrustState::Changed,
            ),
            (
                "/live/project".into(),
                true,
                crate::trust::TrustState::Changed,
            ),
        ];
        let recs = analyze(&inputs);
        let dead = recs.iter().find(|r| r.target == "/gone/project").unwrap();
        assert!(dead.safe_auto, "revoking a dead path only tightens");
        assert!(dead.action.contains("--revoke"));
        let changed = recs.iter().find(|r| r.target == "/live/project").unwrap();
        assert!(!changed.safe_auto);
        assert!(changed.action.contains("agentstack trust /live/project"));
    }

    #[test]
    fn report_json_wraps_recs_with_window_and_call_count() {
        let m = manifest("version = 1\n[servers.dead]\ntype = \"http\"\nurl = \"https://x\"\n");
        let usage = Usage::default();
        let fps = Footprints::default();
        let calls = vec![old_call()];
        let inputs = base_inputs(&m, &usage, &fps, &calls);

        let v = report_json("/proj", &inputs, Some(30));
        assert_eq!(v["project"], "/proj");
        assert_eq!(v["gatewayCalls"], 1);
        assert_eq!(v["sinceDays"], 30);
        assert!(v["windowDays"].as_u64().is_some());
        let recs = v["recommendations"].as_array().expect("recs array");
        assert!(
            recs.iter()
                .any(|r| r["kind"] == "unused-server" && r["target"] == "dead"),
            "the dead server recommendation is carried into the JSON report"
        );
    }

    #[test]
    fn every_recommendation_has_evidence_action_and_safety() {
        // The acceptance bar, enforced as a test: no rec ships without all three.
        let m = manifest(
            "version = 1\n[servers.dead]\ntype = \"http\"\nurl = \"https://x\"\n\
             [skills.ghost]\npath = \"./skills/ghost\"\n",
        );
        let usage = Usage::default();
        let fps = Footprints::default();
        let calls = vec![old_call()];
        let mut inputs = base_inputs(&m, &usage, &fps, &calls);
        inputs.trust = vec![("/gone".into(), false, crate::trust::TrustState::Changed)];
        let recs = analyze(&inputs);
        assert!(recs.len() >= 3);
        for r in &recs {
            assert!(!r.evidence.is_empty(), "{}: evidence missing", r.kind);
            assert!(!r.action.is_empty(), "{}: action missing", r.kind);
            assert!(!r.safety.is_empty(), "{}: safety rationale missing", r.kind);
        }
    }
}
