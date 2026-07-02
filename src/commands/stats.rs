//! `agentstack stats` — local usage analytics (PLAN §9g): per capability, how
//! many times it's been activated, in how many (target, scope) places it's
//! currently live, and — for servers — what it costs in context-window tokens
//! per session. Activation counts come from `usage.json`; the footprint from
//! `state.json`; context cost from `footprint.json` (measure with `--live`).

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use owo_colors::OwoColorize;

use crate::cli::StatsArgs;
use crate::footprint::{fmt_age, fmt_tokens, Footprints};
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
    if let Ok(ctx) = super::load(manifest_dir) {
        let m = &ctx.loaded.manifest;
        names.retain(|n| m.servers.contains_key(n) || m.skills.contains_key(n));
        for n in m.servers.keys().chain(m.skills.keys()) {
            if !names.contains(n) {
                names.push(n.clone());
            }
        }
        names.sort();
    }

    if names.is_empty() {
        println!("No usage recorded yet. Run `apply --write` or `use … --write` first.");
        return Ok(());
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

    println!(
        "{:<24} {:>12}  {:>18}  {}",
        "capability".bold(),
        "activations".bold(),
        "context cost".bold(),
        "live in".bold()
    );
    let mut any_measured = false;
    for name in &names {
        let count = usage.count(name);
        let live = live_slots.get(name).copied().unwrap_or(0);
        let cost = match footprints.get(name) {
            Some(f) => {
                any_measured = true;
                format!("{} ({} tools)", fmt_tokens(f.est_tokens), f.tools)
            }
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
    for name in &names {
        if let Some(f) = footprints.get(name) {
            if f.est_tokens >= DEAD_WEIGHT_TOKENS && usage.count(name) == 0 {
                if !flagged {
                    println!();
                    flagged = true;
                }
                println!(
                    "{} '{name}' costs ~{} every session ({}) but has never been activated — consider `agentstack remove {name}`",
                    "dead weight:".yellow().bold(),
                    fmt_tokens(f.est_tokens),
                    fmt_age(f.measured_at),
                );
            }
        }
    }
    if !any_measured {
        println!(
            "\n{}",
            "Context cost unmeasured — run `agentstack stats --live` to measure each server's tools/list token footprint.".dimmed()
        );
    }
    Ok(())
}
