//! Managed `.gitignore` block for generated project artifacts.
//!
//! Project-scope writes (`.mcp.json`, `.claude/skills/*` symlinks, and the
//! compiled `CLAUDE.md` / `AGENTS.md` instruction files) are generated
//! artifacts: symlinks carry absolute home paths, rendered configs can carry
//! resolved secrets, and instruction files are compiled from the manifest's
//! fragments. By default they are kept out of git via a marked block this
//! module owns — created and updated as the managed set changes, never touching
//! the rest of the file.
//!
//! A path is ignored **iff agentstack wrote it this run, or a persistent record
//! (state / on-disk managed marker) says agentstack currently manages it** —
//! never merely because the manifest declares it. A run whose writes were all
//! blocked (unresolved secrets) records nothing and so contributes nothing: it
//! must not hide a hand-maintained `.mcp.json` / `CLAUDE.md` from `git status`.
//! `apply` and `use` derive the [`Managed`] flags from the SAME records, so
//! alternating them on an unchanged setup yields a byte-identical block.
//!
//! Callers pass **stable, directory-level** paths (the managed config file, the
//! skills dir with a trailing slash) so the block does not churn as profile
//! membership changes, and an emptied managed set (deactivation) **leaves the
//! block intact**: removing it would dirty a `.gitignore` a team may have
//! committed. Files a user already tracks in git are unaffected (gitignore never
//! hides tracked files), so commit-the-artifacts workflows keep working;
//! `--no-gitignore` opts out of the block entirely.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::adapter::AdapterDescriptor;
use crate::scope::Scope;

const BEGIN: &str = "# >>> agentstack — generated project artifacts (machine-local) >>>";
const END: &str = "# <<< agentstack >>>";

/// The one label every ledger entry for this file carries. Shared so `apply`,
/// `use` and `session start` name the same thing the same way — a user reading
/// `undo` or `session end` sees one line, not three spellings of it.
pub const HISTORY_LABEL: &str = ".gitignore · managed artifacts";

/// Which of a target's generated project-scope artifacts agentstack currently
/// manages. Each caller computes these **after** its write sections, from
/// outcomes and persistent records (see the module docs), not from manifest
/// declarations — so a blocked write contributes nothing and both commands
/// agree on an unchanged setup.
#[derive(Debug, Clone, Copy, Default)]
pub struct Managed {
    /// The MCP config file: `state.managed_servers` (or `kept_foreign`) is
    /// non-empty, so a managed — possibly secret-carrying — file is on disk.
    pub config: bool,
    /// The skills dir: skills were materialized this run, or state records that
    /// they were (bare existence of the dir is not enough — a user may hand-own
    /// it).
    pub skills: bool,
    /// The compiled instruction file: it was written this run, or already
    /// carries agentstack's managed region on disk.
    pub instructions: bool,
}

/// The stable, directory-level ignore entries for the artifacts one target
/// currently manages (`managed`). Entries are project-root relative and
/// `/`-prefixed (dirs get a trailing `/`).
pub fn managed_entries(
    desc: &AdapterDescriptor,
    scope: Scope,
    manifest_dir: &Path,
    managed: Managed,
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

    if managed.config {
        if let Some((cfg, _)) = desc.config_for(scope, manifest_dir) {
            push(&cfg, false);
        }
    }
    if managed.skills {
        if let Some(dir) = desc.skills_dir_for(scope, manifest_dir) {
            push(&dir, true);
        }
    }
    if managed.instructions {
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

/// Whether a managed block is currently present in this project's
/// `.gitignore`.
///
/// Only used to *report* a leftover block once a project has opted out
/// (`[meta] gitignore = false`). Routine commands deliberately never strip the
/// block — see [`remove_block`]'s rationale — so a project that opts out with
/// one already committed keeps it until someone removes it on purpose. Saying
/// so is then the only honest move: silence would let a user believe their
/// artifacts are visible to `git status` when they are not.
pub fn has_block(project_root: &Path) -> bool {
    let existing = fs::read_to_string(project_root.join(".gitignore")).unwrap_or_default();
    let lines: Vec<&str> = existing.lines().collect();
    lines.iter().any(|l| l.trim() == BEGIN) && lines.iter().any(|l| l.trim() == END)
}

/// Take the managed block back out entirely, for `uninstall` (review finding
/// N3) and for the explicitly consented opt-out (`[meta] gitignore = false`,
/// chosen in the wizard or through the panel's `set-gitignore` verb). Returns
/// the new file content when a block was present, `None` when there was
/// nothing to remove.
///
/// Both callers share one property that routine commands do not: a human just
/// said, in that moment, that this project should not have the block. That is
/// what separates them from deactivation, which must leave a committed block
/// alone.
///
/// This is deliberately NOT `ensure_block(&[])`: [`splice`] treats an empty
/// entry set as "leave the block alone", because *deactivation* must not strip
/// a block a team may have committed — the entries stay correct for the next
/// activation. Uninstall is the one case where that reasoning inverts. The
/// block names generated files that no longer exist, so leaving it means a repo
/// carries dead AgentStack config after being told AgentStack was uninstalled.
pub fn remove_block(existing: &str) -> Option<String> {
    let lines: Vec<&str> = existing.lines().collect();
    let begin = lines.iter().position(|l| l.trim() == BEGIN)?;
    let end = lines.iter().position(|l| l.trim() == END)?;
    if begin > end {
        return None;
    }
    // Drop the block and any blank line that only existed to separate it, so
    // removing it leaves the file as it would have been had we never written.
    let mut head: Vec<&str> = lines[..begin].to_vec();
    while head.last().is_some_and(|l| l.trim().is_empty()) {
        head.pop();
    }
    let tail = &lines[end + 1..];
    let mut out: Vec<&str> = head;
    out.extend_from_slice(tail);
    let mut s = out.join("\n");
    if !s.trim().is_empty() {
        s.push('\n');
    } else {
        // Only our block was in there — hand back an empty file so the caller
        // can prune it rather than leave a whitespace husk.
        s.clear();
    }
    Some(s)
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
