//! `agentstack instructions` — compile instruction fragments into each
//! harness's CLAUDE.md / AGENTS.md. Read-only by default; `--write` applies.

use std::path::Path;

use anyhow::Result;
use owo_colors::OwoColorize;

use crate::cli::InstructionsArgs;
use crate::render::instructions::plan_instructions;
use crate::render::resolve_targets;
use crate::scope::Scope;

pub fn run(args: &InstructionsArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let ctx = super::load(manifest_dir)?;
    let manifest = &ctx.loaded.manifest;
    let scope = args.scope.unwrap_or(Scope::Global);

    if manifest.instructions.is_empty() {
        println!("Manifest defines no [instructions].");
        return Ok(());
    }

    let target_ids = resolve_targets(manifest, &ctx.registry, &args.targets);
    println!("Scope: {scope}");
    let mut changed = 0;

    for id in &target_ids {
        let Some(desc) = ctx.registry.get(id) else {
            continue;
        };
        let Some(plan) = plan_instructions(manifest, desc, scope, &ctx.dir) else {
            continue;
        };

        println!("\n{} ({})", desc.display.bold(), plan.path.display());
        for m in &plan.missing {
            println!("  {} fragment '{m}' source missing", "✗".red());
        }
        if plan.fragments.is_empty() {
            println!("  no fragments target this harness");
            continue;
        }
        println!("  fragments: {}", plan.fragments.join(", "));

        if plan.changed() {
            changed += 1;
            print!(
                "{}",
                plan.diff()
                    .lines()
                    .map(|l| format!("  {l}\n"))
                    .collect::<String>()
            );
            if args.write {
                plan.write()?;
                println!("  {} wrote managed region", "✓".green());
            } else {
                println!("  {} would update managed region", "→".cyan());
            }
        } else {
            println!("  {} up to date", "✓".green());
        }
    }

    println!();
    if args.write {
        println!("Updated {changed} instruction file(s).");
    } else {
        println!(
            "{changed} instruction file(s) would change. Re-run with {} to write.",
            "--write".bold()
        );
    }
    Ok(())
}
