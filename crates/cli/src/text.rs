//! Hostile-string handling shared by every ingestion boundary.
//!
//! Remote content (SKILL.md frontmatter, `pack.toml`, registry responses,
//! upstream MCP metadata) supplies strings that become manifest keys,
//! filesystem path components, and terminal/agent-context text. This module
//! is the one place their rules live — the way `sys` concentrates unsafe and
//! (soon) `gitx` concentrates git-spawn policy. Design:
//! `docs/design/hardening-remote-ingestion.md`.
//!
//! This file currently holds the name contract (design §C); the display
//! sanitizers (§A) land here next.

use anyhow::{bail, Result};

/// Maximum accepted skill/pack name length, in bytes (== chars: ASCII only).
pub const NAME_MAX: usize = 64;

/// The skill-name contract: `^[a-z0-9][a-z0-9._-]{0,63}$`.
///
/// Fail, never normalize — a hostile name must be rejected with the reason,
/// not silently rewritten into a different valid name. Lowercase-only is a
/// security property, not style: manifest/lock/library lookups are
/// case-sensitive `String` equality, but macOS's default filesystem is
/// case-insensitive, so `PDF` and `pdf` would be distinct index entries
/// sharing one body directory on disk. The grammar makes that collision
/// unrepresentable. Starting alphanumeric rules out dotfiles, `-`-prefixed
/// flag lookalikes, and `.`/`..`; banning separators means every accepted
/// name is exactly one path component (the proptest below is the witness).
pub fn validate_name(name: &str) -> Result<()> {
    let starts_ok = matches!(name.as_bytes().first(), Some(b'a'..=b'z' | b'0'..=b'9'));
    let chars_ok = name
        .bytes()
        .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-'));
    if starts_ok && chars_ok && name.len() <= NAME_MAX {
        return Ok(());
    }
    // `escape_debug` renders control/escape bytes visibly — this error may
    // carry a hostile name into a terminal before the §A sanitizers exist.
    bail!(
        "invalid skill name '{}' — use lowercase letters, digits, '.', '_', '-'; \
         start with a letter or digit; max {NAME_MAX} chars",
        name.escape_debug()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::path::{Component, Path};

    #[test]
    fn name_contract_table() {
        for ok in [
            "sql-review",
            "v1.2_beta",
            "a",
            "0day",
            "x".repeat(64).as_str(),
        ] {
            assert!(validate_name(ok).is_ok(), "should accept {ok:?}");
        }
        for bad in [
            "",
            "../x",
            ".hidden",
            "-flag",
            "PDF",
            "a b",
            "a/b",
            "a\\b",
            "日本語",
            "x".repeat(65).as_str(),
            "a\u{202e}b",
            "a\x1b[2Jb",
        ] {
            assert!(validate_name(bad).is_err(), "should reject {bad:?}");
        }
    }

    proptest! {
        /// Security witness (design §C.4): any accepted name is exactly one
        /// normal path component, and joining it never escapes the base dir.
        #[test]
        fn accepted_names_are_single_safe_components(name in "[a-z0-9][a-z0-9._-]{0,63}") {
            prop_assume!(validate_name(&name).is_ok());
            let p = Path::new(&name);
            let comps: Vec<_> = p.components().collect();
            prop_assert_eq!(comps.len(), 1);
            prop_assert!(matches!(comps[0], Component::Normal(_)));
            let base = Path::new("/base/dir");
            prop_assert!(base.join(&name).starts_with(base));
        }

        /// Arbitrary (including hostile) input never panics, and anything
        /// containing a separator, uppercase, or non-ASCII is rejected.
        #[test]
        fn hostile_input_rejected(name in "\\PC*") {
            let verdict = validate_name(&name);
            let suspicious = name.contains('/')
                || name.contains('\\')
                || name.chars().any(|c| !c.is_ascii() || c.is_ascii_uppercase() || c.is_control());
            if suspicious {
                prop_assert!(verdict.is_err());
            }
        }
    }
}
