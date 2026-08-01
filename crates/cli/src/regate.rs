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
    let hex = bare_hex(pin);
    let approved = store_root.join("content").join(hex);
    // The snapshot must still hash to its own name before it may be shown as
    // "the approved version" (F4): the store directory is writable, and a
    // tampered or truncated snapshot rendered here would put words in the
    // last consent's mouth — the diff card would attribute bytes to the user
    // that they never approved. Verification covers both pin families (skill
    // trees and single-file instruction deposits); failure degrades to the
    // same honest message a never-captured snapshot gets.
    if !crate::store::verified_content(&approved, hex) {
        return PinDiff::NoSnapshot;
    }
    tree_diff(&approved, live)
}

/// Compare the bytes TWO pins cover, reading both sides out of the content
/// store.
///
/// This is the machine-readable card's diff, and it is deliberately
/// pin-to-pin where [`diff_against_pin`] is pin-to-live. `trust --preview` may
/// not write, and locating a skill's live bytes reaches git worktree
/// materialization — so the honest delta it *can* compute is
/// "what the last consent pinned" → "what the lockfile pins now", which is
/// exactly what the consent digest covers (the digest is taken over the lock
/// bytes). The terminal review stays authoritative over live bytes.
///
/// Both sides are re-verified against their own names before either is shown
/// (F4), so a tampered store degrades to [`PinDiff::NoSnapshot`] rather than
/// attributing bytes to a consent that never saw them.
pub fn diff_between_pins(store_root: &Path, prior_pin: &str, current_pin: &str) -> PinDiff {
    let prior_hex = bare_hex(prior_pin);
    let current_hex = bare_hex(current_pin);
    if prior_hex == current_hex {
        // Equal pins are equal content by construction, so this needs no
        // snapshot to be true — and a clean project's preview then touches the
        // content store not at all.
        return PinDiff::Unchanged;
    }
    let before = store_root.join("content").join(prior_hex);
    let after = store_root.join("content").join(current_hex);
    if !crate::store::verified_content(&before, prior_hex)
        || !crate::store::verified_content(&after, current_hex)
    {
        return PinDiff::NoSnapshot;
    }
    tree_diff(&before, &after)
}

/// The lockfile records bare hex; `trust` spells the same digest with a
/// `sha256:` prefix. Accept either at every read site.
fn bare_hex(pin: &str) -> &str {
    pin.rsplit(':').next().unwrap_or(pin)
}

