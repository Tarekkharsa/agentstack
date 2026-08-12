//! `agentstack setup` (hidden alias) + interactive `agentstack init` — the one-command newcomer path (P27).
//!
//! Pure orchestration over the everyday commands: `init` (only if there's no
//! manifest yet), a read-only preflight, inline secret prompts, an `apply`
//! preview, a single confirm, then `install` + `apply --write` + profile
//! activation (skills) + `doctor`. It introduces no rendering or validation
//! logic of its own, and it reuses the shared confirm helper so a
//! non-interactive shell (CI, pipes) only ever previews — it never writes and
//! never blocks on input.

use std::fs;
use std::path::Path;

use agentstack_core::paint::OwoColorize;
use anyhow::{Context, Result};

use crate::cli::{ApplyArgs, ConnectArgs, DoctorArgs, InitArgs, InstallArgs, LockArgs, SetupArgs};
use crate::lock::Lock;
use crate::manifest::load::MANIFEST_FILE;
use crate::manifest::validate_with_context;
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
    // Every phase that records a write during this wizard belongs to one undo
    // batch. `restore --last` reverses the batch newest-to-oldest.
    let _history_batch = crate::history::begin_batch("setup");
    // On Unix, keep Ctrl-C from terminating between the import write and the
    // recovery summary. The guard restores the process's prior handler on exit.
    let sigint = interactive
        .then(crate::sys::SigintGuard::install)
        .transpose()?;

    // P1: open with the plan, so the user knows the shape of the whole run
    // before anything happens — and, crucially, what the import step writes and
    // that CLIs stay untouched until a later confirm. The plan lives here in
    // `setup`, not in plain `init` (which is the scriptable primitive).
    // The five-step plan is orientation, not consent — the consent question
    // states its own promise a few lines later, and stating the same promise
    // twice is how it stops being read. `--verbose` keeps the long form.
    if interactive && args.verbose {
        print_plan();
    }

    // P30/P7: snapshot the write ledger at the very TOP — before the import can
    // write anything — so both the closing summary and any cancel mini-summary
    // reflect EVERY file this run wrote, init's manifest/.env/.gitignore
    // included. (It used to be snapshotted after the import, which hid init's
    // writes from the summary and made "No files were written" a lie.)
    let history_before: std::collections::HashSet<String> =
        crate::history::list().into_iter().map(|e| e.id).collect();

    let mut manifest_path = crate::manifest::resolve_manifest_dir(&base).join(MANIFEST_FILE);
    let mut imported = false;
    if !manifest_path.exists() {
        if !interactive {
            println!(
                "\n{} `agentstack init` is an interactive wizard and will not write in this shell.",
                "→".cyan()
            );
            println!("  For scripts/CI, use:");
            println!("    agentstack init");
            // The bridge comes first: skills and MCP servers travel the live
            // lane by default, so this is the step that makes a scripted setup
            // actually deliver anything. `apply` writes the rendered lane only
            // — house rules, settings, hooks — and a project with none of them
            // has nothing for it to do.
            println!("    agentstack x gateway connect --all --write   # serve what routes live");
            println!("    agentstack apply --write           # write the rendered lane, if any");
            println!("    agentstack use <toolset> --write   # if the manifest has skills");
            return Ok(());
        }
        println!("\nNo manifest here yet — importing the setup already on this machine.");
        println!();
        // P30/Stage 1.2: review first, then ONE explicit gate. The importer
        // prints what detection found — CLIs and their config files, servers
        // by name, lifted secret references, destination files — and asks its
        // confirm AFTER that evidence, before any write. Everything further
        // downstream still has its own confirm.
        let proceeded = super::init::run_for_setup(
            &InitArgs {
                global: false,
                force: false,
                dry_run: false,
                plan: false,
                // None → init prompts for secret storage when it lifts tokens
                // and the shell is interactive (P2); setup is interactive.
                secrets: None,
                no_keychain: false,
                // Carried from the invocation, not hardcoded: a bare
                // `agentstack init` routes here, so hardcoding `false` made
                // `--project-servers` a flag that parsed and then did nothing.
                project_servers: args.project_servers,
                // Same reasoning: the flag shapes the import, and the wizard
                // performs the import.
                include_tool_managed: args.include_tool_managed,
                // The wizard's write gate lives inside `run_for_setup` (which
                // never re-checks the TTY gate), so this field is irrelevant
                // here.
                yes: false,
                consented: None,
                // Never here: the wizard registers the bridge in its own
                // ceremony, after the delivery routing is on screen. Setting
                // it would also make the import confirm's promise ("your CLIs'
                // own configs stay untouched") false at the moment it is read.
                connect: false,
                // Carried from the invocation: the wizard's import IS this
                // import, so `--verbose` there has to reach the evidence
                // blocks here — including the passed-over bridge line.
                verbose: args.verbose,
            },
            manifest_dir,
        )?;
        if !proceeded {
            println!("\n{} Re-run when you're ready to import.", "·".dimmed());
            return Ok(());
        }
        imported = true;
        manifest_path = crate::manifest::resolve_manifest_dir(&base).join(MANIFEST_FILE);
    }
    // `init` may have created `.agentstack/`, so re-resolve before loading.
    if !manifest_path.exists() {
        println!(
            "\n{} Nothing to set up yet. Add a capability, then re-run {}:",
            "→".cyan(),
            "agentstack init".bold()
        );
        println!("    agentstack search <term>        find a server or skill");
        println!("    agentstack add server <name> …  add one you already know");
        return Ok(());
    }

    // 2. Everything past the import can early-stop (a declined confirm, a
    //    validation stop) or hard-cancel (Esc at the mode fork). Route the whole
    //    remainder through `configure`, which closes clean stops with a truthful
    //    mini-summary of what the import already wrote; the outer arm below adds
    //    the same mini-summary on a hard error so a stranded import is never
    //    silent (P30). A run that started from an existing manifest never
    //    imported, so its ledger diff is empty and the mini-summary is a no-op.
    match configure(args, manifest_dir, &history_before) {
        Ok(()) => Ok(()),
        Err(err) => {
            if sigint.as_ref().is_some_and(|guard| guard.interrupted()) {
                println!("\n{} Setup canceled.", "·".dimmed());
                if imported {
                    print_stop_summary(&history_before);
                }
                return Ok(());
            }
            if imported {
                print_stop_summary(&history_before);
            }
            Err(err)
        }
    }
}

/// The post-import remainder of the wizard: load, preflight, secrets, the P28
/// delivery-mode fork, then the machine layer + P7 close. Split from `run` so
/// every early stop routes through one truthful mini-summary of what the import
/// already wrote (P30). Returns `Ok(())` on a clean completion OR a clean stop
/// (its own summary already printed); `Err` only propagates a genuine failure,
/// which the caller also closes with the mini-summary.
fn configure(
    args: &SetupArgs,
    manifest_dir: Option<&Path>,
    history_before: &std::collections::HashSet<String>,
) -> Result<()> {
    let interactive = crate::util::confirm::is_interactive();
    let ctx = super::load(manifest_dir)?;
    // Default scope follows the manifest's home: project for a repo manifest,
    // global only for the machine manifest.
    let scope = args.scope.unwrap_or_else(|| Scope::default_for(&ctx.dir));
    let target_ids = resolve_targets(&ctx.loaded.manifest, &ctx.registry, &args.targets, &ctx.dir)?;

    // Preflight inspection (adapters, skills, secrets) — read-only.
    let pf = preflight(&ctx, &target_ids, args.verbose)?;

    // Missing secrets — offer to set each one now (interactive only).
    let missing = resolve_missing_secrets(&ctx, pf.missing_secrets)?;

    // Blocking issues stop before the fork writes anything further — but the
    // import above may already have landed, so close with the truthful summary.
    if pf.validation_errors {
        println!(
            "\n{} Fix the manifest validation errors above, then re-run {}.",
            "→".cyan(),
            "agentstack init".bold()
        );
        print_stop_summary(history_before);
        return Ok(());
    }
    if !missing.is_empty() {
        println!(
            "\n{} Still missing {}. Set them, then re-run {}:",
            "→".cyan(),
            missing.join(", "),
            "agentstack init".bold()
        );
        for name in &missing {
            println!("    agentstack secret set {name}");
        }
        print_stop_summary(history_before);
        return Ok(());
    }

    // W4 (the flip, 2026-08-03): delivery is **Automatic** by default — the
    // planner routes each capability by kind and harness, and the wizard states
    // what it decided instead of asking. The one override, Render locally,
    // lives behind "more control", and so do the older per-project modes, which
    // the shipped `agentstack set-mode` still switches unchanged.
    //
    // A project that has ALREADY rendered keeps its rendered path: files on
    // disk are a fact, and a fork that quietly stopped maintaining them would
    // leave stale capabilities behind a screen claiming everything is served
    // live. Un-rendering stays the explicit act it has always been.
    let already_rendered = super::overview::has_rendered_artifacts(&ctx, &target_ids);
    let choice = match choose_delivery(already_rendered)? {
        Some(c) => c,
        None => {
            // Esc/q is an explicit cancellation. Ctrl-C interrupts the terminal
            // read and is handled by `run`'s scoped SIGINT guard instead.
            println!("\n{} Setup canceled.", "·".dimmed());
            print_stop_summary(history_before);
            return Ok(());
        }
    };
    if interactive {
        // A one-line plan of exactly what this fork will do next, straight from
        // the same pure mapping the test pins.
        println!(
            "  {} {}",
            "→".cyan(),
            fork_plan(choice).join(" · ").dimmed()
        );
    }

    let proceeded = match choice {
        DeliveryChoice::Automatic => {
            run_automatic(&ctx, &target_ids, manifest_dir, args.connect, args.verbose)?;
            true
        }
        // Render locally is recorded first, so the render that follows is the
        // project's standing answer rather than a one-off.
        DeliveryChoice::RenderLocally => {
            record_render_locally(manifest_dir)?;
            run_static(args, scope, manifest_dir)?
        }
        DeliveryChoice::Legacy(super::overview::Mode::Static) => {
            run_static(args, scope, manifest_dir)?
        }
        DeliveryChoice::Legacy(super::overview::Mode::CleanAtRest) => {
            run_clean_at_rest(&ctx, manifest_dir, args.verbose)?;
            true
        }
        DeliveryChoice::Legacy(super::overview::Mode::ZeroFiles) => {
            run_zero_files(&ctx, manifest_dir, args.connect, args.verbose)?;
            true
        }
    };
    // The static fork returns false only when its write confirm was declined.
    // No CLI config was written, but the import above may have been — so close
    // with the truthful mini-summary (a no-op when the ledger diff is empty).
    if !proceeded {
        print_stop_summary(history_before);
        return Ok(());
    }

    // Machine layer + the P7 transparency close are common to every mode.
    // Reload so a static apply's manifest refresh (owned-server tables) is
    // reflected in the summary; a no-render fork reloads an unchanged manifest.
    let ctx = super::load(manifest_dir)?;
    // Step 3 of the adoption ladder: ONE optional machine-wide step (guard +
    // house rules together) after the project itself is done — not two
    // sequential upsells inside every project init (audit C6).
    let (guard_wired, seeded_house_rules) = offer_machine_protection(&ctx, &target_ids)?;
    print_change_summary(
        &ctx,
        history_before,
        seeded_house_rules,
        guard_wired,
        args.verbose,
    );
    Ok(())
}

