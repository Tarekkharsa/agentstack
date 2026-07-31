//! `agentstack set-mode` — switch this project's delivery mode, previewing the
//! real plan first (`set-mode-v1`).
//!
//! The mode is DERIVED from disk (`overview::mode_from_signals`), never
//! flagged — so "switching" means changing the facts the derivation reads:
//!
//! - **→ static**: render (the one activation path, `use --write`). The
//!   gateway registration, if any, stays: it is machine-wide and other
//!   projects may be served by it.
//! - **→ zero-files**: register the bridge in every installed harness, then
//!   un-render everything this manifest put in the repo (the same engine
//!   `uninstall` uses — [`super::unrender`]). Refuses while the project is
//!   not trusted at its current bytes: an untrusted project is served
//!   control-plane tools only, so the derived mode would keep reading
//!   something else and the panel would display a mode the system refuses.
//!   Trust itself is never granted here — that consent stays in the review.
//! - **→ clean-at-rest**: pin the lock, then un-render. Refuses while the
//!   bridge is registered and the project trusted, because those two facts
//!   READ as zero-files: the honest exit is `agentstack gateway disconnect`,
//!   a machine-scope decision this project-scope verb must not make.
//!
//! Consent follows the house pipeline: `--preview` returns the plan and a
//! digest over (action, params, manifest bytes); apply requires `--yes
//! --consented <digest>` (or the interactive confirm) and refuses on drift.
//! An active session blocks every direction — mid-session the repo's files
//! belong to the session's revert snapshot.

use std::io::IsTerminal;
use std::path::Path;

use anyhow::{Context as _, Result};
use owo_colors::OwoColorize;
use serde_json::{json, Map, Value};

use crate::cli::PanelSetModeArgs;
use crate::commands::overview::Mode;
use crate::scope::Scope;
use crate::state::State;

use super::unrender;

/// The blockers an apply refuses on, computed once so the preview names them
/// and the apply enforces the same facts (recomputed fresh at apply time).
struct Signals {
    current: Mode,
    trusted: bool,
    gateway_connected: bool,
    session: Option<String>,
    /// Recorded active toolset (project scope first), for the render leg.
    active_profile: Option<String>,
}

fn read_signals(ctx: &super::Context) -> Signals {
    let all_ids: Vec<String> = ctx.registry.ids().map(str::to_string).collect();
    let base = crate::manifest::project_root_of(&ctx.dir);
    let state = State::load().unwrap_or_default();
    let scope = Scope::default_for(&ctx.dir);
    let active_profile = all_ids.iter().find_map(|id| {
        let key = crate::state::target_key(id, scope, &ctx.dir);
        state.active_profile(&key)
    });
    Signals {
        current: super::overview::detect_mode(ctx, &all_ids),
        trusted: crate::trust::check(&base) == crate::trust::TrustState::Trusted,
        gateway_connected: super::overview::gateway_connected(ctx, &all_ids),
        session: crate::session::active(&ctx.dir).map(|s| s.profile),
        active_profile,
    }
}

/// Parse the requested mode label (the same kebab labels `doctor-mode-v1`
/// emits, so a panel round-trips the string it read).
fn parse_mode(label: &str) -> Result<Mode> {
    match label {
        "static" => Ok(Mode::Static),
        "clean-at-rest" => Ok(Mode::CleanAtRest),
        "zero-files" => Ok(Mode::ZeroFiles),
        other => anyhow::bail!(
            "unknown delivery mode '{other}' (expected static|clean-at-rest|zero-files)"
        ),
    }
}

/// The machine's bridge coverage: (detected, capable, incapable display names).
/// One definition for doctor's `clis` field and this plan — the two surfaces
/// that must never disagree about which CLIs live delivery reaches.
pub(crate) fn bridge_coverage(registry: &crate::adapter::Registry) -> (usize, usize, Vec<String>) {
    let mut detected = 0usize;
    let mut capable = 0usize;
    let mut incapable = Vec::new();
    for desc in registry.iter() {
        if !desc.detected() {
            continue;
        }
        detected += 1;
        if super::connect::bridge_capable(desc) {
            capable += 1;
        } else {
            incapable.push(desc.display.clone());
        }
    }
    (detected, capable, incapable)
}

