//! Hostile-string handling shared by every ingestion boundary.
//!
//! Remote content (SKILL.md frontmatter, `pack.toml`, registry responses,
//! upstream MCP metadata) supplies strings that become manifest keys,
//! filesystem path components, and terminal/agent-context text. This module
//! is the one place their rules live — the way `sys` concentrates unsafe and
//! (soon) `gitx` concentrates git-spawn policy. Design:
//! `docs/design/hardening-remote-ingestion.md`.
//!
//! Holds the name contract (design §C) and the display/context sanitizers
//! (design §A). The sanitizers *neutralize* hostile bytes in passing display
//! surfaces; `scan.rs` stays the place that *reports* them visibly.

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
    // `escape_debug` renders control/escape bytes visibly — this error names
    // the offending bytes instead of letting them run in a terminal, and it
    // stays safe even on error paths that skip the §A display sanitizers.
    bail!(
        "invalid skill name '{}' — use lowercase letters, digits, '.', '_', '-'; \
         start with a letter or digit; max {NAME_MAX} chars",
        name.escape_debug()
    )
}

/// One-line metadata for display or agent context: strips terminal escape
/// sequences (CSI/OSC/DCS/PM/APC, bare ESC pairs), raw C0/C1 controls, and
/// invisible/bidi/tag characters; converts newlines to spaces, collapses
/// space runs, trims. Tabs survive.
pub fn sanitize_line(s: &str) -> String {
    let stripped = strip_hostile(s, false);
    // Collapse runs of spaces (newlines already became spaces); tabs pass.
    let mut out = String::with_capacity(stripped.len());
    let mut prev_space = false;
    for c in stripped.chars() {
        if c == ' ' {
            if !prev_space {
                out.push(' ');
            }
            prev_space = true;
        } else {
            prev_space = false;
            out.push(c);
        }
    }
    out.trim().to_string()
}

/// Multi-line variant for error/report text: same stripping, but `\n`
/// survives (`\r` is dropped).
pub fn sanitize_block(s: &str) -> String {
    strip_hostile(s, true)
}

/// Char-boundary-safe truncation with ellipsis. Replaces the duplicate
/// helpers in `search.rs`/`lib.rs` and the gateway's byte-level cap, which
/// panicked when the cap fell mid-UTF-8-char in hostile upstream metadata.
pub fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

/// First line of `s`, trimmed, capped at `max` characters.
pub fn one_line(s: &str, max: usize) -> String {
    truncate_chars(s.lines().next().unwrap_or("").trim(), max)
}

/// Recursively sanitize the human-read strings of a JSON schema in place:
/// every string value under a `description` or `title` key gets
/// `sanitize_line`. Keys and structural values stay verbatim — renaming a
/// property key would break the call contract with the upstream server
/// (design §A.2 #4).
pub fn sanitize_schema_docs(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Object(map) => {
            for (k, val) in map.iter_mut() {
                if (k == "description" || k == "title") && val.is_string() {
                    if let Some(s) = val.as_str() {
                        *val = serde_json::Value::String(sanitize_line(s));
                    }
                } else {
                    sanitize_schema_docs(val);
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                sanitize_schema_docs(item);
            }
        }
        _ => {}
    }
}

/// The stripping engine behind `sanitize_line`/`sanitize_block`: a small
/// state machine over `char`s (ECMA-48 sequence shapes ported from
/// vercel-labs/skills' sanitize.ts, extended with the invisible/bidi/tag
/// classes `scan.rs` names).
fn strip_hostile(s: &str, keep_newlines: bool) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.peek().copied() {
                // CSI: ESC `[` params(0x30-3F) intermediates(0x20-2F) final(0x40-7E).
                Some('[') => {
                    chars.next();
                    while chars
                        .peek()
                        .is_some_and(|p| ('\u{30}'..='\u{3f}').contains(p))
                    {
                        chars.next();
                    }
                    while chars
                        .peek()
                        .is_some_and(|p| ('\u{20}'..='\u{2f}').contains(p))
                    {
                        chars.next();
                    }
                    if chars
                        .peek()
                        .is_some_and(|p| ('\u{40}'..='\u{7e}').contains(p))
                    {
                        chars.next();
                    }
                }
                // OSC/DCS/PM/APC: swallow until BEL or ST (ESC `\`);
                // unterminated → swallow to end (never let a partial
                // sequence's payload through).
                Some(']' | 'P' | '^' | '_') => {
                    chars.next();
                    while let Some(t) = chars.next() {
                        if t == '\u{07}' {
                            break;
                        }
                        if t == '\u{1b}' {
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                // Bare ESC + one char (ESC 7, ESC c, lookalikes).
                Some(_) => {
                    chars.next();
                }
                None => {}
            },
            // Raw C1 controls: the 8-bit CSI/OSC/DCS/ST forms JSON never escapes.
            '\u{80}'..='\u{9f}' => {}
            '\n' => out.push(if keep_newlines { '\n' } else { ' ' }),
            '\r' => {
                if !keep_newlines {
                    out.push(' ');
                }
            }
            '\t' => out.push('\t'),
            c if c.is_control() => {}
            c if is_invisible(c) => {}
            c => out.push(c),
        }
    }
    out
}

