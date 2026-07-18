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

    // P1: open with the plan, so the user knows the shape of the whole run
    // before anything happens — and, crucially, that nothing is written until
    // they confirm. The plan lives here in `setup`, not in plain `init` (which
    // is the scriptable primitive).
    if interactive {
        print_plan();
    }

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
                // None → init prompts for secret storage when it lifts tokens
                // and the shell is interactive (P2); setup is interactive.
                secrets: None,
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

    // P7: remember which apply-history entries already existed, so the closing
    // summary can show exactly the files *this run* wrote — reusing the same
    // write ledger `restore` reads, not inventing new tracking.
    let history_before: std::collections::HashSet<String> =
        crate::history::list().into_iter().map(|e| e.id).collect();

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
    // P8: offer the deep content scan at the one moment it's relevant — right
    // after skills landed. Only when there ARE skills (no empty questions), and
    // only interactively (a non-interactive setup never reaches this write path
    // at all, so its preview-only contract is untouched).
    let deep = offer_deep_scan(&ctx)?;
    super::doctor::run(
        &DoctorArgs {
            ci: false,
            live: false,
            fix: false,
            deep,
            all: false,
            json: false,
        },
        manifest_dir,
    )?;

    // 7. Machine layer: offer the agentstack house rules once. They live in
    //    the personal manifest (~/.agentstack), not this project, so every
    //    repo's agents get them via the global CLAUDE.md / AGENTS.md.
    let seeded_house_rules = offer_house_rules(&ctx, &target_ids)?;

    // 7b. P4: the modes are a visible choice, presented now that the user has
    //     seen the setup actually work. v1 prints the command each non-default
    //     mode maps to rather than switching in place — the wizard stays
    //     reversible, and the user runs the switch when they mean to.
    offer_mode_choice(&ctx, &target_ids)?;

    // 8. P7: the transparency close — what this run did to the machine, where
    //    secrets went, what was seeded, and the one-liners to undo or re-inspect.
    print_change_summary(&ctx, &history_before, seeded_house_rules);
    Ok(())
}

/// P1: the opening plan. Four numbered steps and the promise that nothing is
/// written until the user confirms — so a "machine setup" tool never surprises
/// them. Printed only in an interactive `setup`.
fn print_plan() {
    println!("\n{}", "Setup will:".bold());
    println!("  1. detect the agent CLIs on this machine");
    println!("  2. import their existing configs");
    println!(
        "  3. lift any inline tokens to {} placeholders",
        "${REF}".bold()
    );
    println!("  4. write one agentstack manifest");
    println!(
        "\n{} Nothing is written until you confirm. Your CLIs are not touched yet.",
        "·".dimmed()
    );
}

