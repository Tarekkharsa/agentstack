//! `agentstack undo` — the interactive face of `restore`.
//!
//! Strategy v2 Phase 3, item 3. `restore` has always been able to put things
//! back; what it asked for first was an id. That is a fine automation
//! contract and a poor recovery experience: the moment a person needs undo is
//! the moment they least want to run a listing command, read a hex prefix,
//! and retype it correctly. So this reads the same ledger and asks the
//! question the other way round — *here is what changed, newest first; which
//! point do you want to be at?*
//!
//! Three deliberate non-features:
//!
//! - **No new destructive machinery.** Every byte this moves is moved by
//!   `history::undo` → `history::rollback` → `atomic::write`, exactly as
//!   `restore` does. This module chooses *which* entries; it does not know how
//!   to change a file.
//! - **If a change isn't recorded, it isn't offered.** The timeline is built
//!   only from `history::list()`. Trust decisions, secret writes, library
//!   add/remove, and lockfile writes from `add`/`remove`/`install`/`upgrade`/
//!   `lock` are not in that ledger, so they cannot appear here — offering a
//!   revert we cannot perform is a worse failure than not offering one.
//!   Materialized skills are the case where that silence was itself the
//!   failure: `use --write` says "wrote skills to 3 locations" and this
//!   timeline then says "nothing recorded", with nothing joining the two. The
//!   ledger cannot hold them (see the `history` module docs — it stores a
//!   file's bytes, and a delivered skill is a linked directory whose contents
//!   an undo could not prove were still ours), so the honest repair is to name
//!   the gap here rather than widen the ledger. [`skills_outside_the_ledger`].
//! - **`restore` is unchanged.** Its flags, its JSON, and its behaviour are a
//!   declared contract; this is a second door onto the same room, not a
//!   replacement.
//!
//! And one deliberate feature: **the revert is itself recorded**, so undo is
//! undoable. See [`crate::history::undo_recorded`].

use std::io::Write;
use std::path::Path;

use agentstack_core::paint::OwoColorize;
use anyhow::Result;

use crate::cli::UndoArgs;
use crate::history::{self, Entry};

/// One row of the timeline: an entry, and how it is spelled to the user.
struct Row {
    entry: Entry,
    /// 1-based position as displayed. Stable for the length of one run only —
    /// the ledger is machine-global and another process can append to it, so
    /// the id is what is acted on and this is only what is typed.
    index: usize,
}

/// The entries this project may revert: recorded, not already undone, and
/// touching files under `dir`.
///
/// The project filter matters because the ledger is machine-global. A
/// timeline that offered another repository's applies would be offering to
/// break a project the user is not looking at.
fn timeline(dir: &Path) -> Vec<Row> {
    history::list()
        .into_iter()
        .filter(|e| !e.undone)
        .filter(|e| e.files.iter().any(|f| Path::new(&f.path).starts_with(dir)))
        .enumerate()
        .map(|(i, entry)| Row {
            entry,
            index: i + 1,
        })
        .collect()
}

