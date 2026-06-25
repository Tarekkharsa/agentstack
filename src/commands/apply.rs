//! `agentstack apply` — render the manifest into each target's native config.
//! Read-only by default; `--write` performs the (non-destructive) write.

use std::path::Path;

use anyhow::Result;
use owo_colors::OwoColorize;

use crate::cli::ApplyArgs;
use crate::manifest::validate;
use crate::render::{plan_hooks, plan_settings, plan_target, resolve_targets, Selection};
use crate::scope::Scope;
use crate::state::{target_key, State};

pub fn run(args: &ApplyArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let ctx = super::load(manifest_dir)?;
    let manifest = &ctx.loaded.manifest;
    let scope = args.scope.unwrap_or(Scope::Global);

    let has_errors = print_validation(manifest);

    let selection = match &args.profile {
        Some(p) => Selection::Profile(p.clone()),
        None => Selection::All,
    };
    let mut will_write = args.write && !args.dry_run;

    // Structural validation errors would produce broken/partial config — never
    // write on them.
    if will_write && has_errors {
        println!(
            "\n{} manifest has validation errors — not writing. Fix them first.",
            "✗".red()
        );
        will_write = false;
    }

    let target_ids = resolve_targets(manifest, &ctx.registry, &args.targets);
    if target_ids.is_empty() {
        println!("No targets to apply to. Set [targets].default or pass --target.");
        return Ok(());
    }

    println!("Scope: {scope}");
    let mut state = State::load()?;
    let mut changed_count = 0;
    let mut error_count = 0;

    for id in &target_ids {
        let Some(desc) = ctx.registry.get(id) else {
            println!("{} unknown adapter '{id}' — skipping", "⚠".yellow());
            error_count += 1;
            continue;
        };

        let key = target_key(id, scope);
        let previously = state.managed_servers(&key);
        let Some(plan) = plan_target(
            manifest,
            desc,
            &ctx.resolver,
            &selection,
            &previously,
            scope,
            &ctx.dir,
        )?
        else {
            println!("\n{} — no {scope} scope, skipping", desc.display.bold());
            continue;
        };

        println!("\n{} ({})", plan.display.bold(), plan.config_path.display());

        if plan.managed.is_empty() && plan.removed.is_empty() {
            println!("  no servers selected");
        }
        for r in &plan.removed {
            println!("  {} pruning '{r}' (no longer in manifest)", "−".yellow());
        }
        for u in &plan.unresolved {
            println!("  {} unresolved secret {}", "✗".red(), u);
            error_count += 1;
        }
        // Unresolved `${REF}`s must never reach a live config file.
        let blocked = !plan.unresolved.is_empty() && !args.allow_unresolved;

        if plan.changed() {
            changed_count += 1;
            print!("{}", indent(&plan.diff()));
            if will_write && blocked {
                println!(
                    "  {} not written — unresolved secret(s); set them or pass --allow-unresolved",
                    "✗".red()
                );
            } else if will_write {
                plan.write()?;
                state.record(&key, plan.managed.clone(), &plan.proposed);
                crate::usage::bump(&plan.managed);
                println!("  {} wrote {} server(s)", "✓".green(), plan.managed.len());
            } else {
                println!("  {} {} server(s) to apply", "→".cyan(), plan.managed.len());
            }
        } else {
            // Even with no file change, keep state in sync with reality.
            if will_write {
                state.record(&key, plan.managed.clone(), &plan.proposed);
            }
            println!("  {} up to date", "✓".green());
        }

        // Native settings file (permissions, feature flags) — a separate file
        // from the MCP config, merged at the top level.
        let prev_settings = state.managed_settings(&key);
        if let Some(sp) = plan_settings(
            manifest,
            desc,
            &ctx.resolver,
            &prev_settings,
            scope,
            &ctx.dir,
        )? {
            for u in &sp.unresolved {
                println!("  {} unresolved secret {} (settings)", "✗".red(), u);
                error_count += 1;
            }
            let sblocked = !sp.unresolved.is_empty() && !args.allow_unresolved;
            for r in &sp.removed {
                println!(
                    "  {} pruning setting '{r}' (no longer in manifest)",
                    "−".yellow()
                );
            }
            if sp.changed() {
                changed_count += 1;
                println!(
                    "  {} settings → {}",
                    "·".dimmed(),
                    sp.settings_path.display()
                );
                print!("{}", indent(&sp.diff()));
                if will_write && sblocked {
                    println!(
                        "  {} settings not written — unresolved secret(s)",
                        "✗".red()
                    );
                } else if will_write {
                    sp.write()?;
                    state.record_settings(&key, sp.managed.clone());
                    println!("  {} wrote {} setting(s)", "✓".green(), sp.managed.len());
                } else {
                    println!("  {} {} setting(s) to apply", "→".cyan(), sp.managed.len());
                }
            } else if will_write && !sblocked {
                state.record_settings(&key, sp.managed.clone());
            }
        }

        // Lifecycle hooks (compiled into the harness's native hooks config).
        let prev_hooks = !state.managed_hooks(&key).is_empty();
        if let Some(hp) = plan_hooks(manifest, desc, &ctx.resolver, prev_hooks, scope, &ctx.dir)? {
            for u in &hp.unresolved {
                println!("  {} unresolved secret {} (hook)", "✗".red(), u);
                error_count += 1;
            }
            let hblocked = !hp.unresolved.is_empty() && !args.allow_unresolved;
            if hp.changed() {
                changed_count += 1;
                println!("  {} hooks → {}", "·".dimmed(), hp.path.display());
                print!("{}", indent(&hp.diff()));
                if will_write && hblocked {
                    println!("  {} hooks not written — unresolved secret(s)", "✗".red());
                } else if will_write {
                    hp.write()?;
                    state.record_hooks(&key, hp.managed.clone());
                    println!("  {} wrote {} hook(s)", "✓".green(), hp.managed.len());
                } else {
                    println!("  {} {} hook(s) to apply", "→".cyan(), hp.managed.len());
                }
            } else if will_write && !hblocked {
                state.record_hooks(&key, hp.managed.clone());
            }
        }
    }

    if will_write {
        state.save()?;
    }

    println!();
    if will_write {
        println!("Applied to {changed_count} target(s).");
    } else {
        println!(
            "{changed_count} target(s) would change. Re-run with {} to write.",
            "--write".bold()
        );
    }
    if error_count > 0 {
        println!("{error_count} issue(s) — resolve before writing.");
    }

    Ok(())
}

/// Print validation issues; return true if any are structural errors.
fn print_validation(manifest: &crate::manifest::Manifest) -> bool {
    let issues = validate(manifest);
    let mut has_error = false;
    for issue in &issues {
        if issue.kind.is_error() {
            has_error = true;
            println!("{} {}", "✗".red(), issue.message);
        } else {
            println!("{} {}", "⚠".yellow(), issue.message);
        }
    }
    has_error
}

fn indent(s: &str) -> String {
    s.lines().map(|l| format!("  {l}\n")).collect::<String>()
}
