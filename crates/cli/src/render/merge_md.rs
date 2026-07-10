//! Managed-region merge for instruction files (CLAUDE.md / AGENTS.md).
//!
//! Prose files are heavily hand-edited and have no structured keys to "own", so
//! agentstack manages only a marked region and preserves everything outside it
//! verbatim (PLAN §9c — a deliberate exception to the marker-free rule, D4).

pub const START: &str = "<!-- agentstack:start -->";
pub const END: &str = "<!-- agentstack:end -->";

/// Replace agentstack's managed region in `existing` with `content` (or append
/// it if no region exists yet). Returns the new file text. An empty `content`
/// removes the region entirely.
pub fn merge_region(existing: &str, content: &str) -> String {
    let content = content.trim_end_matches('\n');

    // Locate an existing region (START … END).
    if let Some(s) = existing.find(START) {
        if let Some(e_rel) = existing[s..].find(END) {
            let e_end = s + e_rel + END.len();
            let before = &existing[..s];
            let after = &existing[e_end..];
            if content.is_empty() {
                // Remove the region and collapse surrounding blank lines.
                let joined = format!("{}{}", before.trim_end_matches('\n'), after);
                return normalize_trailing(&joined);
            }
            let block = format!("{START}\n{content}\n{END}");
            return normalize_trailing(&format!("{before}{block}{after}"));
        }
    }

    if content.is_empty() {
        return existing.to_string();
    }
    let block = format!("{START}\n{content}\n{END}");
    if existing.trim().is_empty() {
        format!("{block}\n")
    } else {
        format!("{}\n\n{block}\n", existing.trim_end_matches('\n'))
    }
}

fn normalize_trailing(s: &str) -> String {
    format!("{}\n", s.trim_end_matches('\n'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_region_to_existing_prose() {
        let out = merge_region("# My notes\n\nHand-written.\n", "Shared rules.");
        assert!(out.contains("# My notes"));
        assert!(out.contains("Hand-written."));
        assert!(out.contains(START));
        assert!(out.contains("Shared rules."));
        assert!(out.contains(END));
    }

    #[test]
    fn replaces_region_preserving_outside() {
        let existing = format!("top\n\n{START}\nold\n{END}\n\nbottom\n");
        let out = merge_region(&existing, "new content");
        assert!(out.contains("top"));
        assert!(out.contains("bottom"));
        assert!(out.contains("new content"));
        assert!(!out.contains("old"));
        // Exactly one region.
        assert_eq!(out.matches(START).count(), 1);
    }

    #[test]
    fn empty_content_removes_region() {
        let existing = format!("keep\n\n{START}\ngone\n{END}\n");
        let out = merge_region(&existing, "");
        assert!(out.contains("keep"));
        assert!(!out.contains(START));
        assert!(!out.contains("gone"));
    }

    #[test]
    fn empty_file_just_gets_region() {
        let out = merge_region("", "hello");
        assert!(out.starts_with(START));
        assert!(out.contains("hello"));
    }
}
