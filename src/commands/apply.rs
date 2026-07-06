//! `agentstack apply` — render the manifest into each target's native config.
//! Shows a preview first; TTY users can confirm, and `--write` applies directly.

use std::path::Path;

use anyhow::Result;
use owo_colors::OwoColorize;

use crate::cli::ApplyArgs;
use crate::manifest::{validate_with_context, ValidateCtx};
use crate::render::instructions::plan_instructions;
use crate::render::{
    effective_servers, plan_hooks, plan_settings, plan_target_with_servers, resolve_targets,
    Selection,
};
use crate::scope::Scope;
use crate::state::{target_key, State};

/// What a render pass observed, so callers can decide whether to prompt.
pub(crate) struct Outcome {
    /// How many targets (across servers/settings/hooks) have pending changes.
    pub changed_count: usize,
    /// Structural validation errors — nothing will be written until fixed, so
    /// there is nothing to confirm.
    pub validation_errors: bool,
    /// Unresolved secrets that would block at least one write. `apply` may still
    /// let a user confirm a partial write; setup uses this to stop before any
    /// newcomer wizard write.
    pub write_blockers: usize,
}

pub fn run(args: &ApplyArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let requested = args.write && !args.dry_run;
    if requested {
        // `--write`: apply directly. The scripting / CI escape hatch — never
        // prompts, whatever the terminal is.
        render(args, manifest_dir, true, false, true)?;
        return Ok(());
    }
    // No `--write`. An explicit `--dry-run`, or any non-interactive shell (CI,
    // pipes, redirects), keeps the classic read-only behavior exactly: show the
    // diff and the "re-run with --write" hint, write nothing, never block on
    // input.
    if args.dry_run || !crate::util::confirm::is_interactive() {
        render(args, manifest_dir, false, false, true)?;
        return Ok(());
    }
    // Interactive default: show the dry-run diff (no re-run hint), then offer to
    // apply it in place.
    let outcome = render(args, manifest_dir, false, false, false)?;
    if outcome.changed_count == 0 || outcome.validation_errors {
        return Ok(());
    }
    if crate::util::confirm::confirm("\nApply these changes?")? {
        // Confirmed: a quiet second pass writes without re-printing the diff.
        render(args, manifest_dir, true, true, true)?;
    } else {
        println!("Not written. Re-run with {} to apply.", "--write".bold());
    }
    Ok(())
}

/// Show the dry-run diff without the "re-run with `--write`" hint, for a caller
/// (e.g. `setup`) that shows this preview and then drives its own confirm.
pub(crate) fn preview(args: &ApplyArgs, manifest_dir: Option<&Path>) -> Result<Outcome> {
    render(args, manifest_dir, false, false, false)
}

/// Apply for real without re-printing the diff — the write half for a caller
/// (e.g. `setup`) that already showed the `preview` and got its own confirm.
/// Prints only the per-target write results, so the diff isn't shown twice.
pub(crate) fn write_quiet(args: &ApplyArgs, manifest_dir: Option<&Path>) -> Result<()> {
    render(args, manifest_dir, true, true, true).map(|_| ())
}

