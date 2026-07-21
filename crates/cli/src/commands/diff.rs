//! `agentstack diff` — show drift between the manifest and on-disk configs.
//! Always read-only.

use std::path::Path;

use anyhow::Result;
use owo_colors::OwoColorize;

use crate::cli::DiffArgs;
use crate::render::{effective_servers, plan_target_with_servers, resolve_targets, Selection};
use crate::scope::Scope;
use crate::state::{target_key, State};

/// What the diff pass found — beyond the printed report, so callers/tests can
/// assert on it.
#[derive(serde::Serialize)]
pub struct Outcome {
    pub scope: String,
    pub profile: Option<String>,
    /// Targets whose on-disk config differs from the render.
    pub drifted: usize,
    /// Per-target foreign entries the apply guard would keep — surfaced here
    /// instead of being previewed as pending deletions: `(display, names)`.
    pub kept: Vec<(String, Vec<String>)>,
    pub targets: Vec<TargetOutcome>,
    pub owner_refreshes: Vec<OwnerRefresh>,
    pub warnings: Vec<String>,
}

#[derive(serde::Serialize)]
pub struct TargetOutcome {
    pub id: String,
    pub display: String,
    pub path: String,
    pub changed: bool,
    pub diff: String,
    pub kept: Vec<String>,
}

#[derive(serde::Serialize)]
pub struct OwnerRefresh {
    pub name: String,
    pub owner: String,
    pub stale: bool,
}

pub fn run(args: &DiffArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let outcome = collect(args, manifest_dir, !args.json)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&outcome)?);
    }
    Ok(())
}

pub fn report(args: &DiffArgs, manifest_dir: Option<&Path>) -> Result<Outcome> {
    collect(args, manifest_dir, true)
}

fn collect(args: &DiffArgs, manifest_dir: Option<&Path>, print_text: bool) -> Result<Outcome> {
    let ctx = super::load(manifest_dir)?;
    let manifest = &ctx.loaded.manifest;
    let scope = args.scope.unwrap_or_else(|| Scope::default_for(&ctx.dir));

    let selection = match &args.profile {
        Some(p) => Selection::Profile(p.clone()),
        None => Selection::All,
    };

    // Library-aware effective server set (inline-first, then central library),
    // shared across targets so diff sees the same servers render/apply will.
    let libctx = ctx.library_ctx();
    let mut server_map =
        effective_servers(manifest, &libctx.library, &libctx.lib_home, &selection)?;
    // Owner-refreshed servers: diff against the owning app's on-disk values,
    // so drift on an owned server reads "refresh manifest + re-fan out",
    // never a proposed downgrade of what the app wrote (see render::owned).
    let owned =
        crate::render::refresh_owned_servers(&mut server_map, &ctx.registry, scope, &ctx.dir);
    for o in owned.iter().filter(|o| o.stale) {
        if print_text {
            println!(
                "{} {}: changed in {} (owner) — manifest entry is stale ↳ refresh + re-fan out: \
                 agentstack apply --write",
                "↻".cyan(),
                o.name,
                o.owner_display
            );
        }
    }

    let target_ids = resolve_targets(manifest, &ctx.registry, &args.targets)?;
    let state = State::load()?;
    let mut drift = 0;
    let mut kept_all: Vec<(String, Vec<String>)> = Vec::new();
    let mut target_outcomes = Vec::new();
    let mut warnings = Vec::new();

    let ruleset = crate::render::ruleset_for(manifest)?;
    for id in &target_ids {
        let Some(desc) = ctx.registry.get(id) else {
            let warning = format!("unknown CLI '{id}' — skipping");
            if print_text {
                println!("{} {warning}", "⚠".yellow());
            }
            warnings.push(warning);
            continue;
        };
        let key = target_key(id, scope, &ctx.dir);
        let mut previously = state.managed_servers(&key);
        // Same cross-manifest guard as apply: entries another manifest
        // recorded won't be pruned by a bare `apply --write`, so don't
        // preview them as pending deletions here either — surface them.
        let mut kept = state.foreign_prunes(&key, scope, &ctx.dir, &mut previously, |n| {
            server_map.contains_key(n)
        });
        // Plus names an earlier guarded write already kept on disk.
        for n in state.kept_foreign(&key) {
            if !kept.contains(&n) && !server_map.contains_key(&n) {
                kept.push(n);
            }
        }
        let Some(plan) = plan_target_with_servers(
            desc,
            &ctx.resolver,
            &ruleset,
            &server_map,
            &previously,
            scope,
            &ctx.dir,
        )?
        else {
            continue;
        };
        if print_text {
            println!("\n{} ({})", plan.display.bold(), plan.config_path.display());
        }
        let target_kept = kept.clone();
        if !kept.is_empty() {
            if print_text {
                println!(
                    "  {} keeping {} — applied by another manifest ↳ keep: agentstack adopt · \
                     prune: agentstack apply --prune-foreign",
                    "⚠".yellow(),
                    kept.join(", ")
                );
            }
            kept_all.push((plan.display.clone(), kept));
        }
        let changed = plan.changed();
        // Structured consumers always get a plain diff. Terminal coloring is
        // a presentation concern and would leave ANSI escape bytes in JSON.
        let rendered_diff = if changed {
            plan.diff_plain()
        } else {
            String::new()
        };
        if changed {
            drift += 1;
            if print_text {
                for l in plan.diff().lines() {
                    println!("  {l}");
                }
            }
        } else if print_text {
            println!("  {} in sync", "✓".green());
        }
        target_outcomes.push(TargetOutcome {
            id: id.clone(),
            display: plan.display.clone(),
            path: plan.config_path.display().to_string(),
            changed,
            diff: rendered_diff,
            kept: target_kept,
        });
    }

    if print_text {
        println!();
        if drift == 0 {
            println!("{} all targets in sync with the manifest.", "✓".green());
        } else {
            println!(
                "{drift} target(s) drifted. Run {} to reconcile.",
                "agentstack apply --write".bold()
            );
        }
    }

    Ok(Outcome {
        scope: scope.as_str().to_string(),
        profile: args.profile.clone(),
        drifted: drift,
        kept: kept_all,
        targets: target_outcomes,
        owner_refreshes: owned
            .into_iter()
            .map(|o| OwnerRefresh {
                name: o.name,
                owner: o.owner,
                stale: o.stale,
            })
            .collect(),
        warnings,
    })
}
