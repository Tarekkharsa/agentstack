//! `agentstack restore` — undo an apply by restoring a CLI config from the
//! pre-write backup that [`crate::util::atomic`] keeps. With no adapter it lists
//! what backups exist; with one it restores that config (dry-run by default).

use std::path::Path;

use anyhow::{Context, Result};
use owo_colors::OwoColorize;

use crate::adapter::Registry;
use crate::cli::RestoreArgs;
use crate::scope::Scope;
use crate::util::{atomic, diff};

pub fn run(args: &RestoreArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let dir = match manifest_dir {
        Some(d) => d.to_path_buf(),
        None => std::env::current_dir()?,
    };
    let registry = Registry::load()?;

    match &args.adapter {
        None => list(&registry, &dir),
        Some(id) => restore_one(
            &registry,
            &dir,
            id,
            args.scope.unwrap_or(Scope::Global),
            args.write,
        ),
    }
}

fn list(registry: &Registry, dir: &Path) -> Result<()> {
    let mut found = 0;
    println!("Available backups (most recent content before our last write):\n");
    for desc in registry.iter() {
        for scope in [Scope::Global, Scope::Project] {
            if let Some((path, _)) = desc.config_for(scope, dir) {
                if atomic::backup_path(&path).exists() {
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
    if found == 0 {
        println!("  (none yet — backups are created the first time `apply` writes a config)");
    } else {
        println!("\nRestore one with: agentstack restore <adapter> [--scope project] --write");
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