/// The per-file comparison shared by both entry points above. Takes directories
/// (or, for the instruction pin family, single files — see [`read_tree`]).
fn tree_diff(before_root: &Path, after_root: &Path) -> PinDiff {
    let before = read_tree(before_root);
    let after = read_tree(after_root);

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

    let total = changed_line_total(changes);

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

/// What the cap counts: changed lines for a modified file, one summary line for
/// a file that appeared or disappeared. Shared by the terminal renderer and the
/// JSON one so a card and a panel cap at the same place — a diff that is "too
/// large to show" in one and inlined in the other would be two answers to one
/// question.
fn changed_line_total(changes: &[FileChange]) -> usize {
    changes
        .iter()
        .map(|c| match c {
            FileChange::Modified { added, removed, .. } => added + removed,
            _ => 1,
        })
        .sum()
}

/// The machine-readable form of a diff, for the structured consent card.
///
/// Mirrors [`render_lines`]'s decisions rather than restating them: the same
/// cap, the same counts, the same sanitization. Over the cap, `lines` is `null`
/// and `capped` is true — the counts stay exact, because the cap hides detail
/// and never scale. Every path and every line is sanitized: repository content
/// is hostile input and this string ends up in someone else's renderer.
pub fn pin_diff_json(diff: &PinDiff, cap: usize) -> serde_json::Value {
    let (status, changes): (&str, &[FileChange]) = match diff {
        PinDiff::NoSnapshot => ("no_snapshot", &[]),
        PinDiff::Unchanged => ("unchanged", &[]),
        PinDiff::Changed(changes) => ("changed", changes.as_slice()),
    };
    let capped = changed_line_total(changes) > cap;
    let files: Vec<serde_json::Value> = changes
        .iter()
        .map(|c| {
            // An added or removed file has no body to show on either side, so
            // its counts are zero and its lines are absent — the fact worth
            // reporting is that the file appeared or disappeared.
            let (change, added, removed, body) = match c {
                FileChange::Added(_) => ("added", 0, 0, None),
                FileChange::Removed(_) => ("removed", 0, 0, None),
                FileChange::Modified {
                    added,
                    removed,
                    body,
                    ..
                } => ("modified", *added, *removed, Some(body)),
            };
            let lines = match body {
                Some(body) if !capped => serde_json::Value::Array(
                    body.lines()
                        .map(|l| serde_json::Value::String(crate::text::sanitize_line(l)))
                        .collect(),
                ),
                _ => serde_json::Value::Null,
            };
            serde_json::json!({
                "path": crate::text::sanitize_line(c.path()),
                "change": change,
                "added": added,
                "removed": removed,
                "lines": lines,
            })
        })
        .collect();
    serde_json::json!({
        "status": status,
        "headline": headline(diff),
        "files": files,
        "capped": capped,
    })
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
    // Read ceilings (F17): dropped content controls its own size, and this
    // reader used to pull every byte into memory before the display cap
    // applied — DIFF_LINE_CAP bounded what was SHOWN, not what was read.
    // An oversized file yields the same placeholder on both sides, so an
    // unchanged huge file diffs as unchanged and a changed one reads as
    // "review it directly" — bounded and honest, never flooding or lying.
    const MAX_TREE_FILES: usize = 500;
    const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
    const TOO_LARGE: &str = "(too large to diff here — review this file directly)\n";
    let bounded_read = |path: &Path| -> Option<String> {
        let meta = path.symlink_metadata().ok()?;
        if meta.len() > MAX_FILE_BYTES {
            return Some(TOO_LARGE.to_string());
        }
        std::fs::read_to_string(path).ok()
    };
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
        return match bounded_read(root) {
            Some(text) => vec![(name, text)],
            None => Vec::new(),
        };
    }
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    'walk: while let Some(dir) = stack.pop() {
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
                if out.len() >= MAX_TREE_FILES {
                    break 'walk;
                }
                if let Some(text) = bounded_read(&path) {
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

    /// A store root holding one snapshot deposited under its REAL tree
    /// digest. `diff_against_pin` verifies snapshots against their names
    /// (F4), so a fixture under an invented name reads as absent — exactly
    /// as a tampered production snapshot does.
    fn store_with(files: &[(&str, &str)]) -> (assert_fs::TempDir, String) {
        let root = assert_fs::TempDir::new().unwrap();
        let staging = root.child("staging");
        for (name, body) in files {
            staging.child(name).write_str(body).unwrap();
        }
        let digest = crate::store::dir_digest(staging.path())
            .unwrap()
            .hex()
            .to_string();
        std::fs::create_dir_all(root.path().join("content")).unwrap();
        std::fs::rename(staging.path(), root.path().join("content").join(&digest)).unwrap();
        (root, digest)
    }

    /// F4 WITNESS: a snapshot that no longer hashes to its own name is not
    /// shown as "the approved version" — the diff degrades to the honest
    /// no-snapshot message instead of attributing tampered bytes to the last
    /// consent. (Before the fix, a bare `is_dir()` rendered whatever was
    /// there.)
    #[test]
    fn a_tampered_snapshot_is_never_presented_as_approved() {
        let (store, pin) = store_with(&[(
            "SKILL.md",
            "# approved
",
        )]);
        std::fs::write(
            store.path().join("content").join(&pin).join("SKILL.md"),
            "# EVIL
",
        )
        .unwrap();
        let live = assert_fs::TempDir::new().unwrap();
        live.child("SKILL.md")
            .write_str(
                "# live
",
            )
            .unwrap();
        assert_eq!(
            diff_against_pin(store.path(), &pin, live.path()),
            PinDiff::NoSnapshot,
            "a tampered snapshot was rendered as the approved bytes"
        );
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
        let (store, pin) = store_with(&[("SKILL.md", "# hi\nbody\n")]);
        let live = assert_fs::TempDir::new().unwrap();
        live.child("SKILL.md").write_str("# hi\nbody\n").unwrap();
        assert_eq!(
            diff_against_pin(store.path(), &pin, live.path()),
            PinDiff::Unchanged
        );
    }

    // The acceptance target: the actual changed lines, not "digest mismatch".
    #[test]
    fn a_small_edit_shows_the_real_changed_lines() {
        let (store, pin) = store_with(&[("SKILL.md", "# hi\nkeep\nold line\n")]);
        let live = assert_fs::TempDir::new().unwrap();
        live.child("SKILL.md")
            .write_str("# hi\nkeep\nnew line\n")
            .unwrap();
        let d = diff_against_pin(store.path(), &pin, live.path());
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
        let (store, pin) = store_with(&[("SKILL.md", "same\n"), ("gone.md", "bye\n")]);
        let live = assert_fs::TempDir::new().unwrap();
        live.child("SKILL.md").write_str("same\n").unwrap();
        live.child("extra.md").write_str("hello\n").unwrap();
        let d = diff_against_pin(store.path(), &pin, live.path());
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
        let (store, pin) = store_with(&[("SKILL.md", &old)]);
        let live = assert_fs::TempDir::new().unwrap();
        live.child("SKILL.md").write_str(&new).unwrap();

        let d = diff_against_pin(store.path(), &pin, live.path());
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
        let (store, pin) = store_with(&[("SKILL.md", "a\n")]);
        let live = assert_fs::TempDir::new().unwrap();
        live.child("SKILL.md").write_str("a\n").unwrap();
        live.child("evil\u{1b}[2Kname.md").write_str("x\n").unwrap();
        let d = diff_against_pin(store.path(), &pin, live.path());
        let rendered = render_lines(&d, DIFF_LINE_CAP).join("\n");
        assert!(
            !rendered.contains('\u{1b}'),
            "escape survived: {rendered:?}"
        );
    }

    // ---- pin-to-pin (the machine-readable card) ------------------------

    /// Deposit a second snapshot into an EXISTING store root, so two pins can
    /// be compared against each other the way a re-lock leaves them.
    fn deposit(root: &Path, files: &[(&str, &str)]) -> String {
        let staging = root.join("staging-2");
        std::fs::create_dir_all(&staging).unwrap();
        for (name, body) in files {
            std::fs::write(staging.join(name), body).unwrap();
        }
        let digest = crate::store::dir_digest(&staging)
            .unwrap()
            .hex()
            .to_string();
        std::fs::rename(&staging, root.join("content").join(&digest)).unwrap();
        digest
    }

    // Equal pins are equal content by construction — and saying so must not
    // require either snapshot to still be on disk.
    #[test]
    fn identical_pins_are_unchanged_without_reading_the_store() {
        let empty = assert_fs::TempDir::new().unwrap();
        assert_eq!(
            diff_between_pins(empty.path(), "sha256:abc", "abc"),
            PinDiff::Unchanged,
            "the sha256: prefix and the bare hex name the same content"
        );
    }

    #[test]
    fn a_moved_pin_shows_the_lines_between_the_two_snapshots() {
        let (store, prior) = store_with(&[("SKILL.md", "# hi\nkeep\nold line\n")]);
        let current = deposit(store.path(), &[("SKILL.md", "# hi\nkeep\nnew line\n")]);
        let d = diff_between_pins(store.path(), &prior, &current);
        assert_eq!(headline(&d).as_deref(), Some("changed 2 lines"));
        let json = pin_diff_json(&d, DIFF_LINE_CAP);
        assert_eq!(json["status"], "changed");
        assert_eq!(json["capped"], false);
        assert_eq!(json["files"][0]["path"], "SKILL.md");
        assert_eq!(json["files"][0]["change"], "modified");
        assert_eq!(json["files"][0]["added"], 1);
        assert_eq!(json["files"][0]["removed"], 1);
        let lines = json["files"][0]["lines"].as_array().unwrap().len();
        assert!(lines > 0, "under the cap the real lines are carried");
    }

    // Either side missing degrades to the same honest answer a never-captured
    // snapshot gets — the preview never invents a diff.
    #[test]
    fn a_pin_with_no_snapshot_degrades_instead_of_guessing() {
        let (store, prior) = store_with(&[("SKILL.md", "a\n")]);
        let d = diff_between_pins(store.path(), &prior, &"f".repeat(64));
        assert_eq!(d, PinDiff::NoSnapshot);
        let json = pin_diff_json(&d, DIFF_LINE_CAP);
        assert_eq!(json["status"], "no_snapshot");
        assert!(json["headline"].is_null());
        assert_eq!(json["files"].as_array().unwrap().len(), 0);
    }

    // F4, both sides: a tampered snapshot on EITHER end is never rendered as
    // approved content — the same refusal `diff_against_pin` makes.
    #[test]
    fn a_tampered_snapshot_on_either_side_degrades() {
        let (store, prior) = store_with(&[("SKILL.md", "# approved\n")]);
        let current = deposit(store.path(), &[("SKILL.md", "# next\n")]);
        std::fs::write(
            store.path().join("content").join(&current).join("SKILL.md"),
            "# EVIL\n",
        )
        .unwrap();
        assert_eq!(
            diff_between_pins(store.path(), &prior, &current),
            PinDiff::NoSnapshot,
            "a tampered snapshot was rendered as pinned content"
        );
    }

    // The cap hides detail, never scale: over it the lines go away and the
    // counts stay exact.
    #[test]
    fn an_oversized_rewrite_drops_lines_and_keeps_counts() {
        let old: String = (0..200).map(|i| format!("old {i}\n")).collect();
        let new: String = (0..200).map(|i| format!("new {i}\n")).collect();
        let (store, prior) = store_with(&[("SKILL.md", &old)]);
        let current = deposit(store.path(), &[("SKILL.md", &new)]);
        let json = pin_diff_json(
            &diff_between_pins(store.path(), &prior, &current),
            DIFF_LINE_CAP,
        );
        assert_eq!(json["capped"], true);
        assert!(json["files"][0]["lines"].is_null());
        assert_eq!(json["files"][0]["added"], 200);
        assert_eq!(json["files"][0]["removed"], 200);
    }

    // A symlink is excluded from the digest that produced the pin, so it must
    // be excluded here too — otherwise the review shows content the pin never
    // covered, and a link could point anywhere on the filesystem.
    #[cfg(unix)]
    #[test]
    fn symlinks_are_skipped_never_followed() {
        let secret = assert_fs::TempDir::new().unwrap();
        secret.child("id_rsa").write_str("PRIVATE KEY\n").unwrap();
        let (store, pin) = store_with(&[("SKILL.md", "a\n")]);
        let live = assert_fs::TempDir::new().unwrap();
        live.child("SKILL.md").write_str("a\n").unwrap();
        std::os::unix::fs::symlink(secret.child("id_rsa").path(), live.child("leak.md").path())
            .unwrap();
        let d = diff_against_pin(store.path(), &pin, live.path());
        let rendered = render_lines(&d, DIFF_LINE_CAP).join("\n");
        assert!(!rendered.contains("PRIVATE KEY"), "{rendered}");
        assert!(!rendered.contains("leak.md"), "{rendered}");
    }
}
