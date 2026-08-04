//! `agentstack set-mode` — **retired.** The verb refuses; what is left here is
//! the plan builder and the bridge-coverage reading other commands use.
//!
//! The Mode axis retired with STRATEGY.md v3 (TODO.md item 9). Mode asked the
//! user to choose between static, clean-at-rest and zero-files. v3 deleted the
//! choice: the delivery planner routes each capability by kind and harness,
//! and `status` reports what it decided. `set-mode-v1` is listed in
//! [`crate::ui_contract::SUPERSEDED`], so a panel can tell a binary that
//! retired the picker from one too old to have it.
//!
//! What survives, and why:
//!
//! - [`set_mode_preview`] — builds the un-render plan and writes nothing. The
//!   panel-parity witness reads it, and it is the honest answer to "what would
//!   come off disk here".
//! - [`bridge_coverage`] — `doctor`'s `clis` field reads it. One definition,
//!   so the count a UI shows and the set `gateway connect` reaches agree.
//!
//! The apply half is gone; `uninstall` reaches [`super::unrender`] directly.
//! The description below is kept because the preview still computes it.
//!
//! The mode is DERIVED from disk (`overview::mode_from_signals`), never
//! flagged — so a "switch" meant changing the facts the derivation reads:
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
//! The preview still returns a `consent_digest` over (action, params,
//! manifest bytes). Nothing consumes it now that the apply is gone, and it is
//! kept rather than stripped: the digest is what makes the plan quotable, and
//! a preview whose shape changed would break the parity witness for a reason
//! unrelated to the retirement.

use std::path::Path;

use anyhow::Result;
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
    body.insert("undo".into(), "agentstack x restore --last".into());

    Ok(super::panel_edit::build_preview("set-mode", &digest, body))
}

/// `set-mode` — **retired.** It refuses, and names what replaced it.
///
/// The Mode axis retired with STRATEGY.md v3 (TODO.md item 9): delivery is
/// routed by the planner, per kind and per harness, and is not a setting a
/// user picks. A verb that still switched a mode would put back the concept
/// v3 deleted, and the picker it feeds is retired in the ui-contract as
/// `set-mode-v1`.
///
/// It refuses rather than disappearing because a scripted caller deserves a
/// sentence naming the replacement, not a clap usage error. The machinery
/// below stays: `set_mode_preview` is still the un-render plan builder, and
/// the leg that removes rendered files is shared with `uninstall`, which is
/// where a user who wants files gone now goes.
pub fn set_mode(_args: &PanelSetModeArgs, _dir: Option<&Path>) -> Result<()> {
    anyhow::bail!(
        "nothing was changed — `set-mode` is retired, and delivery is no longer a mode you pick.\n\
         AgentStack routes each capability to its lane by kind and harness. \
         `agentstack status` says what it decided, per CLI, and where.\n\
         To stop rendering files for this project: `agentstack x uninstall`.\n\
         To keep one project or harness on rendered files: `agentstack x delivery render-locally`."
    )
}

// The apply half — `set_mode_gated`, `apply`, `unrender_leg` and
// `print_review` — was deleted when the Mode axis retired (STRATEGY.md v3,
// TODO.md item 9). It had no caller left once `set_mode` began refusing, and
// a retired verb keeping a live switch path is exactly the second authority
// this codebase does not allow. Its removal leg was never unique to it:
// `uninstall` reaches the same `super::unrender::plan` directly, and that is
// where a user who wants rendered files gone now goes.
//
// `set_mode_preview` above is kept: it is the un-render PLAN builder, it
// writes nothing, and `bridge_coverage` below it is what `doctor` reads.
