//! `agentstack bootstrap` — the guided first-run/team setup funnel.
//!
//! Read-only by default: inspect adapters, skills, secrets, and show the same
//! apply preview users would otherwise run manually. With `--write`, resolve
//! skill sources, write the lockfile if needed, apply live config changes, then
//! run doctor. Missing secrets or structural validation errors stop before
//! touching live harness configs.

use std::path::Path;

use anyhow::{bail, Result};
use owo_colors::OwoColorize;

use crate::cli::{ApplyArgs, BootstrapArgs, DoctorArgs, InstallArgs};
use crate::lock::Lock;
use crate::manifest::{validate_with_targets, Manifest};
use crate::render::resolve_targets;
use crate::scope::Scope;
use crate::secret::SecretSources;
use crate::store::{dir_digest, local_source_dir, Store};

pub fn run(args: &BootstrapArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let ctx = super::load(manifest_dir)?;
    let manifest = &ctx.loaded.manifest;
    let scope = args.scope.unwrap_or(Scope::Global);
    let target_ids = resolve_targets(manifest, &ctx.registry, &args.targets);

    println!("{}", "AgentStack bootstrap".bold());
    println!("Manifest: {}", ctx.dir.display());
    println!("Scope: {scope}");
    if let Some(profile) = &args.profile {
        println!("Profile: {profile}");
    }
    if target_ids.is_empty() {
        println!("Targets: none");
    } else {
        println!("Targets: {}", target_ids.join(", "));
    }

    let Preflight {
        validation_errors,
        skill_issues,
        missing_secrets,
    } = preflight(&ctx, &target_ids)?;

    println!();
    if args.write {
        println!("{}", "Install".bold());
        super::install::run(
            &InstallArgs {
                locked: args.locked,
                allow_flagged: false,
            },
            manifest_dir,
        )?;

        if validation_errors || !missing_secrets.is_empty() {
            for name in &missing_secrets {
                println!(
                    "  {} set missing secret: agentstack secret set {name}",
                    "→".cyan()
                );
            }
            bail!("bootstrap stopped before apply; fix the preflight issue(s) above");
        }

        println!("\n{}", "Apply".bold());
        super::apply::run(&apply_args(args, true, scope), manifest_dir)?;

        println!("\n{}", "Doctor".bold());
        super::doctor::run(
            &DoctorArgs {
                ci: false,
                live: false,
                fix: false,
                deep: false,
            },
            manifest_dir,
        )?;
    } else {
        println!("{}", "Preview".bold());
        super::apply::run(&apply_args(args, false, scope), manifest_dir)?;

        println!("{}", "Next".bold());
        if validation_errors {
            println!("  {} fix manifest validation errors first", "→".cyan());
        }
        if skill_issues > 0 {
            println!(
                "  {} install/update skill sources: agentstack install",
                "→".cyan()
            );
        }
        for name in &missing_secrets {
            println!(
                "  {} set missing secret: agentstack secret set {name}",
                "→".cyan()
            );
        }
        println!(
            "  {} run this workflow for real: {}",
            "→".cyan(),
            write_command(args, scope).bold()
        );
    }

    Ok(())
}

/// The read-only preflight summary, shared with `setup`.
pub(crate) struct Preflight {
    /// A structural manifest error — nothing should be written until fixed.
    pub validation_errors: bool,
    /// Skill sources that are missing, unlocked, or stale.
    pub skill_issues: usize,
    /// Referenced `${REF}`s that don't resolve on this machine.
    pub missing_secrets: Vec<String>,
}

/// Inspect adapters, skills, and secrets and print the same preflight report
/// `bootstrap` shows, returning a summary so callers can decide what to do next.
/// Read-only — touches no config.
pub(crate) fn preflight(ctx: &super::Context, target_ids: &[String]) -> Result<Preflight> {
    let manifest = &ctx.loaded.manifest;
    let validation_errors = print_validation(manifest, ctx.registry.ids().collect());
    print_adapters(ctx, target_ids);
    let skill_issues = print_skills(ctx)?;
    let missing_secrets = print_secrets(manifest, &ctx.dir);
    Ok(Preflight {
        validation_errors,
        skill_issues,
        missing_secrets,
    })
}

fn apply_args(args: &BootstrapArgs, write: bool, scope: Scope) -> ApplyArgs {
    ApplyArgs {
        targets: args.targets.clone(),
        profile: args.profile.clone(),
        dry_run: !write,
        write,
        scope: Some(scope),
        allow_unresolved: false,
        no_gitignore: false,
    }
}

