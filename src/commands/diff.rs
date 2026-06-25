//! `agentstack diff` — show drift between the manifest and on-disk configs.
//! Always read-only.

use std::path::Path;

use anyhow::Result;
use owo_colors::OwoColorize;

use crate::cli::DiffArgs;
use crate::render::{plan_target, resolve_targets, Selection};
use crate::scope::Scope;
use crate::state::{target_key, State};

pub fn run(args: &DiffArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let ctx = super::load(manifest_dir)?;
    let manifest = &ctx.loaded.manifest;
    let scope = args.scope.unwrap_or(Scope::Global);

    let selection = match &args.profile {
        Some(p) => Selection::Profile(p.clone()),
        None => Selection::All,
    };

    let target_ids = resolve_targets(manifest, &ctx.registry, &args.targets);
    let state = State::load()?;
    let mut drift = 0;

    for id in &target_ids {
        let Some(desc) = ctx.registry.get(id) else {
            println!("{} unknown adapter '{id}' — skipping", "⚠".yellow());
            continue;
        };
        let previously = state.managed_servers(&target_key(id, scope));
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
            continue;
        };
        println!("\n{} ({})", plan.display.bold(), plan.config_path.display());
        if plan.changed() {
            drift += 1;
            for l in plan.diff().lines() {
                println!("  {l}");
            }
        } else {
            println!("  {} in sync", "✓".green());
        }
    }

    println!();
    if drift == 0 {
        println!("{} all targets in sync with the manifest.", "✓".green());
    } else {
        println!(
            "{drift} target(s) drifted. Run {} to reconcile.",
            "agentstack apply --write".bold()
        );
    }

    Ok(())
}
