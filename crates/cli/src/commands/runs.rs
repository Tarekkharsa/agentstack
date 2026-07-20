//! `agentstack run` / `runs` / `kill` — the CLI layer over [`crate::runs`].
//! Launching is a foreground, terminal-attached act; listing and killing also
//! work from the dashboard.

use std::path::Path;

use anyhow::Result;
use owo_colors::OwoColorize;

use crate::cli::{KillArgs, RunArgs, RunsArgs};

pub fn run(args: &RunArgs, dir: Option<&Path>) -> Result<()> {
    // --locked promotes the host run to the Protected tier (fail-closed gates
    // before launch). It owns its combination rules — --locked --sandbox is a
    // named not-yet limitation there, not a silent fall-through.
    if args.locked {
        return crate::commands::locked::run_locked(dir, args);
    }
    // --lockdown is the stronger sandbox mode; it implies --sandbox.
    if args.sandbox || args.lockdown {
        return crate::commands::sandbox::run_sandboxed(dir, args);
    }
    // `--plan` promises "print the plan, run NOTHING" — it is only defined for
    // the locked/sandbox flows above. Bare `run --plan` used to fall through
    // and launch the CLI anyway (audit finding: an unintended launch during a
    // read-only review); refuse instead and name the two real forms.
    if args.plan {
        anyhow::bail!(
            "--plan needs a run mode — nothing was launched\n\
             \n  \
             protected host plan:  agentstack run --locked --plan {h}\n  \
             sandbox plan:         agentstack run --sandbox --plan {h}",
            h = args.harness
        );
    }
    // Validate BEFORE the banner: a missing manifest, unknown id, or absent
    // binary must be the first (and only) thing the user reads — never below a
    // "▶ launching…" line claiming something started.
    let plan = crate::runs::prepare(dir, &args.harness)?;
    if let Some(p) = &args.profile {
        println!(
            "{} launching {} with profile '{}' ({})…",
            "▶".green(),
            args.harness.bold(),
            p,
            args.scope
        );
    } else {
        println!("{} launching {}…", "▶".green(), args.harness.bold());
    }
    // Host mode has no container: name the posture and say — once, honestly, in
    // the same style as the sandbox "unreviewed bundle" warning — that policy is
    // advisory here. The gateway still brokers MCP tool calls, but nothing
    // confines this process's own egress or filesystem; `--sandbox`/`--lockdown`
    // are what enforce those at runtime.
    use crate::commands::sandbox::Posture;
    println!("  posture: {}", Posture::Host.to_string().yellow().bold());
    eprintln!(
        "  {} host mode: policy is advisory — the gateway brokers MCP tool calls, but \
         this process's own egress and filesystem are not confined; use `--sandbox` or \
         `--lockdown` to enforce them at runtime.",
        "⚠".yellow()
    );
    crate::runs::launch(
        plan,
        dir,
        args.profile.as_deref(),
        args.scope,
        &args.args,
        args.keep,
    )
}

pub fn list(args: &RunsArgs) -> Result<()> {
    let runs = crate::runs::list();
    if args.json {
        println!("{}", serde_json::to_string_pretty(&runs)?);
        return Ok(());
    }
    if runs.is_empty() {
        println!("No live runs.");
        return Ok(());
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    for r in runs {
        let profile = r
            .profile
            .as_ref()
            .map(|p| format!(" · profile {p}"))
            .unwrap_or_default();
        println!(
            "{}  {} pid={} up={}{}  {}",
            r.id.bold(),
            r.display,
            r.pid,
            fmt_uptime(now.saturating_sub(r.started_unix)),
            profile,
            r.cwd.dimmed()
        );
    }
    Ok(())
}

pub fn kill(args: &KillArgs) -> Result<()> {
    crate::runs::kill(&args.id, args.force)?;
    let how = if args.force { " (forced)" } else { "" };
    println!("{} killed run {}{}", "✓".green(), args.id.bold(), how);
    Ok(())
}

/// Compact human uptime: `45s`, `12m`, `3h05m`.
fn fmt_uptime(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
    }
}