/// The static fork: the original render path — preview, confirm, install,
/// apply, activate skills, doctor. Returns `false` when the user declines the
/// write confirm (so the caller skips the machine-change summary), `true`
/// once the write path has run.
/// What the wizard's apply step should do once the user has read the preview.
enum ApplyChoice {
    Apply,
    /// Apply, and first record `[meta] gitignore = false` so this project never
    /// manages the block — here or on any later activation.
    ApplyWithoutGitignore,
    Stop,
}

/// The apply confirm.
///
/// A plain yes/no unless this run would actually touch `.gitignore`, in which
/// case the opt-out becomes a third answer to the SAME question rather than a
/// second question. The interaction count never grows, and the boundary is
/// explained only when it is real — a project that isn't a git repo, or has
/// already opted out, sees exactly the prompt it saw before.
fn confirm_apply(gitignore_pending: bool) -> Result<ApplyChoice> {
    if !gitignore_pending {
        return Ok(if crate::util::confirm::confirm("\nApply this setup?")? {
            ApplyChoice::Apply
        } else {
            ApplyChoice::Stop
        });
    }
    // Non-interactive keeps the old contract exactly: stop without blocking.
    // Scripts opt out with `--no-gitignore`, which needs no terminal.
    if !crate::util::confirm::is_interactive() {
        return Ok(ApplyChoice::Stop);
    }
    // The middle label states what it records, because "never" is a claim
    // about every future run and the user is agreeing to it here.
    let items = [
        "Apply".to_string(),
        "Apply, but never manage .gitignore in this project \
         (records `gitignore = false` in agentstack.toml)"
            .to_string(),
        "Cancel".to_string(),
    ];
    let idx = dialoguer::Select::with_theme(&dialoguer::theme::ColorfulTheme::default())
        .with_prompt("\nApply this setup?")
        .items(&items)
        .default(0)
        .interact_opt()?;
    Ok(match idx {
        Some(0) => ApplyChoice::Apply,
        Some(1) => ApplyChoice::ApplyWithoutGitignore,
        // Esc/q, like Cancel.
        _ => ApplyChoice::Stop,
    })
}

/// Record the durable opt-out, and clear any block already on disk.
///
/// Removal belongs here and nowhere else. Routine commands must never strip
/// the block (a team may have committed it — see `gitignore::remove_block`),
/// but this is the moment a human said this project commits its generated
/// files; leaving the block would keep every not-yet-tracked artifact
/// invisible to `git status` immediately after that declaration. One history
/// entry covers both files, so Undo restores the pair together.
fn record_gitignore_optout(manifest_dir: Option<&Path>) -> Result<()> {
    let ctx = super::load(manifest_dir)?;
    let path = ctx.loaded.manifest_path.clone();
    let original =
        fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let updated = super::add::set_meta_gitignore(&original, false)?;

    let mut backups = Vec::new();
    if updated != original {
        // `[meta] gitignore` says whether agentstack manages this project's
        // .gitignore block. It declares nothing and executes nothing, so valid
        // trust is carried across the write (`crate::trust_carry::TrustCarry`).
        let carry = crate::trust_carry::TrustCarry::before_write(&ctx.dir);
        backups.push(crate::history::capture(
            &path,
            "agentstack.toml · gitignore opt-out",
        ));
        crate::util::atomic::write(&path, &updated)
            .with_context(|| format!("writing {}", path.display()))?;
        carry.across_write(&path, &updated)?;
        println!(
            "  {} recorded {} — no command will manage this project's .gitignore",
            "✓".green(),
            "[meta] gitignore = false".bold()
        );
    }

    let root = crate::manifest::project_root_of(&ctx.dir);
    let gitignore = root.join(".gitignore");
    if let Ok(existing) = fs::read_to_string(&gitignore) {
        if let Some(without) = crate::render::gitignore::remove_block(&existing) {
            backups.push(crate::history::capture(
                &gitignore,
                ".gitignore · managed block removed",
            ));
            crate::util::atomic::write(&gitignore, &without)
                .with_context(|| format!("writing {}", gitignore.display()))?;
            println!(
                "  {} removed the managed block from .gitignore",
                "✓".green()
            );
        }
    }

    if !backups.is_empty() {
        let _ = crate::history::record(
            "project",
            "init (gitignore opt-out)".to_string(),
            vec![],
            backups,
        );
    }
    Ok(())
}

fn run_static(args: &SetupArgs, scope: Scope, manifest_dir: Option<&Path>) -> Result<bool> {
    // Preview the exact config changes (no "re-run with --write" hint — we
    // drive our own confirm next).
    println!("\n{}", "Preview".bold());
    let preview = super::apply::preview(&apply_args(args, scope, false), manifest_dir)?;
    if preview.validation_errors || preview.write_blockers > 0 {
        println!(
            "\n{} Resolve the issues above, then re-run {}.",
            "→".cyan(),
            "agentstack init".bold()
        );
        return Ok(false);
    }

    // Nothing to confirm when nothing would change (audit C6: "confirm apply
    // even when 0 target(s) would change") — say so and carry on to the
    // skills/machine steps, which may still have work.
    if preview.changed_count == 0 {
        println!(
            "\n{} Configs already match the manifest — nothing to apply.",
            "·".dimmed()
        );
    } else {
        // `confirm` returns false without blocking when there's no terminal, so
        // CI/pipes stop here. Note the honest scope: no CLI config was written here,
        // but the wizard's import step may already have (the caller closes with the
        // truthful mini-summary), so this line no longer claims "nothing written".
        match confirm_apply(preview.gitignore_pending)? {
            ApplyChoice::Apply => {}
            ApplyChoice::ApplyWithoutGitignore => {
                // BEFORE the write path, deliberately: the apply below reloads
                // the manifest, so recording the opt-out first means its
                // derivation and every later activation's read the same file.
                // A wizard-local flag would have been forgotten by the next
                // `use <toolset>`.
                record_gitignore_optout(manifest_dir)?;
            }
            ApplyChoice::Stop => {
                println!(
                    "\n{} Stopped before writing any CLI config. Re-run in a terminal to apply, or use {}.",
                    "·".dimmed(),
                    "agentstack apply --write".bold()
                );
                return Ok(false);
            }
        }
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

    // Skills — `apply` renders servers/instructions/hooks/settings but never
    // skills; they activate through a profile. Finish the job here via the same
    // prepare/activate seam `use` and `session start` share, so the first agent
    // session actually has the manifest's skills. Reload first: the apply pass
    // above may have refreshed owned-server tables in the manifest on disk.
    let ctx = super::load(manifest_dir)?;
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
            // Configs are already written at this point — surface the problem
            // and the exact recovery command instead of failing the whole setup
            // on its last step.
            println!(
                "  {} could not activate toolset '{label}' ({err:#})",
                "⚠".yellow()
            );
            println!("  Fix the issue, then run: {}", cmd.bold());
        }
    }

    println!("\n{}", "Doctor".bold());
    // P8: offer the deep content scan at the one moment it's relevant — right
    // after skills landed. Only when there ARE skills, and only interactively.
    let deep = offer_deep_scan(&ctx)?;
    run_doctor_step(
        &DoctorArgs {
            ci: false,
            live: false,
            probe: false,
            fix: false,
            deep,
            all: false,
            json: false,
            skip_drift: false,
        },
        manifest_dir,
        args.verbose,
    )?;
    Ok(true)
}

/// The clean-at-rest fork: pin the lock (no render), teach the session rhythm,
/// then a drift-suppressed doctor. Nothing lands in any CLI config — the repo
/// stays pristine for git and capabilities exist only inside a session.
fn run_clean_at_rest(
    ctx: &super::Context,
    manifest_dir: Option<&Path>,
    verbose: bool,
) -> Result<()> {
    use super::overview::Mode;

    println!("\n{}", "Lock".bold());
    // Reuse the `lock` command as a library call: it pins every profile's refs
    // (library-aware) without materializing anything, and prints its own P9
    // trust re-gate warning if this project was already trusted.
    super::lock::run(
        &LockArgs {
            profile: None,
            update: None,
            upgrade: None,
            all: false,
            with_instructions: false,
            yes: false,
            write: false,
            quiet: false,
        },
        manifest_dir,
    )?;

    // Teach the two-command rhythm, threading the manifest's first profile into
    // `session start` (falls back to a placeholder). Reuses the pure
    // `mode_switch_plan` mapping so the wording has one source of truth.
    let profile = ctx
        .loaded
        .manifest
        .profiles
        .keys()
        .next()
        .map(String::as_str);
    let (cmds, what) = mode_switch_plan(Mode::CleanAtRest, profile);
    println!(
        "\n  {} capabilities exist only during a session — the repo stays clean for git:",
        "·".dimmed()
    );
    for c in &cmds {
        println!("    {}", c.bold());
    }
    println!("  {} {what}", "·".dimmed());

    // The un-render leg: files this project already rendered would keep the
    // derived mode reading "static" — and keep serving stale capabilities —
    // however faithfully the session rhythm is followed. The switch command
    // previews exactly what comes off disk and confirms; the wizard only
    // points at it (a removal is consented in its own review, not inside a
    // setup run that was about importing).
    print_switch_pointer(ctx, Mode::CleanAtRest);

    println!("\n{}", "Doctor".bold());
    // skip_drift: nothing is rendered here on purpose, so the "N change(s)
    // pending ↳ apply --write" comparison would be a false alarm pointing back
    // at the render this mode opts out of.
    run_doctor_step(
        &DoctorArgs {
            ci: false,
            live: false,
            probe: false,
            fix: false,
            deep: false,
            all: false,
            json: false,
            skip_drift: true,
        },
        manifest_dir,
        verbose,
    )?;
    Ok(())
}

