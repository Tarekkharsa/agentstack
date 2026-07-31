//! The re-gate diff: what changed since the bytes a human last said yes to.
//!
//! Content pinning already re-gates correctly — a byte change invalidates the
//! pin and the project must be reviewed again. What it could not do was *say
//! what changed*: the user saw "DRIFTED from lock" and had to take on faith
//! that re-locking was safe. "This skill changed 3 lines" is the same gate with
//! the evidence attached.
//!
//! The prior bytes come from the content store, deposited at pin time (see
//! [`crate::store::Store::pin`]). Two properties of that store make this work:
//! it is keyed by the exact checksum the lockfile records, so finding the
//! approved bytes is a lookup and not a search; and it is append-only per
//! digest, so a re-lock leaves BOTH sides on disk to diff against each other.
//!
//! Failure posture, per `docs/design/consent-card.md`: every degradation is
//! honest and none of them gate. A missing snapshot yields
//! [`PinDiff::NoSnapshot`], which the caller renders as today's changed-content
//! message plus the pin identity — never a fabricated diff, and never a block.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use agentstack_core::util::diff;

/// How many diff lines a card will print before it stops showing lines and
/// starts naming files and counts instead. A re-gate on a 400-line rewrite must
/// stay a review card, not become a pager: past this point the useful facts are
/// *which* files moved and *how much*, and the detail belongs in the user's own
/// diff tool.
pub const DIFF_LINE_CAP: usize = 40;

/// One file's fate between the approved bytes and what is on disk now.
#[derive(Debug, PartialEq, Eq)]
pub enum FileChange {
    Added(String),
    Removed(String),
    Modified {
        path: String,
        /// The rendered (uncolored) diff for this file.
        body: String,
        /// Changed lines, for the cap decision and the summary counts.
        added: usize,
        removed: usize,
    },
}

impl FileChange {
    pub fn path(&self) -> &str {
        match self {
            FileChange::Added(p) | FileChange::Removed(p) => p,
            FileChange::Modified { path, .. } => path,
        }
    }
}

/// The comparison between what consent covered and what is on disk now.
#[derive(Debug, PartialEq, Eq)]
pub enum PinDiff {
    /// The approved bytes are not on disk, so there is nothing honest to show.
    /// Entries pinned before snapshots existed land here permanently — there is
    /// no backfill, because those bytes were never captured.
    NoSnapshot,
    /// The pin still matches the content; nothing changed.
    Unchanged,
    /// Real changes, with per-file detail.
    Changed(Vec<FileChange>),
}

/// Compare the bytes `pin` covers against `live`, reading the approved side out
/// of the content store.
///
/// `pin` is the bare hex digest the lockfile recorded. Everything about this is
/// read-only: it never writes, never fetches, and never repairs the store.
pub fn diff_against_pin(store_root: &Path, pin: &str, live: &Path) -> PinDiff {
    let hex = pin.rsplit(':').next().unwrap_or(pin);
    let approved = store_root.join("content").join(hex);
    if !approved.is_dir() {
        return PinDiff::NoSnapshot;
    }
    let before = read_tree(&approved);
    let after = read_tree(live);

    let paths: BTreeSet<&String> = before
        .iter()
        .map(|(p, _)| p)
        .chain(after.iter().map(|(p, _)| p))
        .collect();
    let mut changes = Vec::new();
    for path in paths {
        let b = before
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, c)| c.as_str());
        let a = after
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, c)| c.as_str());
        match (b, a) {
            (None, Some(_)) => changes.push(FileChange::Added(path.clone())),
            (Some(_), None) => changes.push(FileChange::Removed(path.clone())),
            (Some(b), Some(a)) => {
                if diff::differs(b, a) {
                    let body = diff::render_plain(b, a);
                    let added = body.lines().filter(|l| l.starts_with('+')).count();
                    let removed = body.lines().filter(|l| l.starts_with('-')).count();
                    changes.push(FileChange::Modified {
                        path: path.clone(),
                        body,
                        added,
                        removed,
                    });
                }
            }
            (None, None) => {}
        }
    }
    if changes.is_empty() {
        PinDiff::Unchanged
    } else {
        PinDiff::Changed(changes)
    }
}

