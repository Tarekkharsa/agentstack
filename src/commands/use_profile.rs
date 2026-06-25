//! `agentstack use <profile>` — activate a profile: render its servers into each
//! target's config and materialize its skills into the target's skills dir, for
//! the chosen scope. Dry-run by default; `--write` performs changes.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use owo_colors::OwoColorize;

use crate::cli::UseArgs;
use crate::manifest::Manifest;
use crate::render::skills;
use crate::render::{plan_target, resolve_targets, Selection};
use crate::scope::Scope;
use crate::state::{target_key, State};

pub fn run(args: &UseArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let ctx = super::load(manifest_dir)?;
    let manifest = &ctx.loaded.manifest;
    let scope = args.scope.unwrap_or(Scope::Global);

    let profile = manifest
        .profiles
        .get(&args.profile)
        .with_context(|| format!("no profile '{}' in manifest", args.profile))?;

    let selection = Selection::Profile(args.profile.clone());
    let active_skills = resolve_active_skills(manifest, &args.profile, &ctx.dir);

    let target_ids = resolve_targets(manifest, &ctx.registry, &args.targets);
    println!(
        "Activating profile '{}' (scope: {scope}) — {} server(s), {} skill(s)",
        args.profile.bold(),
        profile.servers.len(),
        active_skills.len()
    );

    let mut state = State::load()?;
    let mut wrote = 0;

    for id in &target_ids {
        let Some(desc) = ctx.registry.get(id) else {
            println!("{} unknown adapter '{id}' — skipping", "⚠".yellow());
            continue;
        };
        let key = target_key(id, scope);
        println!("\n{}", desc.display.bold());

        // --- servers ---
        let previously = state.managed_servers(&key);
        match plan_target(
            manifest,
            desc,
            &ctx.resolver,
            &selection,
            &previously,
            scope,
            &ctx.dir,
        )? {
            None => println!("  servers: no {scope} scope"),
            Some(plan) => {
                for u in &plan.unresolved {
                    println!("  {} unresolved secret {}", "✗".red(), u);
                }
                let blocked = !plan.unresolved.is_empty() && !args.allow_unresolved;
                if plan.changed() {
                    if args.write && blocked {
                        println!(
                            "  {} not written — unresolved secret(s); set them or pass --allow-unresolved",
                            "✗".red()
                        );
                    } else if args.write {
                        plan.write()?;
                        state.record(&key, plan.managed.clone(), &plan.proposed);
                        crate::usage::bump(&plan.managed);
                        wrote += 1;
                        println!("  {} servers → {}", "✓".green(), plan.config_path.display());
                    } else {
                        println!("  {} {} server(s) to write", "→".cyan(), plan.managed.len());
                    }
                } else {
                    println!("  {} servers up to date", "✓".green());
                }
            }
        }

        // --- skills ---
        let Some(skills_dir) = desc.skills_dir_for(scope, &ctx.dir) else {
            continue;
        };
        let strategy = desc.skills.as_ref().map(|s| s.strategy).unwrap_or_default();
        let prev_skills = state.managed_skills(&key);
        let plan = skills::plan(
            skills_dir.clone(),
            strategy,
            active_skills.clone(),
            &prev_skills,
        );

        for c in &plan.conflicts {
            println!(
                "  {} skill '{c}' already exists (not managed) — left as is",
                "⚠".yellow()
            );
        }
        for r in &plan.to_remove {
            println!("  {} unlinking skill '{r}'", "−".yellow());
        }
        if plan.has_work() {
            if args.write {
                skills::materialize(&plan)?;
                state.record_skills(&key, plan.managed_names());
                crate::usage::bump(&plan.managed_names());
                println!(
                    "  {} {} skill(s) → {}",
                    "✓".green(),
                    plan.managed_names().len(),
                    skills_dir.display()
                );
            } else {
                println!(
                    "  {} {} skill(s) to {} into {}",
                    "→".cyan(),
                    plan.active.len(),
                    strategy_word(strategy),
                    skills_dir.display()
                );
            }
        } else {
            println!("  {} skills up to date", "✓".green());
        }
    }

    if args.write {
        state.save()?;
        println!(
            "\n{} activated '{}' on {wrote} target(s).",
            "✓".green(),
            args.profile
        );
    } else {
        println!("\nDry run. Re-run with {} to apply.", "--write".bold());
    }
    Ok(())
}

fn strategy_word(s: crate::adapter::descriptor::SkillStrategy) -> &'static str {
    match s {
        crate::adapter::descriptor::SkillStrategy::Symlink => "symlink",
        crate::adapter::descriptor::SkillStrategy::Copy => "copy",
    }
}

/// Resolve a profile's active skills into `(name, absolute source dir)`,
/// expanding the `"*"` wildcard and warning about missing sources.
fn resolve_active_skills(
    manifest: &Manifest,
    profile_name: &str,
    dir: &Path,
) -> Vec<(String, PathBuf)> {
    let profile = match manifest.profiles.get(profile_name) {
        Some(p) => p,
        None => return Vec::new(),
    };
    let names: Vec<String> = if profile.loads_all_skills() {
        manifest.skills.keys().cloned().collect()
    } else {
        profile
            .skills
            .iter()
            .filter(|n| manifest.skills.contains_key(*n))
            .cloned()
            .collect()
    };

    let store = crate::store::Store::default_store();
    let mut out = Vec::new();
    for name in names {
        let skill = &manifest.skills[&name];
        match crate::store::local_source_dir(&store, skill, dir) {
            Some(source) => out.push((name, source)),
            None => println!(
                "{} skill '{name}' not available locally — run `agentstack install`",
                "⚠".yellow()
            ),
        }
    }
    out
}
