//! `agentstack consolidate` — gather scattered skills into the managed home.

use std::path::Path;

use anyhow::Result;
use owo_colors::OwoColorize;

use crate::adapter::Registry;
use crate::cli::ConsolidateArgs;
use crate::scope::Scope;

pub fn run(args: &ConsolidateArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let ctx = super::load(manifest_dir)?;

    if args.list {
        let mut any = false;
        for desc in ctx.registry.iter() {
            let found = desc.discover_skills(Scope::Global, &ctx.dir);
            for sk in found {
                any = true;
                let kind = if sk.is_symlink { "link" } else { "dir " };
                println!(
                    "  {} {:30} {} {}",
                    desc.id.dimmed(),
                    sk.name.bold(),
                    kind.dimmed(),
                    sk.source.display().to_string().dimmed()
                );
            }
        }
        if !any {
            println!("No skills found on disk in any CLI's skills directory.");
        }
        return Ok(());
    }

    let only = if args.names.is_empty() {
        None
    } else {
        Some(args.names.as_slice())
    };
    let registry = Registry::load()?;
    let report =
        crate::consolidate::consolidate(&registry, &ctx.loaded.manifest_path, &ctx.dir, only)?;

    for c in &report {
        let where_ = c.linked_into.join(", ");
        if c.already_home {
            println!(
                "{} {} (already in home) ← {where_}",
                "·".dimmed(),
                c.name.bold()
            );
        } else {
            println!(
                "{} {} → {} ← linked back into {where_}",
                "✓".green(),
                c.name.bold(),
                c.home.display().to_string().dimmed()
            );
        }
    }
    println!(
        "\nConsolidated {} skill(s) into {}.",
        report.len(),
        crate::util::paths::skills_home().display()
    );
    println!("Originals are now symlinks; backups are in ~/.agentstack/backups/skills/.");
    Ok(())
}
