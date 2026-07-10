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

    // Same fail-closed drift gate as `apply --write`: readable project
    // fragments must match their lock pins before compiling; unpinned passes
    // (the write records the first pin below); missing sources keep the
    // per-target blocked-write handling; machine-layer fragments are exempt.
    if args.write {
        let lock = crate::lock::Lock::load(&ctx.dir)?;
        let statuses: Vec<_> = manifest
            .instructions
            .iter()
            .filter(|(_, i)| !i.from_user_layer)
            .map(|(n, i)| {
                let status = crate::resolve::instruction_lock_status(n, i, &ctx.dir, &lock);
                (n.clone(), status)
            })
            .filter(|(_, s)| {
                !matches!(
                    s,
                    crate::resolve::InstructionLockStatus::ResolveFailed { .. }
                )
            })
            .collect();
        crate::verify::ensure_instructions_compilable(&ctx.dir.display().to_string(), &statuses)?;
    }

    let target_ids = resolve_targets(manifest, &ctx.registry, &args.targets);
    println!("Scope: {scope}");
    if let Some(up) = &ctx.loaded.user_path {
        println!(
            "Machine layer: {} (its fragments merge in beneath this project's, global scope only)",
            up.display()
        );
    }
    let mut changed = 0;
    let mut blocked = 0;

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
        let labels: Vec<String> = plan
            .fragments
            .iter()
            .map(|n| {
                if manifest
                    .instructions
                    .get(n)
                    .is_some_and(|i| i.from_user_layer)
                {
                    format!("{n} (machine)")
                } else {
                    n.clone()
                }
            })
            .collect();
        println!("  fragments: {}", labels.join(", "));

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
                // A missing fragment source blocks the write, like apply:
                // compiling without it would silently delete that fragment's
                // previously compiled content from the managed region.
                if plan.missing.is_empty() {
                    plan.write()?;
                    println!("  {} wrote managed region", "✓".green());
                } else {
                    blocked += 1;
                    println!("  {} not written — missing fragment source(s)", "✗".red());
                }
            } else {
                println!("  {} would update managed region", "→".cyan());
            }
        } else {
            println!("  {} up to date", "✓".green());
        }
    }

    println!();
    if args.write {
        // Record first pins for the readable project fragments (the gate
        // above blocked on drift, so nothing recorded here absorbed a change).
        if manifest.instructions.values().any(|i| !i.from_user_layer) {
            super::lock::record_instruction_pins(&ctx.dir, manifest, false)?;
        }
        println!("Updated {changed} instruction file(s).");
    } else {
        println!(
            "{changed} instruction file(s) would change. Re-run with {} to write.",
            "--write".bold()
        );
    }
    if blocked > 0 {
        anyhow::bail!("{blocked} instruction file(s) not written — missing fragment source(s)");
    }
    Ok(())
}