/// The skills `use --write` materialized for this project that no undo can
/// take back, newest state first — deduplicated across targets, because one
/// skill delivered to three CLIs is one thing the user thinks about.
///
/// Read from the state ledger (`managed_skills`), which is where skill
/// ownership genuinely lives; the history ledger has no record of them at all.
/// Project-scope keys only: a global key is shared by every manifest on the
/// machine, so counting one here would let this project's Undo describe
/// another project's delivery.
///
/// Empty when there is nothing to warn about, so the note is a fact about THIS
/// project rather than a permanent disclaimer.
pub(crate) fn skills_outside_the_ledger(dir: &Path) -> Vec<String> {
    let Ok(state) = crate::state::State::load() else {
        return Vec::new();
    };
    let suffix = format!(
        "@project:{}",
        crate::manifest::project_root_of(dir).display()
    );
    let mut names: Vec<String> = state
        .targets
        .iter()
        .filter(|(key, _)| key.ends_with(&suffix))
        .flat_map(|(_, t)| t.managed_skills.iter().cloned())
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

/// Print the boundary — the sentence from the ledger seam, the names it is
/// about, and the commands that DO take them back — or print nothing when this
/// project has no materialized skills.
///
/// Shared by both Undo doors (`undo` and `x restore`) so they cannot drift into
/// two different promises.
pub(crate) fn print_skills_note(dir: &Path) {
    let names = skills_outside_the_ledger(dir);
    if names.is_empty() {
        return;
    }
    println!(
        "  {} {} ({})",
        "·".dimmed(),
        crate::history::SKILLS_ARE_NOT_RECORDED.dimmed(),
        names
            .iter()
            .map(|n| crate::text::sanitize_line(n))
            .collect::<Vec<_>>()
            .join(", ")
            .dimmed()
    );
    println!(
        "  {} {}",
        "·".dimmed(),
        crate::history::SKILLS_COME_OFF_WITH.dimmed()
    );
}

/// "today 15:12" / "yesterday" / a date — the resolution a person actually
/// reasons about when recovering. An exact timestamp is in `--json`.
fn when(time_unix: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(time_unix);
    let age = now.saturating_sub(time_unix);
    if age < 60 {
        "just now".to_string()
    } else if age < 3_600 {
        format!("{}m ago", age / 60)
    } else if age < 86_400 {
        format!("{}h ago", age / 3_600)
    } else if age < 172_800 {
        "yesterday".to_string()
    } else {
        format!("{}d ago", age / 86_400)
    }
}

fn print_timeline(rows: &[Row]) {
    println!("{}", "recent changes (newest first)".bold());
    for r in rows {
        println!(
            "  {}  {:<12} {:<44} {}",
            r.index.to_string().bold(),
            when(r.entry.time_unix).dimmed(),
            crate::text::sanitize_line(&r.entry.operation),
            crate::text::sanitize_line(&r.entry.summary).dimmed(),
        );
    }
}

/// The entries a revert-to-point must reverse: everything from the newest
/// down to and including the chosen row.
///
/// "Revert to that point" means *be in the state you were in before that
/// change*, which is only true if everything after it comes off too. Undoing
/// one middle entry in isolation would leave later changes layered on a base
/// they were never written against — a state that never existed.
fn selection(rows: &[Row], upto: usize) -> Vec<String> {
    rows.iter()
        .filter(|r| r.index <= upto)
        .map(|r| r.entry.id.clone())
        .collect()
}

pub fn run(args: &UndoArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let dir = manifest_dir
        .map(Path::to_path_buf)
        .unwrap_or(std::env::current_dir()?);
    let rows = timeline(&dir);

    if args.json {
        let out = serde_json::json!({
            // A UI drawing an Undo affordance has to know the affordance is
            // bounded, or it repeats the same over-promise in its own words.
            "skills_not_recorded": skills_outside_the_ledger(&dir),
            "entries": rows.iter().map(|r| serde_json::json!({
                "index": r.index,
                "id": r.entry.id,
                "time_unix": r.entry.time_unix,
                "when": when(r.entry.time_unix),
                "operation": crate::text::sanitize_line(&r.entry.operation),
                "summary": crate::text::sanitize_line(&r.entry.summary),
            })).collect::<Vec<_>>(),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&crate::ui_contract::envelope(out))?
        );
        return Ok(());
    }

    if rows.is_empty() {
        println!("nothing recorded to undo for this project.");
        // The line that closed G31. "Nothing recorded" straight after
        // `use --write` reported writing skills to three locations reads as a
        // bug in the ledger; it is a boundary of it, and the boundary has to be
        // stated where the user meets it.
        print_skills_note(&dir);
        println!(
            "  {}   {}",
            "agentstack status".green().bold(),
            "where this project stands".dimmed()
        );
        return Ok(());
    }

    // Show the timeline unless the user already picked from it. Reprinting the
    // list above a confirmed revert buries the one line that matters.
    if !(args.to.is_some() && args.write) {
        print_timeline(&rows);
        // A non-empty timeline over-promises in the other direction: it looks
        // like the complete record of what changed. Bound it here too.
        print_skills_note(&dir);
    }

    // Which point? Either named on the command line, or asked for.
    let upto = match args.to {
        Some(n) => n,
        None => {
            if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                // Non-interactive and no choice given: show the timeline and
                // stop. Guessing "probably the last one" would make a
                // destructive default out of an ambiguous invocation.
                println!();
                println!(
                    "  pick a point: {}",
                    "agentstack undo --to <n> --write".green().bold()
                );
                return Ok(());
            }
            print!("\nrevert to before which change? [1-{}, q] ", rows.len());
            std::io::stdout().flush()?;
            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
            let line = line.trim();
            if line.is_empty() || line.eq_ignore_ascii_case("q") {
                println!("nothing changed.");
                return Ok(());
            }
            line.parse::<usize>()
                .map_err(|_| anyhow::anyhow!("'{line}' is not one of the numbers above"))?
        }
    };

    if upto == 0 || upto > rows.len() {
        anyhow::bail!("pick a number between 1 and {}", rows.len());
    }
    let ids = selection(&rows, upto);
    let target = &rows[upto - 1].entry;

    if !args.write {
        // Undo is named before the first byte changes — the Phase-2 rule,
        // applied to the command whose whole job is changing bytes back.
        println!();
        println!(
            "would revert {} change(s), back to before {}.",
            ids.len(),
            crate::text::sanitize_line(&target.operation)
        );
        for r in rows.iter().filter(|r| r.index <= upto) {
            for f in &r.entry.files {
                println!("  {}", f.path.dimmed());
            }
            // Reversing a write means reversing the directories it made to
            // hold those files, so the preview has to name them too — with the
            // same conditional wording `x uninstall` uses, because the guard is
            // the same one: a parent that still holds anything is kept.
            for dir in history::prunable_dirs(&r.entry.files) {
                println!(
                    "  {} {}",
                    dir.display().to_string().dimmed(),
                    super::empty_parent_label().dimmed()
                );
            }
        }
        println!();
        println!(
            "  {}   {}",
            format!("agentstack undo --to {upto} --write")
                .green()
                .bold(),
            "do it — the revert is recorded, so this is itself undoable".dimmed()
        );
        return Ok(());
    }

    // Named before the revert, reported after it: once the directories are
    // gone there is nothing left to read them from.
    let prunable: Vec<std::path::PathBuf> = rows
        .iter()
        .filter(|r| r.index <= upto)
        .flat_map(|r| history::prunable_dirs(&r.entry.files))
        .collect();
    let recorded = history::undo_recorded(&ids)?;
    println!(
        "{} back to before {}",
        "✓".green(),
        crate::text::sanitize_line(&target.operation)
    );
    for dir in prunable.iter().filter(|dir| !dir.exists()) {
        println!(
            "  {} removed empty {}",
            "✓".green(),
            dir.display().to_string().dimmed()
        );
    }
    // F10: a `yes` row captures only the manifest declaration and the lock pin
    // — deliberately, per its own comment — so undoing it does NOT retract the
    // files `use --write` already delivered into each CLI. "nothing else
    // touched" was true of the files this revert covers and false of the
    // harness state the user actually sees (`ls .claude/skills/` still shows
    // it). So a yes-revert states plainly what it did and did not undo, and
    // names the command that finishes the job — the same honesty the `yes`
    // success line already keeps about its own undo.
    let reconciles_harness = target.operation == "yes" || target.operation.starts_with("yes ");
    if reconciles_harness {
        println!(
            "  {}",
            "the manifest and pin are reverted, but files already delivered to your CLIs are \
             not — run `agentstack use --write` to reconcile what each CLI holds"
                .dimmed()
        );
        return Ok(());
    }
    match recorded {
        Some(_) => println!(
            "  {}",
            "nothing else touched · this undo is itself undoable — run agentstack undo again"
                .dimmed()
        ),
        // Only when the reverted entries captured no files at all, which the
        // ledger should not produce; say so rather than imply a redo exists.
        None => println!(
            "  {}",
            "nothing else touched · this revert captured no files, so it has no redo".dimmed()
        ),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, t: u64, path: &str) -> Entry {
        Entry {
            id: id.to_string(),
            time_unix: t,
            scope: "project".into(),
            summary: "1 file".into(),
            targets: vec![],
            files: vec![history::FileChange {
                path: path.to_string(),
                before: Some("old".into()),
                label: "x".into(),
                created_dirs: Vec::new(),
            }],
            batch: None,
            operation: "apply".into(),
            undone: false,
        }
    }

    /// Reverting to point N takes everything down to and including N — not
    /// just N. Undoing a middle entry alone would leave later changes on a
    /// base they were never written against.
    #[test]
    fn revert_to_a_point_reverses_everything_after_it_too() {
        let rows: Vec<Row> = [3u64, 2, 1]
            .iter()
            .enumerate()
            .map(|(i, t)| Row {
                entry: entry(&format!("id{t}"), *t, "/tmp/x"),
                index: i + 1,
            })
            .collect();

        assert_eq!(selection(&rows, 1), vec!["id3"]);
        assert_eq!(selection(&rows, 2), vec!["id3", "id2"]);
        assert_eq!(selection(&rows, 3), vec!["id3", "id2", "id1"]);
    }
}