/// Build the enveloped preview: the full plan for switching to `args.mode`,
/// with the `consent_digest` an apply must echo back. Public so the panel and
/// the parity witness read the same digest the apply recomputes.
pub fn set_mode_preview(args: &PanelSetModeArgs, dir: Option<&Path>) -> Result<Value> {
    let ctx = super::load(dir)?;
    let target = parse_mode(&args.mode)?;
    let signals = read_signals(&ctx);
    let root = crate::manifest::project_root_of(&ctx.dir);

    // The digest binds the DIRECTION, not just the destination: a switch
    // consented as static→zero-files must not apply after something else
    // already moved the project — the plan the user read no longer describes
    // the change.
    let params = json!({ "mode": target.label(), "current": signals.current.label() });
    let digest = super::panel_edit::action_digest(
        "set-mode",
        &params,
        &super::panel_edit::manifest_bytes(dir)?,
    );

    let state = State::load().unwrap_or_default();
    // Un-render is the plan for the two nothing-at-rest modes; static renders.
    let plan = if target == Mode::Static {
        None
    } else {
        Some(unrender::plan(
            &ctx,
            &state,
            &[Scope::Project, Scope::Global],
            /*own_global_only=*/ true,
        )?)
    };
    let gitignore_removal =
        (target != Mode::Static).then(|| unrender::plan_gitignore_removal(&root).is_some());

    let mut body = Map::new();
    body.insert("mode".into(), target.label().into());
    body.insert("current_mode".into(), signals.current.label().into());
    body.insert("changed".into(), (target != signals.current).into());

    // What comes OFF disk, as display labels + project-relative paths — the
    // exact rows the panel's plan card draws.
    let removes: Vec<Value> = plan
        .as_ref()
        .map(|p| {
            p.removals
                .iter()
                .map(|r| {
                    json!({
                        "label": r.label,
                        "path": super::init::display_path(&r.path, &root),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    body.insert("removes".into(), removes.into());
    body.insert(
        "removes_gitignore_block".into(),
        gitignore_removal.unwrap_or(false).into(),
    );
    body.insert(
        "removes_instructions".into(),
        plan.as_ref().is_some_and(|p| p.removes_instructions).into(),
    );

    // The render leg (→ static): which toolset would be activated. A
    // multi-toolset project with no recorded activation cannot pick one
    // implicitly — carried as a blocker, not guessed.
    let render_blocker = if target == Mode::Static {
        match super::use_profile::selected_profile(
            &ctx.loaded.manifest,
            signals.active_profile.as_deref(),
        ) {
            Ok(profile) => {
                body.insert(
                    "renders".into(),
                    json!({ "profile": profile.unwrap_or_else(|| "everything declared".into()) }),
                );
                None
            }
            Err(e) => Some(crate::text::sanitize_line(&format!("{e:#}"))),
        }
    } else {
        body.insert("renders".into(), Value::Null);
        None
    };
    if let Some(b) = &render_blocker {
        body.insert("render_blocker".into(), b.clone().into());
    }

    // The lock leg (→ clean-at-rest): sessions activate from pinned refs.
    body.insert(
        "locks".into(),
        (target == Mode::CleanAtRest && !crate::lock::Lock::path(&ctx.dir).exists()).into(),
    );

    // The bridge leg (→ zero-files): machine-wide registration + coverage.
    if target == Mode::ZeroFiles {
        let (detected, capable, incapable) = bridge_coverage(&ctx.registry);
        body.insert(
            "bridge".into(),
            json!({
                "registers": !signals.gateway_connected,
                "detected": detected,
                "capable": capable,
                "incapable": incapable,
            }),
        );
        body.insert("requires_trust".into(), (!signals.trusted).into());
        body.insert("machine_scope".into(), true.into());
    } else {
        body.insert("bridge".into(), Value::Null);
        body.insert("requires_trust".into(), false.into());
        body.insert("machine_scope".into(), false.into());
    }

    // → clean-at-rest while the machine serves this trusted project live:
    // those facts derive zero-files whatever we remove, so the switch cannot
    // be honored project-scope (see module doc).
    if target == Mode::CleanAtRest && signals.gateway_connected && signals.trusted {
        body.insert(
            "mode_blocker".into(),
            "this trusted project is served live by the machine-wide bridge; \
             `agentstack gateway disconnect --all` first if you want session-based delivery"
                .into(),
        );
    }

    if let Some(profile) = &signals.session {
        body.insert("session_active".into(), profile.clone().into());
    }
    body.insert("undo".into(), "agentstack restore --last".into());

    Ok(super::panel_edit::build_preview("set-mode", &digest, body))
}

pub fn set_mode(args: &PanelSetModeArgs, dir: Option<&Path>) -> Result<()> {
    set_mode_gated(args, dir, std::io::stdin().is_terminal())
}

fn set_mode_gated(args: &PanelSetModeArgs, dir: Option<&Path>, interactive: bool) -> Result<()> {
    let preview = set_mode_preview(args, dir)?;
    if !args.consent.yes {
        if args.consent.preview {
            return super::panel_edit::emit(&preview);
        }
        if !interactive {
            anyhow::bail!(
                "nothing was changed — this is not a terminal, so there is no one to ask.\n\
                 Run it at a terminal, or pass --preview to get the plan and its consent \
                 digest, then re-run with --yes --consented <digest>."
            );
        }
        print_review(&preview);
        if !super::panel_edit::confirm("Switch?")? {
            println!("cancelled — nothing was written.");
            return Ok(());
        }
        let fresh = set_mode_preview(args, dir)?;
        super::panel_edit::verify_consent(
            Some(super::panel_edit::preview_digest(&preview)?),
            super::panel_edit::preview_digest(&fresh)?,
        )
        .context("the project changed while you were reviewing")?;
    } else {
        super::panel_edit::verify_consent(
            args.consent.consented.as_deref(),
            super::panel_edit::preview_digest(&preview)?,
        )?;
    }

    apply(args, dir)
}

/// Perform the consented switch. Every blocker is recomputed HERE from live
/// state — the preview names them for the user, the apply enforces them.
fn apply(args: &PanelSetModeArgs, dir: Option<&Path>) -> Result<()> {
    let ctx = super::load(dir)?;
    let target = parse_mode(&args.mode)?;
    let signals = read_signals(&ctx);

    if let Some(profile) = &signals.session {
        anyhow::bail!(
            "'{profile}' is in use here (a session is active) — `agentstack session end` \
             puts its files back first, then switch modes."
        );
    }
    if target == signals.current {
        anyhow::bail!(
            "this project already delivers as {} — nothing to switch.",
            target.label()
        );
    }

    match target {
        Mode::Static => {
            // The one activation path renders, records state, and re-pins.
            // An unresolved ${REF} fails it closed unless --allow-unresolved.
            let use_args = crate::cli::UseArgs {
                profile: signals.active_profile.clone(),
                targets: vec![],
                scope: None,
                write: true,
                allow_unresolved: args.consent.allow_unresolved,
                prune_foreign: false,
                no_gitignore: false,
                list: false,
                json: false,
            };
            super::use_profile::run(&use_args, dir)?;
            println!(
                "\n{} delivery is on disk again — configs rendered, kept out of git.",
                "✓".green()
            );
        }
        Mode::ZeroFiles => {
            // Trust is the per-repo gate the gateway serves through. Granting
            // it is a human review (never done here); refusing early keeps
            // the panel from applying a switch whose derived mode would still
            // read something else.
            anyhow::ensure!(
                signals.trusted,
                "this project is not trusted at its current bytes, so the gateway would \
                 serve it control-plane tools only. Review it first (`agentstack trust .`), \
                 then switch."
            );
            // Bridge first: if registration fails, nothing has been removed
            // and the project still works exactly as before.
            super::connect::run_connect(&crate::cli::ConnectArgs {
                harnesses: Vec::new(),
                all: true,
                transparent: false,
                write: true,
                command: None,
            })
            .context("registering the gateway failed; nothing was removed from this project")?;
            unrender_leg(&ctx, "set-mode zero-files")?;
            println!(
                "\n{} served live — nothing of this project's setup is rendered on disk; \
                 your CLIs fetch its capabilities from agentstack.",
                "✓".green()
            );
        }
        Mode::CleanAtRest => {
            anyhow::ensure!(
                !(signals.gateway_connected && signals.trusted),
                "this trusted project is served live by the machine-wide bridge, and those \
                 two facts read as zero-files whatever is removed here. \
                 `agentstack gateway disconnect --all` is the machine-scope exit — run it \
                 first if you want session-based delivery."
            );
            // Pin the lock so `session start` has a reviewed surface to
            // activate from; validates every ref before anything is removed.
            super::lock::run(
                &crate::cli::LockArgs {
                    profile: None,
                    update: None,
                    upgrade: None,
                    all: false,
                    with_instructions: false,
                    yes: false,
                    write: false,
                },
                dir,
            )?;
            unrender_leg(&ctx, "set-mode clean-at-rest")?;
            println!(
                "\n{} nothing stays rendered between sessions now. `agentstack session start \
                 <toolset>` materializes one while you work; `agentstack session end` puts \
                 every file back.",
                "✓".green()
            );
        }
    }
    Ok(())
}

/// The shared removal half: everything this manifest rendered comes off disk,
/// captured into ONE history entry so `agentstack restore --last` undoes it,
/// and the state ledger stops claiming the renders (or the derived mode would
/// keep reading "static" over files that no longer exist).
fn unrender_leg(ctx: &super::Context, operation: &str) -> Result<()> {
    let root = crate::manifest::project_root_of(&ctx.dir);
    let state = State::load()?;
    let plan = unrender::plan(
        ctx,
        &state,
        &[Scope::Project, Scope::Global],
        /*own_global_only=*/ true,
    )?;
    let mut removals = plan.removals;
    if let Some(removal) = unrender::plan_gitignore_removal(&root) {
        removals.push(removal);
    }

    if removals.is_empty() {
        println!(
            "  {} nothing was rendered here — nothing to remove",
            "·".dimmed()
        );
        return Ok(());
    }

    let mut backups = Vec::new();
    let mut labels = Vec::new();
    for r in removals {
        let capture = r
            .capture
            .then(|| crate::history::capture(&r.path, r.label.clone()));
        (r.write)()?;
        println!(
            "  {} {} {}",
            "✓".green(),
            "removed".dimmed(),
            super::init::display_path(&r.path, &root)
        );
        if let Some(capture) = capture {
            backups.push(capture);
            labels.push(r.label);
        }
    }
    crate::history::record("project", operation.to_string(), labels, backups)?;
    unrender::clear_managed_state(&plan.touched_keys)?;
    Ok(())
}

/// The terminal review — the same plan the JSON carries, drawn for a human.
fn print_review(preview: &Value) {
    let body = |ptr: &str| preview.pointer(ptr);
    let s = |ptr: &str| body(ptr).and_then(Value::as_str).unwrap_or("");
    let b = |ptr: &str| body(ptr).and_then(Value::as_bool).unwrap_or(false);

    println!(
        "\nSwitching delivery mode: {} → {}",
        s("/current_mode").bold(),
        s("/mode").bold()
    );
    if !b("/changed") {
        println!(
            "  {} already in that mode — applying will refuse",
            "·".dimmed()
        );
    }
    println!("\nThis would:");
    if let Some(removes) = body("/removes").and_then(Value::as_array) {
        for r in removes {
            println!(
                "  {} remove {}  {}",
                "−".red(),
                r["label"].as_str().unwrap_or(""),
                r["path"].as_str().unwrap_or("").dimmed()
            );
        }
    }
    if b("/removes_gitignore_block") {
        println!("  {} remove the managed .gitignore block", "−".red());
    }
    if b("/locks") {
        println!(
            "  {} pin agentstack.lock (sessions activate from it)",
            "+".green()
        );
    }
    if let Some(renders) = body("/renders").filter(|v| !v.is_null()) {
        println!(
            "  {} render configs for '{}' into your CLIs",
            "+".green(),
            renders["profile"].as_str().unwrap_or("")
        );
    }
    if let Some(blocker) = body("/render_blocker").and_then(Value::as_str) {
        println!("  {} {}", "!".yellow(), blocker);
    }
    if let Some(bridge) = body("/bridge").filter(|v| !v.is_null()) {
        let detected = bridge["detected"].as_u64().unwrap_or(0);
        let capable = bridge["capable"].as_u64().unwrap_or(0);
        if bridge["registers"].as_bool().unwrap_or(false) {
            println!(
                "  {} register the agentstack bridge in your CLI configs ({capable} of \
                 {detected} installed CLIs can host it)",
                "+".green()
            );
        } else {
            println!("  {} the bridge is already registered", "·".dimmed());
        }
        if let Some(incapable) = bridge["incapable"].as_array() {
            if !incapable.is_empty() {
                let names: Vec<&str> = incapable.iter().filter_map(Value::as_str).collect();
                println!(
                    "  {} {} can't consume live delivery: {}",
                    "!".yellow(),
                    super::count(names.len(), "CLI"),
                    names.join(", ")
                );
            }
        }
    }
    if b("/removes_instructions") {
        println!(
            "  {} compiled CLAUDE.md / AGENTS.md instructions are not delivered live",
            "!".yellow()
        );
    }
    println!("  {} undo: {}", "↺".dimmed(), s("/undo").bold());

    if b("/requires_trust") {
        println!(
            "\n{} this project is not trusted at its current bytes — review it first \
             (`agentstack trust .`); applying will refuse until then.",
            "⚠".yellow()
        );
    }
    if let Some(blocker) = body("/mode_blocker").and_then(Value::as_str) {
        println!("\n{} {}", "⚠".yellow(), blocker);
    }
    if let Some(session) = body("/session_active").and_then(Value::as_str) {
        println!(
            "\n{} '{session}' is in use here — `agentstack session end` first.",
            "⚠".yellow()
        );
    }
    if b("/machine_scope") {
        println!(
            "\n{} Machine-wide: registering the bridge changes every CLI's config on this \
             machine, not just this project. Switching this project back later does not \
             unregister it.",
            "⚠".yellow()
        );
    }
    println!();
}
