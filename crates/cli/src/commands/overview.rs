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
            Mode::Static => "rendered configs on disk, kept out of git",
            Mode::CleanAtRest => "materialized only during a session",
            Mode::ZeroFiles => "served live over the gateway, trust-gated",
        }
    }

    /// The full one-line help (docs/design P4 wording), shown when setup
    /// presents the three modes as a choice.
    pub(crate) fn help(self) -> &'static str {
        match self {
            Mode::Static => "Rendered configs stay on disk, kept out of git. Works with every CLI, zero moving parts. This is what you have now.",
            Mode::CleanAtRest => "Nothing generated exists between sessions; `agentstack session start` materializes your profile and `session end` reverts it. Your repo stays pristine for git.",
            Mode::ZeroFiles => "Nothing is ever written; the gateway serves servers and skills live over MCP, trust-gated per repo. Best when you work across many repos.",
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

pub fn run(manifest_dir: Option<&Path>) -> Result<()> {
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
            "agentstack setup",
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
                if !m.profiles.is_empty() {
                    parts.push(format!("{} profile(s)", m.profiles.len()));
                }
                println!(
                    "  {}  {} — {} → {} target(s)",
                    "Manifest".bold(),
                    manifest_path.display(),
                    parts.join(" · "),
                    m.targets.default.len()
                );

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

                // Which delivery mode this project is in, derived from what's on
                // disk (P4) — so "which mode am I in?" is a glance, not a guess.
                let target_ids: Vec<String> = ctx.registry.ids().map(str::to_string).collect();
                let mode = detect_mode(&ctx, &target_ids);
                println!(
                    "  {}  {} {}",
                    "Mode    ".bold(),
                    mode.label(),
                    format!("— {}", mode.short()).dimmed()
                );

                if !locked && (!m.skills.is_empty() || !m.servers.is_empty()) {
                    (
                        "agentstack setup",
                        "finish the first run — preview, apply, activate",
                    )
                } else if trust == crate::trust::TrustState::Changed {
                    (
                        "agentstack trust .",
                        "the manifest or lock changed — review and re-trust",
                    )
                } else {
                    (
                        "agentstack doctor",
                        "verify the wiring — every warning names its fix",
                    )
                }
            }
            Err(err) => {
                println!(
                    "  {}  {} — {}",
                    "Manifest".bold(),
                    manifest_path.display(),
                    format!("failed to load: {err:#}").red()
                );
                ("agentstack doctor", "diagnose the manifest")
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
}
