//! `agentstack search <query>` — discovery across all providers (PLAN §9g/§9h):
//! the embedded catalog and the official MCP Registry. Marks what's already in
//! the manifest and prints how to add the rest. The agent's discovery surface.

use std::path::Path;

use anyhow::Result;
use owo_colors::OwoColorize;

use crate::cli::SearchArgs;
use crate::provider;

pub fn run(args: &SearchArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let query = args.query.clone().unwrap_or_default();
    if query.trim().is_empty() {
        println!(
            "Usage: agentstack search <query>  (searches the catalog + official MCP Registry)"
        );
        return Ok(());
    }

    let results = provider::search_all(&query, 25);

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
        println!("No matches for '{query}' (catalog or registry).");
        return Ok(());
    }

    println!("{} result(s) for '{query}':\n", results.len());
    for c in &results {
        let added = installed.contains(&c.name);
        let badge = if added {
            format!(" {}", "(in manifest)".green())
        } else {
            String::new()
        };
        println!(
            "{} {} {}{badge}",
            c.name.bold(),
            format!("[{}]", c.source).dimmed(),
            truncate(&c.description, 70)
        );
        if c.id != c.name {
            println!("  {}", c.id.dimmed());
        }
        let t = c.trust();
        let mut signals = Vec::new();
        if t.namespaced {
            signals.push("✓ verified namespace".green().to_string());
        }
        if t.runs_code {
            signals.push("⚠ runs code (npx)".yellow().to_string());
        }
        if t.needs_secret {
            signals.push("needs secret".dimmed().to_string());
        }
        if !signals.is_empty() {
            println!("  trust: {}", signals.join(" · "));
        }
        if !added {
            let cmd = match c.source {
                "catalog" => format!("agentstack add from {}", c.name),
                _ => format!("agentstack add from {}", c.id),
            };
            println!("  {} {cmd}", "↳".cyan());
        }
        println!();
    }
    Ok(())
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n).collect::<String>())
    }
}
