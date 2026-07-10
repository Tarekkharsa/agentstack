//! `agentstack run` / `runs` / `kill` — the CLI layer over [`crate::runs`].
//! Launching is a foreground, terminal-attached act; listing and killing also
//! work from the dashboard.

use std::path::Path;

use anyhow::Result;
use owo_colors::OwoColorize;

use crate::cli::{KillArgs, RunArgs, RunsArgs};

pub fn run(args: &RunArgs, dir: Option<&Path>) -> Result<()> {
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
    crate::runs::launch(
        dir,
        &args.harness,
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