/// The zero-files fork: nothing is rendered. Offer to register the gateway in
/// every installed harness (one small entry each), then point at `trust` —
/// which we NEVER run for the user: trust is human consent (principle 3), so
/// the wizard only ever prints the command. If this project already rendered
/// files, the fork also points at `set-mode zero-files`, the switch that
/// removes them — without that leg the derived mode keeps reading "static"
/// however completely the gateway is wired.
fn run_zero_files(
    ctx: &super::Context,
    manifest_dir: Option<&Path>,
    // `agentstack init --connect`: the offer below is already answered. This
    // legacy fork honours it for the same reason the automatic one does — a
    // flag that works on one route and is silently ignored on another is worse
    // than no flag.
    preconsented: bool,
    verbose: bool,
) -> Result<()> {
    use super::overview::Mode;
    let _ = manifest_dir; // the ctx already carries the resolved dir

    println!("\n{}", "Zero-files".bold());
    // Honesty rule: never a bare "nothing is written". The project keeps its
    // manifest and lock, and any house-rules region stays in its file — which
    // is exactly what `ZERO_ARTIFACTS` spells out, so the default states the
    // rule and `--verbose` states its limits.
    if verbose {
        println!(
            "  {} no generated files are written; your CLIs fetch servers and skills\n\
             \x20   live from agentstack — each repo stays inert until you review it.\n\
             \x20   {}",
            "·".dimmed(),
            crate::delivery::ZERO_ARTIFACTS
        );
    } else {
        println!(
            "  {} no generated files; your CLIs fetch servers and skills live from \
             agentstack   (what stays behind: --verbose)",
            "·".dimmed()
        );
    }

    // cmds[0] = "agentstack trust .", cmds[1] = "agentstack set-mode zero-files"
    let (cmds, what) = mode_switch_plan(Mode::ZeroFiles, None);
    const CONNECT_LATER: &str = "agentstack x gateway connect --all --write";

    if preconsented {
        println!(
            "\n  {} registering the gateway now — you asked for it with {}.",
            "·".dimmed(),
            "--connect".bold()
        );
    }
    let register = preconsented
        || (crate::util::confirm::is_interactive()
            && crate::util::confirm::confirm(
                "\n  Register the agentstack gateway in your installed harnesses now?",
            )?);
    if register {
        // Reuse the `gateway connect` code path as a library call.
        register_now(CONNECT_LATER)?;
    } else {
        println!("  {} register it later with:", "·".dimmed());
        println!("    {}", CONNECT_LATER.bold());
    }

    println!(
        "\n  {} then trust this repo so the gateway will serve its capabilities:",
        "·".dimmed()
    );
    println!("    {}", cmds[0].bold());
    println!("  {} {what}", "·".dimmed());

    print_switch_pointer(ctx, Mode::ZeroFiles);
    Ok(())
}

/// Point at the `set-mode` switch when this project still has rendered files —
/// the un-render leg that makes a nothing-at-rest choice real. Printed, never
/// executed: the switch removes files, and that removal is consented in
/// `set-mode`'s own review (the same reason the wizard never runs `trust`).
fn print_switch_pointer(ctx: &super::Context, mode: super::overview::Mode) {
    let all_ids: Vec<String> = ctx.registry.ids().map(str::to_string).collect();
    if !super::overview::has_rendered_artifacts(ctx, &all_ids) {
        return;
    }
    println!(
        "\n  {} this project still has rendered config on disk, so it will keep\n\
         \x20   reading as \"static\" until that comes off. Complete the {} switch with:",
        "·".dimmed(),
        mode.label()
    );
    println!("    {}", "agentstack x uninstall".bold());
    println!(
        "  {} it previews every removal first, and every FILE it removes undoes with \
         `agentstack x restore --last`.",
        "·".dimmed()
    );
    print_skills_are_not_in_that_undo(&ctx.dir);
}

/// Bound the `x restore` the pointer above names, when this project has skills
/// materialized on disk for it to be wrong about.
///
/// The pointer sends a reader to `x uninstall`, whose skills leg is
/// `capture: false` ([`super::unrender::Removal::capture`]) — so "undoes with
/// `agentstack x restore --last`" offered an undo that brings the configs back
/// and silently leaves the skills off. This is the same defect
/// [`super::uninstall`] repaired in its own closing copy, said by a different
/// command, so it carries the same shared sentence.
///
/// Reads [`super::undo::skills_outside_the_ledger`] rather than counting
/// again: the wizard must not invent a second answer to "which skills are
/// outside the ledger here".
///
/// [`crate::history::SKILLS_COME_OFF_WITH`] does not fit and is deliberately
/// not reused — it names `x uninstall --write`, which is the command this
/// pointer just told the reader to run. What is missing here is the way BACK,
/// so that is what the second line names. The reason sentence is shared,
/// because that is the half that must never drift between Undo surfaces.
///
/// Conditional, like every other Undo surface: a project that materialized no
/// skills prints nothing, so this stays a fact about this project rather than a
/// caveat every `setup` run teaches people to skip.
fn print_skills_are_not_in_that_undo(dir: &std::path::Path) {
    let names = super::undo::skills_outside_the_ledger(dir);
    if names.is_empty() {
        return;
    }
    println!(
        "  {} {} ({})",
        "·".dimmed(),
        crate::history::SKILLS_ARE_NOT_RECORDED.dimmed(),
        names
            .iter()
            .map(|n| crate::text::sanitize_line(n))
            .collect::<Vec<_>>()
            .join(", ")
            .dimmed()
    );
    println!(
        "  {} {}",
        "·".dimmed(),
        "so that restore puts the files back, not these — re-materialize them by \
         activating a toolset that includes them (`agentstack use --write`)"
            .dimmed()
    );
}

/// P1: the opening plan. Four numbered steps and a promise made precise for the
/// P30 order: the import step — and only after you confirm it — writes the
/// manifest plus any lifted token values; your CLIs' own configs stay untouched
/// until a later apply confirm. Printed only in an interactive `setup`.
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
        "\n{} The import step writes only the manifest and any lifted token values,\n\
         \x20   and only after you confirm it. Your CLI configs stay untouched until the\n\
         \x20   later apply confirm.",
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

/// Whether the wizard should offer to wire the guard: only when the shell is
/// interactive AND the guard isn't already wired. Pure so the gate is
/// unit-testable without a live wizard or a machine config on disk.
fn should_offer_guard(interactive: bool, guard_wired: bool) -> bool {
    interactive && !guard_wired
}

/// Is the house-rules fragment still missing from the machine manifest? The
/// gate half of the old standalone offer, split out so the combined
/// machine-protection step can name only what's actually pending.
fn house_rules_pending() -> bool {
    let home = crate::util::paths::agentstack_home();
    match crate::manifest::load_from_dir(&home) {
        Ok(loaded) => !loaded
            .manifest
            .instructions
            .contains_key(super::init::HOUSE_RULES_NAME),
        // No machine manifest yet → nothing installed → pending.
        Err(_) => true,
    }
}

