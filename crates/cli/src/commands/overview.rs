//! Bare `agentstack` — orientation instead of a wall of subcommands: what's
//! detected on this machine, what state this directory's manifest is in, and
//! the one next command to run.

use std::path::Path;

use anyhow::Result;
use owo_colors::OwoColorize;

use crate::adapter::Registry;
use crate::manifest::load::MANIFEST_FILE;
use crate::scope::Scope;

/// The three delivery modes a project can be in (see docs/design P4). They are
/// not stored anywhere — a project's mode is *derived* from what's observable on
/// disk, so "which mode am I in?" is never archaeology. Rust enums with methods
/// are like a TypeScript union type paired with a lookup table.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Mode {
    /// Rendered configs live on disk (static, the default).
    Static,
    /// Nothing between sessions; `session start`/`end` materialize + revert.
    CleanAtRest,
    /// Nothing ever written; the gateway serves capabilities live over MCP.
    ZeroFiles,
}

impl Mode {
    /// The short name shown on the orientation line and in the setup choice.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Mode::Static => "static",
            Mode::CleanAtRest => "clean-at-rest",
            Mode::ZeroFiles => "zero-files",
        }
    }

    /// A terse descriptor for the one-line orientation display.
    pub(crate) fn short(self) -> &'static str {
        match self {
            Mode::Static => "config files on disk, kept out of git",
            Mode::CleanAtRest => "active only while you work, then restored",
            Mode::ZeroFiles => "nothing on disk — served live to your CLIs after review",
        }
    }

    /// The full one-line help (docs/design P4 wording), shown when setup
    /// presents the three modes as a choice. Outcome language first (Stage 1.4):
    /// the gateway/trust mechanics stay in the docs and the commands themselves,
    /// not in the first-run copy.
    pub(crate) fn help(self) -> &'static str {
        match self {
            Mode::Static => "Config files stay on disk, kept out of git. Works with every CLI, zero moving parts. This is what you have now.",
            Mode::CleanAtRest => "Use a toolset temporarily: `agentstack session start` activates it and `session end` puts every file back exactly as it was. Nothing stays in your repo between sessions.",
            Mode::ZeroFiles => "Nothing is ever written; your CLIs fetch servers and skills live from agentstack, and each repo stays inert until you review it once. Best when you work across many repos.",
        }
    }
}

/// Decide the mode from the observable signals alone — a pure function so the
/// decision is testable without touching disk. Priority follows P4's
/// definitions: anything rendered on disk *is* static; otherwise a
/// trust-gated gateway registration means zero-files; a lockfile with nothing
/// rendered means clean-at-rest; a bare, never-activated project reads as the
/// default (static). Ambiguity resolves to the closest, without hand-wringing.
pub(crate) fn mode_from_signals(
    rendered: bool,
    gateway_connected: bool,
    trusted: bool,
    locked: bool,
) -> Mode {
    if rendered {
        Mode::Static
    } else if gateway_connected && trusted {
        Mode::ZeroFiles
    } else if locked {
        Mode::CleanAtRest
    } else {
        Mode::Static
    }
}

/// Has this project rendered any managed artifact? Reuses the apply/use write
/// ledger (`State`): a non-empty managed set for one of the project's target
/// keys means agentstack wrote configs or materialized skills here. Global-scope
/// keys are shared across manifests, so an entry only counts as *this* project's
/// when its recorded source manifest matches (the same guard `foreign_prunes`
/// uses); project-scope keys are already per-root.
pub(crate) fn has_rendered_artifacts(ctx: &super::Context, target_ids: &[String]) -> bool {
    let Ok(state) = crate::state::State::load() else {
        return false;
    };
    let scope = Scope::default_for(&ctx.dir);
    let identity = crate::state::manifest_identity(&ctx.dir);
    target_ids.iter().any(|id| {
        let key = crate::state::target_key(id, scope, &ctx.dir);
        let Some(t) = state.targets.get(&key) else {
            return false;
        };
        let ours =
            scope != Scope::Global || t.source_manifest.as_deref().is_none_or(|s| s == identity);
        ours && (!t.managed_servers.is_empty()
            || !t.managed_skills.is_empty()
            || !t.managed_settings.is_empty()
            || !t.managed_hooks.is_empty())
    })
}

/// Is the agentstack gateway registered in any detected harness for this
/// project's targets? Same probe `doctor`'s zero-files section runs.
pub(crate) fn gateway_connected(ctx: &super::Context, target_ids: &[String]) -> bool {
    target_ids.iter().any(|id| {
        let Some(desc) = ctx.registry.get(id) else {
            return false;
        };
        let (Some(cfg), Some(mcp)) = (desc.config.as_ref(), desc.mcp.as_ref()) else {
            return false;
        };
        if !desc.detected() {
            return false;
        }
        let path = crate::util::paths::expand_tilde(&cfg.path);
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        crate::commands::connect::has_bridge_entry(&existing, &mcp.location, cfg.format)
    })
}

/// Observe this project's current delivery mode from disk state.
pub(crate) fn detect_mode(ctx: &super::Context, target_ids: &[String]) -> Mode {
    let base = crate::manifest::project_root_of(&ctx.dir);
    let trusted = crate::trust::check(&base) == crate::trust::TrustState::Trusted;
    let locked = crate::lock::Lock::path(&ctx.dir).exists();
    mode_from_signals(
        has_rendered_artifacts(ctx, target_ids),
        gateway_connected(ctx, target_ids),
        trusted,
        locked,
    )
}

/// The single next command bare orientation recommends, from cheap signals.
/// Trust routing is the headline *only when trusting buys something here*
/// (`trust_relevant`, P16 refined): the gateway/bridge is registered for a
/// harness, or the derived mode depends on the trust gate (zero-files /
/// clean-at-rest). In those cases an untrusted or trust-stale manifest points
/// at `trust .` first, because until the digest is pinned the bridge serves
/// control-plane tools only and no server runs — trusting is the gate. A
/// static, no-gateway project gains nothing from trusting: its configs render
/// through `apply`/`use` whatever the trust state, and no bridge exists to
/// unlock. So it is *not* nagged toward a `trust .` that never converges — its
/// untrusted state stays a true Status label, and the next step falls through
/// to the normal ladder. That ladder: capabilities declared but nothing on
/// disk yet → `apply --write`; otherwise the wiring is in place → `doctor`.
/// Pure over its inputs so the routing is unit-tested without touching disk.
///
/// **This function never recommends `init`.** It is only reached once a
/// manifest has loaded, and `init` refuses when one exists — so recommending
/// it here sent a user who had just finished a successful first run into an
/// error. The state that used to trigger it (`!locked`, i.e. never activated)
/// is not something `init` fixes: a project is unlocked until `use`/`lock`
/// runs, which is a normal resting state for a static setup, not an unfinished
/// import. The honest question is "is this rendered?", which `rendered`
/// answers.
pub(crate) fn next_step(
    trust: crate::trust::TrustState,
    rendered: bool,
    has_capabilities: bool,
    trust_relevant: bool,
    no_toolsets: bool,
    unimported_native: bool,
    undeclared_drops: bool,
) -> (&'static str, &'static str) {
    use crate::trust::TrustState;
    // A dropped-but-undeclared file outranks everything except a pending
    // re-review: the drop is the newest thing the user did, and `agentstack
    // yes` is the one verb built for it. Until this branch existed, `yes` was
    // orphaned — every detection surface routed drops to `adopt` or `trust .`
    // and the funnel was unreachable without reading the docs. Routing here
    // skips no review: `yes` holds clone-supplied content and collisions on
    // its preview and names the explicit path for them. It does NOT outrank
    // `TrustState::Changed` — content the user already approved has drifted,
    // and that re-review keeps the headline; the drop is offered next.
    if undeclared_drops && trust != TrustState::Changed {
        return (
            "agentstack yes",
            "dropped files are waiting — review them and take them live",
        );
    }
    match trust {
        TrustState::Untrusted if trust_relevant => {
            ("agentstack trust .", "review it to unlock its servers")
        }
        // Trust-stale routes here whether or not trust is "relevant". The
        // relevance test asks whether trusting *unlocks* something, which is the
        // right question for a project that has never been trusted. It is the
        // wrong question once content the user already approved has CHANGED:
        // that state is a re-review the Status line is already reporting, and
        // routing it to `doctor` made the cue cost two commands — status naming
        // doctor, doctor naming the review (pilot Run A).
        TrustState::Changed => (
            "agentstack trust .",
            "the content changed since you reviewed it — review and re-trust",
        ),
        // Untrusted but trust changes nothing here (static, no gateway), or
        // already trusted: fall through to the normal ladder.
        _ => {
            // Servers configured natively here that this manifest doesn't know
            // about. Ahead of `apply`, because rendering a manifest that omits
            // half the setup is not the step that helps.
            if unimported_native {
                (
                    "agentstack adopt",
                    "servers are configured here that this setup doesn't cover yet",
                )
            } else if has_capabilities && !rendered {
                (
                    "agentstack apply --write",
                    "render this setup into your CLIs",
                )
            } else if rendered && no_toolsets {
                // The wiring is done. `doctor` here was a dead end: a user who
                // ran it clean was offered it again, with nothing on screen
                // saying the journey continues (pilot Run A). The next rung of
                // the ladder is Switch, and it is stated as one.
                (
                    "agentstack toolset create <name> --server <server>",
                    "group these for a task, then switch between toolsets",
                )
            } else {
                (
                    "agentstack doctor",
                    "verify the wiring — every warning names its fix",
                )
            }
        }
    }
}

