//! `agentstack setup` — the one-command newcomer path.
//!
//! Pure orchestration over the everyday commands: `init` (only if there's no
//! manifest yet), the shared `bootstrap` preflight, inline secret prompts, an
//! `apply` preview, a single confirm, then `install` + `apply --write` +
//! `doctor`. It introduces no rendering or validation logic of its own, and it
//! reuses the shared confirm helper so a non-interactive shell (CI, pipes) only
//! ever previews — it never writes and never blocks on input.

use std::path::Path;

use anyhow::Result;
use owo_colors::OwoColorize;

use crate::cli::{ApplyArgs, DoctorArgs, InitArgs, InstallArgs, SetupArgs};
use crate::manifest::load::MANIFEST_FILE;
use crate::render::resolve_targets;
use crate::scope::Scope;

pub fn run(args: &SetupArgs, manifest_dir: Option<&Path>) -> Result<()> {
    println!("{}", "AgentStack setup".bold());

    // 1. Ensure a manifest exists — import the machine's existing config if not.
    let base = match manifest_dir {
        Some(d) => d.to_path_buf(),
        None => std::env::current_dir()?,
    };
    let interactive = crate::util::confirm::is_interactive();
    let mut manifest_path = crate::manifest::resolve_manifest_dir(&base).join(MANIFEST_FILE);
    if !manifest_path.exists() {
        if !interactive {
            println!(
                "\n{} `agentstack setup` is an interactive wizard and will not write in this shell.",
                "→".cyan()
            );
            println!("  For scripts/CI, use:");
            println!("    agentstack init");
            println!("    agentstack bootstrap --write");
            return Ok(());
        }
        println!("\nNo manifest here yet — importing the setup already on this machine.\n");
        super::init::run_for_setup(
            &InitArgs {
                force: false,
                dry_run: false,
                no_keychain: false,
            },
            manifest_dir,
        )?;
        manifest_path = crate::manifest::resolve_manifest_dir(&base).join(MANIFEST_FILE);
    }
    // `init` may have created `.agentstack/`, so re-resolve before loading.
    if !manifest_path.exists() {
        println!(
            "\n{} Nothing to set up yet. Add a capability, then re-run {}:",
            "→".cyan(),
            "agentstack setup".bold()
        );
        println!("    agentstack search <term>        find a server or skill");
        println!("    agentstack add server <name> …  add one you already know");
        return Ok(());
    }

    let ctx = super::load(manifest_dir)?;
    let scope = args.scope.unwrap_or(Scope::Global);
    let target_ids = resolve_targets(&ctx.loaded.manifest, &ctx.registry, &args.targets);

    // 2. Preflight inspection (adapters, skills, secrets) — shared with bootstrap.
    let pf = super::bootstrap::preflight(&ctx, &target_ids)?;

    // 3. Missing secrets — offer to set each one now (interactive only).
    let missing = resolve_missing_secrets(&ctx, pf.missing_secrets)?;

    // 4. Blocking issues stop before anything is written.
    if pf.validation_errors {
        println!(
            "\n{} Fix the manifest validation error(s) above, then re-run {}.",
            "→".cyan(),
            "agentstack setup".bold()
        );
        return Ok(());
    }
    if !missing.is_empty() {
        println!(
            "\n{} Still missing {}. Set them, then re-run {}:",
            "→".cyan(),
            missing.join(", "),
            "agentstack setup".bold()
        );
        for name in &missing {
            println!("    agentstack secret set {name}");
        }
        return Ok(());
    }

    // 5. Preview the exact config changes (no "re-run with --write" hint — we
    //    drive our own confirm next).
    println!("\n{}", "Preview".bold());
    let preview = super::apply::preview(&apply_args(args, scope, false), manifest_dir)?;
    if preview.validation_errors || preview.write_blockers > 0 {
        println!(
            "\n{} Resolve the issue(s) above, then re-run {}.",
            "→".cyan(),
            "agentstack setup".bold()
        );
        return Ok(());
    }

    // 6. Confirm, then apply for real. `confirm` returns false without blocking
    //    when there's no terminal, so CI/pipes stop here having written nothing.
    if !crate::util::confirm::confirm("\nApply this setup?")? {
        println!(
            "\n{} Nothing written. Re-run in a terminal to apply, or use {}.",
            "·".dimmed(),
            "agentstack apply --write".bold()
        );
        return Ok(());
    }

    println!("\n{}", "Install".bold());
    super::install::run(
        &InstallArgs {
            locked: false,
            allow_flagged: false,
        },
        manifest_dir,
    )?;

    println!("\n{}", "Apply".bold());
    // Quiet write: the diff was already shown in the preview above, so this
    // prints only the per-target write results rather than repeating it.
    super::apply::write_quiet(&apply_args(args, scope, true), manifest_dir)?;

    println!("\n{}", "Doctor".bold());
    super::doctor::run(
        &DoctorArgs {
            ci: false,
            live: false,
            fix: false,
        },
        manifest_dir,
    )?;

    // 7. Done — point at the obvious next step.
    println!("\n{} Setup complete.", "✓".green());
    match ctx.loaded.manifest.profiles.keys().next() {
        Some(profile) => println!(
            "  Launch an agent with it: {}",
            format!("agentstack run <cli> --profile {profile}").bold()
        ),
        None => println!(
            "  See everything in one place: {}",
            "agentstack dashboard".bold()
        ),
    }
    Ok(())
}

fn apply_args(args: &SetupArgs, scope: Scope, write: bool) -> ApplyArgs {
    ApplyArgs {
        targets: args.targets.clone(),
        profile: args.profile.clone(),
        dry_run: !write,
        write,
        scope: Some(scope),
        allow_unresolved: false,
        prune_foreign: false,
        no_gitignore: false,
    }
}

/// Prompt (hidden input) to store each missing secret in the keychain, then
/// re-detect what still doesn't resolve. In a non-interactive shell there's no
/// one to prompt, so the missing set is returned unchanged and the caller stops
/// with the manual `secret set` instructions.
fn resolve_missing_secrets(ctx: &super::Context, missing: Vec<String>) -> Result<Vec<String>> {
    if missing.is_empty() || !crate::util::confirm::is_interactive() {
        return Ok(missing);
    }

    println!("\n{}", "Set missing secrets".bold());
    println!(
        "  {} input is hidden; press Enter to skip one and set it later.",
        "·".dimmed()
    );
    for name in &missing {
        let value = rpassword::prompt_password(format!("  Value for {name}: ")).unwrap_or_default();
        if value.trim().is_empty() {
            println!("    {} skipped", "·".dimmed());
            continue;
        }
        crate::secret::keychain::set(name, &value)?;
        println!("    {} stored in keychain", "✓".green());
    }

    // Re-detect against a fresh view of the sources so anything we just stored
    // (and anything set out-of-band) is reflected.
    let sources = crate::secret::SecretSources::detect(&ctx.dir);
    Ok(ctx
        .loaded
        .manifest
        .referenced_secrets()
        .into_iter()
        .filter(|name| sources.source_of(name).is_none())
        .collect())
}