/// One render pass. `want_write` requests a write (still gated on validation);
/// `quiet` suppresses the diff bodies for the confirmed second pass; `rerun_hint`
/// controls whether the dry-run summary points at `--write` (off when a prompt
/// follows).
fn render(
    args: &ApplyArgs,
    manifest_dir: Option<&Path>,
    want_write: bool,
    quiet: bool,
    rerun_hint: bool,
) -> Result<Outcome> {
    let ctx = super::load(manifest_dir)?;
    let manifest = &ctx.loaded.manifest;
    let scope = args.scope.unwrap_or(Scope::Global);

    let selection = match &args.profile {
        Some(p) => Selection::Profile(p.clone()),
        None => Selection::All,
    };

    // Library-aware validation + the effective server set (inline-first, then
    // central library), shared across targets.
    let libctx = ctx.library_ctx();
    let vctx = libctx.validate_ctx(&ctx.dir);
    let target_ids_for_validation: Vec<&str> = ctx.registry.ids().collect();
    let has_errors = print_validation(manifest, target_ids_for_validation, &vctx, quiet);
    let server_map = effective_servers(manifest, &libctx.library, &libctx.lib_home, &selection)?;

    let mut will_write = want_write;

    // Structural validation errors would produce broken/partial config — never
    // write on them.
    if will_write && has_errors {
        if !quiet {
            println!(
                "\n{} manifest has validation errors — not writing. Fix them first.",
                "✗".red()
            );
        }
        will_write = false;
    }

    let target_ids = resolve_targets(manifest, &ctx.registry, &args.targets);
    if target_ids.is_empty() {
        if !quiet {
            println!("No targets to apply to. Set [targets].default or pass --target.");
        }
        return Ok(Outcome {
            changed_count: 0,
            validation_errors: has_errors,
            write_blockers: 0,
        });
    }

    println!("Scope: {scope}");
    let mut state = State::load()?;
    let identity = crate::state::manifest_identity(&ctx.dir);
    let mut changed_count = 0;
    let mut error_count = 0;
    let mut write_blockers = 0;
    // Pre-write snapshots of every file we touch, grouped into one undoable
    // history entry for this apply.
    let mut backups: Vec<crate::history::FileChange> = Vec::new();
    let project_root = crate::manifest::project_root_of(&ctx.dir);
    let mut ignore_entries: Vec<String> = Vec::new();
    let mut touched_targets: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    // Per-target outcome for the write summary: `changed_count` tallies plans
    // (a target can change servers + settings + hooks), so the summary counts
    // targets — and only ones actually written, not ones a gate blocked.
    let mut changed_targets: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut blocked_targets: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    for id in &target_ids {
        let Some(desc) = ctx.registry.get(id) else {
            println!("{} unknown adapter '{id}' — skipping", "⚠".yellow());
            error_count += 1;
            continue;
        };

        let key = target_key(id, scope, &ctx.dir);
        // Whether this run compiled the instruction file — one input to the
        // managed .gitignore block computed at the end of this target's loop.
        let mut wrote_instructions = false;

        let mut previously = state.managed_servers(&key);
        // Names an earlier guarded write kept on disk (state bookkeeping —
        // they left `managed_servers` when this manifest recorded its own
        // set). Ones the manifest now selects become managed again below.
        let kept_before: Vec<String> = state
            .kept_foreign(&key)
            .into_iter()
            .filter(|n| !server_map.contains_key(n))
            .collect();
        // Guard cross-manifest global prunes: entries another manifest applied
        // are kept (and reported below), not deleted, unless --prune-foreign.
        let foreign = if args.prune_foreign {
            // Fold previously-kept names into the prune set — the escape
            // hatch must still reach them after a guarded write re-recorded
            // this key with only our own managed set.
            for n in &kept_before {
                if !previously.contains(n) {
                    previously.push(n.clone());
                }
            }
            Vec::new()
        } else {
            let mut f = state.foreign_prunes(&key, scope, &ctx.dir, &mut previously, |n| {
                server_map.contains_key(n)
            });
            // Keep surfacing (and tracking) what earlier runs kept.
            for n in &kept_before {
                if !f.contains(n) {
                    f.push(n.clone());
                }
            }
            f
        };
        let Some(plan) = plan_target_with_servers(
            desc,
            &ctx.resolver,
            &server_map,
            &previously,
            scope,
            &ctx.dir,
        )?
        else {
            println!("\n{} — no {scope} scope, skipping", desc.display.bold());
            continue;
        };

        println!("\n{} ({})", plan.display.bold(), plan.config_path.display());

        if plan.managed.is_empty() && plan.removed.is_empty() && plan.skipped.is_empty() {
            println!("  no servers selected");
        }
        for r in &plan.removed {
            println!("  {} pruning '{r}' (no longer in manifest)", "−".yellow());
        }
        if !foreign.is_empty() {
            println!(
                "  {} keeping {} — applied by another manifest ↳ keep: agentstack adopt · \
                 prune: agentstack apply --prune-foreign",
                "⚠".yellow(),
                foreign.join(", ")
            );
        }
        for s in &plan.skipped {
            println!(
                "  {} skipping '{s}' — {} can't represent this server's transport \
                 (add it via the harness's own UI/connector)",
                "↳".cyan(),
                plan.display
            );
        }
        for u in &plan.unresolved {
            println!("  {} unresolved secret {}", "✗".red(), u);
            error_count += 1;
        }
        // Unresolved `${REF}`s must never reach a live config file.
        let blocked = !plan.unresolved.is_empty() && !args.allow_unresolved;
        if blocked {
            write_blockers += 1;
        }

        if plan.changed() {
            changed_count += 1;
            changed_targets.insert(desc.display.clone());
            if !quiet {
                print!("{}", indent(&plan.diff()));
            }
            if will_write && blocked {
                blocked_targets.insert(desc.display.clone());
                println!(
                    "  {} not written — unresolved secret(s); set them or pass --allow-unresolved",
                    "✗".red()
                );
            } else if will_write {
                backups.push(crate::history::capture(
                    &plan.config_path,
                    format!("{} · servers", desc.display),
                ));
                touched_targets.insert(desc.display.clone());
                plan.write()?;
                state.record(&key, plan.managed.clone(), &plan.proposed, &identity);
                // Track what this guarded write kept on disk (empty after a
                // --prune-foreign actually pruned them) — see
                // TargetState::kept_foreign.
                state.record_kept_foreign(&key, foreign.clone());
                crate::usage::bump(&plan.managed);
                if plan.remove_if_empty_shell(desc) {
                    println!(
                        "  {} removed empty {}",
                        "−".yellow(),
                        plan.config_path.display()
                    );
                } else {
                    println!("  {} wrote {} server(s)", "✓".green(), plan.managed.len());
                }
            } else {
                println!("  {} {} server(s) to apply", "→".cyan(), plan.managed.len());
            }
        } else {
            // Even with no file change, keep state in sync with reality.
            if will_write {
                state.record(&key, plan.managed.clone(), &plan.proposed, &identity);
                state.record_kept_foreign(&key, foreign.clone());
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
            if sblocked {
                write_blockers += 1;
            }
            for r in &sp.removed {
                println!(
                    "  {} pruning setting '{r}' (no longer in manifest)",
                    "−".yellow()
                );
            }
            if sp.changed() {
                changed_count += 1;
                changed_targets.insert(desc.display.clone());
                println!(
                    "  {} settings → {}",
                    "·".dimmed(),
                    sp.settings_path.display()
                );
                if !quiet {
                    print!("{}", indent(&sp.diff()));
                }
                if will_write && sblocked {
                    blocked_targets.insert(desc.display.clone());
                    println!(
                        "  {} settings not written — unresolved secret(s)",
                        "✗".red()
                    );
                } else if will_write {
                    backups.push(crate::history::capture(
                        &sp.settings_path,
                        format!("{} · settings", desc.display),
                    ));
                    touched_targets.insert(desc.display.clone());
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
            if hblocked {
                write_blockers += 1;
            }
            if hp.changed() {
                changed_count += 1;
                changed_targets.insert(desc.display.clone());
                println!("  {} hooks → {}", "·".dimmed(), hp.path.display());
                if !quiet {
                    print!("{}", indent(&hp.diff()));
                }
                if will_write && hblocked {
                    blocked_targets.insert(desc.display.clone());
                    println!("  {} hooks not written — unresolved secret(s)", "✗".red());
                } else if will_write {
                    backups.push(crate::history::capture(
                        &hp.path,
                        format!("{} · hooks", desc.display),
                    ));
                    touched_targets.insert(desc.display.clone());
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

        // Instruction fragments (the managed region of CLAUDE.md / AGENTS.md).
        // Only when the manifest declares [instructions.*]: a manifest without
        // any must never touch — let alone empty out — a region another layer
        // (e.g. the machine manifest seeded by `init --global`) owns.
        if !manifest.instructions.is_empty() {
            // …and only when fragments actually apply at THIS scope: project
            // scope filters out every machine-layer fragment, so a project
            // with none of its own compiles to an empty string there — writing
            // that would strip a committed managed region from the repo.
            if let Some(ip) = plan_instructions(manifest, desc, scope, &ctx.dir)
                .filter(|ip| !ip.fragments.is_empty() || !ip.missing.is_empty())
            {
                for m in &ip.missing {
                    println!("  {} instruction fragment '{m}' source missing", "✗".red());
                    error_count += 1;
                }
                // A missing source already dropped its content from the
                // compile — writing now would delete previously compiled
                // fragments (all sources missing empties the whole region).
                // Block the write, mirroring the unresolved-secret path.
                let iblocked = !ip.missing.is_empty();
                if iblocked {
                    write_blockers += 1;
                }
                if ip.changed() {
                    changed_count += 1;
                    changed_targets.insert(desc.display.clone());
                    println!("  {} instructions → {}", "·".dimmed(), ip.path.display());
                    if !quiet {
                        print!("{}", indent(&ip.diff()));
                    }
                    if will_write && iblocked {
                        blocked_targets.insert(desc.display.clone());
                        println!(
                            "  {} instructions not written — missing fragment source(s)",
                            "✗".red()
                        );
                    } else if will_write {
                        backups.push(crate::history::capture(
                            &ip.path,
                            format!("{} · instructions", desc.display),
                        ));
                        touched_targets.insert(desc.display.clone());
                        ip.write()?;
                        wrote_instructions = true;
                        println!(
                            "  {} wrote {} instruction fragment(s)",
                            "✓".green(),
                            ip.fragments.len()
                        );
                    } else {
                        println!(
                            "  {} {} instruction fragment(s) to apply",
                            "→".cyan(),
                            ip.fragments.len()
                        );
                    }
                }
            }
        }

        // Managed .gitignore block: emit an entry only for an artifact this
        // target actually manages now — after the write sections above, so a
        // blocked write (nothing recorded) contributes nothing. Both flags read
        // persistent records `use` shares, keeping the block churn-free across
        // the two commands. `apply` never materializes skills, so its skills
        // flag is purely the record a prior `use` left.
        if scope == Scope::Project && will_write {
            let instr_path = desc
                .instructions
                .as_ref()
                .and_then(|s| s.path_for(scope, &ctx.dir));
            let managed = crate::render::gitignore::Managed {
                config: !state.managed_servers(&key).is_empty()
                    || !state.kept_foreign(&key).is_empty(),
                skills: !state.managed_skills(&key).is_empty(),
                instructions: wrote_instructions
                    || instr_path
                        .as_deref()
                        .is_some_and(crate::render::instructions::manages_file),
            };
            ignore_entries.extend(crate::render::gitignore::managed_entries(
                desc, scope, &ctx.dir, managed,
            ));
        }
    }

    let written_count = touched_targets.len();
    if will_write {
        state.save()?;
        // Record one undoable history entry for everything this apply wrote.
        // Best-effort: never fail an otherwise-successful apply over history.
        let _ = crate::history::record(
            scope.as_str(),
            touched_targets.into_iter().collect(),
            backups,
        );
    }

    if will_write
        && scope == Scope::Project
        && !args.no_gitignore
        && crate::render::gitignore::ensure_block(&project_root, &ignore_entries, true)?
    {
        println!(
            "\n{} .gitignore: managed block updated — generated artifacts stay out of git ({} to commit them instead)",
            "✓".green(),
            "--no-gitignore".bold()
        );
    }

    println!();
    if will_write {
        // Count targets actually written, not pending changes — a gate above
        // (unresolved secret, missing fragment source) may have blocked some
        // or all of the writes.
        if blocked_targets.is_empty() {
            println!("Applied to {written_count} target(s).");
        } else {
            println!(
                "{written_count} of {} target(s) written — {} blocked by unresolved secret(s) or missing fragment source(s); see {} above.",
                changed_targets.len(),
                blocked_targets.len(),
                "✗".red()
            );
        }
    } else if rerun_hint {
        println!(
            "{changed_count} target(s) would change. Re-run with {} to write.",
            "--write".bold()
        );
    } else {
        // A confirm prompt is about to follow — don't tell the user to re-run.
        println!("{changed_count} target(s) would change.");
    }
    if error_count > 0 && !quiet {
        println!("{error_count} issue(s) — resolve before writing.");
    }

    Ok(Outcome {
        changed_count,
        validation_errors: has_errors,
        write_blockers,
    })
}

/// Print validation issues (unless `quiet`); return true if any are structural
/// errors. The error check runs regardless of `quiet` so a write is still gated
/// on a clean manifest.
fn print_validation(
    manifest: &crate::manifest::Manifest,
    target_ids: Vec<&str>,
    vctx: &ValidateCtx,
    quiet: bool,
) -> bool {
    let issues = validate_with_context(manifest, target_ids, vctx);
    let mut has_error = false;
    for issue in &issues {
        if issue.kind.is_error() {
            has_error = true;
        }
        if !quiet {
            let (mark, msg) = if issue.kind.is_error() {
                ("✗".red().to_string(), &issue.message)
            } else {
                ("⚠".yellow().to_string(), &issue.message)
            };
            println!("{mark} {msg}");
        }
    }
    has_error
}

fn indent(s: &str) -> String {
    s.lines().map(|l| format!("  {l}\n")).collect::<String>()
}