/// Delivery-mode override for the normal trust/init/doctor ladder. A trusted,
/// locked clean-at-rest project is ready to use; teach the session rhythm at
/// the moment it matters instead of sending it back through another doctor
/// pass. Active sessions point at their matching close operation.
pub(crate) fn clean_at_rest_next_step(
    mode: Mode,
    trust: crate::trust::TrustState,
    locked: bool,
    session_active: bool,
    profile: &str,
) -> Option<(String, &'static str)> {
    if mode != Mode::CleanAtRest || trust != crate::trust::TrustState::Trusted || !locked {
        return None;
    }
    if session_active {
        Some((
            "agentstack session end".to_string(),
            "finish this session and restore the clean-at-rest state",
        ))
    } else {
        Some((
            format!("agentstack session start {profile}"),
            "materialize the toolset for this session",
        ))
    }
}

/// The one-line explanation of an untrusted (or trust-stale) manifest shown
/// under the Status line (P16). `None` for a trusted manifest — there is
/// nothing to teach. A `&'static str` because the sentence never varies. The
/// caller shows it only when trust is *relevant* here (a bridge exists): the
/// note describes the bridge serving control-plane tools only, which is simply
/// untrue for a static, no-gateway project whose servers render regardless —
/// so that project keeps the honest `· untrusted` Status label without this
/// line.
pub(crate) fn orientation_trust_note(trust: crate::trust::TrustState) -> Option<&'static str> {
    use crate::trust::TrustState;
    match trust {
        TrustState::Untrusted | TrustState::Changed => {
            Some("its servers are inert — the gateway serves control-plane tools only until you review it")
        }
        TrustState::Trusted => None,
    }
}

/// The named profile roster for orientation (P18): every name for a small set,
/// a truncated `N profiles: a, b, c, …` beyond four, with the active profile
/// (when a live session pins one) marked inline. Declaration order is kept —
/// the truncation shows the first three, so an active profile past that window
/// is not marked, which is honest: orientation stays a glance, not a report.
/// Pure over its inputs so the formatting is unit-tested without a manifest.
pub(crate) fn profiles_line(names: &[String], active: Option<&str>) -> String {
    let render = |n: &String| -> String {
        if Some(n.as_str()) == active {
            format!("{n} (active)")
        } else {
            n.clone()
        }
    };
    if names.len() <= 4 {
        names.iter().map(render).collect::<Vec<_>>().join(", ")
    } else {
        let shown = names
            .iter()
            .take(3)
            .map(render)
            .collect::<Vec<_>>()
            .join(", ");
        format!("{} toolsets: {shown}, …", names.len())
    }
}

/// Humanize how long a session has been running, for the Session status line.
/// Pure so the buckets are unit-testable.
pub(crate) fn session_age(secs: u64) -> String {
    if secs < 60 {
        "started just now".to_string()
    } else if secs < 3600 {
        format!("started {}m ago", secs / 60)
    } else {
        format!("started {}h {}m ago", secs / 3600, (secs % 3600) / 60)
    }
}

/// The Session status line's two parts — (headline, recovery hint) — for the
/// default surface. A live session states it is active temporarily; one that
/// reads as abandoned (Stage 2.2) is flagged as such and leads with the same
/// safe `session end` recovery. Pure so both wordings are unit-tested; the
/// abandoned judgment itself lives in `crate::session::is_abandoned`.
pub(crate) fn session_status_line(
    profile: &str,
    age_secs: u64,
    abandoned: bool,
) -> (String, String) {
    let end = "`agentstack session end` restores your files".to_string();
    if abandoned {
        (
            format!("'{profile}' looks abandoned ({})", session_age(age_secs)),
            format!("probably a closed terminal — {end}"),
        )
    } else {
        (
            format!("'{profile}' active temporarily ({})", session_age(age_secs)),
            end,
        )
    }
}

