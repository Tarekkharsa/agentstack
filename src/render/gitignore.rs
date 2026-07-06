//! Managed `.gitignore` block for generated project artifacts.
//!
//! Project-scope writes (`.mcp.json`, `.claude/skills/*` symlinks, and the
//! compiled `CLAUDE.md` / `AGENTS.md` instruction files) are generated
//! artifacts: symlinks carry absolute home paths, rendered configs can carry
//! resolved secrets, and instruction files are compiled from the manifest's
//! fragments. By default they are kept out of git via a marked block this
//! module owns — created and updated as the managed set changes, never touching
//! the rest of the file. Callers pass **stable,
//! directory-level entries** (the managed config file, the skills dir with a
//! trailing slash) so the block does not churn as profile membership changes,
//! and an emptied managed set (deactivation) **leaves the block intact**:
//! removing it would dirty a `.gitignore` a team may have committed. Files a
//! user already tracks in git are unaffected (gitignore never hides tracked
//! files), so commit-the-artifacts workflows keep working; `--no-gitignore`
//! opts out of the block entirely.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::adapter::AdapterDescriptor;
use crate::manifest::Manifest;
use crate::scope::Scope;

const BEGIN: &str = "# >>> agentstack — generated project artifacts (machine-local) >>>";
const END: &str = "# <<< agentstack >>>";

/// The stable, directory-level ignore entries for one target's generated
/// project-scope artifacts, derived from what the manifest **declares** — not
/// from what any single command happens to write this run. `use` and `apply`
/// both emit this set, so the managed block is identical whichever you run (no
/// churn on a possibly-committed `.gitignore`). Entries are project-root
/// relative and `/`-prefixed (dirs get a trailing `/`). Being generous is safe:
/// ignoring a path that isn't generated yet is a no-op, whereas failing to
/// ignore a generated file is the bug this prevents.
pub fn managed_entries(
    manifest: &Manifest,
    desc: &AdapterDescriptor,
    scope: Scope,
    manifest_dir: &Path,
) -> Vec<String> {
    let project_root = crate::manifest::project_root_of(manifest_dir);
    let mut out = Vec::new();
    let mut push = |path: &Path, is_dir: bool| {
        if let Ok(rel) = path.strip_prefix(&project_root) {
            out.push(format!(
                "/{}{}",
                rel.display(),
                if is_dir { "/" } else { "" }
            ));
        }
    };

    // MCP config file — when the manifest declares any servers.
    if !manifest.servers.is_empty() {
        if let Some((cfg, _)) = desc.config_for(scope, manifest_dir) {
            push(&cfg, false);
        }
    }
    // Skills directory — when any skill can be materialized (inline, or via a
    // profile that references library skills).
    let has_skills =
        !manifest.skills.is_empty() || manifest.profiles.values().any(|p| !p.skills.is_empty());
    if has_skills {
        if let Some(dir) = desc.skills_dir_for(scope, manifest_dir) {
            push(&dir, true);
        }
    }
    // Compiled instruction file — only when a fragment actually compiles at
    // THIS scope for THIS target. Machine-layer fragments (from `init --global`,
    // folded in by merge_user_layer) compile at global scope only, so a project
    // carrying only those generates no project CLAUDE.md — and must not gitignore
    // a hand-written one. Mirror plan_instructions' own filter.
    let has_instructions = manifest
        .instructions
        .values()
        .any(|i| i.applies_to(&desc.id) && !(scope == Scope::Project && i.from_user_layer));
    if has_instructions {
        if let Some(p) = desc
            .instructions
            .as_ref()
            .and_then(|s| s.path_for(scope, manifest_dir))
        {
            push(&p, false);
        }
    }
    out
}

/// Ensure the project's `.gitignore` contains exactly `entries` inside the
/// managed block. No-op (Ok(false)) when the project root is not a git repo
/// or nothing would change. Returns whether the file was (or would be)
/// changed.
pub fn ensure_block(project_root: &Path, entries: &[String], write: bool) -> Result<bool> {
    if !project_root.join(".git").exists() {
        return Ok(false);
    }
    let path = project_root.join(".gitignore");
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let updated = splice(&existing, entries);
    if updated == existing {
        return Ok(false);
    }
    if write {
        crate::util::atomic::write(&path, &updated)
            .with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(true)
}

/// Replace (or insert) the managed block in `existing`, leaving every other
/// byte untouched. An empty entry set changes nothing: deactivation must not
/// strip a block a team may have committed (the stable entries stay correct
/// for the next activation anyway).
fn splice(existing: &str, entries: &[String]) -> String {
    let mut sorted: Vec<&str> = entries.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    sorted.dedup();

    if sorted.is_empty() {
        return existing.to_string();
    }
    let block = format!("{BEGIN}\n{}\n{END}\n", sorted.join("\n"));

    let lines: Vec<&str> = existing.lines().collect();
    let begin = lines.iter().position(|l| l.trim() == BEGIN);
    let end = lines.iter().position(|l| l.trim() == END);

    match (begin, end) {
        (Some(b), Some(e)) if b <= e => {
            let mut out: Vec<String> = lines[..b].iter().map(|s| s.to_string()).collect();
            out.push(block.trim_end().to_string());
            out.extend(lines[e + 1..].iter().map(|s| s.to_string()));
            let mut s = out.join("\n");
            if !s.is_empty() {
                s.push('\n');
            }
            s
        }
        _ => {
            let mut s = existing.trim_end().to_string();
            if !s.is_empty() {
                s.push_str("\n\n");
            }
            s.push_str(&block);
            s
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn inserts_block_into_empty_and_existing() {
        let out = splice("", &e(&["/.mcp.json"]));
        assert!(out.contains(BEGIN) && out.contains("/.mcp.json"));
        let out = splice("target/\n", &e(&["/.mcp.json"]));
        assert!(out.starts_with("target/\n"));
        assert!(out.ends_with(&format!("{END}\n")));
    }

    #[test]
    fn updates_and_sorts_block_in_place() {
        let start = splice("node_modules/\n", &e(&["/.mcp.json"]));
        let updated = splice(&start, &e(&["/.claude/skills/b", "/.claude/skills/a"]));
        assert!(!updated.contains("/.mcp.json"));
        let a = updated.find("/.claude/skills/a").unwrap();
        let b = updated.find("/.claude/skills/b").unwrap();
        assert!(a < b, "entries sorted");
        assert!(updated.starts_with("node_modules/\n"), "rest untouched");
        assert_eq!(updated.matches(BEGIN).count(), 1);
    }

    #[test]
    fn empty_entries_leave_the_block_intact() {
        // Deactivation: the existing block stays byte-identical — dropping it
        // would dirty a committed .gitignore in team repos.
        let with = splice("dist/\n", &e(&["/.mcp.json"]));
        assert_eq!(splice(&with, &[]), with);
        // And a no-block file stays byte-identical too.
        assert_eq!(splice("dist/\n", &[]), "dist/\n");
    }

    #[test]
    fn directory_level_entries_are_stable_across_reruns() {
        // Callers emit the skills dir (trailing slash) + the managed config
        // file — not per-skill lines — so re-splicing the same set is a no-op
        // whatever the active skill membership is.
        let first = splice("", &e(&["/.claude/skills/", "/.mcp.json"]));
        assert!(first.contains("/.claude/skills/\n"));
        let second = splice(&first, &e(&["/.mcp.json", "/.claude/skills/"]));
        assert_eq!(first, second);
    }
}
