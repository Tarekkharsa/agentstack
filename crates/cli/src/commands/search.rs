//! `agentstack search <query>` — discovery across all providers (PLAN §9g/§9h):
//! the embedded catalog and the official MCP Registry. Marks what's already in
//! the manifest and prints how to add the rest. The agent's discovery surface.

use std::path::Path;

use anyhow::Result;
use owo_colors::OwoColorize;

use crate::cli::SearchArgs;
use crate::provider::{self, CandidateKind};

pub fn run(args: &SearchArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let query = args.query.clone().unwrap_or_default();
    if query.trim().is_empty() {
        println!(
            "Usage: agentstack search <query>  (searches your central library + the catalog + official MCP Registry)"
        );
        return Ok(());
    }

    let results = provider::search_all(&query, 25);

    // A capability is "installed" if its server is in the manifest, or — for a
    // pack — if its `[packs.<name>]` install ledger exists.
    let installed = super::load(manifest_dir)
        .ok()
        .map(|ctx| {
            let m = &ctx.loaded.manifest;
            m.servers
                .keys()
                .chain(m.skills.keys())
                .chain(m.packs.keys())
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if results.is_empty() {
        println!("No matches for '{query}' (library, catalog, or registry).");
        return Ok(());
    }

    println!("{} result(s) for '{query}':\n", results.len());
    for c in &results {
        let added = installed.contains(&c.name);
        let mut badge = String::new();
        match &c.kind {
            CandidateKind::Pack(_) => badge.push_str(&format!(" {}", "[pack]".magenta())),
            CandidateKind::Skill(_) => badge.push_str(&format!(" {}", "[skill]".cyan())),
            CandidateKind::Extension(_) => badge.push_str(&format!(" {}", "[extension]".red())),
            CandidateKind::Hook(_) => badge.push_str(&format!(" {}", "[hook]".blue())),
            CandidateKind::Server(_) => {}
        }
        if added {
            badge.push_str(&format!(" {}", "(in manifest)".green()));
        }
        println!(
            "{} {} {}{badge}",
            c.name.bold(),
            format!("[{}]", c.source).dimmed(),
            truncate(&c.description, 70)
        );
        if c.id != c.name {
            println!("  {}", c.id.dimmed());
        }
        // Composition / source line per kind.
        match &c.kind {
            CandidateKind::Pack(spec) => {
                let mut parts = Vec::new();
                if spec.server.is_some() {
                    parts.push("1 server".to_string());
                }
                if !spec.skills.is_empty() {
                    parts.push(format!("{} skill", spec.skills.len()));
                }
                if !spec.instructions.is_empty() {
                    parts.push(format!("{} instruction", spec.instructions.len()));
                }
                if !parts.is_empty() {
                    println!("  {} {}", "contains:".dimmed(), parts.join(" · "));
                }
            }
            CandidateKind::Skill(skill) => {
                let source = skill
                    .path
                    .as_deref()
                    .map(|p| format!("path:{p}"))
                    .or_else(|| skill.git.as_deref().map(|g| format!("git:{g}")))
                    .unwrap_or_else(|| "—".into());
                println!("  {} {source}", "source:".dimmed());
            }
            CandidateKind::Extension(ext) => {
                println!("  {} {} extension", "target:".dimmed(), ext.target);
            }
            CandidateKind::Hook(h) => {
                let matcher = h
                    .hook
                    .matcher
                    .as_deref()
                    .map(|m| format!(" · matcher {m}"))
                    .unwrap_or_default();
                println!("  {} {}{matcher}", "event:".dimmed(), h.hook.event);
            }
            CandidateKind::Server(_) => {}
        }
        // Extensions carry the strongest, honest warning of any kind — their
        // code runs in-process, ungoverned at runtime (design doc §7) — rather
        // than the generic "runs code (npx)" line, which is stdio-shaped.
        if let CandidateKind::Extension(_) = &c.kind {
            println!(
                "  {} {}",
                "trust:".to_string().dimmed(),
                "⚠ executable, in-process, ungoverned at runtime (agentstack pins provenance only)"
                    .red()
            );
        } else {
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
        }
        if let CandidateKind::Extension(ext) = &c.kind {
            // Extensions aren't installed via `add from`; they are referenced by
            // name in the manifest, which re-gates trust + lock on the code.
            println!(
                "  {} reference it in [extensions.{}] with target = \"{}\", then `agentstack lock`",
                "↳".cyan(),
                c.name,
                ext.target
            );
        } else if !added {
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
    crate::text::truncate_chars(s, n)
}