/// Everything the orientation screen observes, gathered in one pass so the
/// human screen and `status --json` are provably the same reading. Before
/// this, the screen computed each fact inline as it printed it; a JSON body
/// built alongside would have been a second implementation of the same
/// questions, free to answer them differently.
pub(crate) struct Orientation {
    /// Display names of the CLIs detected FOR THIS DIRECTORY — installed,
    /// configured on this machine, or carrying a project-scope config here.
    detected_clis: Vec<String>,
    /// How many adapters the catalog knows about at all. Kept apart from
    /// `detected_clis` because they answer different questions: the screen used
    /// to print "none detected on this machine" above "13 detected CLI(s)",
    /// which is one screen contradicting itself. The second number was never
    /// detection — it is the fan-out fallback, i.e. the catalog.
    catalog_size: usize,
    /// Native configs found here that the manifest does not (yet) cover.
    native: Vec<crate::discover::NativeConfig>,
    /// Files dropped into this project's own `skills/`/`instructions/` that the
    /// manifest does not declare. Inert until adopted — see `crate::intake`.
    intake: Vec<crate::intake::Item>,
    /// Where the manifest is (or would be).
    manifest_path: std::path::PathBuf,
    manifest: ManifestState,
    /// The single next step: (command, why).
    next: (String, &'static str),
}

/// Which of the three shapes the manifest reading took. A caller branches on
/// this rather than probing for fields: only `Loaded` has project facts, and
/// only `Broken` has a reason.
pub(crate) enum ManifestState {
    /// No manifest in this directory.
    Missing,
    /// A manifest file exists but does not load. Carries the reason, already
    /// formatted for display.
    Broken(String),
    /// It loaded, so every project fact below is real.
    Loaded(Box<ProjectFacts>),
}

pub(crate) struct ProjectFacts {
    servers: usize,
    skills: usize,
    /// The other declared kinds. Phase 3 item 5: `status` counted servers and
    /// skills only, so a project whose setup was mostly instruction fragments,
    /// hooks, or CLI settings reported a footprint of "0 servers" — a true
    /// number and a false impression, on the surface whose whole job is
    /// saying what you have. Counted, not enumerated: `explain` is where a
    /// kind is looked at one at a time.
    instructions: usize,
    settings: usize,
    hooks: usize,
    extensions: usize,
    /// `[targets].default`, empty when nothing is pinned.
    pinned_targets: Vec<String>,
    /// How many detected CLIs the commands would fan out to when nothing is
    /// pinned — the honest number behind "no [targets] pinned".
    fanout_targets: usize,
    /// Whether `fanout_targets` counts CLIs actually detected here, or is the
    /// whole-catalog fallback `resolve_targets` uses when nothing is detected.
    /// Without it the line cannot be phrased truthfully.
    fanout_detected: bool,
    toolsets: Vec<String>,
    session: Option<SessionFacts>,
    locked: bool,
    trust: crate::trust::TrustState,
    /// Whether trusting this project would change what it can do here (a
    /// bridge is registered, or the mode depends on the gate). Drives both the
    /// "inert servers" note and whether trust is the headline next step.
    trust_relevant: bool,
    mode: Mode,
    gateway_connected: bool,
    rendered: bool,
    /// `None` when the caller did not ask for the secrets reading — bare
    /// `agentstack` never does, and asking is not free (it consults every
    /// resolver).
    secrets: Option<SecretFacts>,
    /// Refusals recorded against this project since its last yes (W1). `None`
    /// for a trusted project and for one nothing has tried to use.
    needs_your_yes: Option<NeedsYourYes>,
    /// Installed packs with a newer version resolvable **offline** (see
    /// [`crate::commands::updates::available_updates`]). Empty means "nothing
    /// to offer here", which is NOT the same as "up to date": the check never
    /// touches the network, so a pack whose local clone is stale, absent, or
    /// catalog-sourced is silently missing from this list. Every rendering of
    /// it offers; none of them may claim currency.
    updates: Vec<crate::commands::updates::PackUpdate>,
}

pub(crate) struct SessionFacts {
    profile: String,
    started_unix: u64,
    age_secs: u64,
    abandoned: bool,
}

/// W1 — "needs your yes": the evidence-bearing form of an untrusted or drifted
/// project. Present only when calls were actually refused here since the last
/// yes, which is what separates "you have not reviewed this yet" (a state) from
/// "something tried to work and could not" (a consequence).
///
/// It carries no card. The one authoritative card is rendered by the command
/// named in [`NeedsYourYes::fix`] — `agentstack trust` — and there is
/// deliberately no second construction of it here or anywhere a UI could reach
/// without going through that command.
pub(crate) struct NeedsYourYes {
    /// How many refusals were recorded for this project since it was last
    /// trusted (since the beginning of the log when it never was).
    pub(crate) refused: usize,
    /// The most recent refusal's timestamp — so a surface can say "just now"
    /// rather than only "at some point".
    pub(crate) last_refused_ts: u64,
    /// The one command that answers it, naming the project explicitly so a
    /// caller acting from another directory does not have to guess.
    pub(crate) fix: String,
}

/// Count this project's recorded trust refusals since its last yes.
///
/// `None` for a trusted project — and cheaply so: the whole read is skipped,
/// which matters because `status` must feel instant and this walks the machine
/// audit log. A project that is untrusted or drifted has already lost the fast
/// path in every other sense, and it is the only one that can have anything to
/// report.
///
/// `manifest_dir` must be the same string form `seatbelt::record` files under
/// (the resolved manifest dir), or the filter silently matches nothing.
pub(crate) fn needs_your_yes(
    manifest_dir: &Path,
    root: &Path,
    trust: crate::trust::TrustState,
) -> Option<NeedsYourYes> {
    if trust == crate::trust::TrustState::Trusted {
        return None;
    }
    // Since the last yes, not since forever: refusals from before a grant
    // describe a project state the user already answered. No entry (never
    // trusted, or revoked) means the whole log is in scope.
    let since = crate::trust::TrustStore::load()
        .trusted
        .get(&crate::trust::key_for(root))
        .map(|entry| entry.trusted_at)
        .unwrap_or(0);
    let want = manifest_dir.display().to_string();
    let refusals: Vec<u64> = crate::calllog::read_all()
        .into_iter()
        .filter(|rec| {
            rec.tool == "trust"
                && rec.outcome == crate::calllog::CallOutcome::Denied
                && rec.ts >= since
                && rec.project.as_deref() == Some(want.as_str())
        })
        .map(|rec| rec.ts)
        .collect();
    if refusals.is_empty() {
        return None;
    }
    Some(NeedsYourYes {
        refused: refusals.len(),
        last_refused_ts: refusals.iter().copied().max().unwrap_or_default(),
        fix: format!(
            "agentstack trust {}",
            crate::text::sanitize_line(&root.display().to_string())
        ),
    })
}

pub(crate) struct SecretFacts {
    referenced: usize,
    /// The `${REF}` names that resolve from no layer here. NAMES only — a
    /// value never reaches this struct, let alone a serializer (invariant 5).
    unresolved: Vec<String>,
}

/// `agentstack status` — the orientation screen by name, plus the cheap health
/// signals a glance wants (secrets resolving?) and the pointer to the deep
/// check. Everything expensive (drift rendering, content scans) stays in
/// `doctor`; status must feel instant.
pub fn run_status(manifest_dir: Option<&Path>, json: bool) -> Result<()> {
    // `--json` changes only the rendering: the same collect, with the same
    // deep readings the named `status` screen already asks for.
    let orientation = collect(manifest_dir, true)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&crate::ui_contract::envelope(status_json(&orientation)))?
        );
        return Ok(());
    }
    print_orientation(&orientation, true);
    Ok(())
}

pub fn run(manifest_dir: Option<&Path>) -> Result<()> {
    let orientation = collect(manifest_dir, false)?;
    print_orientation(&orientation, false);
    Ok(())
}

/// The `status --json` body without the envelope (its caller wraps it) — the
/// testable seam, same shape as `workflow_list_json`.
///
/// `project` is `null` unless the manifest loaded: a consumer branches on that
/// one field instead of finding a dozen nulls, and `manifest.error` says why
/// when it is null for the second reason. `next_action` reuses the key
/// `doctor --json` already established for the same idea, because it IS the
/// same idea — one command, and why.
pub(crate) fn status_json(o: &Orientation) -> serde_json::Value {
    let (present, error, project) = match &o.manifest {
        ManifestState::Missing => (false, None, serde_json::Value::Null),
        ManifestState::Broken(err) => (
            true,
            Some(crate::text::sanitize_line(err)),
            serde_json::Value::Null,
        ),
        ManifestState::Loaded(f) => (true, None, project_json(f)),
    };
    serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        // Display names, not ids — the same list the screen prints. Adapter
        // ids live in `adapters list --json`, which is the read for that.
        "clis_detected": o.detected_clis,
        // The catalog size, kept a separate key for the same reason the screen
        // keeps it a separate sentence — it is not a detection count.
        "clis_supported": o.catalog_size,
        // Native configs found here carrying servers this manifest doesn't
        // declare. Names only; a discovered config's contents never land here.
        "native_unimported": o.native.iter()
            .filter(|n| !n.unimported.is_empty())
            .map(|n| serde_json::json!({
                "id": n.id,
                "scope": n.scope.as_str(),
                "path": n.path.display().to_string(),
                "servers": n.unimported,
            }))
            .collect::<Vec<_>>(),
        // Dropped-but-undeclared content in this project's own intake dirs.
        // Names, kinds, and the provenance classification — never file bodies.
        "intake": o.intake.iter().map(|i| serde_json::json!({
            "kind": i.kind.noun(),
            "name": i.name,
            "path": i.rel_path,
            "summary": i.summary,
            "locally_authored": i.provenance.is_local(),
            "provenance": i.provenance.reason(),
        })).collect::<Vec<_>>(),
        "manifest": {
            "path": o.manifest_path.display().to_string(),
            "present": present,
            "loaded": matches!(o.manifest, ManifestState::Loaded(_)),
            "error": error,
        },
        "project": project,
        "next_action": { "command": o.next.0, "why": o.next.1 },
    })
}