/// Step 3 of the adoption ladder: ONE optional machine-wide protection step
/// (audit C6). The project's own setup is finished by the time this runs; the
/// guard and the house rules are machine-global products, so they get exactly
/// one question, together, naming only what's still missing. Accepting
/// installs the pending items with no further prompts; declining prints each
/// one's manual command. Never fails setup — install errors are surfaced with
/// their retry command and swallowed, as before.
fn offer_machine_protection(ctx: &super::Context, target_ids: &[String]) -> Result<(bool, bool)> {
    let interactive = crate::util::confirm::is_interactive();
    let guard_pending = should_offer_guard(interactive, super::guard::is_wired());
    let rules_pending = interactive && house_rules_pending();
    if !guard_pending && !rules_pending {
        return Ok((false, false));
    }

    println!("\n{}", "Optional: machine-wide protection".bold());
    println!(
        "  {}",
        "One question, then this project's setup is done. Both are machine-global\n\
         \x20 (they cover every project on this machine), and `agentstack restore` undoes them."
            .dimmed()
    );
    if guard_pending {
        println!(
            "  · {} — blocks rm -rf, git reset --hard, and .env reads via a\n\
             \x20   pre-tool-use hook in each detected CLI",
            "guard".bold()
        );
    }
    if rules_pending {
        println!(
            "  · {} — a fragment in each CLI's global CLAUDE.md / AGENTS.md that\n\
             \x20   teaches agents the manifest-first workflow",
            "house rules".bold()
        );
    }
    if !crate::util::confirm::confirm("  Set these up now?")? {
        if guard_pending {
            println!(
                "  {} guard skipped — later: {}",
                "·".dimmed(),
                "agentstack guard install --write".bold()
            );
        }
        if rules_pending {
            println!(
                "  {} house rules skipped — later: {}",
                "·".dimmed(),
                "agentstack init --global".bold()
            );
        }
        return Ok((false, false));
    }

    let guard_wired = if guard_pending {
        // `guard install` prints its own per-CLI write lines, so the summary
        // surfaces those rather than duplicating them here.
        println!();
        match super::guard::install(true) {
            Ok(()) => true,
            Err(err) => {
                println!(
                    "  {} guard install failed ({err:#}) — setup itself succeeded; retry with {}.",
                    "⚠".yellow(),
                    "agentstack guard install --write".bold()
                );
                false
            }
        }
    } else {
        false
    };
    let seeded_house_rules = if rules_pending {
        offer_house_rules(ctx, target_ids)?
    } else {
        false
    };
    Ok((guard_wired, seeded_house_rules))
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
                "\nThis manifest declares {} toolsets: {}.",
                names.len(),
                names
                    .iter()
                    .map(|n| n.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            if crate::util::confirm::confirm(&format!(
                "Activate '{first}' now? (switch later with `agentstack use <toolset> --write`)"
            ))? {
                Ok(Some((*first).clone()))
            } else {
                println!(
                    "  {} skipped — activate one later with {}",
                    "·".dimmed(),
                    "agentstack use <toolset> --write".bold()
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
        list: false,
        json: false,
        quiet: false,
    };
    let libctx = ctx.library_ctx();
    let prepared = super::use_profile::prepare(ctx, &libctx, &use_args)?;
    // STRICT: `setup` runs its own consent ceremony before it gets here, so
    // the trust state it delivers under is the one on disk — it has no
    // pre-write answer to fall back on and must not invent one.
    super::use_profile::activate(
        ctx,
        &libctx,
        &use_args,
        &prepared,
        crate::render::PriorTrust::STRICT,
    )
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
        // The newcomer wizard is the last place to dump 100 lines of rendered
        // JSON: it shows the managed-file list and its own review instead.
        verbose: false,
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

/// The write half of the house-rules install. Gate and consent live in
/// [`offer_machine_protection`] — by the time this runs, the fragment is
/// pending and the user already said yes to the combined step.
fn offer_house_rules_inner(ctx: &super::Context, target_ids: &[String]) -> Result<bool> {
    let home = crate::util::paths::agentstack_home();
    println!("\n{}", "House rules".bold());
    let manifest_path = home.join(MANIFEST_FILE);
    let fragment_path = home
        .join("instructions")
        .join(format!("{}.md", super::init::HOUSE_RULES_NAME));
    let mut backups = vec![
        crate::history::capture(&manifest_path, "machine manifest · house rules"),
        crate::history::capture(&fragment_path, "agentstack house-rules fragment"),
    ];

    let writes = (|| -> Result<()> {
        super::init::ensure_global_manifest()?;
        super::init::seed_house_rules(&home)?;
        let loaded = crate::manifest::load_from_dir(&home)?;

        // Consent was just given — compile the machine layer for the same
        // targets this setup configured, at global scope (the layer's home turf).
        for id in target_ids {
            let Some(desc) = ctx.registry.get(id) else {
                continue;
            };
            let Some(plan) = crate::render::instructions::plan_instructions(
                &loaded.manifest,
                desc,
                Scope::Global,
                &home,
                // The machine layer: no project lock, so no package members.
                &[],
                &crate::instructions::Selecting::for_command(None),
                // `home` IS the machine manifest dir, so the gate exempts this
                // compile outright — no project's review governs the user's own
                // house rules. STRICT is therefore the honest value: nothing
                // here relies on a relaxation.
                crate::render::PriorTrust::STRICT,
            ) else {
                continue;
            };
            if plan.changed() {
                backups.push(crate::history::capture(
                    &plan.path,
                    format!("{} · house-rules instructions", desc.display),
                ));
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
    })();

    if let Err(err) = writes {
        crate::history::rollback(&backups)
            .context("house-rules write failed and rollback also failed")?;
        return Err(err).context("house-rules write failed; completed writes were rolled back");
    }

    // The initial captures include files that may already have existed and
    // stayed byte-identical. Keep only actual writes in history and the summary.
    backups.retain(file_change_differs_now);
    // Display names, not raw adapter ids (`claude-code`) — the ledger's
    // `targets` feed the summary `restore` prints, and an id there was
    // review finding H7's second bug (undo history mixing ids and names).
    let target_display_names: Vec<String> = target_ids
        .iter()
        .filter_map(|id| ctx.registry.get(id))
        .map(|d| d.display.clone())
        .collect();
    if let Err(err) = crate::history::record(
        "global",
        "setup (house rules)",
        target_display_names,
        backups.clone(),
    ) {
        crate::history::rollback(&backups)
            .context("house-rules history failed and rollback also failed")?;
        return Err(err).context("recording house-rules writes failed; writes were rolled back");
    }
    Ok(true)
}

fn file_change_differs_now(change: &crate::history::FileChange) -> bool {
    let current = std::fs::read_to_string(&change.path).ok();
    current != change.before
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
    let p = profile.unwrap_or("<toolset>");
    match mode {
        Mode::Static => (
            vec!["agentstack apply --write".into()],
            "Keep rendering configs to disk; re-run after any manifest change.",
        ),
        Mode::CleanAtRest => (
            vec![
                format!("agentstack x session start {p}"),
                "agentstack x session end".into(),
            ],
            "Materialize your toolset for a session, then revert it so the repo stays clean.",
        ),
        Mode::ZeroFiles => (
            vec!["agentstack trust .".into(), "agentstack x uninstall".into()],
            "Review this repo once, then switch: the switch registers the gateway in your CLIs \
             and removes anything this project rendered, so its capabilities serve live.",
        ),
    }
}

/// What the wizard does about delivery. **Not** a mode: `Automatic` is the
/// default and the only thing most projects ever see, `RenderLocally` is the
/// one override the contract keeps, and `Legacy` is the older per-project
/// delivery modes — reachable behind "more control" and switched by the
/// shipped `agentstack set-mode` exactly as before.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DeliveryChoice {
    Automatic,
    RenderLocally,
    Legacy(super::overview::Mode),
}

/// Present delivery as one recommended answer plus a door, BEFORE any write.
///
/// The flip is here: **Automatic is preselected, and it is what a
/// non-interactive shell gets** on a project that has not already rendered.
/// Before this, a wizard with no terminal kept whatever mode was derived, which
/// on a fresh project meant "static" — the old default — so a scripted setup
/// rendered everything however the planner routed it.
///
/// A project with files on disk keeps its rendered path: the files are already
/// there, and the honest way to remove them is the explicit `set-mode`
/// un-render, not a wizard quietly abandoning them.
fn choose_delivery(already_rendered: bool) -> Result<Option<DeliveryChoice>> {
    use super::overview::Mode;

    if !crate::util::confirm::is_interactive() {
        return Ok(Some(if already_rendered {
            DeliveryChoice::Legacy(Mode::Static)
        } else {
            DeliveryChoice::Automatic
        }));
    }

    println!("\n{}", "Delivery".bold());
    println!(
        "  {} how capabilities reach your CLIs. The recommended answer is automatic: \
         agentstack routes each one for you and says what it decided.",
        "·".dimmed()
    );
    println!();

    let top = [
        "automatic — agentstack routes each capability (recommended)".to_string(),
        "more control…".to_string(),
    ];
    // `interact_opt` distinguishes an explicit Esc/q cancellation from a real
    // terminal error. Ctrl-C is converted into an interrupted read by the
    // wizard's scoped SIGINT guard and handled by `run`.
    let idx = dialoguer::Select::with_theme(&dialoguer::theme::ColorfulTheme::default())
        .with_prompt("Delivery")
        .items(&top)
        .default(0)
        .interact_opt()?;
    match idx {
        None => return Ok(None),
        Some(0) => return Ok(Some(DeliveryChoice::Automatic)),
        Some(_) => {}
    }

    // "More control": the one delivery override first, then the older
    // per-project modes. They are listed after it and never above it, because
    // they are no longer the product's answer — but the wizard keeps offering
    // them rather than stranding a project that already uses one.
    println!("\n  {}", "render locally".bold());
    println!(
        "    {}",
        "Write files even where the live channel would have worked — for offline work, \
         deterministic native files, inspection with ordinary filesystem tools, a rule \
         against a persistent background process, debugging without another runtime \
         dependency, or testing a CLI's own behaviour."
            .dimmed()
    );
    let modes = [Mode::Static, Mode::CleanAtRest, Mode::ZeroFiles];
    for m in &modes {
        println!("\n  {}", m.label().bold());
        println!("    {}", m.help().dimmed());
    }
    println!();

    let mut items = vec!["render locally — write files anyway".to_string()];
    items.extend(
        modes
            .iter()
            .map(|m| format!("{} — {}", m.label(), m.short())),
    );
    let idx = dialoguer::Select::with_theme(&dialoguer::theme::ColorfulTheme::default())
        .with_prompt("Pick one")
        .items(&items)
        .default(0)
        .interact_opt()?;
    Ok(idx.map(|selected| match selected {
        0 => DeliveryChoice::RenderLocally,
        other => DeliveryChoice::Legacy(modes[other - 1]),
    }))
}

/// The ordered steps each fork runs, as plain labels. Pure, so "which steps run
/// per choice" is unit-testable without a live wizard; it also backs the
/// one-line plan the wizard prints once the choice is made.
fn fork_plan(choice: DeliveryChoice) -> &'static [&'static str] {
    use super::overview::Mode;
    match choice {
        DeliveryChoice::Automatic => &["routing", "bridge-offer", "trust-pointer", "doctor"],
        // Render locally is the render path plus the durable setting that makes
        // it this project's standing answer rather than a one-off.
        DeliveryChoice::RenderLocally => &[
            "record-override",
            "preview",
            "confirm",
            "install",
            "apply",
            "skills",
            "doctor",
        ],
        DeliveryChoice::Legacy(Mode::Static) => {
            &["preview", "confirm", "install", "apply", "skills", "doctor"]
        }
        DeliveryChoice::Legacy(Mode::CleanAtRest) => {
            &["lock", "session-rhythm", "switch-pointer", "doctor"]
        }
        DeliveryChoice::Legacy(Mode::ZeroFiles) => {
            &["gateway-offer", "trust-pointer", "switch-pointer"]
        }
    }
}

/// Record the durable **Render locally** override the user just picked.
///
/// Goes through the one editor that owns this key
/// ([`super::delivery::set_render_locally`]) so the wizard cannot write a shape
/// the `delivery` command would not recognise, and captures the file for undo
/// like every other wizard write.
fn record_render_locally(manifest_dir: Option<&Path>) -> Result<()> {
    let ctx = super::load(manifest_dir)?;
    let path = ctx.loaded.manifest_path.clone();
    let original =
        fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let updated = super::delivery::set_render_locally(&original, None, true)?;
    if updated == original {
        return Ok(());
    }
    // Same key, same rule as `x delivery render-locally --write`: a routing
    // preference declares no capability, so trust that was valid before this
    // write is carried across it rather than sending the user to a re-review
    // for a choice the wizard just made on their behalf.
    let carry = crate::trust_carry::TrustCarry::before_write(&ctx.dir);
    let backup = crate::history::capture(&path, "agentstack.toml · render locally");
    crate::util::atomic::write(&path, &updated)
        .with_context(|| format!("writing {}", path.display()))?;
    carry.across_write(&path, &updated)?;
    let _ = crate::history::record("project", "setup (render locally)", vec![], vec![backup]);
    println!(
        "  {} recorded {} — this project writes files even where the live channel would work.",
        "✓".green(),
        "[delivery] render_locally = true".bold()
    );
    Ok(())
}

/// The **Automatic** fork — the default after the flip (W4, 2026-08-03).
///
/// It states the routing rather than asking for it, offers the one registration
/// the live lane needs, and points at the review. It deliberately renders
/// nothing itself: the rendered lane's command is `apply --write`, an explicit
/// act, and a wizard that ran it here would put a native copy of every server on
/// disk one line under a screen saying they are served live.
fn run_automatic(
    ctx: &super::Context,
    target_ids: &[String],
    manifest_dir: Option<&Path>,
    // The user typed `agentstack init --connect`, so the bridge question is
    // already answered and the wizard states the registration instead of
    // asking it again.
    preconsented: bool,
    verbose: bool,
) -> Result<()> {
    let plan =
        crate::delivery::Plan::build(&ctx.loaded.manifest.delivery, &ctx.registry, target_ids);

    println!("\n{}", "Delivery".bold());
    if plan.harnesses.is_empty() {
        println!(
            "  {} no CLIs targeted yet — nothing to route.",
            "·".dimmed()
        );
    }
    // The routing table, ONCE per run and only under `--verbose`: the import's
    // pre-write review and the closing summary both used to state the same
    // per-tool answer, so one `init` printed it three times. Both of those now
    // stand down, and this is the copy that survives.
    //
    // What does NOT move behind the flag is the honesty reading itself: a
    // harness with no bridge registered says so at every verbosity, because
    // invariant 8 is about a claim the output must not make.
    if verbose {
        let width = plan
            .harnesses
            .iter()
            .map(|h| h.display.len())
            .max()
            .unwrap_or(0);
        for h in &plan.harnesses {
            // Per-harness bridge reading, never the raw routing sentence: with
            // no gateway registered nothing reaches this tool, and `status`,
            // `doctor` and `delivery` all say "planned live (not connected)".
            println!(
                "  {:width$}   {}",
                h.display,
                crate::commands::delivery::harness_sentence(
                    h,
                    super::overview::bridge_registered(&ctx.registry, &h.id)
                )
            );
        }
        // The two binding honesty rules, each on its own line.
        if plan.has_dynamic_lane() {
            println!("  {} {}", "·".dimmed(), crate::delivery::ZERO_ARTIFACTS);
        }
        if let Some(line) = crate::delivery::rendered_lane_line(&plan) {
            println!("  {} {line}", "·".dimmed());
            println!(
                "  {} write them with {}",
                "·".dimmed(),
                "agentstack apply --write".bold()
            );
        }
    } else if !plan.harnesses.is_empty() {
        // Invariant 8 in one line: "served live" only where a bridge really is
        // registered, "planned live (not connected)" — the product's own
        // wording — everywhere it is not, and the un-registered harnesses named
        // when it is only some of them.
        let unconnected = crate::commands::delivery::unconnected_live(&plan, &ctx.registry);
        let live_count = plan.live_harnesses().len();
        let live = if !plan.has_dynamic_lane() {
            String::new()
        } else if unconnected.is_empty() {
            " · skills + MCP servers served live".to_string()
        } else if unconnected.len() == live_count {
            " · skills + MCP servers planned live (not connected)".to_string()
        } else {
            format!(
                " · skills + MCP servers planned live (not connected in {})",
                unconnected.join(", ")
            )
        };
        let files = crate::delivery::rendered_lane_line(&plan)
            .map(|_| " · the rest written to files by `agentstack apply --write`")
            .unwrap_or_default();
        println!(
            "  {} targeted{live}{files}   (per tool: --verbose)",
            super::count(plan.harnesses.len(), "CLI")
        );
    }

    // The live lane needs the bridge registered once per CLI. That is a
    // machine-wide write, so it is always a confirm and never happens without a
    // terminal — a scripted setup gets the command printed instead.
    if plan.has_dynamic_lane() {
        offer_bridge(preconsented)?;
        // The review pointer that used to sit here is now the close's single
        // `Next:` — and computed from the trust state this run actually left
        // behind, rather than printed unconditionally beside the bridge offer.
    }

    println!("\n{}", "Doctor".bold());
    run_doctor_step(
        &DoctorArgs {
            ci: false,
            live: false,
            probe: false,
            fix: false,
            deep: false,
            all: false,
            json: false,
            skip_drift: false,
        },
        manifest_dir,
        verbose,
    )
}

/// `doctor`, as one step of the wizard: the full report under `--verbose`, its
/// one-line reading otherwise. The line names `agentstack doctor`, so the
/// sections are one command away and the wizard keeps its single closing step.
fn run_doctor_step(args: &DoctorArgs, manifest_dir: Option<&Path>, verbose: bool) -> Result<()> {
    if verbose {
        return super::doctor::run(args, manifest_dir);
    }
    println!("  {}", super::doctor::summary_line(args, manifest_dir)?);
    Ok(())
}

/// Offer to register the agentstack bridge in the installed harnesses — the one
/// thing the live lane needs that a project cannot provide for itself. Shared by
/// the automatic fork and the legacy zero-files fork, so there is one copy of
/// this offer and one failure message.
fn offer_bridge(preconsented: bool) -> Result<()> {
    const CONNECT_LATER: &str = "agentstack x gateway connect --all --write";
    // `--connect` is an answer already given, in the same breath as the
    // command that got here. Asking again would be theatre, not consent — but
    // the run still SAYS what it is about to write, because a machine-wide
    // change must never be silent even when it was asked for.
    if preconsented {
        println!(
            "\n  {} registering the bridge now — you asked for it with {}.",
            "·".dimmed(),
            "--connect".bold()
        );
        return register_now(CONNECT_LATER);
    }
    // The consequence belongs IN the question. This prompt keeps `confirm`'s
    // no-is-the-default contract — it is a machine-wide write, and the design
    // law automates everything except the yes — but a bare Enter used to
    // decline the one step the live lane depends on, with nothing on screen
    // saying so, and the wizard then closed with "Setup complete".
    println!(
        "\n  {} Until this is registered, nothing is served live — the capabilities",
        "·".dimmed()
    );
    println!(
        "    {}",
        "above stay pinned in the manifest and reach no CLI.".dimmed()
    );
    let register = crate::util::confirm::is_interactive()
        && crate::util::confirm::confirm(
            "  Register the agentstack bridge in your installed CLIs now?",
        )?;
    // Whether the registration happened is deliberately NOT returned: the close
    // asks the machine (`gateway_connected`) instead of remembering this
    // answer, so a bridge registered earlier, or by `gateway connect` in
    // another terminal, reads as done rather than as declined.
    if !register {
        println!("  {} register it later with:", "·".dimmed());
        println!("    {}", CONNECT_LATER.bold());
        return Ok(());
    }
    register_now(CONNECT_LATER)
}

/// The registration itself, shared by the answered-yes and the `--connect`
/// paths so there is one call into `gateway connect` and one failure message.
///
/// A failure here (no MCP-capable harness, say) must not sink the whole setup —
/// surface it with the manual command, like the house-rules offer.
fn register_now(connect_later: &str) -> Result<()> {
    if let Err(err) = super::connect::run_connect(&ConnectArgs {
        harnesses: Vec::new(),
        all: true,
        transparent: false,
        write: true,
        command: None,
    }) {
        println!(
            "  {} bridge registration failed ({err:#}) — register it later with:",
            "⚠".yellow()
        );
        println!("    {}", connect_later.bold());
    }
    Ok(())
}

/// The files written since `history_before` was snapshotted, deduped by path
/// (an apply and a profile activation can touch the same file). New history
/// entries hold the pre-write snapshot of each touched file; we surface the
/// paths + labels. Shared basis for both the full P7 close and the P30 cancel
/// mini-summary, so "what this run wrote" has one definition.
fn files_written_since(
    history_before: &std::collections::HashSet<String>,
) -> Vec<(String, String)> {
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
    files
}

/// The CLI display names whose native files this run actually touched,
/// derived from the ledger labels (every capture label is
/// `"<display> · <category>"`), filtered to native CLI-side paths. First-seen
/// order, deduped — the "CLIs updated" fact in the close.
fn clis_updated(files: &[(String, String)]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (path, label) in files {
        if !is_cli_config_path(path) {
            continue;
        }
        let cli = label.split(" · ").next().unwrap_or(label).to_string();
        if !out.contains(&cli) {
            out.push(cli);
        }
    }
    out
}

/// Whether any written path is a native CLI-side config (server config,
/// instruction file, settings, or a materialized skill) rather than
/// agentstack's own bookkeeping (the manifest, a lifted-secret `.env`, the
/// `.gitignore` line, or the lockfile). Only a CLI-config change warrants the
/// "restart your CLIs" advice (P30) — importing a manifest does not.
fn cli_config_touched(files: &[(String, String)]) -> bool {
    files.iter().any(|(path, _)| is_cli_config_path(path))
}

fn is_cli_config_path(path: &str) -> bool {
    // Everything under agentstack's own home is bookkeeping, whatever it is
    // called: the library the import now writes into, the trust store, the
    // history ledger, the backups. This clause is why the rule is not a
    // filename denylist alone — the library inversion added a writer whose
    // files are named `library.toml` and `<server>.toml`, so the close reported
    // a CLI called "library" and told the user to restart CLIs that had not
    // been touched.
    if Path::new(path).starts_with(crate::util::paths::agentstack_home()) {
        return false;
    }
    let name = Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    // The project-side artifacts are the other exception; everything else
    // written during setup is a CLI-side file the harness reads at startup.
    name != MANIFEST_FILE && name != ".env" && name != ".gitignore" && name != "agentstack.lock"
}

/// P30: a truthful mini-summary for any post-import stop — list whatever this
/// run has ALREADY written (from the same ledger `restore` reads) and the one
/// undo one-liner. A no-op when nothing was written this run (e.g. the manifest
/// already existed), so callers can invoke it unconditionally at any stop.
fn print_stop_summary(history_before: &std::collections::HashSet<String>) {
    let files = files_written_since(history_before);
    if files.is_empty() {
        return;
    }
    print!("{}", render_stop_summary(&files));
}

/// Pure formatter for the P30 cancel mini-summary (what the import already
/// wrote + the undo one-liner), so the cancel path is unit-testable without a
/// live wizard. Only reached with a non-empty `files`.
fn render_stop_summary(files: &[(String, String)]) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "\n  The import already wrote {} this run:\n",
        super::count(files.len(), "file")
    ));
    for (path, label) in files {
        out.push_str(&format!("    {path}  ({label})\n"));
    }
    out.push_str("  Undo recorded files:  agentstack x restore --last --write\n");
    out.push_str(
        "  Keychain values are outside file history; inspect with `agentstack secret list` and remove with `agentstack secret rm <NAME>`.\n",
    );
    out
}