fn print_validation(manifest: &Manifest, target_ids: Vec<&str>) -> bool {
    let issues = validate_with_targets(manifest, target_ids);
    if issues.is_empty() {
        println!("\n{} {}", "✓".green(), "Manifest validates".bold());
        return false;
    }

    println!("\n{}", "Manifest".bold());
    let mut has_errors = false;
    for issue in issues {
        if issue.kind.is_error() {
            has_errors = true;
            println!("  {} {}", "✗".red(), issue.message);
        } else {
            println!("  {} {}", "⚠".yellow(), issue.message);
        }
    }
    has_errors
}

fn print_adapters(ctx: &super::Context, target_ids: &[String]) {
    println!("\n{}", "Adapters".bold());
    if target_ids.is_empty() {
        println!("  {} no target adapters selected", "⚠".yellow());
        return;
    }
    for id in target_ids {
        match ctx.registry.get(id) {
            Some(desc) if desc.is_installed() => {
                println!("  {} {:<14} installed", "✓".green(), desc.display)
            }
            Some(desc) if desc.config_present() => println!(
                "  {} {:<14} config present, binary not on PATH",
                "⚠".yellow(),
                desc.display
            ),
            Some(desc) => println!("  {} {:<14} not detected", "⚠".yellow(), desc.display),
            None => println!("  {} unknown adapter '{id}'", "✗".red()),
        }
    }
}

fn print_skills(ctx: &super::Context) -> Result<usize> {
    println!("\n{}", "Skills".bold());
    let manifest = &ctx.loaded.manifest;
    if manifest.skills.is_empty() {
        println!("  {} no skills defined", "✓".green());
        return Ok(0);
    }

    let store = Store::default_store();
    let lock = Lock::load(&ctx.dir)?;
    let mut issues = 0;
    for (name, skill) in &manifest.skills {
        let Some(local) = local_source_dir(&store, skill, &ctx.dir) else {
            issues += 1;
            println!(
                "  {} {name:<20} source missing — run agentstack install",
                "⚠".yellow()
            );
            continue;
        };
        let Some(locked) = lock.get(name) else {
            issues += 1;
            println!("  {} {name:<20} present, not locked", "⚠".yellow());
            continue;
        };
        match dir_digest(&local) {
            Ok(sum) if sum == locked.checksum => {
                println!("  {} {name:<20} present · locked", "✓".green());
            }
            Ok(_) => {
                issues += 1;
                println!("  {} {name:<20} lockfile checksum stale", "⚠".yellow());
            }
            Err(e) => {
                issues += 1;
                println!("  {} {name:<20} cannot checksum: {e}", "✗".red());
            }
        }
    }
    Ok(issues)
}

fn print_secrets(manifest: &Manifest, dir: &Path) -> Vec<String> {
    println!("\n{}", "Secrets".bold());
    let refs = manifest.referenced_secrets();
    if refs.is_empty() {
        println!("  {} no secrets referenced", "✓".green());
        return Vec::new();
    }

    let sources = SecretSources::detect(dir);
    let mut missing = Vec::new();
    for name in refs {
        match sources.source_of(&name) {
            Some(source) => println!("  {} {name:<20} resolved from {source}", "✓".green()),
            None => {
                println!("  {} {name:<20} missing", "✗".red());
                missing.push(name);
            }
        }
    }
    missing
}

fn write_command(args: &BootstrapArgs, scope: Scope) -> String {
    let mut parts = vec![
        "agentstack".to_string(),
        "bootstrap".to_string(),
        "--write".to_string(),
    ];
    if args.locked {
        parts.push("--locked".to_string());
    }
    if scope != Scope::Global {
        parts.push("--scope".to_string());
        parts.push(scope.as_str().to_string());
    }
    if let Some(profile) = &args.profile {
        parts.push("--profile".to_string());
        parts.push(profile.clone());
    }
    for target in &args.targets {
        parts.push("--target".to_string());
        parts.push(target.clone());
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_command_includes_selected_options() {
        let args = BootstrapArgs {
            targets: vec!["codex".into(), "claude-code".into()],
            profile: Some("backend".into()),
            scope: Some(Scope::Project),
            locked: true,
            write: false,
        };
        assert_eq!(
            write_command(&args, Scope::Project),
            "agentstack bootstrap --write --locked --scope project --profile backend --target codex --target claude-code"
        );
    }
}
