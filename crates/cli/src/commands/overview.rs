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
            scope != Scope::Global || t.source_manifest.as_deref().map_or(true, |s| s == identity);
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
/// to the normal ladder. That ladder: a manifest never activated but holding
/// capabilities → `init`; otherwise the wiring is in place → `doctor`. Pure
/// over its inputs so the routing is unit-tested without touching disk.
pub(crate) fn next_step(
    trust: crate::trust::TrustState,
    locked: bool,
    has_capabilities: bool,
    trust_relevant: bool,
) -> (&'static str, &'static str) {
    use crate::trust::TrustState;
    match trust {
        TrustState::Untrusted if trust_relevant => {
            ("agentstack trust .", "review it to unlock its servers")
        }
        TrustState::Changed if trust_relevant => (
            "agentstack trust .",
            "the manifest or lock changed — review and re-trust",
        ),
        // Untrusted or trust-stale but trust changes nothing here (static, no
        // gateway), or already trusted: fall through to the normal ladder. A
        // never-activated manifest with capabilities finishes its first run via
        // `init`; otherwise the wiring is in place → `doctor`.
        _ => {
            if !locked && has_capabilities {
                (
                    "agentstack init",
                    "finish the first run — preview, apply, activate",
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
            "materialize the profile for this session",
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
        format!("{} profiles: {shown}, …", names.len())
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

/// `agentstack status` — the orientation screen by name, plus the cheap health
/// signals a glance wants (secrets resolving?) and the pointer to the deep
/// check. Everything expensive (drift rendering, content scans) stays in
/// `doctor`; status must feel instant.
pub fn run_status(manifest_dir: Option<&Path>) -> Result<()> {
    render(manifest_dir, true)
}

pub fn run(manifest_dir: Option<&Path>) -> Result<()> {
    render(manifest_dir, false)
}

/// Secrets at a glance for `status`: the single most common thing broken after
/// setup. One aligned line when everything resolves; one line per missing
/// secret, each carrying its exact fix command.
fn print_secrets_line(ctx: &super::Context) {
    let refs = ctx.loaded.manifest.referenced_secrets();
    if refs.is_empty() {
        return;
    }
    let sources = crate::secret::SecretSources::detect(&ctx.dir);
    let missing: Vec<&String> = refs
        .iter()
        .filter(|n| sources.source_of(n).is_none())
        .collect();
    if missing.is_empty() {
        println!(
            "  {}  {} referenced, all resolve",
            "Secrets ".bold(),
            refs.len()
        );
    } else {
        for name in missing {
            println!(
                "  {}  {} {name} not set   {}",
                "Secrets ".bold(),
                "✗".red(),
                format!("fix: agentstack secret set {name}").dimmed()
            );
        }
    }
}

fn render(manifest_dir: Option<&Path>, status: bool) -> Result<()> {
    println!(
        "{} {} — one portable manifest, every agent CLI\n",
        "agentstack".bold(),
        env!("CARGO_PKG_VERSION")
    );

    let registry = Registry::load()?;
    let detected: Vec<&str> = registry
        .iter()
        .filter(|d| d.detected())
        .map(|d| d.display.as_str())
        .collect();
    if detected.is_empty() {
        println!("  {}  none detected on this machine", "CLIs    ".bold());
    } else {
        println!(
            "  {}  {} detected: {}",
            "CLIs    ".bold(),
            detected.len(),
            detected.join(" · ")
        );
    }

    // Walk up to the project root so `agentstack` from `src/deep` describes
    // the ROOT manifest instead of steering toward a nested `init`.
    let base = super::project_base(manifest_dir)?;
    let dir = crate::manifest::resolve_manifest_dir(&base);
    let manifest_path = dir.join(MANIFEST_FILE);

    let next = if !manifest_path.exists() {
        println!("  {}  none in this directory", "Manifest".bold());
        (
            "agentstack init".to_string(),
            "guided one-command setup — import, preview, apply",
        )
    } else {
        match super::load(manifest_dir) {
            Ok(ctx) => {
                let m = &ctx.loaded.manifest;
                let mut parts = vec![format!("{} server(s)", m.servers.len())];
                if !m.skills.is_empty() {
                    parts.push(format!("{} skill(s)", m.skills.len()));
                }
                // No [targets] pinned → commands fan out to the detected CLIs
                // (see render::resolve_targets); "0 target(s)" would be false.
                let targets_note = if m.targets.default.is_empty() {
                    let n = crate::render::resolve_targets(m, &ctx.registry, &[])
                        .map(|t| t.len())
                        .unwrap_or_default();
                    format!("{n} detected CLI(s), no [targets] pinned")
                } else {
                    format!("{} target(s)", m.targets.default.len())
                };
                println!(
                    "  {}  {} — {} → {}",
                    "Manifest".bold(),
                    manifest_path.display(),
                    parts.join(" · "),
                    targets_note
                );

                // Profiles get their own line, named rather than counted (P18):
                // "which profiles do I have" stops being archaeology through the
                // manifest or a triggered disambiguation error. The active one is
                // marked only when a live session pins it — the one signal that
                // *reliably* says which profile is loaded right now.
                let active_session = crate::session::active(&ctx.dir);
                if !m.profiles.is_empty() {
                    let names: Vec<String> = m.profiles.keys().cloned().collect();
                    println!(
                        "  {}  {}",
                        "Profiles".bold(),
                        profiles_line(&names, active_session.as_ref().map(|s| s.profile.as_str()))
                    );
                }

                // Stage 2.2: an active temporary session is a first-class fact
                // of the default status surface — its own line, not just the
                // (active) marker inside the profiles list, with the one
                // command that reverts it.
                if let Some(sess) = &active_session {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    let (headline, hint) = session_status_line(
                        &sess.profile,
                        now.saturating_sub(sess.started_unix),
                        sess.is_abandoned(now),
                    );
                    println!("  {}  {} — {}", "Session ".bold(), headline, hint.dimmed());
                }

                // Where this project actually stands, from cheap signals:
                // lockfile (was it ever activated/pinned?) and trust state.
                let base = crate::manifest::project_root_of(&ctx.dir);
                let trust = crate::trust::check(&base);
                let locked = crate::lock::Lock::path(&ctx.dir).exists();
                println!(
                    "  {}  {}{}",
                    "Status  ".bold(),
                    if locked {
                        "locked"
                    } else {
                        "not locked (never activated)"
                    },
                    match trust {
                        crate::trust::TrustState::Trusted => " · trusted",
                        crate::trust::TrustState::Changed => " · trust stale (content changed)",
                        crate::trust::TrustState::Untrusted => " · untrusted",
                    }
                );

                // Delivery mode and gateway state, derived from what's on disk
                // (P4) — computed here (before the trust note) because whether
                // trust is even *relevant* depends on them. Trust genuinely
                // gates capability delivery only through the bridge (zero-files)
                // or the trust-gated run/session paths (clean-at-rest); a
                // static, no-gateway project renders through `apply`/`use`
                // regardless. So trust is the headline next-step, and the
                // "inert servers" note is shown, only when a bridge is
                // registered or the mode depends on the gate.
                let target_ids: Vec<String> = ctx.registry.ids().map(str::to_string).collect();
                let gateway = gateway_connected(&ctx, &target_ids);
                let mode = detect_mode(&ctx, &target_ids);
                let trust_relevant = gateway || matches!(mode, Mode::ZeroFiles | Mode::CleanAtRest);

                // Untrusted (or trust-stale) teaches the human what that state
                // *means*, not just the label (P16): an untrusted manifest's
                // servers stay inert — the gateway serves control-plane tools
                // only until the digest is reviewed and pinned. One line,
                // aligned under the Status content it explains. Only shown when
                // trust is relevant — for a static, no-gateway project the note
                // would be false (its servers are not inert), so the honest
                // `· untrusted` Status label stands alone.
                if trust_relevant {
                    if let Some(note) = orientation_trust_note(trust) {
                        println!("            {}", note.dimmed());
                    }
                }

                // Which delivery mode this project is in — a glance, not a guess.
                println!(
                    "  {}  {} {}",
                    "Mode    ".bold(),
                    mode.label(),
                    format!("— {}", mode.short()).dimmed()
                );

                if status {
                    print_secrets_line(&ctx);
                }

                let has_capabilities = !m.skills.is_empty() || !m.servers.is_empty();
                let fallback = next_step(trust, locked, has_capabilities, trust_relevant);
                let profile = if m.profiles.len() == 1 {
                    m.profiles
                        .keys()
                        .next()
                        .map(String::as_str)
                        .unwrap_or("<profile>")
                } else {
                    "<profile>"
                };
                clean_at_rest_next_step(mode, trust, locked, active_session.is_some(), profile)
                    .unwrap_or_else(|| (fallback.0.to_string(), fallback.1))
            }
            Err(err) => {
                println!(
                    "  {}  {} — {}",
                    "Manifest".bold(),
                    manifest_path.display(),
                    format!("failed to load: {err:#}").red()
                );
                ("agentstack doctor".to_string(), "diagnose the manifest")
            }
        }
    };

    println!(
        "\n  {}  {}   {}",
        "Next:".bold(),
        next.0.green(),
        next.1.dimmed()
    );
    println!("  {}", "All commands: agentstack --help".dimmed());
    if status {
        println!(
            "  {}",
            "Deep check (drift, quirks, supply chain): agentstack doctor".dimmed()
        );
    }
    Ok(())
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
        // and stale both send the human to `trust .`, whatever the lock holds.
        assert_eq!(
            next_step(TrustState::Untrusted, false, true, true).0,
            "agentstack trust ."
        );
        assert_eq!(
            next_step(TrustState::Untrusted, true, false, true).0,
            "agentstack trust ."
        );
        assert_eq!(
            next_step(TrustState::Changed, true, true, true).0,
            "agentstack trust ."
        );

        // Static, no-gateway (trust irrelevant): the untrusted/stale state does
        // NOT hijack the headline — it falls through to the normal ladder. A
        // never-activated manifest with capabilities finishes its first run via
        // `init`; an activated (or empty) one verifies via `doctor`. This is the
        // fix for the never-converging trust nag.
        assert_eq!(
            next_step(TrustState::Untrusted, false, true, false).0,
            "agentstack init"
        );
        assert_eq!(
            next_step(TrustState::Untrusted, true, false, false).0,
            "agentstack doctor"
        );
        assert_eq!(
            next_step(TrustState::Changed, true, true, false).0,
            "agentstack doctor"
        );
        assert_eq!(
            next_step(TrustState::Changed, false, false, false).0,
            "agentstack doctor"
        );

        // Once trusted the trust-relevance flag is moot: the first-run vs. verify
        // ladder applies either way.
        for relevant in [true, false] {
            assert_eq!(
                next_step(TrustState::Trusted, false, true, relevant).0,
                "agentstack init"
            );
            assert_eq!(
                next_step(TrustState::Trusted, true, false, relevant).0,
                "agentstack doctor"
            );
            assert_eq!(
                next_step(TrustState::Trusted, false, false, relevant).0,
                "agentstack doctor"
            );
        }
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
        assert_eq!(profiles_line(&five, None), "5 profiles: a, b, c, …");
        assert_eq!(
            profiles_line(&five, Some("b")),
            "5 profiles: a, b (active), c, …"
        );
    }

    // Stage 2.2: the Session status line humanizes the session's age.
    #[test]
    fn session_age_buckets() {
        assert_eq!(session_age(5), "started just now");
        assert_eq!(session_age(240), "started 4m ago");
        assert_eq!(session_age(3900), "started 1h 5m ago");
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
