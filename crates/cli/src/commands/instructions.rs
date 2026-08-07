//! `agentstack instructions` — compile instruction fragments into each
//! harness's CLAUDE.md / AGENTS.md. Read-only by default; `--write` applies.

use std::path::Path;

use agentstack_core::paint::OwoColorize;
use anyhow::Result;

use crate::cli::InstructionsArgs;
use crate::render::instructions::plan_instructions;
use crate::render::resolve_targets;
use crate::scope::Scope;

pub fn run(args: &InstructionsArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let ctx = super::load(manifest_dir)?;
    let manifest = &ctx.loaded.manifest;
    let scope = args.scope.unwrap_or_else(|| Scope::default_for(&ctx.dir));

    // Variant selection needs the linked sources and the toolset this
    // command was explicitly given; one read, before the per-target loop.
    let sel = crate::instructions::Selecting::for_command(args.toolset.as_deref());

    // The project's pinned package expansions: a package's instruction members
    // compile into the same managed region (W5, rendered lane), so a project
    // whose only house rules arrive through a package still has something to
    // compile.
    let pinned = crate::lock::Lock::load(&ctx.dir).unwrap_or_default();
    let packages = crate::package::effective_members(&pinned);

    if manifest.instructions.is_empty()
        && crate::package::members_of_kind(
            &pinned,
            crate::lock::PackageMemberKind::Instruction,
            None,
        )
        .is_empty()
    {
        println!("Manifest defines no [instructions].");
        return Ok(());
    }

    // Same fail-closed drift gate as `apply --write`: readable project
    // fragments must match their lock pins before compiling; unpinned passes
    // (the write records the first pin below); missing sources keep the
    // per-target blocked-write handling; machine-layer fragments are exempt.
    if args.write {
        let lock = crate::lock::Lock::load(&ctx.dir)?;
        // Fragments under a standing re-gate answer are exempt from the drift
        // gate, exactly like keep-pinned/blocked skills in `use --write`: a
        // keep-pinned fragment is drifted BY DEFINITION — that is what the
        // human was asked — and it is not compiled from the drifted file
        // anyway (the plan reads the approved store copy); a blocked one is
        // not compiled at all. Re-blocking here would make an answered
        // question unanswerable.
        let decided = super::use_profile::decided_names(&ctx.dir, "instruction");
        let statuses: Vec<_> = manifest
            .instructions
            .iter()
            .filter(|(n, i)| !i.from_user_layer && !decided.contains(*n))
            .map(|(n, i)| {
                let status = crate::resolve::instruction_lock_status_with(
                    n,
                    i,
                    &ctx.dir,
                    &lock,
                    &sel.library,
                );
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

    let target_ids = resolve_targets(manifest, &ctx.registry, &args.targets, &ctx.dir)?;

    // The unknown-target validation `apply`/`doctor` already run, scoped to
    // the instruction issues this command owns: a typo'd adapter id means the
    // fragment can never be delivered anywhere. Surface it on the dedicated
    // command too, and gate --write on it exactly like `apply` does.
    let known: Vec<&str> = ctx.registry.ids().collect();
    let bad_targets: Vec<_> = crate::manifest::validate_with_targets(manifest, known)
        .into_iter()
        .filter(|i| i.kind == crate::manifest::IssueKind::UnknownInstructionTarget)
        .collect();
    for issue in &bad_targets {
        println!("{} {}", "✗".red(), issue.message);
    }
    if args.write && !bad_targets.is_empty() {
        anyhow::bail!("manifest has validation errors — not writing. Fix them first.");
    }

    println!("Scope: {scope}");
    if let Some(up) = &ctx.loaded.user_path {
        println!(
            "Machine layer: {} (its fragments merge in beneath this project's, global scope only)",
            up.display()
        );
    }
    let mut changed = 0;
    let mut blocked = 0;
    // Blocked *by the trust gate* specifically: it changes both the closing
    // sentence and whether first pins may be recorded below.
    let mut refused = 0;

    for id in &target_ids {
        let Some(desc) = ctx.registry.get(id) else {
            continue;
        };
        // This command authors nothing the consent digest covers before it
        // compiles (its first pins are recorded after the loop), so the gate is
        // judged against the state on disk.
        let Some(plan) = plan_instructions(
            manifest,
            desc,
            scope,
            &ctx.dir,
            packages,
            &sel,
            crate::render::PriorTrust::STRICT,
        ) else {
            continue;
        };

        println!("\n{} ({})", desc.display.bold(), plan.path.display());
        for m in &plan.missing {
            println!("  {} fragment '{m}' source missing", "✗".red());
        }
        // Trust: an untrusted or drifted project compiles none of its own
        // prose into the region (`render::instructions::trust_refusal`). Said
        // here, before the diff, so the user reads WHY the preview below will
        // not be written.
        //
        // Only when the compile would actually move bytes, the same guard
        // `apply` applies (G23): an unchanged region is already what the
        // manifest declares, so "refusing to render" printed above "✓ up to
        // date" is two contradictory claims, and it would raise the issue
        // count on a run that then exits 0. The gate itself is untouched —
        // `InstrPlan::write` still refuses on `refusal` alone.
        if plan.refusal.is_some() && plan.changed() {
            if let Some(why) = &plan.refusal {
                println!("  {} {why}", "✗".red());
            }
        }
        for (name, why) in &plan.excluded {
            println!("  {} fragment '{name}' {why}", "⊘".dimmed());
        }
        if plan.fragments.is_empty() && plan.excluded.is_empty() {
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
        // Which BODY each fragment sent here, when it was not the base one.
        // The model and where it came from ride along, so a wrong variant is
        // diagnosable from the line that chose it.
        for (name, variant, why) in &plan.selected {
            println!("  {} {name} → variant {variant} ({why})", "↳".dimmed());
        }
        // The channel this harness is actually known to take house rules
        // through — and, when it declares a live one, that it is not used and
        // why. The same three sentences `status` prints.
        let rows = crate::instructions::channels(
            manifest,
            &ctx.registry,
            std::slice::from_ref(id),
            scope,
            &ctx.dir,
            &sel.library,
            sel.toolset(),
        );
        if let Some(live) = rows.first().and_then(|row| row.live.as_ref()) {
            println!(
                "  {}",
                format!(
                    "live channel {}: {}",
                    live.display,
                    match live.confirmation {
                        crate::adapter::Confirmation::Confirmed =>
                            crate::instructions::CONFIRMED_BUT_UNUSED,
                        crate::adapter::Confirmation::Unconfirmed =>
                            crate::instructions::UNCONFIRMED_NEVER_USED,
                    }
                )
                .dimmed()
            );
        }

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
                // previously compiled content from the managed region. A trust
                // refusal blocks it through the same seam — and leaves the
                // region as the human last approved it rather than emptying it.
                if plan.missing.is_empty() && plan.refusal.is_none() {
                    plan.write()?;
                    println!("  {} wrote managed region", "✓".green());
                } else if plan.refusal.is_some() {
                    blocked += 1;
                    refused += 1;
                    println!(
                        "  {} not written — the project has not been trusted for this content",
                        "✗".red()
                    );
                } else {
                    blocked += 1;
                    println!("  {} not written — missing fragment sources", "✗".red());
                }
            } else {
                println!("  {} would update managed region", "→".cyan());
            }
        } else {
            println!("  {} up to date", "✓".green());
        }
    }

    // Silent-drop guard: 6 of the 13 adapters have an instruction file. A
    // resolved target without one, that fragments nonetheless apply to, would
    // silently receive nothing — say so once, aggregated.
    let unreachable = crate::render::instructions::unreachable_instruction_targets(
        manifest,
        &ctx.registry,
        &target_ids,
    );
    if !unreachable.is_empty() {
        println!(
            "\n{} no instructions file for {} — fragments cannot reach these CLIs",
            "⚠".yellow(),
            unreachable.join(", ")
        );
    }
    // A fragment that EXPLICITLY names an incapable CLI (not via `"*"`) is a
    // per-fragment authoring mistake — name it and point at the fix.
    for (frag, target) in
        crate::render::instructions::explicit_incapable_instruction_targets(manifest, &ctx.registry)
    {
        println!(
            "{} instruction '{frag}' targets '{target}', which has no instructions file",
            "⚠".yellow()
        );
        println!("  {} remove the target or use a supported CLI", "↳".cyan());
    }

    println!();
    if args.write {
        // Record first pins for the readable project fragments (the gate
        // above blocked on drift, so nothing recorded here absorbed a change).
        // A trust refusal does NOT suppress this, deliberately: pinning is not
        // consenting — `lock --write` pins an untrusted project happily and
        // always has — and `apply` records the same pins after a blocked write.
        // Making the two commands differ here would only mean one of them left
        // a project in a state the other could not reproduce.
        if manifest.instructions.values().any(|i| !i.from_user_layer) {
            super::lock::record_instruction_pins(&ctx.dir, manifest, false)?;
        }
        println!("Updated {}.", super::count(changed, "instruction file"));
    } else {
        println!(
            "{} would change. Re-run with {} to write.",
            super::count(changed, "instruction file"),
            "--write".bold()
        );
    }
    if blocked > 0 {
        // Name the cause the user can act on. Two exist now — a missing source
        // and the trust gate — and a summary that named only one would send
        // them to the wrong fix, so a run that hit both defers to the ✗ line
        // per file. The single-cause wording is unchanged in both directions.
        let cause = match (refused > 0, blocked > refused) {
            (true, true) => {
                "missing fragment sources or an unreviewed project — \
                             see the ✗ line for each"
            }
            (true, false) => {
                "the project has not been trusted for this content — \
                              review and `agentstack trust .`"
            }
            _ => "missing fragment sources",
        };
        anyhow::bail!(
            "{} not written — {cause}",
            super::count(blocked, "instruction file")
        );
    }
    Ok(())
}
