//! Managed `.gitignore` block for generated project artifacts.
//!
//! Project-scope writes (`.mcp.json`, `.claude/skills/*` symlinks) are
//! machine-local: symlinks carry absolute home paths and rendered configs can
//! carry resolved secrets. By default they are kept out of git via a marked
//! block this module owns — created, updated, and emptied as the managed set
//! changes, never touching the rest of the file. Files a user already tracks
//! in git are unaffected (gitignore never hides tracked files), so
//! commit-the-artifacts workflows keep working; `--no-gitignore` opts out of
//! the block entirely.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

const BEGIN: &str = "# >>> agentstack — generated project artifacts (machine-local) >>>";
const END: &str = "# <<< agentstack >>>";

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

/// Replace (or insert, or drop) the managed block in `existing`, leaving every
/// other byte untouched.
fn splice(existing: &str, entries: &[String]) -> String {
    let mut sorted: Vec<&str> = entries.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    sorted.dedup();

    let block = if sorted.is_empty() {
        String::new()
    } else {
        format!("{BEGIN}\n{}\n{END}\n", sorted.join("\n"))
    };

    let lines: Vec<&str> = existing.lines().collect();
    let begin = lines.iter().position(|l| l.trim() == BEGIN);
    let end = lines.iter().position(|l| l.trim() == END);

    match (begin, end) {
        (Some(b), Some(e)) if b <= e => {
            let mut out: Vec<String> = lines[..b].iter().map(|s| s.to_string()).collect();
            if !block.is_empty() {
                out.push(block.trim_end().to_string());
            } else {
                // Dropping the block: also drop a now-dangling blank line above.
                while out.last().is_some_and(|l| l.trim().is_empty()) {
                    out.pop();
                }
            }
            out.extend(lines[e + 1..].iter().map(|s| s.to_string()));
            let mut s = out.join("\n");
            if !s.is_empty() {
                s.push('\n');
            }
            s
        }
        _ if block.is_empty() => existing.to_string(),
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
    fn empty_entries_drop_the_block() {
        let with = splice("dist/\n", &e(&["/.mcp.json"]));
        let without = splice(&with, &[]);
        assert_eq!(without, "dist/\n");
        // And a no-block file stays byte-identical.
        assert_eq!(splice("dist/\n", &[]), "dist/\n");
    }
}
