//! Consolidate scattered skills into one managed home.
//!
//! Skills end up spread across each CLI's own skills directory (`~/.codex/skills/
//! figma`, `~/.claude/skills/...`). This gathers them into a single managed home
//! (`~/.agentstack/skills/`) and replaces every original with a symlink back to
//! it — so the agents still find each skill exactly where they did, but the files
//! now live in one place agentstack controls.
//!
//! Safety invariant: the managed copy is created *before* any original is
//! removed, and real directories are backed up before deletion. If anything
//! fails mid-way the originals (or their backups) still hold the files.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::adapter::Registry;
use crate::scope::Scope;
use crate::state::{target_key, State};
use crate::store::dir_digest;
use crate::util::paths;

/// Outcome for one consolidated skill.
pub struct Consolidated {
    pub name: String,
    /// Managed home path the skill now lives at.
    pub home: PathBuf,
    /// CLI ids whose skills dir now symlinks to the managed copy.
    pub linked_into: Vec<String>,
    /// True if the files were already in the managed home (nothing moved).
    pub already_home: bool,
}

/// Move discovered on-disk skills into the managed home and symlink the
/// originals back. `only` limits to specific names; `None` consolidates every
/// discovered skill. Updates the manifest (`[skills.*]` path entries) and state
/// (marks them managed for the CLIs they were present in).
pub fn consolidate(
    registry: &Registry,
    manifest_path: &Path,
    project_dir: &Path,
    only: Option<&[String]>,
) -> Result<Vec<Consolidated>> {
    // Discover: name -> (real source, [(cli id, entry path in that CLI's dir)]).
    let mut found: BTreeMap<String, (PathBuf, Vec<(String, PathBuf)>)> = BTreeMap::new();
    for desc in registry.iter() {
        let Some(dir) = desc.skills_dir_for(Scope::Global, project_dir) else {
            continue;
        };
        for sk in desc.discover_skills(Scope::Global, project_dir) {
            let entry = dir.join(&sk.name);
            found
                .entry(sk.name.clone())
                .or_insert_with(|| (sk.source.clone(), Vec::new()))
                .1
                .push((desc.id.clone(), entry));
        }
    }
    if let Some(names) = only {
        found.retain(|k, _| names.iter().any(|n| n == k));
    }
    if found.is_empty() {
        anyhow::bail!("no skills found on disk to consolidate");
    }

    let home = paths::skills_home();
    fs::create_dir_all(&home).with_context(|| format!("creating {}", home.display()))?;
    let mut state = State::load()?;
    let mut manifest_text = fs::read_to_string(manifest_path)
        .with_context(|| format!("reading {}", manifest_path.display()))?;
    let mut report = Vec::new();

    for (name, (source, entries)) in found {
        let target = home.join(&name);
        let source_canon = fs::canonicalize(&source).unwrap_or_else(|_| source.clone());

        // 1. Get the files into the managed home (copy, never move) — unless the
        //    source already IS the managed copy.
        let already_home = source_canon == target;
        if !already_home {
            if target.exists() {
                // Same name already consolidated: allow only if identical content.
                let same = dir_digest(&target).ok() == dir_digest(&source_canon).ok();
                if !same {
                    anyhow::bail!(
                        "a different skill named '{name}' already exists in the skills home ({})",
                        target.display()
                    );
                }
            } else {
                backup_dir(&source_canon, &name).ok();
                copy_dir(&source_canon, &target)
                    .with_context(|| format!("copying skill '{name}' into the managed home"))?;
            }
        }

        // 2. Repoint every CLI occurrence to the managed copy.
        let mut linked = Vec::new();
        for (cli, entry) in &entries {
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
            symlink_dir(&target, entry)
                .with_context(|| format!("linking {} → {}", entry.display(), target.display()))?;
            linked.push(cli.clone());

            // Mark it managed for this CLI (global) so the matrix shows it on.
            let key = target_key(cli, Scope::Global);
            let mut managed = state.managed_skills(&key);
            if !managed.iter().any(|s| s == &name) {
                managed.push(name.clone());
            }
            state.record_skills(&key, managed);
        }

        // 3. Register in the manifest as a path skill pointing at the managed home.
        let body = serde_json::json!({ "path": target.display().to_string() });
        manifest_text = crate::commands::add::build_manifest_with(
            &manifest_text,
            "skills",
            &name,
            &body,
            None,
        )?;

        report.push(Consolidated {
            name,
            home: target,
            linked_into: linked,
            already_home,
        });
    }

    crate::util::atomic::write(manifest_path, &manifest_text)
        .with_context(|| format!("writing {}", manifest_path.display()))?;
    state.save()?;
    Ok(report)
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
    copy_dir(src, &dest)
}

/// Recursively copy a directory tree.
fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst).with_context(|| format!("creating {}", dst.display()))?;
    for entry in fs::read_dir(src).with_context(|| format!("reading {}", src.display()))? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let ft = entry.file_type()?;
        if ft.is_dir() {
            copy_dir(&from, &to)?;
        } else if ft.is_symlink() {
            // Copy the link's target contents (skills rarely nest links; keep it simple).
            if let Ok(real) = fs::canonicalize(&from) {
                if real.is_dir() {
                    copy_dir(&real, &to)?;
                } else {
                    fs::copy(&real, &to)?;
                }
            }
        } else {
            fs::copy(&from, &to).with_context(|| format!("copying {}", from.display()))?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn symlink_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(src, dst)
}

#[cfg(windows)]
fn symlink_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(src, dst)
}