/// P7: the transparency close. Gathers what THIS run changed — every file
/// written (from the apply-history entries new since `history_before`), where
/// each referenced secret resolves now, and what was seeded — then prints it
/// with the undo + inspect one-liners.
fn print_change_summary(
    ctx: &super::Context,
    history_before: &std::collections::HashSet<String>,
    seeded_house_rules: bool,
    guard_wired: bool,
    verbose: bool,
) {
    let files = files_written_since(history_before);

    // Secrets: re-derive where each referenced ref resolves now (the resolver is
    // the source of truth; we never stored a value to echo).
    let sources = SecretSources::detect(&ctx.dir);
    // Library-resolved: the import's lifted `${REF}`s live in the library
    // definition, not in `[servers]`, so the manifest's own answer is empty for
    // exactly the secrets this run just stored.
    let libctx = ctx.library_ctx();
    let referenced = crate::resolve::effective_referenced_secrets(
        &ctx.loaded.manifest,
        &libctx.library,
        &libctx.lib_home,
    );
    let secrets: Vec<(String, String)> = referenced
        .iter()
        .filter_map(|name| {
            sources
                .source_of(name)
                .map(|s| (name.clone(), s.to_string()))
        })
        .collect();
    let keychain_secrets: Vec<String> = secrets
        .iter()
        .filter(|(_, source)| source == "keychain")
        .map(|(name, _)| name.clone())
        .collect();

    let mut seeded: Vec<String> = Vec::new();
    if seeded_house_rules {
        let path = crate::util::paths::agentstack_home().join(MANIFEST_FILE);
        seeded.push(format!(
            "agentstack house rules → {} (edit under [instructions])",
            path.display()
        ));
    }

    // Referenced `${REF}`s that still resolve nowhere on this machine — the
    // skip store, a declined prompt, or an unreachable keychain. The close
    // must name them, or "what still needs a value" is buried in scrollback.
    let still_needed: Vec<String> = referenced
        .into_iter()
        .filter(|name| sources.source_of(name).is_none())
        .collect();

    // Restart advice is warranted only when a native CLI config actually
    // changed — a rendered config/skill in the ledger, the house-rules fragment
    // we compiled into the global instruction files (NOT in the ledger), or the
    // guard hooks `guard install` wrote into each CLI's config (also outside the
    // ledger — hence the explicit ORs).
    let cli_config_changed = cli_config_touched(&files) || seeded_house_rules || guard_wired;

    // "Complete" is a claim about delivery, not about files written. On the
    // live lane nothing reaches a CLI until the bridge is registered, and the
    // wizard used to close with `✓ Setup complete.` over a machine where the
    // offer had just been declined with a bare Enter.
    //
    // Asked of the machine rather than of the answer given a moment ago, so a
    // bridge registered earlier, or by `gateway connect`, reads as done.
    let target_ids: Vec<String> = ctx.registry.ids().map(str::to_string).collect();
    let plan =
        crate::delivery::Plan::build(&ctx.loaded.manifest.delivery, &ctx.registry, &target_ids);
    let live_lane_unwired =
        plan.has_dynamic_lane() && !super::overview::gateway_connected(ctx, &target_ids);
    if live_lane_unwired {
        println!("\n{} Setup imported, not yet delivering.", "·".dimmed());
        println!(
            "  {}",
            "The bridge is not registered, so nothing is served live yet:".dimmed()
        );
        println!(
            "    {}",
            "agentstack x gateway connect --all --write".bold()
        );
    } else {
        println!("\n{} Setup complete.", "✓".green());
    }
    print!(
        "{}",
        render_setup_facts(
            &ctx.loaded.manifest_path.display().to_string(),
            &clis_updated(&files),
            // Named, not defined-inline: the import writes its servers into the
            // library and references them from `[toolsets.default]`, so reading
            // `[servers]` closed the wizard with "0 MCP servers" over the six it
            // had just pinned.
            ctx.loaded.manifest.declared_server_names().len(),
            ctx.loaded.manifest.skills.len(),
            &still_needed,
        )
    );
    // The one `Next:` this run ends on. A close that named the review, the
    // bridge, `apply`, `doctor` and undo as five equally-weighted "next"
    // commands named none of them: the reader has to pick, and picking is the
    // job this line exists to do. The gate that is actually standing in the way
    // wins — until a project is trusted nothing it declares is delivered — and
    // everything else stays reachable on the compact line under it.
    let next = if super::overview::trust_blocks_delivery(
        crate::trust::check(&ctx.dir),
        super::overview::declares_capabilities(&ctx.loaded.manifest),
    ) {
        (
            "agentstack trust .",
            "review this project once, so it can be served",
        )
    } else {
        ("agentstack doctor", "check the result")
    };
    if verbose {
        println!("\n{}", "What changed on this machine".bold());
    }
    print!(
        "{}",
        render_change_summary(&ChangeSummary {
            files: &files,
            secrets: &secrets,
            seeded: &seeded,
            cli_config_changed,
            keychain_secrets: &keychain_secrets,
            guard_wired,
            next,
            verbose,
        })
    );
}

