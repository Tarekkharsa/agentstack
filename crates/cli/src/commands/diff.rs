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
    /// Per-target foreign entries ANOTHER agentstack manifest applied, which
    /// the apply guard keeps — surfaced here instead of being previewed as
    /// pending deletions: `(display, names)`. Unchanged `diff-v1` meaning;
    /// undeclared hand-added entries live on each target's
    /// `foreign_untracked`.
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
    /// Foreign entries **another agentstack manifest** applied here, which
    /// this one keeps: `adopt` pulls them in, `apply --prune-foreign` removes
    /// them. This is exactly what `diff-v1` has always meant by `kept`, and it
    /// stays that narrow on purpose — a panel gated on `diff-v1` offers Adopt
    /// for these, and those two commands only reach entries the guard recorded
    /// as another manifest's.
    pub kept: Vec<String>,
    /// Foreign entries **nobody ever declared to agentstack** — hand-added
    /// straight into the file. `apply`'s merge preserves them exactly like
    /// `kept`, which is why H8 wanted them named; but no `adopt`/`--prune-foreign`
    /// command acts on them, so they are a SEPARATE field rather than swelling
    /// `kept`. Folding them together would have a `diff-v1` panel offering an
    /// Adopt button the CLI cannot honor — the same mistake `library-remove-v1`
    /// exists to prevent. Read this only when `diff-ownership-v1` is advertised.
    pub foreign_untracked: Vec<String>,
    /// Server names this target renders from the manifest right now.
    pub managed: Vec<String>,
    /// Whether this target's file changed on disk, in the region we manage,
    /// since our last recorded write — the same signal `doctor` calls
    /// "edited on disk since last apply".
    pub hand_edited: bool,
    /// Whether the config file was on disk when this diff was computed. With
    /// `changed`, this splits the two stories a pending render can tell:
    /// `false` means "never rendered here / file absent", `true` means "the
    /// manifest moved ahead of a rendered file". External UIs used to infer
    /// this from the `@@ -0,0` hunk header, which an empty-but-present file
    /// breaks. Read this only when `diff-existence-v1` is advertised.
    pub existed_before: bool,
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

    let target_ids = resolve_targets(manifest, &ctx.registry, &args.targets, &ctx.dir)?;
    let state = State::load()?;
    let mut drift = 0;
    // The reconcile command for the closing hint — `use <p> --write` when an
    // active toolset drives the expected render, else the full apply.
    let mut reconcile_cmd = "agentstack apply --write".to_string();
    let mut kept_all: Vec<(String, Vec<String>)> = Vec::new();
    let mut target_outcomes = Vec::new();
    let mut warnings = Vec::new();
    // Whether this run printed any ownership annotation at all — the legend
    // only earns its line when there was something to explain.
    let mut any_ownership_notes = false;

    let ruleset = crate::render::ruleset_for(manifest)?;
    // `diff` REPORTS drift between the manifest and disk. It may only claim
    // "in sync" about a lane it actually compared: where the planner routes a
    // harness's MCP servers live, `apply` writes no server config, so there is
    // no rendered file to compare and a `✓ in sync` line here would be a claim
    // about nothing (invariant 8). Same `delivery::Plan` reading `apply` uses.
    let plan_delivery =
        crate::delivery::Plan::build(&manifest.delivery, &ctx.registry, &target_ids);
    // Harnesses this pass did not compare, named in the closing summary so the
    // report says what it checked and what it did not.
    let mut live_withheld: Vec<String> = Vec::new();
    for id in &target_ids {
        let Some(desc) = ctx.registry.get(id) else {
            let warning = format!("unknown CLI '{id}' — skipping");
            if print_text {
                println!("{} {warning}", "⚠".yellow());
            }
            warnings.push(warning);
            continue;
        };
        if plan_delivery.servers_route_live(id) {
            // No `TargetOutcome` is pushed: `changed: false` is the wire form
            // of "in sync", and a structured consumer must not read that about
            // a comparison that never happened. The target is named in
            // `warnings` instead — an existing free-text field, so no
            // `diff-v1` consumer is broken by learning about the live lane.
            live_withheld.push(desc.display.clone());
            // Same per-harness bridge reading every other surface prints from:
            // "served live" asserts delivery, and with no gateway registered
            // there is none. `warnings` is free text, but a false claim in it
            // is still a false claim (invariant 8).
            let lane = if crate::commands::overview::bridge_registered(&ctx.registry, id) {
                "are served live"
            } else {
                "are planned live (not connected)"
            };
            let note = format!(
                "{} — MCP servers {lane}, not written; nothing rendered to compare",
                desc.display
            );
            if print_text {
                println!("\n{}", desc.display.bold());
                println!(
                    "  {} MCP servers {lane}, not written — nothing rendered to compare",
                    "·".dimmed()
                );
            }
            warnings.push(note);
            continue;
        }
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
        // Active-toolset awareness (mirrors doctor's drift section): when no
        // explicit --toolset was passed, compare against the selection a
        // `use <p> --write` last rendered for this key — so diff and doctor
        // keep telling the same story after a switch. Ownership checks above
        // stay on the full map (a manifest server outside the selection is
        // still ours, not foreign); only the expected render narrows. A
        // recorded toolset gone from the manifest falls back to the full map.
        let active_profile = match &args.profile {
            Some(_) => None,
            None => state
                .active_profile(&key)
                .filter(|p| manifest.profiles.contains_key(p)),
        };
        let profile_map: indexmap::IndexMap<String, crate::manifest::Server>;
        let render_map = match &active_profile {
            Some(p) => {
                reconcile_cmd = format!("agentstack use {p} --write");
                profile_map = server_map
                    .iter()
                    .filter(|(n, _)| manifest.profiles[p.as_str()].servers.contains(n))
                    .map(|(n, s)| (n.clone(), s.clone()))
                    .collect();
                &profile_map
            }
            None => &server_map,
        };
        let Some(plan) = plan_target_with_servers(
            desc,
            &ctx.resolver,
            &ruleset,
            render_map,
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
        // Text mode above splits the two kinds because their next steps
        // differ. The STRUCTURED output keeps them split for a stronger
        // reason: `kept` is a `diff-v1` field whose consumers offer Adopt, and
        // `adopt`/`--prune-foreign` cannot act on an entry agentstack never
        // recorded. So `kept` keeps its original narrow meaning and the
        // undeclared names ride on their own field, behind their own feature
        // name (`diff-ownership-v1`).
        if !kept.is_empty() {
            kept_all.push((plan.display.clone(), kept.clone()));
        }
        let target_kept = kept.clone();
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
            foreign_untracked: untracked,
            managed: plan.managed.clone(),
            hand_edited,
            existed_before: plan.existed_before,
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
            // The claim names its own denominator. With a live-routed harness
            // in play, "all targets in sync" would cover targets this pass
            // never compared.
            if live_withheld.is_empty() {
                println!("{} all targets in sync with the manifest.", "✓".green());
            } else {
                println!(
                    "{} every target compared here is in sync with the manifest.",
                    "✓".green()
                );
                println!(
                    "  {} not compared: {} — their MCP servers are routed to the live lane and \
                     nothing is written for them.",
                    "·".dimmed(),
                    live_withheld.join(", ")
                );
                println!(
                    "  {} check that lane instead: agentstack x delivery · write files anyway: \
                     agentstack x delivery render-locally --write",
                    "→".cyan()
                );
            }
        } else {
            println!(
                "{} drifted. Run {} to reconcile.",
                super::count(drift, "target"),
                reconcile_cmd.bold()
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
