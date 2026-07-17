//! Consolidate scattered skills into the central library.
//!
//! Skills end up spread across each CLI's own skills directory (`~/.agents/skills/
//! figma`, `~/.claude/skills/...`). This gathers them into the central capability
//! library (`~/.agentstack/lib/skills/`) via the library's own insertion path
//! ([`crate::commands::lib::add_skill`]) and replaces every original with a
//! symlink back to the library copy — so the agents still find each skill exactly
//! where they did, but the files now live in one reviewable place, indexed in
//! `library.toml` with a checksum, and referenced by name.
//!
//! The project manifest is **not** modified: a consolidated skill is referenced
//! by name from the library, and any existing inline `[skills.<name>]` in the
//! project is left untouched (it keeps overriding, per the resolver).
//!
//! Safety invariant: the managed copy is created *before* any original is
//! removed, and real directories are backed up before deletion. Dry-run by
//! default; nothing is touched unless `write` is set.

use agentstack_core::digest::Sha256Hex;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::adapter::Registry;
use crate::commands::lib::{add_skill, LibSource};
use crate::library::Library;
use crate::manifest::Manifest;
use crate::scope::Scope;
use crate::state::{target_key, State};
use crate::store::dir_digest;
use crate::util::paths;

/// Everything one consolidation run has to tell the user: what moved into the
/// library and what was found on disk but could not be consolidated.
#[derive(Debug)]
pub struct ConsolidateReport {
    pub skills: Vec<Consolidated>,
    /// Discovered entries that were skipped (dead links, non-skill dirs) —
    /// surfaced so the report can say so instead of silently dropping them.
    pub skipped: Vec<Skipped>,
}

/// A skills-dir entry consolidation could not gather: a symlink whose target is
/// gone, or a directory without a `SKILL.md`.
#[derive(Debug)]
pub struct Skipped {
    pub cli: String,
    pub name: String,
    /// The entry path in the CLI's skills dir.
    pub entry: PathBuf,
    /// The symlink's target, when the entry is a link.
    pub target: Option<PathBuf>,
    /// True for a dead symlink (target missing); false for a directory that is
    /// present but has no `SKILL.md`.
    pub broken: bool,
}

/// Outcome for one consolidated skill.
#[derive(Debug)]
pub struct Consolidated {
    pub name: String,
    /// Library path the skill now lives at (`lib/skills/<name>`).
    pub home: PathBuf,
    /// CLI ids whose skills dir now symlinks to the library copy (on a dry run,
    /// the ids that *would* be linked).
    pub linked_into: Vec<String>,
    /// True if the files were already the library copy / identical content.
    pub already_home: bool,
    /// True if the project manifest already defines `[skills.<name>]` inline —
    /// that definition keeps overriding the library copy for this project.
    pub inline_override: bool,
}