/// Pure formatter for the concise facts block that leads the close (Stage
/// 1.2): manifest path, which CLIs were updated, what the manifest now
/// carries, and which secrets still need values. The detailed per-file list
/// follows in [`render_change_summary`]; this block is the at-a-glance answer.
fn render_setup_facts(
    manifest_path: &str,
    clis: &[String],
    server_count: usize,
    skill_count: usize,
    still_needed: &[String],
) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "\n  Manifest:      {manifest_path}   (the source of truth your CLIs render from)\n"
    ));
    if clis.is_empty() {
        out.push_str("  CLIs updated:  none — their configs already matched the manifest\n");
    } else {
        out.push_str(&format!("  CLIs updated:  {}\n", clis.join(" · ")));
    }
    let mut caps = super::count(server_count, "MCP server");
    if skill_count > 0 {
        caps.push_str(&format!(" · {}", super::count(skill_count, "skill")));
    }
    out.push_str(&format!("  Capabilities:  {caps}\n"));
    // Undo is one of the product's four ideas, and the guided first-timer is
    // exactly the person who needs to know the way back — but it is not this
    // run's next step, so it moved onto the one compact line the close ends on
    // (`render_change_summary`) rather than standing here as a third command
    // competing with the single `Next:`.
    if !still_needed.is_empty() {
        out.push_str(&format!(
            "  Still needed:  {} before this setup can run:\n",
            super::count(still_needed.len(), "secret value")
        ));
        for name in still_needed {
            out.push_str(&format!(
                "                   agentstack secret set {name}\n"
            ));
        }
    }
    out
}

/// Pure formatter for the P7 close body (files / secrets / seeded / one-liners),
/// so the transparency block is unit-testable without a live setup run. Sections
/// with nothing to show are omitted, except the always-present undo/inspect
/// one-liners. The restart-CLIs line prints only when a native CLI config
/// changed this run (P30).
#[derive(Default)]
struct ChangeSummary<'a> {
    files: &'a [(String, String)],
    secrets: &'a [(String, String)],
    seeded: &'a [String],
    cli_config_changed: bool,
    keychain_secrets: &'a [String],
    guard_wired: bool,
    /// The single next step this run ends on, as (command, why), chosen by the
    /// caller from the state the run actually left behind.
    next: (&'a str, &'a str),
    /// Spell every file and secret out rather than counting them.
    verbose: bool,
}

