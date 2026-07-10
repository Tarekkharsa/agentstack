//! `agentstack stats` — local usage analytics (PLAN §9g): per capability, how
//! many times it's been activated, in how many (target, scope) places it's
//! currently live, and — for servers — what it costs in context-window tokens
//! per session. Activation counts come from `usage.json`; the footprint from
//! `state.json`; context cost from `footprint.json` (measure with `--live`).

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use owo_colors::OwoColorize;
use serde_json::{json, Value};

use crate::cli::StatsArgs;
use crate::footprint::{fmt_age, fmt_tokens, Footprints};
use crate::manifest::Manifest;
use crate::state::State;
use crate::usage::Usage;

/// A server this costly (in estimated tokens) that has never been activated
/// gets called out as dead weight worth removing.
const DEAD_WEIGHT_TOKENS: u64 = 2_000;

pub fn run(args: &StatsArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let usage = Usage::load()?;
    let state = State::load()?;

    let mut footprints = Footprints::load().unwrap_or_default();
    if args.live {
        // One live discovery pass through the gateway (HTTP + stdio alike),
        // then persist so every later stats/explain/dashboard read is offline.
        let gw = crate::gateway::Gateway::from_manifest(manifest_dir);
        let measured = crate::footprint::measure(&gw.namespaced_tools());
        if measured.is_empty() {
            println!("{}", "No servers answered a live tools/list.".dimmed());
        }
        footprints.servers.extend(measured);
        footprints.save()?;
    }

    // Constrain to the current manifest's capabilities when one is loadable.
    let ctx = super::load(manifest_dir).ok();
    let manifest = ctx.as_ref().map(|c| &c.loaded.manifest);
    let report = build_report(&usage, &state, &footprints, manifest);
    print_human(&report);
    Ok(())
}

/// The stats report as JSON — the same numbers `run` prints, in the shape the
/// dashboard's Insights panel embeds. Best-effort: every source degrades to
/// empty rather than failing the snapshot.
pub fn collect(manifest_dir: Option<&Path>) -> Value {
    let usage = Usage::load().unwrap_or_default();
    let state = State::load().unwrap_or_default();
    let footprints = Footprints::load().unwrap_or_default();
    let ctx = super::load(manifest_dir).ok();
    let manifest = ctx.as_ref().map(|c| &c.loaded.manifest);
    build_report(&usage, &state, &footprints, manifest)
}

/// Pure core: fold usage counts, live slots, and measured footprints into one
/// ranked per-capability report. Shared by the CLI renderer and the dashboard,
/// so the two can never drift.
fn build_report(
    usage: &Usage,
    state: &State,
    footprints: &Footprints,
    manifest: Option<&Manifest>,
) -> Value {
    // Footprint: how many target/scope slots each capability is live in.
    let mut live_slots: BTreeMap<String, usize> = BTreeMap::new();
    for ts in state.targets.values() {
        for name in ts.managed_servers.iter().chain(ts.managed_skills.iter()) {
            *live_slots.entry(name.clone()).or_insert(0) += 1;
        }
    }

    // Union of all known capability names.
    let mut names: Vec<String> = usage
        .activations
        .keys()
        .chain(live_slots.keys())
        .cloned()
        .collect();
    names.sort();
    names.dedup();

    // Constrain to the current manifest's capabilities when one is loadable.
    if let Some(m) = manifest {
        names.retain(|n| m.servers.contains_key(n) || m.skills.contains_key(n));
        for n in m.servers.keys().chain(m.skills.keys()) {
            if !names.contains(n) {
                names.push(n.clone());
            }
        }
        names.sort();
    }

    // Sort by activation count, descending; context cost breaks ties so the
    // most expensive idle capability floats to the top of its count band.
    names.sort_by(|a, b| {
        let cost = |n: &str| footprints.get(n).map(|f| f.est_tokens).unwrap_or(0);
        usage
            .count(b)
            .cmp(&usage.count(a))
            .then(cost(b).cmp(&cost(a)))
            .then(a.cmp(b))
    });

    let mut any_measured = false;
    let capabilities: Vec<Value> = names
        .iter()
        .map(|name| {
            let count = usage.count(name);
            let live = live_slots.get(name).copied().unwrap_or(0);
            let fp = footprints.get(name);
            if fp.is_some() {
                any_measured = true;
            }
            let dead_weight = fp
                .map(|f| f.est_tokens >= DEAD_WEIGHT_TOKENS && count == 0)
                .unwrap_or(false);
            json!({
                "name": name,
                "activations": count,
                "liveSlots": live,
                "estTokens": fp.map(|f| f.est_tokens),
                "tools": fp.map(|f| f.tools),
                "costLabel": fp.map(|f| fmt_tokens(f.est_tokens)),
                "measuredAt": fp.map(|f| f.measured_at),
                "deadWeight": dead_weight,
            })
        })
        .collect();

    json!({ "capabilities": capabilities, "anyMeasured": any_measured })
}