fn project_json(f: &ProjectFacts) -> serde_json::Value {
    let mut body = serde_json::json!({
        "servers": f.servers,
        "skills": f.skills,
        // Pinned targets and the fan-out count are different questions, so
        // they are different fields: an empty `pinned` with `fanout` = 6 is
        // "no [targets] pinned, six detected CLIs", which one number cannot say.
        "targets": {
            "pinned": f.pinned_targets.iter().map(|t| crate::text::sanitize_line(t)).collect::<Vec<_>>(),
            "fanout": f.fanout_targets,
        },
        "toolsets": f.toolsets.iter().map(|t| crate::text::sanitize_line(t)).collect::<Vec<_>>(),
        "session": f.session.as_ref().map(|s| serde_json::json!({
            "profile": crate::text::sanitize_line(&s.profile),
            "started_unix": s.started_unix,
            "age_seconds": s.age_secs,
            "abandoned": s.abandoned,
        })),
        "locked": f.locked,
        // Same vocabulary `use --list --json` uses, so a UI holding both reads
        // does not need two trust lookup tables.
        "trust": match f.trust {
            crate::trust::TrustState::Trusted => "trusted",
            crate::trust::TrustState::Changed => "drifted",
            crate::trust::TrustState::Untrusted => "untrusted",
        },
        "trust_relevant": f.trust_relevant,
        "mode": f.mode.label(),
        "gateway_connected": f.gateway_connected,
        "rendered": f.rendered,
        // Names of unresolved refs, never values (invariant 5). `null` when
        // the caller did not ask for the reading at all — distinct from an
        // empty list, which means "asked, everything resolves".
        "secrets": f.secrets.as_ref().map(|s| serde_json::json!({
            "referenced": s.referenced,
            "unresolved": s.unresolved,
        })),
    });
    // `needs-your-yes-v1`. Inserted rather than emitted as `null`, because the
    // question it answers is "has anything been refused here", and a project
    // where nothing has must read exactly as it did before this field existed.
    // No card payload rides along: `fix` names the command that renders the one
    // authoritative card, and that command is the only thing that renders it.
    if let Some(n) = &f.needs_your_yes {
        body["needs_your_yes"] = serde_json::json!({
            "refused": n.refused,
            "last_refused_ts": n.last_refused_ts,
            "fix": n.fix,
        });
    }

    // `update-offer-v1`. Inserted, never emitted as `null` or as an empty
    // list: presence means "there is an offer", and its ABSENCE means only
    // "no offer was produced" — never "you are current" (the check is offline
    // and stays silent about everything it cannot answer). Keeping the key out
    // entirely is what stops a consumer from rendering an "up to date" badge
    // off an empty array.
    if !f.updates.is_empty() {
        if let Some(map) = body.as_object_mut() {
            map.insert(
                "updates".into(),
                serde_json::json!({
                    "packs": f.updates.iter().map(|u| serde_json::json!({
                        "name": u.name,
                        "current": u.current,
                        "available": u.available,
                    })).collect::<Vec<_>>(),
                    "fix": super::updates::fix_command(&f.updates),
                }),
            );
        }
    }
    body
}

/// Secrets at a glance for `status`: the single most common thing broken after
/// setup. Only the NAMES that fail to resolve are kept — `source_of` answers a
/// presence question, and nothing it learns beyond that leaves this function.
fn secret_facts(ctx: &super::Context) -> Option<SecretFacts> {
    let refs = ctx.loaded.manifest.referenced_secrets();
    if refs.is_empty() {
        return None;
    }
    let sources = crate::secret::SecretSources::detect(&ctx.dir);
    let unresolved: Vec<String> = refs
        .iter()
        .filter(|n| sources.source_of(n).is_none())
        .cloned()
        .collect();
    Some(SecretFacts {
        referenced: refs.len(),
        unresolved,
    })
}

/// The update **offer** (design §Update model rule 2): name that newer
/// versions exist and the one command that takes them. Nothing is printed when
/// there is no offer — deliberately not a green "up to date", because the
/// check behind this is offline and cannot prove currency (see
/// [`crate::commands::updates::available_updates`]).
///
/// Rule 4 shapes the second line: keep-pinned is the resting state, so this
/// offers and then says so. It must not read as a warning, a nag, or a claim
/// that staying put is a fault.
fn print_updates_line(updates: &[crate::commands::updates::PackUpdate]) {
    if updates.is_empty() {
        return;
    }
    let list = updates
        .iter()
        .map(|u| format!("{} {} → {}", u.name, u.current, u.available))
        .collect::<Vec<_>>()
        .join(" · ");
    println!(
        "  {}  {} with a newer version: {list}",
        "Updates ".bold(),
        super::count(updates.len(), "pack")
    );
    println!(
        "            {}",
        format!(
            "take it with `{}` — staying on the pinned version is a complete answer",
            crate::commands::updates::fix_command(updates)
        )
        .dimmed()
    );
}

/// One aligned line when everything resolves; one line per missing secret,
/// each carrying its exact fix command.
fn print_secrets_line(facts: &SecretFacts) {
    if facts.unresolved.is_empty() {
        println!(
            "  {}  {} referenced, all resolve",
            "Secrets ".bold(),
            facts.referenced
        );
    } else {
        for name in &facts.unresolved {
            println!(
                "  {}  {} {name} not set   {}",
                "Secrets ".bold(),
                "✗".red(),
                format!("fix: agentstack secret set {name}").dimmed()
            );
        }
    }
}

/// The "you already have a setup here" line. Silent when every native config
/// found is already covered by the manifest — there is nothing to act on and a
/// permanent line would be noise. `no_manifest` changes only the pointer:
/// before a manifest exists the path in is `init`, after it, `adopt`.
fn print_native_line(native: &[crate::discover::NativeConfig], no_manifest: bool) {
    let pending: Vec<&crate::discover::NativeConfig> =
        native.iter().filter(|n| !n.unimported.is_empty()).collect();
    if pending.is_empty() {
        return;
    }
    let servers: usize = pending.iter().map(|n| n.unimported.len()).sum();
    let files = pending
        .iter()
        .map(|n| crate::text::sanitize_line(&tidy(&n.path)))
        .collect::<Vec<_>>()
        .join(" · ");
    println!(
        "  {}  {} configured here, not in this setup — {}",
        "Found   ".bold(),
        super::count(servers, "server"),
        files.dimmed()
    );
    println!(
        "            {}",
        if no_manifest {
            "`agentstack init` imports them"
        } else {
            "`agentstack adopt` brings them into the manifest"
        }
        .dimmed()
    );
}

/// Name dropped-but-undeclared skills/instructions, and what to do about them.
fn print_intake_line(items: &[crate::intake::Item]) {
    if items.is_empty() {
        return;
    }
    let names = items
        .iter()
        .map(|i| i.name.as_str())
        .collect::<Vec<_>>()
        .join(" · ");
    println!(
        "  {}  {} here, not in this setup — {}",
        "Dropped ".bold(),
        super::count(items.len(), "file"),
        names.dimmed()
    );
    // `yes` is the funnel built for exactly this moment. It is also the safe
    // router for content that may NOT take the short path: clone-supplied
    // drops and collisions are held on its preview with the reason and the
    // explicit-path commands, so naming `yes` here never skips a review.
    println!(
        "            {}",
        "`agentstack yes` reviews them and takes them live".dimmed()
    );
}

/// Project-relative display for a discovered config path, falling back to the
/// full path when it is not under the current directory.
fn tidy(path: &std::path::Path) -> String {
    std::env::current_dir()
        .ok()
        .and_then(|cwd| path.strip_prefix(cwd).ok())
        .unwrap_or(path)
        .display()
        .to_string()
}