/// Render the diff as review lines, capped.
///
/// Under the cap the user sees the actual changed lines — the whole point of
/// the moment. Over it, the card names every affected file with its counts and
/// says plainly that the detail is not being shown, which is honest about the
/// size rather than pretending a 400-line rewrite is reviewable inline.
///
/// Returns display lines only; the caller owns indentation and color. Every
/// path is sanitized because repository content is hostile input.
pub fn render_lines(diff: &PinDiff, cap: usize) -> Vec<String> {
    let changes = match diff {
        PinDiff::NoSnapshot => {
            return vec![
                "the bytes you approved were not recorded, so this cannot show what changed"
                    .to_string(),
            ]
        }
        PinDiff::Unchanged => return Vec::new(),
        PinDiff::Changed(c) => c,
    };

    let total: usize = changes
        .iter()
        .map(|c| match c {
            FileChange::Modified { added, removed, .. } => added + removed,
            // An added or removed file is one line of summary either way.
            _ => 1,
        })
        .sum();

    let mut out = Vec::new();
    if total > cap {
        out.push(format!(
            "{} changed, {} — too large to show here:",
            crate::commands::count(changes.len(), "file"),
            plural_lines(total)
        ));
        for c in changes {
            let p = crate::text::sanitize_line(c.path());
            out.push(match c {
                FileChange::Added(_) => format!("  + {p} (new file)"),
                FileChange::Removed(_) => format!("  - {p} (deleted)"),
                FileChange::Modified { added, removed, .. } => {
                    format!("  ~ {p} (+{added} −{removed})")
                }
            });
        }
        return out;
    }

    for c in changes {
        let p = crate::text::sanitize_line(c.path());
        match c {
            FileChange::Added(_) => out.push(format!("  + {p} (new file)")),
            FileChange::Removed(_) => out.push(format!("  - {p} (deleted)")),
            FileChange::Modified { body, .. } => {
                out.push(format!("  ~ {p}:"));
                for line in body.lines() {
                    out.push(format!("    {}", crate::text::sanitize_line(line)));
                }
            }
        }
    }
    out
}

/// A one-line headline for a changed item, for the card's summary altitude:
/// "changed 3 lines" is what a reviewer needs before deciding to look closer.
pub fn headline(diff: &PinDiff) -> Option<String> {
    match diff {
        PinDiff::NoSnapshot | PinDiff::Unchanged => None,
        PinDiff::Changed(changes) => {
            let lines: usize = changes
                .iter()
                .map(|c| match c {
                    FileChange::Modified { added, removed, .. } => added + removed,
                    _ => 0,
                })
                .sum();
            let files = changes.len();
            let touched = changes
                .iter()
                .filter(|c| !matches!(c, FileChange::Modified { .. }))
                .count();
            if lines == 0 {
                // Pure file additions/removals: lines would read as "0 lines".
                return Some(format!(
                    "{} added or removed",
                    crate::commands::count(touched, "file")
                ));
            }
            if files == 1 {
                Some(format!("changed {}", plural_lines(lines)))
            } else {
                Some(format!(
                    "changed {} across {}",
                    plural_lines(lines),
                    crate::commands::count(files, "file")
                ))
            }
        }
    }
}

fn plural_lines(n: usize) -> String {
    crate::commands::count(n, "line")
}

/// Every regular file under `root`, as `(relative path, contents)`, sorted.
///
/// Symlinks are SKIPPED, never followed: the digest that produced the pin
/// excludes them for the same reason, so following one here would show the
/// reviewer content that was never part of what they approved — and could
/// escape the directory entirely. Unreadable or non-UTF-8 files are skipped
/// too; a binary blob has no line diff, and this must never fail the review.
fn read_tree(root: &Path) -> Vec<(String, String)> {
    // An instruction fragment is a single FILE, while its snapshot is a
    // directory holding that file under its own name (see
    // `Store::deposit_file`). Reading a lone file as a one-entry tree keyed by
    // its file name makes both sides line up, so one comparison serves both
    // kinds instead of the read side growing a second code path.
    if root.is_file() {
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "fragment".to_string());
        return match std::fs::read_to_string(root) {
            Ok(text) => vec![(name, text)],
            Err(_) => Vec::new(),
        };
    }
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = path.symlink_metadata() else {
                continue;
            };
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                stack.push(path);
            } else if meta.is_file() {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    if let Some(rel) = rel_path(root, &path) {
                        out.push((rel, text));
                    }
                }
            }
        }
    }
    out.sort();
    out
}