fn print_human(report: &Value) {
    let caps = report["capabilities"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if caps.is_empty() {
        println!("No usage recorded yet. Run `apply --write` or `use … --write` first.");
        return;
    }

    println!(
        "{:<24} {:>12}  {:>18}  {}",
        "capability".bold(),
        "activations".bold(),
        "context cost".bold(),
        "live in".bold()
    );
    for c in &caps {
        let name = c["name"].as_str().unwrap_or("?");
        let count = c["activations"].as_u64().unwrap_or(0);
        let live = c["liveSlots"].as_u64().unwrap_or(0);
        let cost = match c["costLabel"].as_str() {
            Some(label) => format!("{} ({} tools)", label, c["tools"].as_u64().unwrap_or(0)),
            None => "—".to_string(),
        };
        let bar = "▮".repeat((count.min(20)) as usize);
        println!(
            "{:<24} {:>12}  {:>18}  {} {}",
            name,
            count,
            cost,
            format!("{live} slot(s)").dimmed(),
            bar.cyan()
        );
    }

    // Dead weight: a server taxing every session that nothing ever activates.
    let mut flagged = false;
    for c in &caps {
        if c["deadWeight"].as_bool().unwrap_or(false) {
            if !flagged {
                println!();
                flagged = true;
            }
            let name = c["name"].as_str().unwrap_or("?");
            println!(
                "{} '{name}' costs ~{} every session ({}) but has never been activated — consider `agentstack remove {name}`",
                "dead weight:".yellow().bold(),
                c["costLabel"].as_str().unwrap_or("?"),
                fmt_age(c["measuredAt"].as_u64().unwrap_or(0)),
            );
        }
    }
    if !report["anyMeasured"].as_bool().unwrap_or(false) {
        println!(
            "\n{}",
            "Context cost unmeasured — run `agentstack stats --live` to measure each server's tools/list token footprint.".dimmed()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::footprint::ServerFootprint;

    fn footprint(tools: usize, est_tokens: u64) -> ServerFootprint {
        ServerFootprint {
            tools,
            est_tokens,
            measured_at: 0,
        }
    }

    #[test]
    fn ranks_by_activations_then_flags_dead_weight() {
        let mut usage = Usage::default();
        usage.activations.insert("hot".into(), 5);
        usage.activations.insert("idle".into(), 0);
        let state = State::default();
        let mut fps = Footprints::default();
        // Costly + never activated → dead weight.
        fps.servers.insert("idle".into(), footprint(30, 5_000));
        fps.servers.insert("hot".into(), footprint(4, 300));

        let report = build_report(&usage, &state, &fps, None);
        let caps = report["capabilities"].as_array().unwrap();
        // Highest activation count leads.
        assert_eq!(caps[0]["name"], "hot");
        assert_eq!(caps[0]["activations"], 5);
        assert_eq!(caps[0]["deadWeight"], false);

        let idle = caps.iter().find(|c| c["name"] == "idle").unwrap();
        assert_eq!(idle["deadWeight"], true, "costly + never activated");
        assert_eq!(idle["estTokens"], 5_000);
        assert_eq!(report["anyMeasured"], true);
    }

    #[test]
    fn empty_when_nothing_recorded() {
        let report = build_report(
            &Usage::default(),
            &State::default(),
            &Footprints::default(),
            None,
        );
        assert_eq!(report["capabilities"].as_array().unwrap().len(), 0);
        assert_eq!(report["anyMeasured"], false);
    }
}
