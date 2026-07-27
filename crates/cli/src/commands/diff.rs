//! `agentstack diff` — show drift between the manifest and on-disk configs.
//! Always read-only.

use std::path::Path;

use anyhow::Result;
use owo_colors::OwoColorize;

use crate::cli::DiffArgs;
use crate::render::{
    effective_servers, plan_target_with_servers, resolve_targets, section_keys, Selection,
};
use crate::scope::Scope;
use crate::state::{self, target_key, State};

/// What the diff pass found — beyond the printed report, so callers/tests can
/// assert on it.
#[derive(serde::Serialize)]
pub struct Outcome {
    pub scope: String,
    pub profile: Option<String>,
    /// Targets whose on-disk config differs from the render.
    pub drifted: usize,
    /// Per-target foreign entries agentstack keeps but does not own —
    /// another manifest's servers, or an entry nobody declared to
    /// agentstack at all — surfaced here instead of being previewed as
    /// pending deletions: `(display, names)`.
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
    /// Foreign entries present in the live config that agentstack keeps but
    /// does not own: another manifest's servers (adopt-or-`--prune-foreign`
    /// eligible) AND entries nobody ever declared to agentstack at all — a
    /// hand-added server, for instance. Both are "kept" in the same sense —
    /// `apply`'s merge never deletes either — this list is what H8 asked
    /// `diff` to actually say out loud instead of leaving as unlabeled
    /// context in the raw diff.
    pub kept: Vec<String>,
    /// Server names this target renders from the manifest right now.
    pub managed: Vec<String>,
    /// Whether this target's file changed on disk, in the region we manage,
    /// since our last recorded write — the same signal `doctor` calls
    /// "edited on disk since last apply".
    pub hand_edited: bool,
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
        let body = serde_json::to_value(&outcome)?;
        println!(
            "{}",
            serde_json::to_string_pretty(&crate::ui_contract::envelope(body))?
        );
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
    // Whether this run printed any ownership annotation at all — the legend
    // only earns its line when there was something to explain.
    let mut any_ownership_notes = false;

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

        // H8: names physically present in the live config's managed section
        // right now. Anything here that isn't in `plan.managed` (ours, going
        // forward) or `plan.removed` (ours, being pruned) was never written
        // by this apply — a hand-added entry, or one left by another
        // manifest. `merge_json`/`merge_toml` already preserve those bytes
        // untouched; this is only naming what was always true so `diff`
        // stops showing them as unlabeled context (review finding H8).
        let on_disk = match desc.config_for(scope, &ctx.dir) {
            Some((_, format)) => desc
                .mcp
                .as_ref()
                .map(|mcp| section_keys(&plan.existing, &mcp.location, format))
                .unwrap_or_default(),
            None => Vec::new(),
        };
        let mut foreign = kept.clone();
        for name in &on_disk {
            if !plan.managed.contains(name)
                && !plan.removed.contains(name)
                && !foreign.contains(name)
            {
                foreign.push(name.clone());
            }
        }
        // Split out the ones with no state-tracked provenance purely for the
        // hint text: `adopt`/`--prune-foreign` only reach entries the guard
        // above recorded as another manifest's — a name we never owned at
        // all has no such command, so promising one would be a lie.
        let untracked: Vec<String> = foreign
            .iter()
            .filter(|n| !kept.contains(n))
            .cloned()
            .collect();

        // Same file-level "touched since our last write" signal `doctor`
        // reports as "edited on disk since last apply", gated the same way
        // (only when the touch reached the region we actually manage) so the
        // two commands never disagree.
        let hand_edited = state.targets.get(&key).is_some_and(|ts| {
            !ts.last_hash.is_empty() && state::hash(&plan.existing) != ts.last_hash
        }) && plan.changed();

        if print_text {
            let managed_suffix = if plan.managed.is_empty() {
                String::new()
            } else {
                format!(
                    " {}",
                    format!("· managed: {}", plan.managed.join(", ")).dimmed()
                )
            };
            println!(
                "\n{} ({}){managed_suffix}",
                plan.display.bold(),
                plan.config_path.display()
            );
            if hand_edited {
                any_ownership_notes = true;
                println!(
                    "  {} hand-edited: this file no longer matches what agentstack last wrote",
                    "⚠".yellow()
                );
            }
        }
        if !kept.is_empty() {
            any_ownership_notes = true;
            if print_text {
                println!(
                    "  {} foreign (kept), applied by another manifest: {} ↳ keep: agentstack \
                     adopt · prune: agentstack apply --prune-foreign",
                    "⚠".yellow(),
                    kept.join(", ")
                );
            }
        }
        if !untracked.is_empty() {
            any_ownership_notes = true;
            if print_text {
                println!(
                    "  {} foreign (kept), not agentstack's: {} — never added by us, never \
                     removed by us",
                    "⚠".yellow(),
                    untracked.join(", ")
                );
            }
        }
        // One structured "foreign" list per target — the union of both kinds
        // above. Text mode splits them (different next steps: one has an
        // adopt/prune command, the other doesn't); JSON just needs the names.
        if !foreign.is_empty() {
            kept_all.push((plan.display.clone(), foreign.clone()));
        }
        let target_kept = foreign;
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
            managed: plan.managed.clone(),
            hand_edited,
        });
    }

    if print_text {
        if any_ownership_notes {
            println!(
                "\n{} managed = rendered by agentstack from the manifest · foreign (kept) = \
                 present on disk but not ours, left alone · hand-edited = this file changed \
                 outside agentstack since the last write",
                "Legend:".dimmed()
            );
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