/// Read every fact the orientation screen states. `deep_reads` is the one
/// knob, and it covers the two readings only the named `status` screen asks
/// for: the secrets resolution (bare `agentstack` has never consulted the
/// resolvers) and the offline update check (one local git ref read per
/// installed git pack). Gathering either unconditionally would make bare
/// `agentstack` slower for lines it does not print.
fn collect(manifest_dir: Option<&Path>, deep_reads: bool) -> Result<Orientation> {
    let registry = Registry::load()?;

    // Walk up to the project root so `agentstack` from `src/deep` describes
    // the ROOT manifest instead of steering toward a nested `init`.
    let base = super::project_base(manifest_dir)?;
    let dir = crate::manifest::resolve_manifest_dir(&base);
    let manifest_path = dir.join(MANIFEST_FILE);

    // Detection is asked OF THIS DIRECTORY (`detected_in`), not of the machine.
    // A repo whose only setup is a project-scope `.mcp.json` used to be reported
    // as "none detected on this machine" while four servers sat in the working
    // directory — the pilot's blocker #1.
    let detected_clis: Vec<String> = registry
        .iter()
        .filter(|d| d.detected_in(&dir))
        .map(|d| d.display.clone())
        .collect();

    if !manifest_path.exists() {
        // Nothing of ours here yet — so anything native we can see is the whole
        // story, and `init` must be told to expect it.
        let native = crate::discover::native_configs(&registry, &dir, &Default::default(), false);
        return Ok(Orientation {
            detected_clis,
            catalog_size: registry.ids().count(),
            native,
            intake: Vec::new(),
            manifest_path,
            manifest: ManifestState::Missing,
            next: (
                "agentstack init".to_string(),
                "guided one-command setup — import, preview, apply",
            ),
        });
    }

    let ctx = match super::load(manifest_dir) {
        Ok(ctx) => ctx,
        Err(err) => {
            return Ok(Orientation {
                detected_clis,
                catalog_size: registry.ids().count(),
                native: Vec::new(),
                intake: Vec::new(),
                manifest_path,
                manifest: ManifestState::Broken(format!("{err:#}")),
                next: ("agentstack doctor".to_string(), "diagnose the manifest"),
            })
        }
    };

    let m = &ctx.loaded.manifest;
    // No [targets] pinned → commands fan out to the detected CLIs (see
    // render::resolve_targets); "0 target(s)" would be false.
    let fanout_targets = crate::render::resolve_targets(m, &ctx.registry, &[], &ctx.dir)
        .map(|t| t.len())
        .unwrap_or_default();

    // The active profile is marked only when a live session pins it — the one
    // signal that *reliably* says which profile is loaded right now.
    let active_session = crate::session::active(&ctx.dir);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let session = active_session.as_ref().map(|s| SessionFacts {
        profile: s.profile.clone(),
        started_unix: s.started_unix,
        age_secs: now.saturating_sub(s.started_unix),
        abandoned: s.is_abandoned(now),
    });

    // Where this project actually stands, from cheap signals: lockfile (was it
    // ever activated/pinned?) and trust state.
    let project_root = crate::manifest::project_root_of(&ctx.dir);
    let trust = crate::trust::check(&project_root);
    let locked = crate::lock::Lock::path(&ctx.dir).exists();

    // Trust genuinely gates capability delivery only through the bridge
    // (zero-files) or the trust-gated run/session paths (clean-at-rest); a
    // static, no-gateway project renders through `apply`/`use` regardless. So
    // trust is the headline next-step, and the "inert servers" note is shown,
    // only when a bridge is registered or the mode depends on the gate.
    let target_ids: Vec<String> = ctx.registry.ids().map(str::to_string).collect();
    let gateway = gateway_connected(&ctx, &target_ids);
    let mode = detect_mode(&ctx, &target_ids);
    let trust_relevant = gateway || matches!(mode, Mode::ZeroFiles | Mode::CleanAtRest);

    let has_capabilities = !m.skills.is_empty() || !m.servers.is_empty();
    // "Is anything on disk for these targets?" — the signal that actually
    // distinguishes "imported but not applied" from "set up and resting".
    // `locked` does not: a static project stays unlocked until `use`/`lock` runs.
    let rendered = has_rendered_artifacts(&ctx, &target_ids);

    // Native configs here whose servers this manifest does not declare. Cheap
    // (a handful of small files at project scope) and the answer to the pilot's
    // silent case: a manifest that covers none of what is actually configured.
    let native = crate::discover::native_configs(&ctx.registry, &ctx.dir, &m.servers, false);
    let unimported = native.iter().any(|n| !n.unimported.is_empty());
    let any_detected = !detected_clis.is_empty();

    // Scanned before the next step is chosen: a dropped file changes what the
    // one next action is (`agentstack yes`), so the routing has to know.
    let intake = crate::intake::scan(&ctx.dir, &project_root, m).items;
    let undeclared_drops = !intake.is_empty();

    // W1: what actually got refused here since the last yes. Computed before
    // the next step is chosen, because evidence outranks every other routing
    // signal — see the `pending` override below.
    let pending = needs_your_yes(&ctx.dir, &project_root, trust);

    let fallback = next_step(
        trust,
        rendered,
        has_capabilities,
        trust_relevant,
        m.profiles.is_empty(),
        unimported,
        undeclared_drops,
    );
    let profile = if m.profiles.len() == 1 {
        m.profiles
            .keys()
            .next()
            .map(String::as_str)
            .unwrap_or("<toolset>")
    } else {
        "<toolset>"
    };
    // A waiting drop also outranks the clean-at-rest session rhythm: starting
    // a session materializes only what is declared, so it would deliver
    // everything EXCEPT the file the user just dropped.
    let next = clean_at_rest_next_step(mode, trust, locked, active_session.is_some(), profile)
        .filter(|_| !undeclared_drops)
        .unwrap_or_else(|| (fallback.0.to_string(), fallback.1));
    // ...and a refusal outranks the drop. `next_step`'s ladder puts a waiting
    // drop above an untrusted (not drifted) project, which is right when
    // nothing has tried to use the project yet. Once something HAS — and been
    // refused — the screen would otherwise print the needs-your-yes line and
    // then recommend a different command, which is the two-surfaces-disagree
    // failure the single-next-step rule exists to prevent. Same verb the
    // untrusted and drifted branches already name, so nothing new is invented
    // here; only the ordering is made explicit.
    let next = match &pending {
        Some(_) => (
            "agentstack trust .".to_string(),
            "calls were refused here — review this project and say yes",
        ),
        None => next,
    };

    Ok(Orientation {
        catalog_size: ctx.registry.ids().count(),
        detected_clis,
        native,
        intake,
        manifest_path,
        manifest: ManifestState::Loaded(Box::new(ProjectFacts {
            servers: m.servers.len(),
            skills: m.skills.len(),
            instructions: m.instructions.len(),
            settings: m.settings.len(),
            hooks: m.hooks.len(),
            extensions: m.extensions.len(),
            pinned_targets: m.targets.default.clone(),
            fanout_targets,
            fanout_detected: any_detected,
            toolsets: m.profiles.keys().cloned().collect(),
            session,
            locked,
            trust,
            trust_relevant,
            mode,
            gateway_connected: gateway,
            rendered,
            secrets: if deep_reads { secret_facts(&ctx) } else { None },
            updates: if deep_reads {
                super::updates::available_updates(m)
            } else {
                Vec::new()
            },
            needs_your_yes: pending,
        })),
        next,
    })
}

