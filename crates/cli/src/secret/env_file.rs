//! Writer for the project `.env` file — the plaintext secret store that `init`
//! and `secret set --env-file` target when the user picks the `.env` option.
//!
//! The resolver ([`super::DotEnvResolver`]) already *reads* `.env`; this module
//! is the missing *writer*. It is deliberately minimal and non-destructive:
//! existing lines and comments are preserved, a `NAME=` line is updated in
//! place when it already exists, and new names are appended. Values are quoted
//! only when they would otherwise not round-trip through the reader (which
//! trims whitespace and strips surrounding quotes).
//!
//! Security note (rule 5): a `.env` holds real secret *values*, never `${REF}`
//! placeholders — those live in the manifest, which stays commit-safe. So the
//! writer also makes sure the project keeps `.env` out of git.

use std::path::Path;

use anyhow::{Context, Result};

use crate::util::atomic;

/// Append-or-update `NAME=value` lines in `<dir>/.env`, creating the file if it
/// does not exist. Existing lines (including comments and unrelated vars) are
/// preserved; a name that already has an assignment is rewritten in place.
pub fn write(dir: &Path, entries: &[(String, String)]) -> Result<()> {
    let path = dir.join(".env");
    // `unwrap_or_default` == "" for a missing file, so the create-if-absent and
    // update-existing paths share one code path.
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let updated = upsert(&existing, entries);
    atomic::write(&path, &updated).with_context(|| format!("writing {}", path.display()))
}

/// Pure core of [`write`], split out so it can be unit-tested without touching
/// the filesystem. Returns the new file contents.
fn upsert(existing: &str, entries: &[(String, String)]) -> String {
    // Track which names we still need to write; drain as we rewrite them in place.
    let mut pending: Vec<(&str, &str)> = entries
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    let mut out_lines: Vec<String> = Vec::new();
    for line in existing.lines() {
        // An assignment whose key is one we're setting is replaced in place
        // (duplicates too — `retain` clears every copy); everything else is
        // copied through verbatim.
        if let Some(key) = line_key(line) {
            if let Some((_, value)) = pending.iter().find(|(k, _)| *k == key).copied() {
                out_lines.push(format!("{key}={}", format_value(value)));
                pending.retain(|(k, _)| *k != key);
                continue;
            }
        }
        out_lines.push(line.to_string());
    }

    // Names not already present are appended in their original order.
    for (k, v) in &pending {
        out_lines.push(format!("{k}={}", format_value(v)));
    }

    let mut text = out_lines.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    text
}

/// If `line` is a `NAME=…` assignment (optionally `export NAME=…`), return
/// `NAME`. Matches how [`super::DotEnvResolver`] parses keys, so a line we
/// write is a line we can later find and update.
fn line_key(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    let trimmed = trimmed.strip_prefix("export ").unwrap_or(trimmed);
    let (key, _) = trimmed.split_once('=')?;
    let key = key.trim();
    if key.is_empty() {
        None
    } else {
        Some(key)
    }
}

/// Quote a value only when leaving it bare would change what the reader sees.
/// The reader trims surrounding whitespace and strips one layer of quotes, so a
/// value that has neither leading/trailing whitespace nor shell-significant
/// characters can be written as-is.
fn format_value(value: &str) -> String {
    let needs_quote = value.is_empty()
        || value != value.trim()
        || value
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '#' | '"' | '\''));
    if needs_quote {
        format!("\"{value}\"")
    } else {
        value.to_string()
    }
}

/// Ensure the project keeps `.env` out of git. A plaintext secret file must
/// never be committable, so we add a plain `.env` line to `.gitignore` when the
/// project is a git repo and nothing already ignores it. This is intentionally
/// *outside* the managed-artifacts block that `apply`/`use` own and rewrite:
/// those commands splice only their marked region, so a standalone `.env` line
/// survives every re-render. No-op (`Ok(false)`) when the root is not a git
/// repo or `.env` is already ignored. Returns whether the file changed.
pub fn ensure_gitignored(project_root: &Path, write: bool) -> Result<bool> {
    if !project_root.join(".git").exists() {
        return Ok(false);
    }
    let path = project_root.join(".gitignore");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    if already_ignores_env(&existing) {
        return Ok(false);
    }
    let mut updated = existing.trim_end().to_string();
    if !updated.is_empty() {
        updated.push_str("\n\n");
    }
    updated.push_str("# agentstack: local secrets — never commit\n.env\n");
    if write {
        atomic::write(&path, &updated).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(true)
}

/// Whether some existing `.gitignore` line already ignores `.env`. Kept simple
/// on purpose: the exact patterns a developer would use (`​.env`, `/.env`).
fn already_ignores_env(gitignore: &str) -> bool {
    gitignore.lines().any(|l| {
        let l = l.trim();
        l == ".env" || l == "/.env"
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one behavior that matters: appending new names while preserving
    /// existing lines/comments and updating a name in place.
    #[test]
    fn upsert_appends_preserves_and_updates() {
        let existing = "# my env\nEXISTING=keep\nTOKEN=old\n";
        let out = upsert(
            existing,
            &[
                ("TOKEN".into(), "new".into()),   // updated in place
                ("ADDED".into(), "value".into()), // appended
            ],
        );
        assert_eq!(out, "# my env\nEXISTING=keep\nTOKEN=new\nADDED=value\n");
    }

    #[test]
    fn upsert_into_empty_creates_lines() {
        let out = upsert("", &[("A".into(), "1".into())]);
        assert_eq!(out, "A=1\n");
    }

    #[test]
    fn values_are_quoted_only_when_needed() {
        // Bare token: no quotes.
        assert_eq!(format_value("ghp_abc123"), "ghp_abc123");
        // Whitespace, comment char, or quotes force quoting so the reader
        // round-trips the exact value.
        assert_eq!(format_value("has space"), "\"has space\"");
        assert_eq!(format_value("a#b"), "\"a#b\"");
        assert_eq!(format_value(" pad "), "\" pad \"");
    }

    #[test]
    fn round_trips_through_the_reader() {
        // A quoted value written here must parse back to the same string via
        // the DotEnvResolver the runtime actually uses.
        let out = upsert(
            "",
            &[
                ("PLAIN".into(), "abc".into()),
                ("SPACED".into(), "one two".into()),
            ],
        );
        let r = crate::secret::DotEnvResolver::parse(&out);
        assert_eq!(
            crate::secret::Resolver::resolve(&r, "PLAIN").as_deref(),
            Some("abc")
        );
        assert_eq!(
            crate::secret::Resolver::resolve(&r, "SPACED").as_deref(),
            Some("one two")
        );
    }

    #[test]
    fn gitignore_env_detection() {
        assert!(already_ignores_env("node_modules/\n.env\n"));
        assert!(already_ignores_env("/.env"));
        assert!(!already_ignores_env("node_modules/\n.env.local\n"));
        assert!(!already_ignores_env(""));
    }
}
