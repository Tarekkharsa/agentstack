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

/// Render a colored unified-ish diff. Returns an empty string when equal.
pub fn render(before: &str, after: &str) -> String {
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
        out.push_str(&format!("{}\n", format!("- {line}").red()));
    }
    for line in &after[start..end_a] {
        out.push_str(&format!("{}\n", format!("+ {line}").green()));
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
}