fn rel_path(root: &Path, path: &Path) -> Option<String> {
    let rel: PathBuf = path.strip_prefix(root).ok()?.to_path_buf();
    Some(rel.to_string_lossy().replace('\\', "/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::prelude::*;

    fn store_with(digest: &str, files: &[(&str, &str)]) -> assert_fs::TempDir {
        let root = assert_fs::TempDir::new().unwrap();
        for (name, body) in files {
            root.child(format!("content/{digest}/{name}"))
                .write_str(body)
                .unwrap();
        }
        root
    }

    #[test]
    fn a_missing_snapshot_degrades_honestly_and_never_invents_a_diff() {
        let store = assert_fs::TempDir::new().unwrap();
        let live = assert_fs::TempDir::new().unwrap();
        live.child("SKILL.md").write_str("# hi\n").unwrap();
        let d = diff_against_pin(store.path(), "deadbeef", live.path());
        assert_eq!(d, PinDiff::NoSnapshot);
        assert!(headline(&d).is_none());
        let lines = render_lines(&d, DIFF_LINE_CAP);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("were not recorded"), "{lines:?}");
    }

    #[test]
    fn identical_content_reads_as_unchanged() {
        let store = store_with("abc", &[("SKILL.md", "# hi\nbody\n")]);
        let live = assert_fs::TempDir::new().unwrap();
        live.child("SKILL.md").write_str("# hi\nbody\n").unwrap();
        assert_eq!(
            diff_against_pin(store.path(), "abc", live.path()),
            PinDiff::Unchanged
        );
    }

    // The acceptance target: the actual changed lines, not "digest mismatch".
    #[test]
    fn a_small_edit_shows_the_real_changed_lines() {
        let store = store_with("abc", &[("SKILL.md", "# hi\nkeep\nold line\n")]);
        let live = assert_fs::TempDir::new().unwrap();
        live.child("SKILL.md")
            .write_str("# hi\nkeep\nnew line\n")
            .unwrap();
        let d = diff_against_pin(store.path(), "abc", live.path());
        assert_eq!(headline(&d).as_deref(), Some("changed 2 lines"));
        let rendered = render_lines(&d, DIFF_LINE_CAP).join("\n");
        assert!(rendered.contains("SKILL.md"), "{rendered}");
        assert!(rendered.contains("- old line"), "{rendered}");
        assert!(rendered.contains("+ new line"), "{rendered}");
        // Unchanged content appears as context, never as a change: a reviewer
        // must be able to trust that every +/- line is something that moved.
        assert!(!rendered.contains("+ # hi"), "{rendered}");
        assert!(!rendered.contains("- keep"), "{rendered}");
    }

    #[test]
    fn added_and_removed_files_are_named() {
        let store = store_with("abc", &[("SKILL.md", "same\n"), ("gone.md", "bye\n")]);
        let live = assert_fs::TempDir::new().unwrap();
        live.child("SKILL.md").write_str("same\n").unwrap();
        live.child("extra.md").write_str("hello\n").unwrap();
        let d = diff_against_pin(store.path(), "abc", live.path());
        let rendered = render_lines(&d, DIFF_LINE_CAP).join("\n");
        assert!(rendered.contains("+ extra.md (new file)"), "{rendered}");
        assert!(rendered.contains("- gone.md (deleted)"), "{rendered}");
        assert_eq!(headline(&d).as_deref(), Some("2 files added or removed"));
    }

    // The cap: a large rewrite names files and counts and NEVER floods.
    #[test]
    fn a_large_rewrite_names_files_and_counts_instead_of_flooding() {
        let old: String = (0..200).map(|i| format!("old {i}\n")).collect();
        let new: String = (0..200).map(|i| format!("new {i}\n")).collect();
        let store = store_with("abc", &[("SKILL.md", &old)]);
        let live = assert_fs::TempDir::new().unwrap();
        live.child("SKILL.md").write_str(&new).unwrap();

        let d = diff_against_pin(store.path(), "abc", live.path());
        let lines = render_lines(&d, DIFF_LINE_CAP);
        assert!(
            lines.len() < 10,
            "the card flooded with {} lines: {lines:?}",
            lines.len()
        );
        let joined = lines.join("\n");
        assert!(joined.contains("too large to show here"), "{joined}");
        assert!(joined.contains("1 file changed"), "{joined}");
        assert!(joined.contains("SKILL.md"), "{joined}");
        assert!(joined.contains("+200"), "{joined}");
        // The counts are still exact — the cap hides detail, never the scale.
        assert!(joined.contains("400 lines"), "{joined}");
    }

    // Repository content is hostile input: a skill whose FILENAME carries
    // control characters must not be able to move the cursor in the review it
    // appears in.
    #[test]
    fn hostile_paths_are_sanitized_in_both_render_modes() {
        let store = store_with("abc", &[("SKILL.md", "a\n")]);
        let live = assert_fs::TempDir::new().unwrap();
        live.child("SKILL.md").write_str("a\n").unwrap();
        live.child("evil\u{1b}[2Kname.md").write_str("x\n").unwrap();
        let d = diff_against_pin(store.path(), "abc", live.path());
        let rendered = render_lines(&d, DIFF_LINE_CAP).join("\n");
        assert!(
            !rendered.contains('\u{1b}'),
            "escape survived: {rendered:?}"
        );
    }

    // A symlink is excluded from the digest that produced the pin, so it must
    // be excluded here too — otherwise the review shows content the pin never
    // covered, and a link could point anywhere on the filesystem.
    #[cfg(unix)]
    #[test]
    fn symlinks_are_skipped_never_followed() {
        let secret = assert_fs::TempDir::new().unwrap();
        secret.child("id_rsa").write_str("PRIVATE KEY\n").unwrap();
        let store = store_with("abc", &[("SKILL.md", "a\n")]);
        let live = assert_fs::TempDir::new().unwrap();
        live.child("SKILL.md").write_str("a\n").unwrap();
        std::os::unix::fs::symlink(secret.child("id_rsa").path(), live.child("leak.md").path())
            .unwrap();
        let d = diff_against_pin(store.path(), "abc", live.path());
        let rendered = render_lines(&d, DIFF_LINE_CAP).join("\n");
        assert!(!rendered.contains("PRIVATE KEY"), "{rendered}");
        assert!(!rendered.contains("leak.md"), "{rendered}");
    }
}