/// The human screen. `status` distinguishes `agentstack status` (which adds
/// the secrets line and the deep-check pointer) from bare `agentstack`.
fn print_orientation(o: &Orientation, status: bool) {
    println!(
        "{} {} — one portable manifest, every agent CLI\n",
        "agentstack".bold(),
        env!("CARGO_PKG_VERSION")
    );

    // Two different facts, never merged into one number: how many CLIs are
    // detected HERE, and how many the catalog supports. Printing "none
    // detected" above "13 detected CLI(s)" was one screen contradicting itself
    // (pilot Run B); the second number was always the catalog.
    if o.detected_clis.is_empty() {
        println!(
            "  {}  none detected here — {} supported",
            "CLIs    ".bold(),
            o.catalog_size
        );
    } else {
        println!(
            "  {}  {} of {} supported detected here: {}",
            "CLIs    ".bold(),
            o.detected_clis.len(),
            o.catalog_size,
            o.detected_clis.join(" · ")
        );
    }

    // Native config found in this directory that our manifest doesn't cover.
    // Naming it is the whole point: `adopt` could always read these files, but
    // no surface said they existed, so an uncoached user never reached it.
    print_native_line(&o.native, matches!(o.manifest, ManifestState::Missing));

    // Files the user dropped into their own `.agentstack/` tree. Same reason
    // the native line exists: the content is sitting right there, and until a
    // surface names it, nothing tells the user it has to be adopted to count.
    print_intake_line(&o.intake);

    match &o.manifest {
        ManifestState::Missing => println!("  {}  none in this directory", "Setup".bold()),
        ManifestState::Broken(err) => println!(
            "  {}  {} — {}",
            "Setup".bold(),
            o.manifest_path.display(),
            format!("failed to load: {err}").red()
        ),
        ManifestState::Loaded(f) => {
            let mut parts = vec![super::count(f.servers, "server")];
            if f.skills > 0 {
                parts.push(super::count(f.skills, "skill"));
            }
            // Every other declared kind, on the same line and in the same
            // shape. Shown only when non-zero, so a plain server project reads
            // exactly as it did before.
            for (n, noun) in [
                (f.instructions, "instruction"),
                (f.settings, "CLI's settings"),
                (f.hooks, "hook"),
                (f.extensions, "extension"),
            ] {
                if n > 0 {
                    parts.push(super::count(n, noun));
                }
            }
            let targets_note = if f.pinned_targets.is_empty() {
                if f.fanout_detected {
                    format!(
                        "{}, no CLIs pinned",
                        super::count(f.fanout_targets, "detected CLI")
                    )
                } else {
                    // Nothing detected: `resolve_targets` falls back to the
                    // whole catalog. Say that, rather than calling the catalog
                    // "detected".
                    format!(
                        "no CLIs pinned and none detected here — would try all {}",
                        super::count(f.fanout_targets, "supported CLI")
                    )
                }
            } else {
                super::count(f.pinned_targets.len(), "target")
            };
            println!(
                "  {}  {} — {} → {}",
                "Setup".bold(),
                o.manifest_path.display(),
                parts.join(" · "),
                targets_note
            );

            // Profiles get their own line, named rather than counted (P18):
            // "which profiles do I have" stops being archaeology through the
            // manifest or a triggered disambiguation error.
            if !f.toolsets.is_empty() {
                println!(
                    "  {}  {}",
                    "Toolsets".bold(),
                    profiles_line(&f.toolsets, f.session.as_ref().map(|s| s.profile.as_str()))
                );
            }

            // Stage 2.2: an active temporary session is a first-class fact of
            // the default status surface — its own line, not just the (active)
            // marker inside the profiles list, with the command that reverts it.
            if let Some(sess) = &f.session {
                let (headline, hint) =
                    session_status_line(&sess.profile, sess.age_secs, sess.abandoned);
                println!("  {}  {} — {}", "Session ".bold(), headline, hint.dimmed());
            }

            println!(
                "  {}  {}{}",
                "Status  ".bold(),
                if f.locked {
                    "locked"
                } else {
                    "not locked (never activated)"
                },
                match f.trust {
                    crate::trust::TrustState::Trusted => " · trusted",
                    crate::trust::TrustState::Changed => " · trust stale (content changed)",
                    crate::trust::TrustState::Untrusted => " · untrusted",
                }
            );

            // Untrusted (or trust-stale) teaches the human what that state
            // *means*, not just the label (P16). Only shown when trust is
            // relevant — for a static, no-gateway project the note would be
            // false (its servers are not inert), so the honest `· untrusted`
            // Status label stands alone.
            if f.trust_relevant {
                if let Some(note) = orientation_trust_note(f.trust) {
                    println!("            {}", note.dimmed());
                }
            }

            // W1. The line above says what an untrusted project *means*; this
            // one says what it already cost, so it is not dimmed — it is the
            // louder, evidence-bearing version of the same fact, and it is
            // shown whether or not trust is "relevant" here, because a recorded
            // refusal is not a judgement about relevance. `agentstack trust .`
            // is deliberately the same command the Next line prints: the JSON
            // `fix` carries the resolved path for a caller working elsewhere,
            // the screen keeps the phrasing the human is about to read again.
            if let Some(pending) = &f.needs_your_yes {
                println!(
                    "            needs your yes: {} refused since you last said yes · review with `agentstack trust .`",
                    super::count(pending.refused, "call")
                );
            }

            // Which delivery mode this project is in — a glance, not a guess.
            println!(
                "  {}  {} {}",
                "Mode    ".bold(),
                f.mode.label(),
                format!("— {}", f.mode.short()).dimmed()
            );

            if status {
                print_updates_line(&f.updates);
                if let Some(secrets) = &f.secrets {
                    print_secrets_line(secrets);
                }
            }
        }
    }

    println!(
        "\n  {}  {}   {}",
        "Next:".bold(),
        o.next.0.green(),
        o.next.1.dimmed()
    );
    println!("  {}", "All commands: agentstack --help".dimmed());
    // The deep-check pointer is redundant when `doctor` is already the one next
    // step: printing the same command twice, described two different ways, made
    // it look like two different things and undercut the single-next-step rule.
    if status && !o.next.0.starts_with("agentstack doctor") {
        println!(
            "  {}",
            "Deep check (drift, quirks, supply chain): agentstack doctor".dimmed()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // P4 witness: mode is derived from observable signals, with the priority
    // the doc lays out. Rendered artifacts always read as static (even if the
    // gateway is also connected); a trust-gated gateway with nothing rendered is
    // zero-files; a lockfile alone is clean-at-rest; a bare project defaults to
    // static.
    #[test]
    fn mode_derivation_follows_signal_priority() {
        // rendered wins over everything, including a connected+trusted gateway.
        assert_eq!(mode_from_signals(true, true, true, true), Mode::Static);
        assert_eq!(mode_from_signals(true, false, false, false), Mode::Static);
        // zero-files: gateway registered AND trusted, nothing rendered.
        assert_eq!(mode_from_signals(false, true, true, true), Mode::ZeroFiles);
        // connected but not trusted is not yet zero-files; falls to clean-at-rest
        // when locked.
        assert_eq!(
            mode_from_signals(false, true, false, true),
            Mode::CleanAtRest
        );
        // locked, nothing rendered, no gateway → clean-at-rest.
        assert_eq!(
            mode_from_signals(false, false, false, true),
            Mode::CleanAtRest
        );
        // bare, never activated → the default (static).
        assert_eq!(mode_from_signals(false, false, false, false), Mode::Static);
    }

    #[test]
    fn clean_at_rest_next_step_teaches_the_session_rhythm() {
        use crate::trust::TrustState;

        let start =
            clean_at_rest_next_step(Mode::CleanAtRest, TrustState::Trusted, true, false, "dev")
                .expect("trusted clean-at-rest starts a session");
        assert_eq!(start.0, "agentstack session start dev");

        let end =
            clean_at_rest_next_step(Mode::CleanAtRest, TrustState::Trusted, true, true, "dev")
                .expect("active clean-at-rest session points at its close");
        assert_eq!(end.0, "agentstack session end");

        assert!(
            clean_at_rest_next_step(Mode::Static, TrustState::Trusted, true, false, "dev")
                .is_none()
        );
    }

    // P16 witness (refined): trust is the headline next-step only when trusting
    // buys something here — a bridge is registered, or the mode depends on the
    // trust gate (`trust_relevant`). When it does, an untrusted or trust-stale
    // manifest routes to `trust .` ahead of `init`/`doctor` and teaches what the
    // state means. When it does not (a static, no-gateway project whose configs
    // render regardless of trust), the trust route is NOT the headline: the next
    // step falls through to the normal ladder, and the "inert servers" note is
    // withheld — because it would be false — leaving only the true Status label.
    #[test]
    fn untrusted_orientation_teaches_and_routes_to_trust() {
        use crate::trust::TrustState;

        // The one-line note appears for untrusted AND trust-stale, and explains
        // the *consequence* (inert servers), not just the label. (Its caller
        // gates it on trust relevance; the sentence itself is unchanged.)
        for st in [TrustState::Untrusted, TrustState::Changed] {
            let note = orientation_trust_note(st).expect("untrusted states teach");
            assert!(note.contains("inert"), "explains the consequence: {note}");
            assert!(
                note.contains("control-plane tools only"),
                "names the reduced surface: {note}"
            );
        }
        // A trusted manifest has nothing to teach here.
        assert_eq!(orientation_trust_note(TrustState::Trusted), None);

        // Trust-relevant (bridge registered / gate-dependent mode): untrusted
        // and stale both send the human to `trust .`, whatever is rendered.
        assert_eq!(
            next_step(
                TrustState::Untrusted,
                false,
                true,
                true,
                false,
                false,
                false
            )
            .0,
            "agentstack trust ."
        );
        assert_eq!(
            next_step(
                TrustState::Untrusted,
                true,
                false,
                true,
                false,
                false,
                false
            )
            .0,
            "agentstack trust ."
        );
        assert_eq!(
            next_step(TrustState::Changed, true, true, true, false, false, false).0,
            "agentstack trust ."
        );

        // Static, no-gateway (trust irrelevant): a NEVER-trusted project does
        // NOT hijack the headline — it falls through to the normal ladder.
        // Declared but unrendered → `apply --write`; rendered (or empty) →
        // `doctor`. This is the fix for the never-converging trust nag.
        assert_eq!(
            next_step(
                TrustState::Untrusted,
                false,
                true,
                false,
                false,
                false,
                false
            )
            .0,
            "agentstack apply --write"
        );
        assert_eq!(
            next_step(
                TrustState::Untrusted,
                true,
                false,
                false,
                false,
                false,
                false
            )
            .0,
            "agentstack doctor"
        );

        // Trust-STALE is different, and routes to the review whatever the
        // relevance flag says (v0.17.1): content the user already approved has
        // changed, `status` is already reporting it, and sending them to
        // `doctor` first made the cue cost two commands instead of one.
        assert_eq!(
            next_step(TrustState::Changed, true, true, false, false, false, false).0,
            "agentstack trust ."
        );
        assert_eq!(
            next_step(
                TrustState::Changed,
                false,
                false,
                false,
                false,
                false,
                false
            )
            .0,
            "agentstack trust ."
        );

        // The wiring is done and nothing is grouped yet → the next rung is
        // Switch, named as a runnable command. `doctor` here was a dead end: a
        // user who ran it clean was offered it again (pilot Run A).
        let (cmd, _) = next_step(TrustState::Trusted, true, true, false, true, false, false);
        assert_eq!(cmd, "agentstack toolset create <name> --server <server>");

        // Servers configured here that the manifest doesn't cover outrank both
        // — rendering a manifest that omits half the setup is not the step
        // that helps.
        assert_eq!(
            next_step(TrustState::Trusted, false, true, false, false, true, false).0,
            "agentstack adopt"
        );

        // Once trusted the trust-relevance flag is moot: the render vs. verify
        // ladder applies either way.
        for relevant in [true, false] {
            assert_eq!(
                next_step(
                    TrustState::Trusted,
                    false,
                    true,
                    relevant,
                    false,
                    false,
                    false
                )
                .0,
                "agentstack apply --write"
            );
            assert_eq!(
                next_step(
                    TrustState::Trusted,
                    true,
                    false,
                    relevant,
                    false,
                    false,
                    false
                )
                .0,
                "agentstack doctor"
            );
            assert_eq!(
                next_step(
                    TrustState::Trusted,
                    false,
                    false,
                    relevant,
                    false,
                    false,
                    false
                )
                .0,
                "agentstack doctor"
            );
        }
    }

    /// F9 witness (FINDINGS.md, rc.1 review): a dropped-but-undeclared file
    /// must route the one next action to `agentstack yes` — the funnel's
    /// activation verb — not to `adopt` or `trust .`. Before this, `yes`
    /// appeared on no detection surface at all: a participant who dropped a
    /// file was told to `trust .` a project with zero servers, and the funnel
    /// the study exists to measure was unreachable. The one state that still
    /// outranks a drop is trust-stale: content the user already approved has
    /// changed, and that re-review keeps the headline.
    #[test]
    fn a_waiting_drop_routes_to_yes() {
        use crate::trust::TrustState;
        // Every non-stale combination of the other signals: the drop wins —
        // including over unimported native servers and the trust unlock.
        for trust in [TrustState::Trusted, TrustState::Untrusted] {
            for rendered in [true, false] {
                for has_capabilities in [true, false] {
                    for trust_relevant in [true, false] {
                        for unimported in [true, false] {
                            let (cmd, _) = next_step(
                                trust,
                                rendered,
                                has_capabilities,
                                trust_relevant,
                                false,
                                unimported,
                                true,
                            );
                            assert_eq!(
                                cmd, "agentstack yes",
                                "a waiting drop must route to the funnel \
                                 (trust={trust:?} rendered={rendered} caps={has_capabilities} \
                                 relevant={trust_relevant} unimported={unimported})"
                            );
                        }
                    }
                }
            }
        }
        // Trust-stale keeps the headline; the drop is offered after re-review.
        assert_eq!(
            next_step(TrustState::Changed, true, true, false, false, false, true).0,
            "agentstack trust ."
        );
    }

    /// F02 regression: the recommended next step must never be a command that
    /// refuses. `init` errors out once a manifest exists ("init has nothing
    /// left to do here"), and `next_step` only runs when one has loaded — so
    /// no combination of its inputs may produce it. Before this, a normal
    /// finished setup (imported, applied, never `use`d) recommended exactly
    /// that, and following the advice printed an error.
    #[test]
    fn next_step_never_recommends_a_command_that_refuses() {
        use crate::trust::TrustState;
        for trust in [
            TrustState::Trusted,
            TrustState::Untrusted,
            TrustState::Changed,
        ] {
            for rendered in [true, false] {
                for has_capabilities in [true, false] {
                    for trust_relevant in [true, false] {
                        for drops in [true, false] {
                            let (cmd, why) = next_step(
                                trust,
                                rendered,
                                has_capabilities,
                                trust_relevant,
                                false,
                                false,
                                drops,
                            );
                            assert!(
                                !cmd.contains("init"),
                                "a loaded manifest must never be sent back to init \
                                 (trust={trust:?} rendered={rendered} caps={has_capabilities} \
                                 relevant={trust_relevant} drops={drops}) → {cmd} / {why}"
                            );
                        }
                    }
                }
            }
        }

        // And the specific shape that used to break: trusted or not, a project
        // holding capabilities that are already on disk is set up — verify it,
        // don't re-import it.
        assert_eq!(
            next_step(
                TrustState::Untrusted,
                true,
                true,
                false,
                false,
                false,
                false
            )
            .0,
            "agentstack doctor"
        );
    }

    // P18(a) witness: orientation names profiles rather than counting them, one
    // line for a small set, truncated beyond four, with the active one marked.
    #[test]
    fn profiles_line_names_and_marks_active() {
        let two = vec!["dev".to_string(), "prod".to_string()];
        assert_eq!(profiles_line(&two, None), "dev, prod");
        assert_eq!(profiles_line(&two, Some("dev")), "dev (active), prod");

        // Exactly four still lists every name.
        let four: Vec<String> = ["a", "b", "c", "d"].iter().map(|s| s.to_string()).collect();
        assert_eq!(profiles_line(&four, None), "a, b, c, d");

        // Beyond four truncates to the count plus the first three names, and the
        // active marker still shows when it falls inside that window.
        let five: Vec<String> = ["a", "b", "c", "d", "e"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(profiles_line(&five, None), "5 toolsets: a, b, c, …");
        assert_eq!(
            profiles_line(&five, Some("b")),
            "5 toolsets: a, b (active), c, …"
        );
    }

    // Stage 2.2: the Session status line humanizes the session's age.
    #[test]
    fn session_age_buckets() {
        assert_eq!(session_age(5), "started just now");
        assert_eq!(session_age(240), "started 4m ago");
        assert_eq!(session_age(3900), "started 1h 5m ago");
    }

    fn loaded_orientation(secrets: Option<SecretFacts>) -> Orientation {
        Orientation {
            detected_clis: vec!["Claude Code".into()],
            catalog_size: 13,
            native: Vec::new(),
            intake: Vec::new(),
            manifest_path: std::path::PathBuf::from("/repo/.agentstack/agentstack.toml"),
            manifest: ManifestState::Loaded(Box::new(ProjectFacts {
                servers: 2,
                instructions: 0,
                settings: 0,
                hooks: 0,
                extensions: 0,
                skills: 1,
                pinned_targets: vec!["claude-code".into()],
                fanout_targets: 1,
                fanout_detected: true,
                toolsets: vec!["dev".into(), "prod".into()],
                session: Some(SessionFacts {
                    profile: "dev".into(),
                    started_unix: 1_700_000_000,
                    age_secs: 240,
                    abandoned: false,
                }),
                locked: true,
                trust: crate::trust::TrustState::Changed,
                trust_relevant: true,
                mode: Mode::CleanAtRest,
                gateway_connected: false,
                rendered: false,
                secrets,
                needs_your_yes: None,
                updates: Vec::new(),
            })),
            next: ("agentstack trust .".into(), "review and re-trust"),
        }
    }

    /// `needs-your-yes-v1`, at the serializer: the key appears only when
    /// something was actually refused, and it carries the fix — never a card.
    /// A project with nothing refused must read byte-for-byte as it did before
    /// the field existed, which is why absence (not `null`) is the contract.
    #[test]
    fn needs_your_yes_appears_only_with_evidence_and_carries_no_card() {
        let clean = status_json(&loaded_orientation(None));
        assert!(
            clean["project"].get("needs_your_yes").is_none(),
            "a project with no recorded refusal must not carry the key: {clean}"
        );

        let mut o = loaded_orientation(None);
        if let ManifestState::Loaded(f) = &mut o.manifest {
            f.needs_your_yes = Some(NeedsYourYes {
                refused: 3,
                last_refused_ts: 1_700_000_100,
                fix: "agentstack trust /repo".to_string(),
            });
        }
        let out = status_json(&o);
        let pending = &out["project"]["needs_your_yes"];
        assert_eq!(pending["refused"], 3);
        assert_eq!(pending["last_refused_ts"], 1_700_000_100u64);
        assert_eq!(pending["fix"], "agentstack trust /repo");
        // The card stays behind `agentstack trust` — one walk, one renderer.
        // Anything resembling a reviewable surface here would be a second one.
        for absent in ["items", "review", "servers", "skills", "surface_digest"] {
            assert!(
                pending.get(absent).is_none(),
                "status must not carry card payload ({absent}): {pending}"
            );
        }
    }

    /// `json-reads-v1`: `status --json` is the orientation screen's own
    /// reading, keyed. `project` carries the per-project facts; `trust` uses
    /// the same vocabulary `use --list --json` does, so a UI holding both
    /// reads needs one lookup table, not two.
    #[test]
    fn status_json_carries_the_orientation_reading() {
        let out = status_json(&loaded_orientation(Some(SecretFacts {
            referenced: 2,
            unresolved: vec!["NOTION_TOKEN".into()],
        })));
        assert_eq!(out["manifest"]["present"], true);
        assert_eq!(out["manifest"]["loaded"], true);
        assert_eq!(out["manifest"]["error"], serde_json::Value::Null);
        let p = &out["project"];
        assert_eq!(p["servers"], 2);
        assert_eq!(p["skills"], 1);
        assert_eq!(p["targets"]["pinned"][0], "claude-code");
        assert_eq!(p["targets"]["fanout"], 1);
        assert_eq!(p["toolsets"][1], "prod");
        assert_eq!(p["session"]["profile"], "dev");
        assert_eq!(p["session"]["abandoned"], false);
        assert_eq!(p["locked"], true);
        assert_eq!(p["trust"], "drifted");
        assert_eq!(p["trust_relevant"], true);
        assert_eq!(p["mode"], "clean-at-rest");
        assert_eq!(p["secrets"]["referenced"], 2);
        assert_eq!(p["secrets"]["unresolved"][0], "NOTION_TOKEN");
        assert_eq!(out["next_action"]["command"], "agentstack trust .");
    }

    /// The two readings that have no project: `project` is `null`, and a
    /// manifest that exists but will not load says why. A consumer branches on
    /// one field instead of probing a dozen for null.
    #[test]
    fn status_json_distinguishes_missing_from_broken() {
        let missing = status_json(&Orientation {
            detected_clis: Vec::new(),
            catalog_size: 13,
            native: Vec::new(),
            intake: Vec::new(),
            manifest_path: std::path::PathBuf::from("/repo/.agentstack/agentstack.toml"),
            manifest: ManifestState::Missing,
            next: ("agentstack init".into(), "guided one-command setup"),
        });
        assert_eq!(missing["manifest"]["present"], false);
        assert_eq!(missing["manifest"]["error"], serde_json::Value::Null);
        assert_eq!(missing["project"], serde_json::Value::Null);

        let broken = status_json(&Orientation {
            detected_clis: Vec::new(),
            catalog_size: 13,
            native: Vec::new(),
            intake: Vec::new(),
            manifest_path: std::path::PathBuf::from("/repo/.agentstack/agentstack.toml"),
            manifest: ManifestState::Broken("missing field `type`\nin `servers.a`".into()),
            next: ("agentstack doctor".into(), "diagnose the manifest"),
        });
        assert_eq!(broken["manifest"]["present"], true);
        assert_eq!(broken["manifest"]["loaded"], false);
        // Sanitized to one line — the screen may wrap, a JSON string may not
        // smuggle control bytes into a consumer's UI (rule 7).
        assert_eq!(
            broken["manifest"]["error"],
            "missing field `type` in `servers.a`"
        );
        assert_eq!(broken["project"], serde_json::Value::Null);
    }

    /// Invariant 5 witness for this payload: the secrets reading carries
    /// COUNTS and unresolved NAMES. A resolved secret contributes to the count
    /// and nothing else — its name is not even listed, let alone its value.
    #[test]
    fn status_json_secrets_never_carry_a_value() {
        let all_resolve = status_json(&loaded_orientation(Some(SecretFacts {
            referenced: 2,
            unresolved: Vec::new(),
        })));
        let secrets = &all_resolve["project"]["secrets"];
        assert_eq!(secrets["referenced"], 2);
        assert_eq!(secrets["unresolved"].as_array().unwrap().len(), 0);
        assert_eq!(secrets.as_object().unwrap().len(), 2, "two keys, no value");

        // Not asked for (bare `agentstack`) is a third state, distinct from
        // "asked, everything resolves".
        let unasked = status_json(&loaded_orientation(None));
        assert_eq!(unasked["project"]["secrets"], serde_json::Value::Null);
    }

    /// `update-offer-v1`, both directions. The offer carries the three fields
    /// and the SHIPPED command; no offer means the key is absent entirely —
    /// not `null`, not `[]` — because absence must stay unreadable as
    /// "current" (the check is offline and never proves currency).
    #[test]
    fn status_json_offers_updates_and_omits_the_key_when_there_is_none() {
        let none = status_json(&loaded_orientation(None));
        assert!(
            none["project"].get("updates").is_none(),
            "no offer must not materialize the key: {}",
            none["project"]
        );

        let mut o = loaded_orientation(None);
        if let ManifestState::Loaded(f) = &mut o.manifest {
            f.updates = vec![crate::commands::updates::PackUpdate {
                name: "acme".into(),
                current: "v0.1.0".into(),
                available: "v0.2.0".into(),
            }];
        }
        let out = status_json(&o);
        let updates = &out["project"]["updates"];
        assert_eq!(updates["packs"][0]["name"], "acme");
        assert_eq!(updates["packs"][0]["current"], "v0.1.0");
        assert_eq!(updates["packs"][0]["available"], "v0.2.0");
        assert_eq!(updates["fix"], "agentstack lock --upgrade acme");
    }

    // Stage 2.2: a live session reads as active; an abandoned one is flagged
    // and both offer the same safe `session end` recovery.
    #[test]
    fn session_status_line_flags_abandoned_and_offers_recovery() {
        let (head, hint) = session_status_line("dev", 240, false);
        assert_eq!(head, "'dev' active temporarily (started 4m ago)");
        assert!(hint.contains("agentstack session end"));
        assert!(!hint.contains("abandoned"));

        let (head, hint) = session_status_line("dev", 14 * 3600, true);
        assert!(head.contains("looks abandoned"), "flags it: {head}");
        assert!(head.contains("started 14h 0m ago"));
        assert!(
            hint.contains("agentstack session end"),
            "still offers the safe recovery: {hint}"
        );
    }
}