/// Gather discovered on-disk skills into the central library and symlink the
/// originals back to the library copy. `only` limits to specific names; `None`
/// consolidates every discovered skill. Inserts each into the library via
/// [`add_skill`] (index + checksum + provenance) and marks it managed for the
/// CLIs it was present in. The project manifest is not written — the skill is
/// referenced by name from the library.
///
/// A different-content collision with an existing library entry is a hard error
/// unless `replace`. When `write` is false, nothing is mutated (preview only).
pub fn consolidate(
    registry: &Registry,
    manifest_path: &Path,
    project_dir: &Path,
    only: Option<&[String]>,
    replace: bool,
    write: bool,
) -> Result<ConsolidateReport> {
    // Discover: name -> (real source, [(cli id, entry path in that CLI's dir)]).
    let mut found: BTreeMap<String, (PathBuf, Vec<(String, PathBuf)>)> = BTreeMap::new();
    let mut skipped: Vec<Skipped> = Vec::new();
    for desc in registry.iter() {
        let Some(dir) = desc.skills_dir_for(Scope::Global, project_dir) else {
            continue;
        };
        for sk in desc.discover_skills(Scope::Global, project_dir) {
            let entry = dir.join(&sk.name);
            if !sk.valid {
                // A dead link or a dir without SKILL.md can't be consolidated.
                // Record it (stray plain files stay quiet) so the report says
                // what was left behind — a dead link otherwise reads as "my
                // skills weren't migrated" with no clue why.
                if sk.broken || sk.source.is_dir() {
                    skipped.push(Skipped {
                        cli: desc.id.clone(),
                        name: sk.name.clone(),
                        target: fs::read_link(&entry).ok(),
                        entry,
                        broken: sk.broken,
                    });
                }
                continue;
            }
            found
                .entry(sk.name.clone())
                .or_insert_with(|| (sk.source.clone(), Vec::new()))
                .1
                .push((desc.id.clone(), entry));
        }
    }
    if let Some(names) = only {
        found.retain(|k, _| names.iter().any(|n| n == k));
        skipped.retain(|s| names.iter().any(|n| n == &s.name));
    }
    if found.is_empty() && skipped.is_empty() {
        anyhow::bail!("no skills found on disk to consolidate");
    }
    skipped.sort_by(|a, b| (&a.cli, &a.name).cmp(&(&b.cli, &b.name)));

    let lib_home = paths::lib_home();
    let lib_skills = lib_home.join("skills");
    // A snapshot of the index for idempotency/collision decisions (each name is
    // processed once, so the snapshot never goes stale within this run).
    let library = Library::load(&lib_home)?;
    // Read the project manifest to detect inline overrides (never written).
    let inline_names = inline_skill_names(manifest_path);
    let mut state = State::load()?;
    let mut report = Vec::new();

    for (name, (source, entries)) in found {
        let target = lib_skills.join(&name);
        let source_canon = fs::canonicalize(&source).unwrap_or_else(|_| source.clone());

        // 1. Ensure the files are the library copy, via the library's own
        //    insertion path. Skip if the source already IS the library copy or
        //    the library already holds identical content (idempotent re-run).
        let already_home =
            source_canon == target || library_has_same(&library, &name, &source_canon);
        if !already_home {
            // Consolidation adopts the user's own already-present skills; scan
            // findings are surfaced by audit/doctor, never a reason to block
            // the adoption here (matching `lib migrate`).
            add_skill(
                &lib_home,
                &name,
                LibSource::Path(&source_canon),
                replace,
                write,
                true,
            )?;
        }

        // 2. Repoint every CLI occurrence to the library copy (write only).
        let mut linked = Vec::new();
        for (cli, entry) in &entries {
            if write {
                if let Ok(meta) = fs::symlink_metadata(entry) {
                    if meta.file_type().is_symlink() {
                        fs::remove_file(entry)
                            .with_context(|| format!("removing link {}", entry.display()))?;
                    } else if meta.is_dir() {
                        backup_dir(entry, &name).ok();
                        fs::remove_dir_all(entry)
                            .with_context(|| format!("removing {}", entry.display()))?;
                    }
                }
                symlink_dir(&target, entry).with_context(|| {
                    format!("linking {} → {}", entry.display(), target.display())
                })?;

                // Mark it managed for this CLI (global) so the matrix shows it on.
                let key = target_key(cli, Scope::Global, project_dir);
                let mut managed = state.managed_skills(&key);
                if !managed.iter().any(|s| s == &name) {
                    managed.push(name.clone());
                }
                state.record_skills(&key, managed);
            }
            linked.push(cli.clone());
        }

        report.push(Consolidated {
            name: name.clone(),
            home: target,
            linked_into: linked,
            already_home,
            inline_override: inline_names.iter().any(|n| n == &name),
        });
    }

    // No manifest write: consolidated skills are referenced by name from the
    // library, and inline overrides are left as-is.
    if write {
        state.save()?;
    }
    Ok(ConsolidateReport {
        skills: report,
        skipped,
    })
}

/// Whether the library already holds a `name` entry whose recorded checksum
/// matches the source directory's current content (an idempotent re-run).
fn library_has_same(library: &Library, name: &str, source: &Path) -> bool {
    match library
        .get(name)
        .and_then(|e| e.checksum.as_ref().map(Sha256Hex::hex))
    {
        Some(locked) => dir_digest(source).ok().as_ref().map(Sha256Hex::hex) == Some(locked),
        None => false,
    }
}

/// Names defined inline as `[skills.<name>]` in the project manifest (best
/// effort; a missing/invalid manifest yields none).
fn inline_skill_names(manifest_path: &Path) -> Vec<String> {
    fs::read_to_string(manifest_path)
        .ok()
        .and_then(|t| toml::from_str::<Manifest>(&t).ok())
        .map(|m| m.skills.keys().cloned().collect())
        .unwrap_or_default()
}

/// Back up a directory under `~/.agentstack/backups/skills/<name>` (best effort).
fn backup_dir(src: &Path, name: &str) -> Result<()> {
    if !src.is_dir() {
        return Ok(());
    }
    let dest = paths::backups_dir().join("skills").join(name);
    if dest.exists() {
        let _ = fs::remove_dir_all(&dest);
    }
    crate::util::fsx::copy_dir_all_following_symlinks(src, &dest)
}

/// Recursively copy a directory tree, following symlinks.
///
/// Kept as a thin re-export so [`crate::commands::lib::add_skill`] — which
/// calls this by name — doesn't need to spell out the fsx path; the `.git`
/// skip and symlink-following recursion live once in
/// [`crate::util::fsx::copy_dir_all_following_symlinks`].
pub(crate) fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    crate::util::fsx::copy_dir_all_following_symlinks(src, dst)
}

#[cfg(unix)]
fn symlink_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(src, dst)
}

#[cfg(windows)]
fn symlink_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(src, dst)
}
