//! `agentstack setup` — the one-command newcomer path.
//!
//! Pure orchestration over the everyday commands: `init` (only if there's no
//! manifest yet), a read-only preflight, inline secret prompts, an `apply`
//! preview, a single confirm, then `install` + `apply --write` + profile
//! activation (skills) + `doctor`. It introduces no rendering or validation
//! logic of its own, and it reuses the shared confirm helper so a
//! non-interactive shell (CI, pipes) only ever previews — it never writes and
//! never blocks on input.

use std::path::Path;

use anyhow::Result;
use owo_colors::OwoColorize;

use crate::cli::{ApplyArgs, DoctorArgs, InitArgs, InstallArgs, SetupArgs};
use crate::lock::Lock;
use crate::manifest::load::MANIFEST_FILE;
use crate::manifest::{validate_with_context, Manifest};
use crate::render::resolve_targets;
use crate::scope::Scope;
use crate::secret::SecretSources;
use crate::store::{dir_digest, local_source_dir, Store};

pub fn run(args: &SetupArgs, manifest_dir: Option<&Path>) -> Result<()> {
    println!("{}", "AgentStack setup".bold());

    // 1. Ensure a manifest exists — import the machine's existing config if
    //    not. The base walks up to the nearest ancestor project, so `setup`
    //    from a subdirectory continues the ROOT project instead of nesting.
    let base = super::project_base(manifest_dir)?;
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
            println!("    agentstack apply --write");
            println!("    agentstack use <profile> --write   # if the manifest has skills");
            return Ok(());
        }
        println!("\nNo manifest here yet — importing the setup already on this machine.\n");
        super::init::run_for_setup(
            &InitArgs {
                global: false,
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
    // Default scope follows the manifest's home: project for a repo manifest,
    // global only for the machine manifest (see docs/design/default-scope.md).
    let scope = args.scope.unwrap_or_else(|| Scope::default_for(&ctx.dir));
    let target_ids = resolve_targets(&ctx.loaded.manifest, &ctx.registry, &args.targets);

    // 2. Preflight inspection (adapters, skills, secrets) — read-only.
    let pf = preflight(&ctx, &target_ids)?;

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

    // 6b. Skills — `apply` renders servers/instructions/hooks/settings but
    //     never skills; they activate through a profile. Finish the job here
    //     via the same prepare/activate seam `use` and `session start` share,
    //     so the first agent session actually has the manifest's skills.
    //     Reload first: the apply pass above may have refreshed owned-server
    //     tables in the manifest on disk.
    let ctx = super::load(manifest_dir)?;
    // What to activate: a selected profile, the implicit default (no profiles
    // declared, but inline skills exist), or nothing left to do.
    let selection: Option<Option<String>> = match select_profile(&ctx, args)? {
        Some(p) => Some(Some(p)),
        None if !ctx.loaded.manifest.skills.is_empty() => Some(None),
        None => None,
    };
    if let Some(profile) = selection {
        let label = profile.clone().unwrap_or_else(|| "default".into());
        let cmd = match &profile {
            Some(p) => format!("agentstack use {p} --write"),
            None => "agentstack use --write".to_string(),
        };
        println!("\n{}", "Skills".bold());
        if let Err(err) = materialize_profile(&ctx, args, scope, profile.as_deref()) {
            // Configs are already written at this point — surface the
            // problem and the exact recovery command instead of failing
            // the whole setup on its last step.
            println!(
                "  {} could not activate profile '{label}' ({err:#})",
                "⚠".yellow()
            );
            println!("  Fix the issue, then run: {}", cmd.bold());
        }
    }

    println!("\n{}", "Doctor".bold());
    super::doctor::run(
        &DoctorArgs {
            ci: false,
            live: false,
            fix: false,
            deep: false,
            all: false,
            json: false,
        },
        manifest_dir,
    )?;

    // 7. Machine layer: offer the agentstack house rules once. They live in
    //    the personal manifest (~/.agentstack), not this project, so every
    //    repo's agents get them via the global CLAUDE.md / AGENTS.md.
    offer_house_rules(&ctx, &target_ids)?;

    // 8. Done — point at the obvious next steps. Harnesses read their config
    //    at startup, so an already-open session won't see what was written.
    println!("\n{} Setup complete.", "✓".green());
    println!("\n{}", "Next".bold());
    println!("  Restart or reopen your agent CLI(s) so they pick up the new config.");
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

/// Pick the profile setup should activate: an explicit `--profile` wins, a
/// single declared profile is unambiguous, and with several we offer the
/// first-declared (manifest order) rather than guessing silently — `use`
/// remains the way to switch later. `Ok(None)` means "activate nothing".
fn select_profile(ctx: &super::Context, args: &SetupArgs) -> Result<Option<String>> {
    if let Some(p) = &args.profile {
        return Ok(Some(p.clone()));
    }
    let names: Vec<&String> = ctx.loaded.manifest.profiles.keys().collect();
    match names.as_slice() {
        [] => Ok(None),
        [only] => Ok(Some((*only).clone())),
        [first, ..] => {
            println!(
                "\nThis manifest declares {} profiles: {}.",
                names.len(),
                names
                    .iter()
                    .map(|n| n.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            if crate::util::confirm::confirm(&format!(
                "Activate '{first}' now? (switch later with `agentstack use <profile> --write`)"
            ))? {
                Ok(Some((*first).clone()))
            } else {
                println!(
                    "  {} skipped — activate one later with {}",
                    "·".dimmed(),
                    "agentstack use <profile> --write".bold()
                );
                Ok(None)
            }
        }
    }
}

/// Activate `profile` (servers + skills) through the shared `use` seam — the
/// same `prepare`/`activate` pair `session start` composes. Public so the
/// integration test can drive this phase directly: `setup::run` stops at its
/// interactive confirm in a test shell, so the phase is otherwise unreachable.
pub fn materialize_profile(
    ctx: &super::Context,
    args: &SetupArgs,
    scope: Scope,
    profile: Option<&str>,
) -> Result<()> {
    let use_args = crate::cli::UseArgs {
        profile: profile.map(str::to_string),
        targets: args.targets.clone(),
        scope: Some(scope),
        write: true,
        allow_unresolved: false,
        prune_foreign: false,
        no_gitignore: false,
    };
    let libctx = ctx.library_ctx();
    let prepared = super::use_profile::prepare(ctx, &libctx, &use_args)?;
    super::use_profile::activate(ctx, &libctx, &use_args, &prepared)
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

/// Offer to install the agentstack house-rules fragment into the machine-level
/// manifest and compile it right away. Interactive-only (it's an offer), a
/// silent no-op when the fragment is already declared, and never fails setup:
/// the setup itself succeeded either way, so any error here is logged and
/// swallowed.
fn offer_house_rules(ctx: &super::Context, target_ids: &[String]) -> Result<()> {
    if let Err(err) = offer_house_rules_inner(ctx, target_ids) {
        println!(
            "  {} house-rules offer failed ({err:#}) — setup itself succeeded; retry with `agentstack init --global`.",
            "⚠".yellow()
        );
    }
    Ok(())
}

fn offer_house_rules_inner(ctx: &super::Context, target_ids: &[String]) -> Result<()> {
    if !crate::util::confirm::is_interactive() {
        return Ok(());
    }
    let home = crate::util::paths::agentstack_home();
    if let Ok(loaded) = crate::manifest::load_from_dir(&home) {
        if loaded
            .manifest
            .instructions
            .contains_key(super::init::HOUSE_RULES_NAME)
        {
            return Ok(());
        }
    }

    println!("\n{}", "House rules".bold());
    println!(
        "  agentstack ships a house-rules fragment that teaches every agent the\n\
         \x20 manifest-first workflow (never edit rendered configs, drift rules,\n\
         \x20 clean-at-rest projects). It lives in your machine manifest and compiles\n\
         \x20 into each CLI's global CLAUDE.md / AGENTS.md."
    );
    if !crate::util::confirm::confirm("  Install them?")? {
        println!(
            "  {} skipped — install later with `agentstack init --global`.",
            "·".dimmed()
        );
        return Ok(());
    }

    super::init::ensure_global_manifest()?;
    super::init::seed_house_rules(&home)?;
    let loaded = crate::manifest::load_from_dir(&home)?;

    // Consent was just given — compile the machine layer for the same targets
    // this setup configured, at global scope (the layer's home turf).
    for id in target_ids {
        let Some(desc) = ctx.registry.get(id) else {
            continue;
        };
        let Some(plan) = crate::render::instructions::plan_instructions(
            &loaded.manifest,
            desc,
            Scope::Global,
            &home,
        ) else {
            continue;
        };
        if plan.changed() {
            plan.write()?;
            println!(
                "  {} {} — wrote managed region ({})",
                "✓".green(),
                desc.display,
                plan.path.display()
            );
        } else {
            println!("  {} {} — up to date", "✓".green(), desc.display);
        }
    }
    Ok(())
}

/// The read-only preflight summary the wizard starts from.
pub(crate) struct Preflight {
    /// A structural manifest error — nothing should be written until fixed.
    pub validation_errors: bool,
    /// Referenced `${REF}`s that don't resolve on this machine.
    pub missing_secrets: Vec<String>,
}

/// Inspect adapters, skills, and secrets and print the preflight report,
/// returning a summary so the wizard can decide what to do next. Read-only —
/// touches no config. (Moved here from the retired `bootstrap` command.)
pub(crate) fn preflight(ctx: &super::Context, target_ids: &[String]) -> Result<Preflight> {
    let manifest = &ctx.loaded.manifest;
    let validation_errors = print_validation(ctx);
    print_adapters(ctx, target_ids);
    print_skills(ctx)?;
    let missing_secrets = print_secrets(manifest, &ctx.dir);
    Ok(Preflight {
        validation_errors,
        missing_secrets,
    })
}

fn print_validation(ctx: &super::Context) -> bool {
    let manifest = &ctx.loaded.manifest;
    // Library-aware, mirroring `doctor`/`apply`: a profile ref to a
    // central-library skill/server resolves here too, so it is not flagged
    // as unknown the way an inline-only view would flag it.
    let libctx = ctx.library_ctx();
    let vctx = libctx.validate_ctx(&ctx.dir);
    let target_ids: Vec<&str> = ctx.registry.ids().collect();
    let issues = validate_with_context(manifest, target_ids, &vctx);
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
