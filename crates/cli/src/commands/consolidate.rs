//! `agentstack lib consolidate` — gather scattered skills into the central library.

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
                let note = if sk.broken {
                    format!(" {}", "(target missing)".red())
                } else {
                    String::new()
                };
                println!(
                    "  {} {:30} {} {}{note}",
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
    let report = crate::consolidate::consolidate(
        &registry,
        &ctx.loaded.manifest_path,
        &ctx.dir,
        only,
        args.replace,
        args.write,
    )?;

    for c in &report.skills {
        let where_ = c.linked_into.join(", ");
        if c.already_home {
            println!(
                "{} {} (already in library) ← {where_}",
                "·".dimmed(),
                c.name.bold()
            );
        } else {
            let mark = if args.write {
                "✓".green().to_string()
            } else {
                "→".cyan().to_string()
            };
            println!(
                "{mark} {} → {} ← linked back into {where_}",
                c.name.bold(),
                c.home.display().to_string().dimmed()
            );
        }
        if c.inline_override {
            println!(
                "  {} project defines [skills.{}] inline — that keeps overriding the library copy",
                "⚠".yellow(),
                c.name
            );
        }
    }

    // What was found but left behind — a dead link otherwise reads as "my
    // skills weren't migrated" with nothing saying why. Printed in dry-run and
    // write modes alike.
    let broken: Vec<_> = report.skipped.iter().filter(|s| s.broken).collect();
    let no_skill_md: Vec<_> = report.skipped.iter().filter(|s| !s.broken).collect();
    if !broken.is_empty() {
        println!(
            "\n{} skipped {} broken link(s):",
            "⚠".yellow(),
            broken.len()
        );
        for s in &broken {
            let target = s.target.as_ref().unwrap_or(&s.entry);
            println!(
                "  {}: {} → {} (target missing)",
                s.cli.dimmed(),
                s.name.bold(),
                target.display()
            );
        }
        println!("  remove the dead link(s) or reinstall the skill(s) they point at");
    }
    if !no_skill_md.is_empty() {
        println!(
            "\n{} skipped {} dir(s) without SKILL.md:",
            "⚠".yellow(),
            no_skill_md.len()
        );
        for s in &no_skill_md {
            println!(
                "  {}: {} ({})",
                s.cli.dimmed(),
                s.name.bold(),
                s.entry.display().to_string().dimmed()
            );
        }
    }

    let verb = if args.write {
        "Consolidated"
    } else {
        "Would consolidate"
    };
    println!(
        "\n{verb} {} skill(s) into {}.",
        report.skills.len(),
        crate::util::paths::lib_home().join("skills").display()
    );
    if report.skills.is_empty() {
        return Ok(());
    }
    if args.write {
        println!("Originals are now symlinks; backups are in ~/.agentstack/backups/skills/.");
        println!("Skills are referenced by name from the library (`agentstack lib list`).");
    } else {
        println!("\nDry run. Re-run with {} to apply.", "--write".bold());
    }
    Ok(())
}
