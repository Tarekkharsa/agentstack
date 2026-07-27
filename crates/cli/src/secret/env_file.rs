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
//! writer keeps the file out of git *and* off other local accounts: it is
//! written through [`atomic::write_private`], i.e. `0600`, never the ambient
//! umask.

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
    // `write_private`, not `write`: these are real token values. It also
    // tightens a file an older version left at the umask default.
    atomic::write_private(&path, &updated).with_context(|| format!("writing {}", path.display()))
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

/// Ensure the project keeps our `.env` out of git. A plaintext secret file must
/// never be committable, so we add an anchored rule for the exact file we wrote
/// — `/.agentstack/.env`, or `/.env` for a legacy root manifest — when the
/// project is a git repo and nothing already ignores it.
///
/// The rule is deliberately *scoped*. A bare `.env` line matches at every depth,
/// so it would also silence the user's own `.env` files anywhere in the
/// repository; AgentStack should not change the ignore semantics of files it did
/// not write.
///
/// The line is intentionally *outside* the managed-artifacts block that
/// `apply`/`use` own and rewrite: those commands splice only their marked
/// region, so a standalone rule survives every re-render. No-op (`Ok(false)`)
/// when the root is not a git repo or the file is already ignored. Returns
/// whether the file changed.
pub fn ensure_gitignored(project_root: &Path, env_dir: &Path, write: bool) -> Result<bool> {
    if !project_root.join(".git").exists() {
        return Ok(false);
    }
    let rule = gitignore_rule(project_root, env_dir);
    let path = project_root.join(".gitignore");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    if already_ignores_env(&existing, &rule) {
        return Ok(false);
    }
    let mut updated = existing.trim_end().to_string();
    if !updated.is_empty() {
        updated.push_str("\n\n");
    }
    updated.push_str(&format!(
        "# agentstack: local secrets — never commit\n{rule}\n"
    ));
    if write {
        atomic::write(&path, &updated).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(true)
}

/// The anchored `.gitignore` pattern for the `.env` inside `env_dir`, expressed
/// relative to `project_root`. Falls back to the repo-wide `.env` only if
/// `env_dir` somehow sits outside the root (it never should — the manifest dir
/// is always at or under it — but a bad rule is better than an unignored
/// secret).
pub fn gitignore_rule(project_root: &Path, env_dir: &Path) -> String {
    match env_dir.strip_prefix(project_root) {
        Ok(rel) if rel.as_os_str().is_empty() => "/.env".to_string(),
        // Forward slashes: a `.gitignore` pattern is not a platform path.
        Ok(rel) => format!(
            "/{}/.env",
            rel.components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/")
        ),
        Err(_) => ".env".to_string(),
    }
}

/// Whether some existing `.gitignore` line already ignores the file. Accepts our
/// own anchored rule plus the broader patterns a developer would plausibly have
/// written by hand — if any of those already covers it, we add nothing.
fn already_ignores_env(gitignore: &str, rule: &str) -> bool {
    gitignore.lines().any(|l| {
        let l = l.trim();
        l == rule || l == ".env" || l == "/.env" || l == "**/.env"
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
        let rule = "/.agentstack/.env";
        // Our own anchored rule, and the broader hand-written patterns that
        // already cover the file.
        assert!(already_ignores_env(
            "node_modules/\n/.agentstack/.env\n",
            rule
        ));
        assert!(already_ignores_env("node_modules/\n.env\n", rule));
        assert!(already_ignores_env("/.env", rule));
        assert!(already_ignores_env("**/.env", rule));
        assert!(!already_ignores_env("node_modules/\n.env.local\n", rule));
        assert!(!already_ignores_env("", rule));
    }

    /// The rule names the exact file we wrote instead of every `.env` in the
    /// repository — AgentStack must not change the ignore semantics of files it
    /// did not write.
    #[test]
    fn gitignore_rule_is_anchored_to_our_own_env() {
        let root = Path::new("/repo");
        assert_eq!(
            gitignore_rule(root, &root.join(".agentstack")),
            "/.agentstack/.env"
        );
        // Legacy root manifest: the `.env` sits at the repo root.
        assert_eq!(gitignore_rule(root, root), "/.env");
        // Nested manifest (a package inside a monorepo).
        assert_eq!(
            gitignore_rule(root, &root.join("apps/api/.agentstack")),
            "/apps/api/.agentstack/.env"
        );
        // Outside the root should never happen; fail safe rather than silently
        // writing a rule that ignores nothing.
        assert_eq!(gitignore_rule(root, Path::new("/elsewhere")), ".env");
    }

    /// Rule 5's filesystem half, at the layer that owns the `.env`: the writer
    /// must not leave real token values readable by other local accounts.
    #[cfg(unix)]
    #[test]
    fn written_env_is_not_readable_by_other_accounts() {
        let tmp = assert_fs::TempDir::new().unwrap();
        write(tmp.path(), &[("TOKEN".into(), "ghp_secret".into())]).unwrap();
        let path = tmp.path().join(".env");
        assert_eq!(
            atomic::is_group_or_world_readable(&path),
            Some(false),
            "the .env leaked to group/other"
        );
    }
}
