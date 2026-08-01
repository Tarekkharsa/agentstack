//! Recognition — a machine-local memory of what has already been approved.
//!
//! Reviewing the same skill for the fifth project on one machine is the same
//! reading five times. Recognition makes the fifth reading *shorter*: the card
//! can say "this exact content is approved in 2 other projects on this machine"
//! instead of re-listing what the human has already read.
//!
//! What it must never do is the entire point:
//!
//! - **It never shortens the gate.** The per-project yes still happens, in
//!   full. Path-keyed trust exists to bind consent to a context, and
//!   recognition preserves that binding deliberately — a machine-level "always
//!   allow this content anywhere" is a *widening* of the gate, not a
//!   presentation of it, and is explicitly not built here.
//! - **It changes lines only** — never the outcome, never the recorded events.
//! - **It stores digests and project keys, never content.** The content store
//!   holds bytes; duplicating them here would create a second, unverified copy
//!   of approved content with none of the CAS's write-once discipline.
//! - **It never crosses machines.** It lives under the machine's own
//!   `~/.agentstack`, is never rendered into a project, and is never synced.
//!   That is structural, not a policy promise.
//!
//! Failure posture, per `docs/design/consent-card.md`: every degradation is to
//! the *full* card. A missing, unreadable, or corrupt index means no
//! recognition lines — never a blocked review, and never a shortened one on a
//! guess.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// `content digest -> the project keys that have approved it`.
///
/// Deliberately the smallest thing that answers "how many other projects here
/// already said yes to exactly this": no timestamps, no names, no bytes.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Index {
    #[serde(default)]
    pub approved: BTreeMap<String, Vec<String>>,
}

fn path() -> std::path::PathBuf {
    crate::util::paths::agentstack_home().join("recognition.json")
}

impl Index {
    /// Read the index, or an empty one. A corrupt file reads as empty — the
    /// card then shows its full body, which is the safe direction.
    pub fn load() -> Index {
        let Ok(text) = std::fs::read_to_string(path()) else {
            return Index::default();
        };
        serde_json::from_str(&text).unwrap_or_default()
    }

