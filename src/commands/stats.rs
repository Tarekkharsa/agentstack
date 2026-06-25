//! `agentstack stats` — local usage analytics (PLAN §9g): per capability, how
//! many times it's been activated and in how many (target, scope) places it's
//! currently live. Activation counts come from `usage.json`; the footprint from
//! `state.json`.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use owo_colors::OwoColorize;

use crate::state::State;
use crate::usage::Usage;

pub fn run(manifest_dir: Option<&Path>) -> Result<()> {
    let usage = Usage::load()?;
    let state = State::load()?;

    // Footprint: how many target/scope slots each capability is live in.
    let mut footprint: BTreeMap<String, usize> = BTreeMap::new();
    for ts in state.targets.values() {
        for name in ts.managed_servers.iter().chain(ts.managed_skills.iter()) {
            *footprint.entry(name.clone()).or_insert(0) += 1;
        }
    }

    // Union of all known capability names.
    let mut names: Vec<String> = usage
        .activations
        .keys()
        .chain(footprint.keys())
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

    // Sort by activation count, descending.
    names.sort_by(|a, b| usage.count(b).cmp(&usage.count(a)).then(a.cmp(b)));

    println!(
        "{:<24} {:>12}  {}",
        "capability".bold(),
        "activations".bold(),
        "live in".bold()
    );
    for name in &names {
        let count = usage.count(name);
        let live = footprint.get(name).copied().unwrap_or(0);
        let bar = "▮".repeat((count.min(20)) as usize);
        println!(
            "{:<24} {:>12}  {} {}",
            name,
            count,
            format!("{live} slot(s)").dimmed(),
            bar.cyan()
        );
    }
    Ok(())
}
