//! Bare `agentstack` — orientation instead of a wall of subcommands: what's
//! detected on this machine, what state this directory's manifest is in, and
//! the one next command to run.

use std::path::Path;

use anyhow::Result;
use owo_colors::OwoColorize;

use crate::adapter::Registry;
use crate::manifest::load::MANIFEST_FILE;

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
