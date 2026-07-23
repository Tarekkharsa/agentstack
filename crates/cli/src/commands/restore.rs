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
    let dir = super::project_base(manifest_dir)?;
    let registry = Registry::load()?;

    if args.last {
        let entries = history::list();
        let entry = entries
            .iter()
            .find(|e| !e.undone)
            .context("nothing to undo — no recorded write that isn't already undone")?;
        return undo_selection(entry, &entries, args.write, args.json);
    }

    match &args.adapter {
        None if args.json => list_json(&registry, &dir),
        None => list(&registry, &dir),
        // An adapter id keeps the original single-slot behavior; anything else
        // is treated as a history-entry id (unique prefix is enough).
        Some(id) if registry.get(id).is_some() => restore_one(
            &registry,
            &dir,
            id,
            // Same context-derived default as apply/use: the slot restore
            // targets the scope those commands write by default here.
            args.scope.unwrap_or_else(|| {
                Scope::default_for(&crate::manifest::resolve_manifest_dir(&dir))
            }),
            args.write,
        ),
        Some(id) => {
            let entries = history::list();
            let matches: Vec<_> = entries.iter().filter(|e| id_matches(&e.id, id)).collect();
            match matches.as_slice() {
                [one] => undo_selection(one, &entries, args.write, args.json),
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

/// The machine-readable undo inventory (UI control-plane §"Activity and
/// recovery"): recorded history entries newest-first plus the per-adapter
/// slot backups. `short_id` is what a fixed `restore <id>` action passes
/// back; summaries came from our own writer, but sanitize anyway — they name
/// files from potentially hostile manifests.
fn list_json(registry: &Registry, dir: &Path) -> Result<()> {
    let out = list_json_value(registry, dir);
    println!(
        "{}",
        serde_json::to_string_pretty(&crate::ui_contract::envelope(out))?
    );
    Ok(())
}

/// The undo inventory as a value — the body `restore --json` prints (without
/// the envelope). Public so integrations and witnesses read the exact
/// production shape.
pub fn list_json_value(registry: &Registry, dir: &Path) -> serde_json::Value {
    let entries = history::list();
    let entries_json: Vec<serde_json::Value> = entries
        .iter()
        .map(|e| {
            // The ledger is machine-global (an entry may belong to any
            // project or to user-level configs). `touches_project` marks the
            // entries whose captured files live under THIS project, so a
            // project-scoped Undo surface can offer the right entry instead
            // of blindly undoing the machine-wide newest.
            let touches_project = e.files.iter().any(|f| Path::new(&f.path).starts_with(dir));
            serde_json::json!({
                "id": e.id,
                "short_id": short_id(&e.id, &entries),
                "time_unix": e.time_unix,
                "scope": e.scope,
                "summary": crate::text::sanitize_line(&e.summary),
                "undone": e.undone,
                "touches_project": touches_project,
            })
        })
        .collect();
    let mut backups: Vec<serde_json::Value> = Vec::new();
    for desc in registry.iter() {
        for scope in [Scope::Global, Scope::Project] {
            if let Some((path, _)) = desc.config_for(scope, dir) {
                if atomic::backup_path(&path).exists() {
                    backups.push(serde_json::json!({
                        "adapter": desc.id,
                        "scope": scope.to_string(),
                        "path": path.display().to_string(),
                    }));
                }
            }
        }
    }
    serde_json::json!({
        "entries": entries_json,
        "adapter_backups": backups,
    })
}

/// Does user input `input` select entry `entry_id`? A plain prefix works; so
/// does a prefix of the id with leading zeros stripped, which keeps short ids
/// working for entries recorded by older builds that zero-padded ids to 32
/// hex digits.
fn id_matches(entry_id: &str, input: &str) -> bool {
    entry_id.starts_with(input) || entry_id.trim_start_matches('0').starts_with(input)
}

/// The short id shown in the listing: the shortest prefix of `id` (leading
/// zeros stripped, minimum 8 chars) that selects no other recorded entry — so
/// what's printed always works verbatim as `restore <id>`.
///
/// Rust note: the `'a` lifetime says the returned `&str` borrows from `id`
/// (it's a slice of it), not from `entries` — like returning a view into the
/// argument instead of allocating a new string.
fn short_id<'a>(id: &'a str, entries: &[history::Entry]) -> &'a str {
    let trimmed = id.trim_start_matches('0');
    let mut len = trimmed.len().min(8);
    while len < trimmed.len()
        && entries
            .iter()
            .any(|e| e.id != id && id_matches(&e.id, &trimmed[..len]))
    {
        len += 1;
    }
    &trimmed[..len]
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
                format!("{:<8}", short_id(&e.id, &entries)).bold(),
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
fn undo_selection(
    entry: &history::Entry,
    entries: &[history::Entry],
    write: bool,
    json: bool,
) -> Result<()> {
    let selected: Vec<&history::Entry> = match &entry.batch {
        Some(batch) => entries
            .iter()
            .filter(|candidate| candidate.batch.as_ref() == Some(batch) && !candidate.undone)
            .collect(),
        None => vec![entry],
    };
    if selected.is_empty() {
        anyhow::bail!("this change batch was already undone");
    }

    if !json {
        if selected.len() > 1 {
            println!(
                "{} undo one setup batch ({} recorded phases, newest first)",
                "↩".cyan(),
                selected.len()
            );
        }
        for selected_entry in &selected {
            preview_entry(selected_entry, entries);
        }
    }

    // One JSON shape for preview and result: `performed` says which this was.
    // Per-file `action` mirrors the text preview — `revert` (had prior bytes),
    // `delete` (file did not exist before the write), `match` (already
    // identical, undo is a no-op there). Computed BEFORE the undo runs so the
    // reported actions describe what the undo will do, not the state after it.
    let selected_json: Option<Vec<serde_json::Value>> = json.then(|| {
        selected
            .iter()
            .map(|e| {
                serde_json::json!({
                    "id": e.id,
                    "short_id": short_id(&e.id, entries),
                    "scope": e.scope,
                    "summary": crate::text::sanitize_line(&e.summary),
                    "files": e.files.iter().map(|f| {
                        let current = std::fs::read_to_string(&f.path).unwrap_or_default();
                        let action = match &f.before {
                            Some(before) if !diff::differs(&current, before) => "match",
                            Some(_) => "revert",
                            None => "delete",
                        };
                        serde_json::json!({
                            "label": f.label,
                            "path": f.path,
                            "action": action,
                        })
                    }).collect::<Vec<_>>(),
                })
            })
            .collect()
    });

    if write {
        // Newest-to-oldest is essential when two phases touched the same path:
        // first restore the state before the newest phase, then the state from
        // before the whole batch began.
        for selected_entry in &selected {
            history::undo(&selected_entry.id)?;
        }
        if !json {
            println!(
                "{} undone — reverted files show up as pending again; re-run `agentstack apply` to re-render.",
                "✓".green()
            );
        }
    } else if !json {
        println!("\nDry run. Re-run with {} to undo.", "--write".bold());
    }

    if let Some(selected_json) = selected_json {
        let out = serde_json::json!({
            "performed": write,
            "entries": selected_json,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&crate::ui_contract::envelope(out))?
        );
    }
    Ok(())
}

fn preview_entry(entry: &history::Entry, entries: &[history::Entry]) {
    println!(
        "  {} {} ({}, {}): {}",
        "↩".cyan(),
        short_id(&entry.id, entries),
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
            "no backup for {} ({scope}) — none has been written yet; run `agentstack apply --write` once first, then `agentstack restore` can revert it",
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::TEST_ENV_LOCK;

    /// Two back-to-back recorded changes (nanosecond timestamps sharing most
    /// of their high digits) must still list distinct short ids, and each
    /// short id must select exactly its own entry.
    #[test]
    fn two_recorded_changes_list_distinct_short_ids() {
        let _guard = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", home.path());
        let work = assert_fs::TempDir::new().unwrap();
        let file = work.path().join("c.json");

        for content in ["one", "two"] {
            let cap = history::capture(&file, "Test · servers");
            std::fs::write(&file, content).unwrap();
            history::record("global", vec!["Test".into()], vec![cap]).unwrap();
        }

        let entries = history::list();
        assert_eq!(entries.len(), 2);
        let a = short_id(&entries[0].id, &entries);
        let b = short_id(&entries[1].id, &entries);
        assert_ne!(a, b, "listed short ids must be unique");
        for (short, entry) in [(a, &entries[0]), (b, &entries[1])] {
            let hits: Vec<_> = entries
                .iter()
                .filter(|e| id_matches(&e.id, short))
                .collect();
            assert_eq!(hits.len(), 1, "short id {short} must be unambiguous");
            assert_eq!(hits[0].id, entry.id);
        }
        std::env::remove_var("AGENTSTACK_HOME");
    }
}