    /// The index only if there IS one — absent, unreadable, or corrupt reads
    /// as `None`.
    ///
    /// [`Index::load`] flattens all three into an empty index, which is right
    /// for the terminal card (no index, no line) and wrong for a machine
    /// payload: a reader cannot tell "approved nowhere else" from "this
    /// machine has nothing to ask", and only one of those is a fact. Callers
    /// that must report the difference use this and emit `null`.
    pub fn load_existing() -> Option<Index> {
        let text = std::fs::read_to_string(path()).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// How many projects OTHER than `project_key` have approved `digest`.
    pub fn others(&self, digest: &str, project_key: &str) -> usize {
        self.approved
            .get(digest)
            .map(|keys| keys.iter().filter(|k| *k != project_key).count())
            .unwrap_or(0)
    }
}

/// Record that `project_key` approved these content digests.
///
/// Best-effort in every branch: recognition is a convenience, and a convenience
/// must never be able to fail a grant. Called after the grant has been written,
/// so a failure here cannot leave a project half-trusted.
pub fn record(project_key: &str, digests: impl IntoIterator<Item = String>) {
    let mut index = Index::load();
    let mut changed = false;
    for digest in digests {
        if digest.is_empty() {
            continue;
        }
        let keys = index.approved.entry(digest).or_default();
        if !keys.iter().any(|k| k == project_key) {
            keys.push(project_key.to_string());
            keys.sort();
            changed = true;
        }
    }
    if !changed {
        return;
    }
    if let Ok(text) = serde_json::to_string_pretty(&index) {
        let p = path();
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = crate::util::atomic::write(&p, &text);
    }
}

/// Drop `project_key` from every digest it appears under — called when trust is
/// revoked, so recognition cannot outlive the consent it was derived from.
pub fn forget(project_key: &str) {
    let mut index = Index::load();
    let mut changed = false;
    index.approved.retain(|_, keys| {
        let before = keys.len();
        keys.retain(|k| k != project_key);
        changed |= keys.len() != before;
        !keys.is_empty()
    });
    if !changed {
        return;
    }
    if let Ok(text) = serde_json::to_string_pretty(&index) {
        let _ = crate::util::atomic::write(&path(), &text);
    }
}

/// The one line recognition contributes to a card, if any.
///
/// `None` when nothing is recognized — the caller then renders its full body,
/// unchanged. Pure, so a test can assert on exactly what the human sees.
pub fn line(recognized: usize, total: usize, other_projects: usize) -> Option<String> {
    if recognized == 0 || other_projects == 0 {
        return None;
    }
    // "1 of the 1 item" is how a machine counts, not how a person reads.
    let what = if recognized == total {
        match total {
            1 => "this content is".to_string(),
            n => format!("all {n} items are"),
        }
    } else {
        format!("{recognized} of these {total} items are")
    };
    Some(format!(
        "  {what} already approved in {} on this machine",
        crate::commands::count(other_projects, "other project"),
    ))
}

/// How many distinct OTHER projects corroborate any of `items`.
///
/// Deliberately a count of projects, not of items: "approved in 2 other
/// projects" is the fact a reviewer can act on, and reusing the item count
/// there — as a first version of this did — states a number that happens to
/// look plausible and means nothing.
pub fn other_projects(
    index: &Index,
    project_key: &str,
    items: &[agentstack_trust::SurfaceItem],
) -> usize {
    let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for item in items {
        if let Some(pin) = item.pin.as_deref() {
            if let Some(keys) = index.approved.get(pin) {
                seen.extend(
                    keys.iter()
                        .map(String::as_str)
                        .filter(|k| *k != project_key),
                );
            }
        }
    }
    seen.len()
}

/// Count how many of `items` are recognized from other projects on this
/// machine. Read-only; `None`-pinned items can never be recognized, because
/// recognition is keyed by the content digest and they have none.
pub fn recognized_count(
    index: &Index,
    project_key: &str,
    items: &[agentstack_trust::SurfaceItem],
) -> usize {
    items
        .iter()
        .filter(|i| {
            i.pin
                .as_deref()
                .is_some_and(|p| index.others(p, project_key) > 0)
        })
        .count()
}

/// Every content digest in a reviewed surface — what a grant records.
pub fn digests_of(items: &[agentstack_trust::SurfaceItem]) -> Vec<String> {
    items.iter().filter_map(|i| i.pin.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str, pin: Option<&str>) -> agentstack_trust::SurfaceItem {
        agentstack_trust::SurfaceItem {
            kind: "skill".into(),
            name: name.into(),
            identity: "inline".into(),
            pin: pin.map(str::to_string),
        }
    }

    // The index answers "how many OTHER projects", never "how many projects" —
    // a project recognizing itself would tell the user their own yes is
    // corroboration for itself.
    #[test]
    fn a_project_never_recognizes_itself() {
        let mut index = Index::default();
        index
            .approved
            .insert("sha256:aaa".into(), vec!["/a".into()]);
        assert_eq!(index.others("sha256:aaa", "/a"), 0);
        assert_eq!(index.others("sha256:aaa", "/b"), 1);
    }

    // An item with no recorded pin cannot be recognized: recognition is keyed
    // by content, and there is no content digest to key on.
    #[test]
    fn unpinned_items_are_never_recognized() {
        let mut index = Index::default();
        index
            .approved
            .insert("sha256:aaa".into(), vec!["/a".into(), "/b".into()]);
        let items = vec![item("has-pin", Some("sha256:aaa")), item("no-pin", None)];
        assert_eq!(recognized_count(&index, "/c", &items), 1);
    }

    // No recognition means no line at all — the caller renders its full body.
    #[test]
    fn nothing_recognized_contributes_no_line() {
        assert!(line(0, 5, 3).is_none(), "nothing recognized, no line");
        assert!(line(2, 5, 0).is_none(), "no corroborating project, no line");
    }

    // The project count and the item count are different numbers, and saying
    // one where the other belongs states something plausible and untrue.
    #[test]
    fn the_line_counts_projects_and_items_separately() {
        let l = line(2, 5, 3).unwrap();
        assert!(l.contains("2 of these 5 items"), "{l}");
        assert!(l.contains("3 other projects"), "{l}");
    }

    // "1 of the 1 item" is how a machine counts, not how a person reads.
    #[test]
    fn the_line_reads_naturally_at_the_edges() {
        assert!(line(1, 1, 1)
            .unwrap()
            .contains("this content is already approved in 1 other project"));
        assert!(line(4, 4, 2).unwrap().contains("all 4 items are"));
    }

    #[test]
    fn other_projects_counts_distinct_projects_not_items() {
        let mut index = Index::default();
        // Two different items, both approved by the SAME other project.
        index
            .approved
            .insert("sha256:aaa".into(), vec!["/a".into()]);
        index
            .approved
            .insert("sha256:bbb".into(), vec!["/a".into()]);
        let items = vec![
            item("one", Some("sha256:aaa")),
            item("two", Some("sha256:bbb")),
        ];
        assert_eq!(
            other_projects(&index, "/c", &items),
            1,
            "one project, not two"
        );
    }

    // The index holds digests and project keys. If it ever grows a content
    // field, this fails — the CAS stores content, and a second unverified copy
    // of approved bytes is exactly what this must not become.
    #[test]
    fn the_index_serializes_digests_and_keys_and_nothing_else() {
        let mut index = Index::default();
        index
            .approved
            .insert("sha256:aaa".into(), vec!["/a".into(), "/b".into()]);
        let json = serde_json::to_string(&index).unwrap();
        assert_eq!(json, r#"{"approved":{"sha256:aaa":["/a","/b"]}}"#);
    }
}
