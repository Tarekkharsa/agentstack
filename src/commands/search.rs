//! `agentstack search <query>` — find capabilities in the catalog (registry v0),
//! marking ones already in the manifest and printing `add` suggestions. This is
//! the discovery half of the lifecycle (PLAN §9g) and a key surface for the
//! agent provisioning itself.

use std::path::Path;

use anyhow::Result;
use owo_colors::OwoColorize;

use crate::catalog;
use crate::cli::SearchArgs;

pub fn run(args: &SearchArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let query = args.query.clone().unwrap_or_default();
    let results = catalog::search(&query);

    // Best-effort: know which are already in the manifest.
    let installed = super::load(manifest_dir)
        .ok()
        .map(|ctx| {
            ctx.loaded
                .manifest
                .servers
                .keys()
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if results.is_empty() {
        println!("No catalog matches for '{query}'.");
        return Ok(());
    }

    println!("{} match(es):\n", results.len());
    for e in &results {
        let added = installed.contains(&e.name);
        let badge = if added {
            format!(" {}", "(in manifest)".green())
        } else {
            String::new()
        };
        println!(
            "{} {}  {}{badge}",
            e.name.bold(),
            format!("[{}]", e.kind).dimmed(),
            e.description
        );
        if !e.tags.is_empty() {
            println!("  {}", e.tags.join(", ").dimmed());
        }
        if !added {
            println!("  {} {}", "↳".cyan(), e.add_command());
        }
        println!();
    }
    Ok(())
}