/// P8: ask whether to run the deep content scan, with the help line the
/// maintainer decided. Returns `false` (no deep scan) when the project has no
/// skills — there's nothing to scan, so we don't ask — or in a non-interactive
/// shell. The scan reads every skill/instruction body for hidden Unicode and
/// prompt-injection tricks; it's slow on big libraries, hence a choice.
fn offer_deep_scan(ctx: &super::Context) -> Result<bool> {
    if ctx.loaded.manifest.skills.is_empty() || !crate::util::confirm::is_interactive() {
        return Ok(false);
    }
    println!(
        "  {} reads every skill and instruction body for hidden Unicode and\n\
         \x20   prompt-injection tricks; slow on big libraries; re-run anytime\n\
         \x20   with {}.",
        "·".dimmed(),
        "agentstack doctor --deep".bold()
    );
    Ok(crate::util::confirm::confirm(
        "  Run a deep content scan now?",
    )?)
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
/// Returns whether the house-rules fragment was seeded this run, so the P7
/// close can list it under "what got seeded".
fn offer_house_rules(ctx: &super::Context, target_ids: &[String]) -> Result<bool> {
    match offer_house_rules_inner(ctx, target_ids) {
        Ok(seeded) => Ok(seeded),
        Err(err) => {
            println!(
                "  {} house-rules offer failed ({err:#}) — setup itself succeeded; retry with `agentstack init --global`.",
                "⚠".yellow()
            );
            Ok(false)
        }
    }
}

fn offer_house_rules_inner(ctx: &super::Context, target_ids: &[String]) -> Result<bool> {
    if !crate::util::confirm::is_interactive() {
        return Ok(false);
    }
    let home = crate::util::paths::agentstack_home();
    if let Ok(loaded) = crate::manifest::load_from_dir(&home) {
        if loaded
            .manifest
            .instructions
            .contains_key(super::init::HOUSE_RULES_NAME)
        {
            return Ok(false);
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
        return Ok(false);
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
    Ok(true)
}

/// P4: the commands a non-default mode maps to (v1 prints, never executes), plus
/// one sentence on what running them does. Static returns the maintenance
/// command; the other two return the switch sequence. Pure so the mapping is
/// unit-testable. `profile` fills the `session start` argument (falling back to
/// a placeholder when the manifest declares none).
fn mode_switch_plan(
    mode: super::overview::Mode,
    profile: Option<&str>,
) -> (Vec<String>, &'static str) {
    use super::overview::Mode;
    let p = profile.unwrap_or("<profile>");
    match mode {
        Mode::Static => (
            vec!["agentstack apply --write".into()],
            "Keep rendering configs to disk; re-run after any manifest change.",
        ),
        Mode::CleanAtRest => (
            vec![
                format!("agentstack session start {p}"),
                "agentstack session end".into(),
            ],
            "Materialize your profile for a session, then revert it so the repo stays clean.",
        ),
        Mode::ZeroFiles => (
            vec![
                "agentstack gateway connect --all".into(),
                "agentstack trust .".into(),
            ],
            "Register the gateway once per harness, then trust this repo so it serves capabilities live.",
        ),
    }
}

/// P4: present the three delivery modes as an explicit choice, help text and
/// all, once the user has seen the setup work. The current mode is preselected
/// (bare Enter keeps it). Choosing another mode PRINTS the command(s) it maps to
/// — v1 never switches in place, keeping the wizard reversible. Interactive
/// only; a no-op otherwise.
fn offer_mode_choice(ctx: &super::Context, target_ids: &[String]) -> Result<()> {
    if !crate::util::confirm::is_interactive() {
        return Ok(());
    }
    use super::overview::Mode;
    let current = super::overview::detect_mode(ctx, target_ids);
    let modes = [Mode::Static, Mode::CleanAtRest, Mode::ZeroFiles];

    println!("\n{}", "Delivery mode".bold());
    for (i, m) in modes.iter().enumerate() {
        let marker = if *m == current { " (current)" } else { "" };
        println!("  {}. {}{}", i + 1, m.label().bold(), marker.dimmed());
        println!("     {}", m.help().dimmed());
    }

    let choice = prompt_choice("  Pick a mode", modes.len(), &current, &modes)?;
    if choice == current {
        println!("  {} staying on {}.", "·".dimmed(), current.label());
        return Ok(());
    }

    let profile = ctx
        .loaded
        .manifest
        .profiles
        .keys()
        .next()
        .map(String::as_str);
    let (cmds, what) = mode_switch_plan(choice, profile);
    println!("\n  To move to {} — run:", choice.label().bold());
    for c in &cmds {
        println!("    {}", c.bold());
    }
    println!("  {} {what}", "·".dimmed());
    Ok(())
}

/// Read a 1-based menu choice, defaulting to `current` on empty/invalid input.
/// Its own tiny stdin read (the shared `confirm` helper is yes/no only); reached
/// only interactively, so `cargo test` never blocks here.
fn prompt_choice(
    prompt: &str,
    n: usize,
    current: &super::overview::Mode,
    modes: &[super::overview::Mode],
) -> Result<super::overview::Mode> {
    use std::io::Write;
    print!("{prompt} [1-{n}, Enter = current] ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    match line.trim().parse::<usize>() {
        Ok(i) if i >= 1 && i <= n => Ok(modes[i - 1]),
        _ => Ok(*current),
    }
}

/// P7: the transparency close. Gathers what THIS run changed — every file
/// written (from the apply-history entries new since `history_before`), where
/// each referenced secret resolves now, and what was seeded — then prints it
/// with the undo + inspect one-liners.
fn print_change_summary(
    ctx: &super::Context,
    history_before: &std::collections::HashSet<String>,
    seeded_house_rules: bool,
) {
    // Files: new history entries hold the pre-write snapshot of each touched
    // file; we only want the paths + labels, deduped by path (an apply and a
    // profile activation can touch the same file).
    let mut files: Vec<(String, String)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for entry in crate::history::list() {
        if history_before.contains(&entry.id) {
            continue;
        }
        for f in entry.files {
            if seen.insert(f.path.clone()) {
                files.push((f.path, f.label));
            }
        }
    }

    // Secrets: re-derive where each referenced ref resolves now (the resolver is
    // the source of truth; we never stored a value to echo).
    let sources = SecretSources::detect(&ctx.dir);
    let secrets: Vec<(String, String)> = ctx
        .loaded
        .manifest
        .referenced_secrets()
        .into_iter()
        .filter_map(|name| sources.source_of(&name).map(|s| (name, s.to_string())))
        .collect();

    let mut seeded: Vec<String> = Vec::new();
    if seeded_house_rules {
        let path = crate::util::paths::agentstack_home().join(MANIFEST_FILE);
        seeded.push(format!(
            "agentstack house rules → {} (edit under [instructions])",
            path.display()
        ));
    }

    println!("\n{} Setup complete.", "✓".green());
    println!("\n{}", "What changed on this machine".bold());
    print!("{}", render_change_summary(&files, &secrets, &seeded));

    // Harnesses read config at startup, so an open session won't see the writes.
    println!("\n  Restart your agent CLI(s) so they pick up the new config.");
}

/// Pure formatter for the P7 close body (files / secrets / seeded / one-liners),
/// so the transparency block is unit-testable without a live setup run. Sections
/// with nothing to show are omitted, except the always-present undo/inspect
/// one-liners.
fn render_change_summary(
    files: &[(String, String)],
    secrets: &[(String, String)],
    seeded: &[String],
) -> String {
    let mut out = String::new();
    if files.is_empty() {
        out.push_str("  No files were written.\n");
    } else {
        out.push_str(&format!("  Files written ({}):\n", files.len()));
        for (path, label) in files {
            out.push_str(&format!("    {path}  ({label})\n"));
        }
    }
    if !secrets.is_empty() {
        out.push_str("  Secrets:\n");
        for (name, source) in secrets {
            out.push_str(&format!("    {name}  resolved from {source}\n"));
        }
    }
    if !seeded.is_empty() {
        out.push_str("  Seeded:\n");
        for s in seeded {
            out.push_str(&format!("    {s}\n"));
        }
    }
    out.push_str("  Undo everything this run wrote:  agentstack restore --last --write\n");
    out.push_str("  Inspect any time:  agentstack doctor  ·  agentstack\n");
    out
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

#[cfg(test)]
mod tests {
    use super::super::overview::Mode;
    use super::{mode_switch_plan, render_change_summary};

    // P4: choosing a non-default mode prints a command sequence, never runs it.
    // The clean-at-rest plan threads the profile name into `session start`.
    #[test]
    fn mode_switch_plan_maps_each_mode_to_its_commands() {
        let (cmds, _) = mode_switch_plan(Mode::Static, Some("dev"));
        assert_eq!(cmds, vec!["agentstack apply --write".to_string()]);

        let (cmds, _) = mode_switch_plan(Mode::CleanAtRest, Some("dev"));
        assert_eq!(cmds[0], "agentstack session start dev");
        assert_eq!(cmds[1], "agentstack session end");

        // No profile declared → a visible placeholder, not a panic.
        let (cmds, _) = mode_switch_plan(Mode::CleanAtRest, None);
        assert_eq!(cmds[0], "agentstack session start <profile>");

        let (cmds, _) = mode_switch_plan(Mode::ZeroFiles, None);
        assert_eq!(cmds[0], "agentstack gateway connect --all");
        assert_eq!(cmds[1], "agentstack trust .");
    }

    // P7: the transparency close lists every written file, names each secret's
    // source, shows what was seeded, and always offers the undo + inspect
    // one-liners.
    #[test]
    fn change_summary_reports_files_secrets_seeded_and_undo() {
        let files = vec![
            (
                "~/.claude.json".to_string(),
                "Claude Code · servers".to_string(),
            ),
            ("~/.claude/skills/helper".to_string(), "skills".to_string()),
        ];
        let secrets = vec![("API_TOKEN".to_string(), "keychain".to_string())];
        let seeded = vec!["agentstack house rules → ~/.agentstack/agentstack.toml".to_string()];
        let out = render_change_summary(&files, &secrets, &seeded);

        assert!(out.contains("Files written (2)"));
        assert!(out.contains("~/.claude.json  (Claude Code · servers)"));
        assert!(out.contains("API_TOKEN  resolved from keychain"));
        assert!(out.contains("house rules"));
        assert!(out.contains("agentstack restore --last --write"));
        assert!(out.contains("agentstack doctor"));
    }

    // With nothing written, the summary says so but still offers the one-liners.
    #[test]
    fn change_summary_with_no_writes_still_offers_undo() {
        let out = render_change_summary(&[], &[], &[]);
        assert!(out.contains("No files were written"));
        assert!(out.contains("agentstack restore --last --write"));
    }
}
