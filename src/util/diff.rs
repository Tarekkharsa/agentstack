//! A small, dependency-free line diff for human-readable `apply`/`diff` output.
//!
//! Not a full Myers diff — it trims the common prefix and suffix and shows the
//! changed block in between. That is exact for the common case (a few keys
//! upserted into an otherwise-identical file) and never misleading: identical
//! files always produce an empty diff.

use owo_colors::OwoColorize;

/// Whether two texts differ at all (ignoring a trailing newline mismatch).
pub fn differs(before: &str, after: &str) -> bool {
    before.trim_end_matches('\n') != after.trim_end_matches('\n')
}

/// Redact resolved secret values in already-rendered, human-facing diff text so
/// a preview never prints a real credential. Each `(ref-name, resolved-value)`
/// pair replaces every occurrence of the value with its `${REF}` name — on
/// context, `+`, and `-` lines alike, so an unchanged secret line is masked too.
///
/// Masking runs on the rendered diff (not on the inputs) so a rotated secret
/// still shows as a changed line, just with both sides redacted. The values are
/// the ones the renderer actually substituted in, so it can't miss a token by
/// guessing what "looks like" a secret. Longer values are masked first, so a
/// short secret that is a substring of a longer one can't partially unmask it;
/// empty values are skipped (nothing to leak, and would match everywhere).
///
/// This is display-only: it never touches the bytes an apply writes to disk.
pub fn mask_secrets(text: &str, secrets: &[(String, String)]) -> String {
    let mut pairs: Vec<&(String, String)> =
        secrets.iter().filter(|(_, v)| !v.is_empty()).collect();
    // Longest value first; ties don't matter for correctness.
    pairs.sort_by_key(|p| std::cmp::Reverse(p.1.len()));
    let mut out = text.to_string();
    for (name, value) in pairs {
        if out.contains(value.as_str()) {
            out = out.replace(value.as_str(), &format!("${{{name}}}"));
        }
    }
    out
}

/// Render a colored unified-ish diff (for the terminal). Empty when equal.
pub fn render(before: &str, after: &str) -> String {
    build(before, after, true)
}

/// Render a plain (uncolored) diff with `+`/`-`/` ` line prefixes — for the
/// web dashboard, which colorizes by prefix in CSS.
pub fn render_plain(before: &str, after: &str) -> String {
    build(before, after, false)
}

fn build(before: &str, after: &str, color: bool) -> String {
    if !differs(before, after) {
        return String::new();
    }
    let before: Vec<&str> = before.lines().collect();
    let after: Vec<&str> = after.lines().collect();

    // Common prefix.
    let mut start = 0;
    while start < before.len() && start < after.len() && before[start] == after[start] {
        start += 1;
    }
    // Common suffix (not overlapping the prefix).
    let mut end_b = before.len();
    let mut end_a = after.len();
    while end_b > start && end_a > start && before[end_b - 1] == after[end_a - 1] {
        end_b -= 1;
        end_a -= 1;
    }

    let mut out = String::new();
    let context = 2usize;
    let ctx_start = start.saturating_sub(context);
    for line in &before[ctx_start..start] {
        out.push_str(&format!("  {line}\n"));
    }
    for line in &before[start..end_b] {
        let l = format!("- {line}");
        out.push_str(&format!(
            "{}\n",
            if color { l.red().to_string() } else { l }
        ));
    }
    for line in &after[start..end_a] {
        let l = format!("+ {line}");
        out.push_str(&format!(
            "{}\n",
            if color { l.green().to_string() } else { l }
        ));
    }
    let ctx_end = (end_b + context).min(before.len());
    for line in &before[end_b..ctx_end] {
        out.push_str(&format!("  {line}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_texts_produce_no_diff() {
        assert!(!differs("a\nb\n", "a\nb"));
        assert_eq!(render("a\nb", "a\nb"), "");
    }

    #[test]
    fn shows_changed_block() {
        let d = render("a\nb\nc", "a\nB\nc");
        assert!(d.contains("- b"));
        assert!(d.contains("+ B"));
    }

    #[test]
    fn mask_replaces_secret_value_with_ref_name() {
        let secrets = vec![("GUMLET_TOKEN".to_string(), "gumlet_5f3bc35892ab".to_string())];
        let d = render_plain("x\n", "x\nAuthorization: Bearer gumlet_5f3bc35892ab\n");
        let masked = mask_secrets(&d, &secrets);
        assert!(!masked.contains("gumlet_5f3bc35892ab"), "secret leaked: {masked}");
        assert!(masked.contains("Bearer ${GUMLET_TOKEN}"), "{masked}");
    }

    #[test]
    fn mask_handles_context_and_removed_lines_and_odd_values() {
        // Same secret on a context line, a removed line, and an added line.
        let secrets = vec![("TOK".to_string(), "s3cr3t".to_string())];
        let before = "keep s3cr3t\nold s3cr3t\n";
        let after = "keep s3cr3t\nnew s3cr3t\n";
        let masked = mask_secrets(&render_plain(before, after), &secrets);
        assert!(!masked.contains("s3cr3t"), "secret leaked: {masked}");

        // Empty and overlapping values don't crash or over-mask.
        let secrets = vec![
            ("EMPTY".to_string(), String::new()),
            ("LONG".to_string(), "abcdef".to_string()),
            ("SHORT".to_string(), "abc".to_string()),
        ];
        let masked = mask_secrets("value abcdef here", &secrets);
        assert_eq!(masked, "value ${LONG} here", "longest match wins: {masked}");
    }
}
