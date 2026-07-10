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
