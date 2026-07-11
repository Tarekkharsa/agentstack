//! `${REF}` placeholder *syntax* — pure parsing, no resolution.
//!
//! Resolution (keychain, varlock, env) lives in the `cli` crate; the manifest
//! model only needs to know which names a string references, so the scanner
//! lives here with the model.

/// Whether `s` is a valid reference name: `[A-Za-z_][A-Za-z0-9_]*`. Anything
/// else between `${` and `}` — e.g. shell fallback syntax like
/// `${VAR:-$OTHER}` inside a `zsh -lc` argument — is the shell's business,
/// not a secret reference.
pub fn is_ref_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Extract the `${NAME}` reference names from a string, in order of appearance.
/// `${…}` spans that are not valid names are skipped (their interior is still
/// scanned, so `${A:-${B}}` yields `B`).
pub fn refs_in(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            if let Some(end) = s[i + 2..].find('}') {
                let name = &s[i + 2..i + 2 + end];
                if is_ref_name(name) {
                    out.push(name.to_string());
                    i = i + 2 + end + 1;
                    continue;
                }
            }
            // Not a reference — step past `${` and keep scanning the interior.
            i += 2;
            continue;
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ref_name_rules() {
        assert!(is_ref_name("GH_PAT"));
        assert!(is_ref_name("_x"));
        assert!(is_ref_name("A1_b2"));
        assert!(!is_ref_name("")); // empty
        assert!(!is_ref_name("1ABC")); // leading digit
        assert!(!is_ref_name("A-B")); // hyphen
        assert!(!is_ref_name("A B")); // space
        assert!(!is_ref_name("VAR:-x")); // shell fallback syntax
    }

    #[test]
    fn extracts_refs_in_order() {
        assert_eq!(refs_in("${A} and ${B}"), vec!["A", "B"]);
        assert_eq!(refs_in("Bearer ${TOKEN}"), vec!["TOKEN"]);
        assert!(refs_in("no refs here").is_empty());
    }

    #[test]
    fn skips_non_names_but_scans_their_interior() {
        // A shell fallback `${A:-${B}}` is not itself a ref name, but the
        // nested `${B}` inside it is still found.
        assert_eq!(refs_in("${A:-${B}}"), vec!["B"]);
        // A bare `${...}` with junk yields nothing.
        assert!(refs_in("${1bad}").is_empty());
        assert!(refs_in("${a-b}").is_empty());
    }

    #[test]
    fn hostile_input_never_panics() {
        // Unterminated, empty, multibyte, and adversarial brace soup must all
        // return cleanly (the scanner walks bytes, so UTF-8 boundaries matter).
        for s in [
            "",
            "$",
            "${",
            "${}",
            "${unterminated",
            "${${${",
            "€${MÜNZE}€", // multibyte around and inside
            "}{}{}{",
            "${A}${B}${C}",
        ] {
            let _ = refs_in(s); // must not panic
        }
        // Multibyte inside a ${…} span is not a valid name → skipped, no panic.
        assert!(refs_in("${MÜNZE}").is_empty());
        // But a valid name adjacent to multibyte is still found.
        assert_eq!(refs_in("€${TOKEN}"), vec!["TOKEN"]);
    }
}