fn render_change_summary(s: &ChangeSummary<'_>) -> String {
    let ChangeSummary {
        files,
        secrets,
        seeded,
        cli_config_changed,
        keychain_secrets,
        guard_wired,
        next,
        verbose,
    } = *s;
    let mut out = String::new();
    if files.is_empty() {
        out.push_str("  No files were written.\n");
    } else if verbose {
        out.push_str(&format!("  Files written ({}):\n", files.len()));
        for (path, label) in files {
            out.push_str(&format!("    {path}  ({label})\n"));
        }
    } else {
        // The count is the fact; the paths are the evidence. Both are honest —
        // this is not a "nothing was written" claim, which is the one thing
        // this line may never become.
        out.push_str(&format!(
            "  Wrote {} on this machine   (each path: --verbose)\n",
            super::count(files.len(), "file")
        ));
    }
    if !secrets.is_empty() {
        if verbose {
            out.push_str("  Secrets:\n");
            for (name, source) in secrets {
                out.push_str(&format!("    {name}  resolved from {source}\n"));
            }
        } else {
            out.push_str(&format!(
                "  Secrets: {} resolved\n",
                super::count(secrets.len(), "reference")
            ));
        }
    }
    if !seeded.is_empty() {
        out.push_str("  Seeded:\n");
        for s in seeded {
            out.push_str(&format!("    {s}\n"));
        }
    }
    // The guard manages its own install/uninstall (its hook writes are outside
    // the apply history `restore` reads), so it carries its own undo line. The
    // per-CLI writes were already listed by `guard install` above — surface the
    // fact and the reversal here, don't re-enumerate them.
    if guard_wired {
        out.push_str(
            "  Guard wired into your CLIs' pre-tool-use hooks (listed above) — \
             undo: agentstack guard uninstall\n",
        );
    }
    // Kept at every verbosity: a value the file ledger cannot reach is exactly
    // what a reader would otherwise assume `restore` took back.
    if !keychain_secrets.is_empty() {
        out.push_str(&format!(
            "  Keychain values are outside file history — remove explicitly: {}\n",
            keychain_secrets
                .iter()
                .map(|name| format!("agentstack secret rm {name}"))
                .collect::<Vec<_>>()
                .join("  ·  ")
        ));
    }
    // Harnesses read config at startup, so an open session won't see the writes
    // — but only say so when a CLI config actually changed this run (P30).
    if cli_config_changed {
        out.push_str("\n  Restart your agent CLIs so they pick up the new config.\n");
    }
    // ONE next step, and one compact line for everything that stays reachable.
    let (cmd, why) = next;
    out.push_str(&format!("\n  Next: {cmd}   ({why})\n"));
    out.push_str("  Also: agentstack   ·   undo this setup: agentstack x restore --last --write\n");
    // P29.1: the closing doorway is the summary's FINAL line — it hands the user
    // to the walkthrough exactly when curiosity peaks, or back to bare
    // `agentstack` for the next step. Every delivery-mode fork ends through this
    // one formatter, so all three summaries carry it. (The `\` is a Rust string
    // line-continuation: it and the following indentation collapse to nothing,
    // leaving one space before the em dash.)
    out.push_str(
        "\n  Learn the rest: https://tarekkharsa.github.io/agentstack/start.html \
         — or run `agentstack` anytime for your next step.\n",
    );
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
pub(crate) fn preflight(
    ctx: &super::Context,
    target_ids: &[String],
    verbose: bool,
) -> Result<Preflight> {
    let validation_errors = print_validation(ctx);
    print_adapters(ctx, target_ids, verbose);
    print_skills(ctx, verbose)?;
    let missing_secrets = print_secrets(ctx, verbose);
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
        let mark = if issue.kind.is_error() {
            has_errors = true;
            "✗".red().to_string()
        } else {
            "⚠".yellow().to_string()
        };
        match &issue.fix {
            Some(fix) => println!("  {mark} {} ↳ {fix}", issue.message),
            None => println!("  {mark} {}", issue.message),
        }
    }
    has_errors
}