/// The invisible/bidi/tag characters `scan.rs::invisible_label` detects —
/// same set, different job: scan reports them, display surfaces drop them.
fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{2060}' | '\u{FEFF}' | '\u{00AD}' | '\u{180E}'
    ) || matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{E0000}'..='\u{E007F}')
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

    #[test]
    fn sanitize_table() {
        // (input, sanitize_line output)
        let cases: &[(&str, &str)] = &[
            // OSC title spoof, BEL-terminated.
            ("safe\x1b]0;EVIL\x07name", "safename"),
            // OSC, ST-terminated.
            ("a\x1b]8;;http://x\x1b\\b", "ab"),
            // CSI clear-screen + cursor games.
            ("a\x1b[2J\x1b[1;1Hb", "ab"),
            // Raw C1 CSI (U+009B) — survives JSON escaping, must not survive us.
            ("a\u{9b}31mb", "a31mb"),
            // Bare ESC pair.
            ("a\x1bcb", "ab"),
            // Unterminated OSC swallows to end.
            ("a\x1b]0;never-terminated", "a"),
            // Bidi override and zero-width joiner dropped.
            ("a\u{202e}txt.exe\u{202c}b", "atxt.exeb"),
            ("a\u{200d}b", "ab"),
            // Newlines to single spaces, runs collapsed, trimmed.
            ("  foo\nbar\r\nbaz  ", "foo bar baz"),
            // Tabs survive.
            ("a\tb", "a\tb"),
            // Plain multibyte text passes untouched.
            ("héllo — 日本語", "héllo — 日本語"),
        ];
        for (input, want) in cases {
            assert_eq!(&sanitize_line(input), want, "input {input:?}");
        }
        // sanitize_block keeps newlines, still strips sequences.
        assert_eq!(sanitize_block("a\x1b[31m\nb\r"), "a\nb");
        // Multibyte char straddling the cap: the old gateway byte-truncate
        // panicked here; truncate_chars must not.
        assert_eq!(truncate_chars("ééé", 2), "éé…");
        assert_eq!(truncate_chars("abc", 3), "abc");
        assert_eq!(one_line("first\nsecond", 10), "first");
        // Schema walk: description/title values cleaned, keys/values intact.
        let mut schema = serde_json::json!({
            "type": "object",
            "description": "do\x1b[2J things",
            "properties": { "x": { "title": "a\u{202e}b", "enum": ["\x1b[2J"] } }
        });
        sanitize_schema_docs(&mut schema);
        assert_eq!(schema["description"], "do things");
        assert_eq!(schema["properties"]["x"]["title"], "ab");
        assert_eq!(schema["properties"]["x"]["enum"][0], "\x1b[2J");
    }

    proptest! {
        /// Security witness (design §A.3): sanitize_line output never
        /// contains an ESC byte, raw C1, other control chars (tab excepted),
        /// or an invisible/bidi/tag char — for ANY input. `any::<char>()`
        /// (not a printable-only regex strategy) so controls/ESC/C1 are
        /// actually generated.
        #[test]
        fn sanitized_output_has_no_hostile_chars(chars in proptest::collection::vec(proptest::char::any(), 0..64)) {
            let s: String = chars.into_iter().collect();
            let out = sanitize_line(&s);
            prop_assert!(out.chars().all(|c| {
                c == '\t' || (!c.is_control() && !is_invisible(c))
            }), "hostile char survived in {out:?}");
        }

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
