//! `agentstack restore` — the one undo verb. With no argument it lists what
//! can be undone: recorded apply/use/session events (the history engine —
//! servers, settings, hooks, instructions, even owned-manifest refreshes) plus
//! the per-adapter single-slot backups [`crate::util::atomic`] keeps. Undo a
//! recorded event by id (`restore <id> --write`, unique prefix is enough, or
//! `--last` for the most recent), or restore one adapter's config from its
//! slot backup (`restore <adapter> --write`). Dry-run by default.

use std::path::Path;

use anyhow::{Context, Result};
use owo_colors::OwoColorize;

use crate::adapter::Registry;
use crate::cli::RestoreArgs;
use crate::history;
use crate::scope::Scope;
use crate::util::{atomic, diff};

pub fn run(args: &RestoreArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let dir = match manifest_dir {
        Some(d) => d.to_path_buf(),
        None => std::env::current_dir()?,
    };
    let registry = Registry::load()?;

    if args.last {
        let entry = history::list()
            .into_iter()
            .find(|e| !e.undone)
            .context("nothing to undo — no recorded write that isn't already undone")?;
        return undo_entry(&entry, args.write);
    }

    match &args.adapter {
        None => list(&registry, &dir),
        // An adapter id keeps the original single-slot behavior; anything else
        // is treated as a history-entry id (unique prefix is enough).
        Some(id) if registry.get(id).is_some() => restore_one(
            &registry,
            &dir,
            id,
            args.scope.unwrap_or(Scope::Global),
            args.write,
        ),
        Some(id) => {
            let entries = history::list();
            let matches: Vec<_> = entries.iter().filter(|e| e.id.starts_with(id)).collect();
            match matches.as_slice() {
                [one] => undo_entry(one, args.write),
                [] => anyhow::bail!(
                    "'{id}' is neither an adapter id nor a recorded change — `agentstack restore` lists both"
                ),
                _ => anyhow::bail!(
                    "'{id}' matches {} recorded changes — use more of the id",
                    matches.len()
                ),
            }
        }
    }
}

fn fmt_age(time_unix: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let s = now.saturating_sub(time_unix);
    match s {
        0..=59 => format!("{s}s ago"),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86_399 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86_400),
    }
}

fn list(registry: &Registry, dir: &Path) -> Result<()> {
    let entries = history::list();
    if entries.is_empty() {
        println!("No recorded changes yet — history fills as `apply`/`use` write configs.\n");
    } else {
        println!("Recorded changes (newest first):\n");
        for e in entries.iter().take(15) {
            let mark = if e.undone {
                "· undone".dimmed().to_string()
            } else {
                String::new()
            };
            println!(
                "  {}  {:<8} {:<8} {} {mark}",
                (&e.id[..8]).bold(),
                fmt_age(e.time_unix),
                e.scope,
                e.summary
            );
        }
        println!(
            "\nUndo one with: {} (or {} for the newest)",
            "agentstack restore <id> --write".bold(),
            "--last".bold()
        );
    }

    // The per-adapter single-slot backups remain available as a fallback for
    // writes that predate their history entry (or a pruned history).
    let mut found = 0;
    for desc in registry.iter() {
        for scope in [Scope::Global, Scope::Project] {
            if let Some((path, _)) = desc.config_for(scope, dir) {
                if atomic::backup_path(&path).exists() {
                    if found == 0 {
                        println!("\nAdapter config backups (content before our last write):");
                    }
                    found += 1;
                    println!(
                        "  {:<14} {:<8} {}",
                        desc.display.bold(),
                        scope,
                        path.display()
                    );
                }
            }
        }
    }
    if found > 0 {
        println!("\nRestore one with: agentstack restore <adapter> [--scope project] --write");
    }
    Ok(())
}

/// Preview (or perform) a recorded event's undo: every captured file goes back
/// to its pre-write bytes; files that didn't exist before are deleted.
fn undo_entry(entry: &history::Entry, write: bool) -> Result<()> {
    println!(
        "{} undo {} ({}, {}): {}",
        "↩".cyan(),
        &entry.id[..8],
        entry.scope,
        fmt_age(entry.time_unix),
        entry.summary
    );
    for f in &entry.files {
        let current = std::fs::read_to_string(&f.path).unwrap_or_default();
        match &f.before {
            Some(before) if !diff::differs(&current, before) => {
                println!("  {} {:<28} already matches", "✓".green(), f.label);
            }
            Some(_) => println!("  {} {:<28} revert {}", "↩".cyan(), f.label, f.path),
            None => println!("  {} {:<28} delete {}", "✗".red(), f.label, f.path),
        }
    }

    if write {
        history::undo(&entry.id)?;
        println!(
            "{} undone — reverted files show up as pending again; re-run `agentstack apply` to re-render.",
            "✓".green()
        );
    } else {
        println!("\nDry run. Re-run with {} to undo.", "--write".bold());
    }
    Ok(())
}

fn restore_one(registry: &Registry, dir: &Path, id: &str, scope: Scope, write: bool) -> Result<()> {
    let desc = registry
        .get(id)
        .with_context(|| format!("unknown adapter '{id}' (try `agentstack adapters list`)"))?;
    let (path, _) = desc
        .config_for(scope, dir)
        .with_context(|| format!("{} has no {scope} config", desc.display))?;
    let backup = atomic::backup_path(&path);
    if !backup.exists() {
        anyhow::bail!(
            "no backup for {} ({scope}) — none has been written yet",
            desc.display
        );
    }

    let restored = std::fs::read_to_string(&backup)
        .with_context(|| format!("reading backup {}", backup.display()))?;
    let current = std::fs::read_to_string(&path).unwrap_or_default();

    println!(
        "{} restore {} ({scope}) ← {}",
        "↩".cyan(),
        path.display(),
        backup.display()
    );
    if !diff::differs(&current, &restored) {
        println!(
            "  {} already matches the backup — nothing to do",
            "✓".green()
        );
        return Ok(());
    }
    print!(
        "{}",
        diff::render(&current, &restored)
            .lines()
            .map(|l| format!("  {l}\n"))
            .collect::<String>()
    );

    if write {
        // atomic::write backs up the current content first, so this is itself
        // reversible.
        atomic::write(&path, &restored)?;
        println!("{} restored {}", "✓".green(), path.display());
    } else {
        println!("\nDry run. Re-run with {} to restore.", "--write".bold());
    }
    Ok(())
}