/// The per-CLI adapter readings.
///
/// A row per CLI is a table, and on a machine with eight of them it buries the
/// only rows that need an answer. The default therefore prints the exceptions
/// (a config with no binary, a CLI that is not installed, an unknown id) and
/// counts the rest; `--verbose` prints every row.
fn print_adapters(ctx: &super::Context, target_ids: &[String], verbose: bool) {
    if target_ids.is_empty() {
        println!("\n{}", "Adapters".bold());
        println!("  {} no target adapters selected", "⚠".yellow());
        return;
    }
    if !verbose {
        let installed = target_ids
            .iter()
            .filter(|id| ctx.registry.get(id).is_some_and(|d| d.is_installed()))
            .count();
        let odd: Vec<String> = target_ids
            .iter()
            .filter_map(|id| match ctx.registry.get(id) {
                Some(desc) if desc.is_installed() => None,
                Some(desc) if desc.config_present() => {
                    Some(format!("{} (config, no binary)", desc.display))
                }
                Some(desc) => Some(format!("{} (not detected)", desc.display)),
                None => Some(format!("unknown adapter '{id}'")),
            })
            .collect();
        let mut line = format!("  {} {} installed", "✓".green(), installed);
        if !odd.is_empty() {
            line.push_str(&format!(" · {} {}", "⚠".yellow(), odd.join(" · ")));
        }
        println!("\n{}", "Adapters".bold());
        println!("{line}");
        return;
    }
    println!("\n{}", "Adapters".bold());
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

/// The per-skill readings. Same rule as the adapters above: the default names
/// only the skills with something wrong and counts the healthy ones.
fn print_skills(ctx: &super::Context, verbose: bool) -> Result<usize> {
    println!("\n{}", "Skills".bold());
    let manifest = &ctx.loaded.manifest;
    if manifest.skills.is_empty() {
        println!("  {} no skills defined", "✓".green());
        return Ok(0);
    }

    let store = Store::default_store();
    let lock = Lock::load(&ctx.dir)?;
    let mut issues = 0;
    let mut healthy = 0;
    for (name, skill) in &manifest.skills {
        let locked = lock.get(name);
        let pinned_rev = locked.and_then(|entry| entry.rev.as_deref());
        let Some(local) = local_source_dir(&store, skill, &ctx.dir, pinned_rev) else {
            issues += 1;
            println!(
                "  {} {name:<20} source missing — run agentstack install",
                "⚠".yellow()
            );
            continue;
        };
        let Some(locked) = locked else {
            issues += 1;
            println!("  {} {name:<20} present, not locked", "⚠".yellow());
            continue;
        };
        match dir_digest(&local) {
            Ok(sum) if sum == locked.checksum => {
                healthy += 1;
                if verbose {
                    println!("  {} {name:<20} present · locked", "✓".green());
                }
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
    if !verbose && healthy > 0 {
        println!(
            "  {} {} present · locked",
            "✓".green(),
            super::count(healthy, "skill")
        );
    }
    Ok(issues)
}

/// The per-secret readings. Same rule again: a missing value is an answer the
/// user has to act on, a resolved one is a count.
fn print_secrets(ctx: &super::Context, verbose: bool) -> Vec<String> {
    println!("\n{}", "Secrets".bold());
    // Library-resolved, for the reason in `effective_referenced_secrets`: the
    // inline reading printed "no secrets referenced" in the same wizard run
    // that had just lifted a live token into `.env`.
    let libctx = ctx.library_ctx();
    let refs = crate::resolve::effective_referenced_secrets(
        &ctx.loaded.manifest,
        &libctx.library,
        &libctx.lib_home,
    );
    if refs.is_empty() {
        println!("  {} no secrets referenced", "✓".green());
        return Vec::new();
    }

    let sources = SecretSources::detect(&ctx.dir);
    let mut missing = Vec::new();
    let mut resolved = 0;
    for name in refs {
        match sources.source_of(&name) {
            Some(source) => {
                resolved += 1;
                if verbose {
                    println!("  {} {name:<20} resolved from {source}", "✓".green());
                }
            }
            None => {
                println!("  {} {name:<20} missing", "✗".red());
                missing.push(name);
            }
        }
    }
    if !verbose && resolved > 0 {
        println!(
            "  {} {} resolved",
            "✓".green(),
            super::count(resolved, "secret")
        );
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
    use super::{
        choose_delivery, cli_config_touched, clis_updated, fork_plan, is_cli_config_path,
        mode_switch_plan, render_change_summary, render_setup_facts, render_stop_summary,
        should_offer_guard, ChangeSummary, DeliveryChoice,
    };

    // TASK 3: the guard offer is gated — shown only when the shell is
    // interactive AND the guard isn't already wired. Every other combination
    // stays silent (a scripted shell never offers; an already-wired machine
    // isn't nagged).
    #[test]
    fn guard_offer_shows_only_when_interactive_and_not_wired() {
        assert!(
            should_offer_guard(true, false),
            "interactive + unwired → offer"
        );
        assert!(!should_offer_guard(true, true), "already wired → silent");
        assert!(!should_offer_guard(false, false), "scripted → silent");
        assert!(
            !should_offer_guard(false, true),
            "scripted + wired → silent"
        );
    }

    // W4: the delivery choice is a real fork — each answer runs a distinct,
    // fixed sequence of steps. Automatic (the default after the flip) states
    // the routing and offers the bridge; it never renders. Render locally is
    // the render path plus the durable override that records the decision.
    #[test]
    fn fork_plan_maps_each_choice_to_its_step_sequence() {
        assert_eq!(
            fork_plan(DeliveryChoice::Automatic),
            &["routing", "bridge-offer", "trust-pointer", "doctor"]
        );
        assert_eq!(
            fork_plan(DeliveryChoice::RenderLocally),
            &[
                "record-override",
                "preview",
                "confirm",
                "install",
                "apply",
                "skills",
                "doctor"
            ]
        );
        assert_eq!(
            fork_plan(DeliveryChoice::Legacy(Mode::Static)),
            &["preview", "confirm", "install", "apply", "skills", "doctor"]
        );
        assert_eq!(
            fork_plan(DeliveryChoice::Legacy(Mode::CleanAtRest)),
            &["lock", "session-rhythm", "switch-pointer", "doctor"]
        );
        assert_eq!(
            fork_plan(DeliveryChoice::Legacy(Mode::ZeroFiles)),
            &["gateway-offer", "trust-pointer", "switch-pointer"]
        );

        // The no-render forks must never render into a CLI config. Automatic
        // joins them: after the flip the default fork writes no native files at
        // all, because skills and MCP servers are served live and the rendered
        // lane's command is the explicit `apply --write`.
        for no_render in [
            DeliveryChoice::Automatic,
            DeliveryChoice::Legacy(Mode::CleanAtRest),
            DeliveryChoice::Legacy(Mode::ZeroFiles),
        ] {
            assert!(!fork_plan(no_render).contains(&"apply"), "{no_render:?}");
        }
        assert!(!fork_plan(DeliveryChoice::Legacy(Mode::CleanAtRest)).contains(&"install"));
        // Automatic and zero-files both point at the review rather than
        // granting it.
        assert!(fork_plan(DeliveryChoice::Automatic).contains(&"trust-pointer"));
        assert!(fork_plan(DeliveryChoice::Legacy(Mode::ZeroFiles)).contains(&"trust-pointer"));
        // Both legacy no-render forks point at the un-render switch: without
        // it, a project with rendered files keeps deriving "static" whatever
        // was chosen — the exact display lie set-mode exists to remove.
        assert!(fork_plan(DeliveryChoice::Legacy(Mode::CleanAtRest)).contains(&"switch-pointer"));
        assert!(fork_plan(DeliveryChoice::Legacy(Mode::ZeroFiles)).contains(&"switch-pointer"));
    }

    /// The flip itself, at the wizard: a scripted run on a project that has
    /// never rendered gets **Automatic**, not the old static render path. A
    /// project that already has files on disk keeps them — the files are a
    /// fact, and abandoning them silently would leave stale capabilities behind
    /// a screen claiming everything is served live.
    #[test]
    fn a_scripted_setup_defaults_to_automatic_unless_the_project_already_rendered() {
        assert_eq!(
            choose_delivery(false).unwrap(),
            Some(DeliveryChoice::Automatic)
        );
        assert_eq!(
            choose_delivery(true).unwrap(),
            Some(DeliveryChoice::Legacy(Mode::Static))
        );
    }

    // P4: choosing a non-default mode prints a command sequence, never runs it.
    // The clean-at-rest plan threads the profile name into `session start`.
    #[test]
    fn mode_switch_plan_maps_each_mode_to_its_commands() {
        let (cmds, _) = mode_switch_plan(Mode::Static, Some("dev"));
        assert_eq!(cmds, vec!["agentstack apply --write".to_string()]);

        let (cmds, _) = mode_switch_plan(Mode::CleanAtRest, Some("dev"));
        assert_eq!(cmds[0], "agentstack x session start dev");
        assert_eq!(cmds[1], "agentstack x session end");

        // No profile declared → a visible placeholder, not a panic.
        let (cmds, _) = mode_switch_plan(Mode::CleanAtRest, None);
        assert_eq!(cmds[0], "agentstack x session start <toolset>");

        // Zero-files: trust FIRST (set-mode refuses an untrusted project so
        // the derived mode can't disagree with the choice), then the switch
        // that registers the gateway and un-renders. The un-render leg used to
        // be missing entirely: a rendered project that "switched" kept
        // deriving — and displaying — static.
        let (cmds, _) = mode_switch_plan(Mode::ZeroFiles, None);
        assert_eq!(cmds[0], "agentstack trust .");
        assert_eq!(cmds[1], "agentstack x uninstall");
    }

    // Stage 1.2: the close leads with the concise facts — manifest path, CLIs
    // updated, capabilities, and secrets still needing values (with the exact
    // command). "CLIs updated" derives from the ledger labels of native-side
    // paths, so agentstack's own bookkeeping never counts as a CLI update.
    #[test]
    fn setup_facts_name_manifest_clis_capabilities_and_missing_secrets() {
        let files = vec![
            (
                ".agentstack/agentstack.toml".to_string(),
                "manifest · import".to_string(),
            ),
            (
                "~/.claude.json".to_string(),
                "Claude Code · servers".to_string(),
            ),
            (
                "~/.claude/settings.json".to_string(),
                "Claude Code · settings".to_string(),
            ),
            (
                "~/.codex/config.toml".to_string(),
                "Codex CLI · servers".to_string(),
            ),
        ];
        let clis = clis_updated(&files);
        assert_eq!(
            clis,
            vec!["Claude Code".to_string(), "Codex CLI".to_string()]
        );

        let out = render_setup_facts(
            "/p/.agentstack/agentstack.toml",
            &clis,
            8,
            2,
            &["GITHUB_TOKEN".to_string()],
        );
        assert!(out.contains("Manifest:      /p/.agentstack/agentstack.toml"));
        assert!(out.contains("CLIs updated:  Claude Code · Codex CLI"));
        assert!(out.contains("8 MCP servers · 2 skills"));
        assert!(out.contains("agentstack secret set GITHUB_TOKEN"));

        // Import-only run: nothing native touched → says so plainly; no
        // missing secrets → no "Still needed" section at all.
        let quiet = render_setup_facts("/p/agentstack.toml", &clis_updated(&files[..1]), 1, 0, &[]);
        assert!(quiet.contains("CLIs updated:  none"));
        assert!(!quiet.contains("Still needed"));
        assert!(!quiet.contains("skill"));
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
        let keychain = ["API_TOKEN".to_string()];
        let summary = |verbose| ChangeSummary {
            files: &files,
            secrets: &secrets,
            seeded: &seeded,
            cli_config_changed: true,
            keychain_secrets: &keychain,
            guard_wired: true,
            next: ("agentstack doctor", "check the result"),
            verbose,
        };
        let out = render_change_summary(&summary(true));

        assert!(out.contains("Files written (2)"));
        assert!(out.contains("~/.claude.json  (Claude Code · servers)"));
        assert!(out.contains("API_TOKEN  resolved from keychain"));
        assert!(out.contains("house rules"));
        assert!(out.contains("agentstack x restore --last --write"));
        assert!(out.contains("agentstack doctor"));
        assert!(out.contains("agentstack secret rm API_TOKEN"));
        // A CLI config changed → the restart advice is present.
        assert!(out.contains("Restart your agent CLIs"));
        // guard_wired → the guard carries its own undo line (its writes are
        // outside the apply history `restore` reverses).
        assert!(out.contains("agentstack guard uninstall"));

        // The default is the same facts as counts: the file COUNT is stated
        // (never "nothing was written"), the paths are named as `--verbose`,
        // and the keychain caveat — the one thing `restore` cannot take back —
        // survives the compression untouched.
        let brief = render_change_summary(&summary(false));
        assert!(brief.contains("Wrote 2 files on this machine"), "{brief}");
        assert!(!brief.contains("Files written (2)"), "{brief}");
        assert!(brief.contains("--verbose"), "{brief}");
        assert!(brief.contains("agentstack secret rm API_TOKEN"), "{brief}");
        assert!(brief.contains("Restart your agent CLIs"), "{brief}");
    }

    /// The close ends on exactly ONE `Next:`, chosen by the caller, with
    /// everything else on the compact line under it. Five equally-weighted
    /// "next" commands were five ways of recommending nothing.
    #[test]
    fn change_summary_ends_on_one_next_step() {
        let out = render_change_summary(&ChangeSummary {
            next: ("agentstack trust .", "review this project once"),
            ..Default::default()
        });
        assert_eq!(out.matches("Next:").count(), 1, "{out}");
        assert!(out.contains("Next: agentstack trust ."), "{out}");
        assert!(out.contains("Also: agentstack"), "{out}");
    }

    // With nothing written, the summary says so but still offers the one-liners.
    #[test]
    fn change_summary_with_no_writes_still_offers_undo() {
        let out = render_change_summary(&ChangeSummary {
            next: ("agentstack doctor", "check the result"),
            ..Default::default()
        });
        assert!(out.contains("No files were written"));
        assert!(out.contains("agentstack x restore --last --write"));
    }

    // P30: the restart-CLIs advice appears ONLY when a native CLI config
    // changed this run. An import-only run (manifest but no rendered config)
    // must not tell the user to restart harnesses that never changed.
    #[test]
    fn change_summary_restart_line_gates_on_cli_config_change() {
        // Import-only: just the manifest was written, no CLI config.
        let files = vec![(
            ".agentstack/agentstack.toml".to_string(),
            "manifest · import".to_string(),
        )];
        let summary = |cli_config_changed| ChangeSummary {
            files: &files,
            cli_config_changed,
            next: ("agentstack doctor", "check the result"),
            verbose: true,
            ..Default::default()
        };
        let out = render_change_summary(&summary(false));
        assert!(out.contains("manifest · import"));
        assert!(
            !out.contains("Restart your agent CLIs"),
            "an import-only run must not advise a restart:\n{out}"
        );
        // But it is still present when a CLI config did change.
        let out_changed = render_change_summary(&summary(true));
        assert!(out_changed.contains("Restart your agent CLIs"));
    }

    // P30: the classifier separates agentstack's own bookkeeping (manifest,
    // .env, .gitignore, lockfile) from native CLI-side files that warrant a
    // restart.
    #[test]
    fn cli_config_classifier_excludes_agentstack_bookkeeping() {
        assert!(!is_cli_config_path("proj/.agentstack/agentstack.toml"));
        assert!(!is_cli_config_path("proj/agentstack.toml")); // legacy root layout
        assert!(!is_cli_config_path("proj/.env"));
        assert!(!is_cli_config_path("proj/.gitignore"));
        assert!(!is_cli_config_path("proj/agentstack.lock"));
        // Native CLI-side artifacts.
        assert!(is_cli_config_path("~/.claude.json"));
        assert!(is_cli_config_path("proj/.mcp.json"));
        assert!(is_cli_config_path("proj/CLAUDE.md"));
        assert!(is_cli_config_path("~/.claude/skills/helper"));

        // An import-only file set is not a CLI-config change; adding any
        // rendered file flips it.
        let import_only = vec![
            (
                ".agentstack/agentstack.toml".to_string(),
                "manifest · import".to_string(),
            ),
            (".env".to_string(), ".env · lifted secrets".to_string()),
        ];
        assert!(!cli_config_touched(&import_only));
        let mut with_render = import_only.clone();
        with_render.push((
            "~/.claude.json".to_string(),
            "Claude Code · servers".to_string(),
        ));
        assert!(cli_config_touched(&with_render));
    }

    // P30: the cancel mini-summary lists what the import already wrote and the
    // undo one-liner — the truthful close for a post-import stop.
    #[test]
    fn stop_summary_lists_import_writes_and_the_undo() {
        let files = vec![
            (
                ".agentstack/agentstack.toml".to_string(),
                "manifest · import".to_string(),
            ),
            (".env".to_string(), ".env · lifted secrets".to_string()),
        ];
        let out = render_stop_summary(&files);
        assert!(out.contains("The import already wrote 2 files"));
        assert!(out.contains(".agentstack/agentstack.toml  (manifest · import)"));
        assert!(out.contains(".env  (.env · lifted secrets)"));
        assert!(out.contains("agentstack x restore --last --write"));
    }

    // P29.1: the summary's FINAL line is the start-page doorway, present on
    // every delivery-mode fork (all three end through this one formatter).
    #[test]
    fn change_summary_ends_with_the_start_page_doorway() {
        let out = render_change_summary(&ChangeSummary {
            next: ("agentstack doctor", "check the result"),
            ..Default::default()
        });
        // The exact URL + single-space em dash pins that the string
        // line-continuation collapsed to one space, not zero or two.
        assert!(out.contains(
            "https://tarekkharsa.github.io/agentstack/start.html — or run `agentstack` anytime"
        ));
        assert!(
            out.trim_end()
                .ends_with("or run `agentstack` anytime for your next step."),
            "the doorway must be the summary's final line, got:\n{out}"
        );
    }
}
