//! Intake detection — the funnel's first step (STRATEGY.md Phase 1).
//!
//! Content dropped into the project's own `skills/` and `instructions/`
//! directories is inert until the manifest declares it: nothing walks those
//! directories, so an undeclared file is never resolved, rendered, pinned, or
//! shown to an agent. That inertness is the property this module must not
//! weaken. It only *notices* such content at command time — no daemon — and
//! reports it so `status`/`doctor`/`use`/`lock` can offer adoption.
//!
//! Two rules shape the implementation:
//!
//! - **Invariant 7 — all repository content is hostile input.** Everything
//!   below reads bounded amounts, validates names before they can become
//!   manifest keys, refuses symlinks, and sanitizes every byte that reaches a
//!   terminal. Nothing here is interpolated into a shell command.
//! - **Provenance before compression.** Each item is classified as locally
//!   authored or as having arrived with the project. Slice A only *reports*
//!   that split; the single-action path Phase 1 adds consumes it, and content
//!   that fails the signal always keeps the full staged review.
//!
//! Scope guard, from the strategy: skills and instructions only. Servers carry
//! commands, env, and secrets — there is no file to drop. Hooks and extensions
//! are executable kinds and are excluded from the funnel entirely.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use agentstack_recorder::TrustAction;

use crate::manifest::Manifest;

/// How many entries a single scan will look at before it stops. A dropped
/// directory is user content, so it is bounded like any other hostile input:
/// far above a plausible hand-authored set, far below "walk whatever is there".
const MAX_ENTRIES: usize = 256;

/// How much of a file is read to derive its one-line summary. Summaries come
/// from the head of the file; a multi-megabyte "skill" gets a summary from its
/// first bytes rather than a multi-megabyte read.
const SUMMARY_READ_BYTES: usize = 8 * 1024;

/// The two inert kinds the funnel covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Skill,
    Instruction,
}

impl Kind {
    /// The directory this kind is dropped into, relative to the manifest dir.
    fn dir(self) -> &'static str {
        match self {
            Kind::Skill => "skills",
            Kind::Instruction => "instructions",
        }
    }

    /// The manifest table this kind is declared in.
    pub fn section(self) -> &'static str {
        match self {
            Kind::Skill => "skills",
            Kind::Instruction => "instructions",
        }
    }

    pub fn noun(self) -> &'static str {
        match self {
            Kind::Skill => "skill",
            Kind::Instruction => "instruction",
        }
    }
}

/// Whether an item carries a local-authorship signal.
///
/// This is the gate on compression, not on adoption: both variants can be
/// adopted, but only [`Provenance::LocallyAuthored`] may ever take a shortened
/// path. The reason string is carried so the user is told *why* they are on the
/// path they are on — a classification the user cannot see is not a consent
/// story.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Provenance {
    /// Demonstrably written here: untracked in git, or modified after this
    /// project's last recorded trust grant.
    LocallyAuthored(&'static str),
    /// Arrived with the project, or carries no signal either way. Always takes
    /// the full staged review.
    Arrived(&'static str),
}

impl Provenance {
    pub fn is_local(&self) -> bool {
        matches!(self, Provenance::LocallyAuthored(_))
    }

    pub fn reason(&self) -> &'static str {
        match self {
            Provenance::LocallyAuthored(r) | Provenance::Arrived(r) => r,
        }
    }
}

/// One piece of undeclared content found in an intake directory.
#[derive(Debug, Clone)]
pub struct Item {
    pub kind: Kind,
    /// Validated name — safe to use as a manifest key.
    pub name: String,
    /// Manifest-relative path, in the form manifest entries already use.
    pub rel_path: String,
    pub abs_path: PathBuf,
    /// Sanitized one-line summary, when the content offers one.
    pub summary: Option<String>,
    pub provenance: Provenance,
}

/// What one scan found.
#[derive(Debug, Clone, Default)]
pub struct Found {
    /// Undeclared content that can be adopted as-is.
    pub items: Vec<Item>,
    /// Dropped files whose name is already taken by a manifest entry. Reported
    /// rather than adopted: see the collision note in `collect`.
    pub collisions: Vec<Collision>,
}

/// A dropped file whose name a manifest entry already uses.
#[derive(Debug, Clone)]
pub struct Collision {
    pub kind: Kind,
    pub name: String,
    pub rel_path: String,
}

impl Found {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty() && self.collisions.is_empty()
    }
}

/// Scan both intake directories for content the manifest does not declare.
///
/// `dir` is the manifest directory (`.agentstack/`, or the project root under
/// the legacy layout), so the paths produced here resolve exactly the way an
/// existing manifest entry's `path` does. `base` is the project base dir — the
/// git work tree and the trust store's key.
pub fn scan(dir: &Path, base: &Path, manifest: &Manifest) -> Found {
    let clock = ProvenanceClock::for_project(base);
    let mut found = Found::default();
    for kind in [Kind::Skill, Kind::Instruction] {
        collect(kind, dir, base, manifest, &clock, &mut found);
    }
    found
}

fn collect(
    kind: Kind,
    dir: &Path,
    base: &Path,
    manifest: &Manifest,
    clock: &ProvenanceClock,
    out: &mut Found,
) {
    let root = dir.join(kind.dir());
    let Ok(entries) = std::fs::read_dir(&root) else {
        return;
    };
    let declared = declared_paths(kind, dir, manifest);

    // `read_dir` order is filesystem order; sorting makes the notice stable
    // across runs (and makes the witnesses assert on something deterministic).
    let mut candidates: Vec<PathBuf> = entries
        .take(MAX_ENTRIES)
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    candidates.sort();

    for path in candidates {
        // Symlinks are refused rather than followed: a dropped link could point
        // anywhere (including outside the project), and "adopt what you see in
        // this directory" must mean exactly that.
        if std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
            continue;
        }
        let Some((name, content, rel)) = shape(kind, &path, dir) else {
            continue;
        };
        // The name becomes a manifest key and appears in output. Validating
        // here means nothing downstream has to trust the filesystem.
        if crate::text::validate_name(&name).is_err() {
            continue;
        }
        if declared.iter().any(|d| same_file(d, &content)) {
            continue;
        }
        // A name this manifest already uses is NOT an undeclared drop, even
        // when the existing entry points somewhere else — a git- or
        // library-sourced skill has no `path` at all, so the path comparison
        // above does not see it. Adopting such a name would replace a pinned,
        // possibly already-reviewed declaration with repo-controlled bytes
        // while the preview called it an addition. Detection stops here and
        // says so; resolving the collision is the user's call, by renaming the
        // file or removing the old entry.
        if declares_name(kind, manifest, &name) {
            out.collisions.push(Collision {
                kind,
                name,
                rel_path: rel,
            });
            continue;
        }
        out.items.push(Item {
            kind,
            name,
            rel_path: rel,
            summary: summarize(kind, &content),
            provenance: clock.classify(base, &content),
            abs_path: content,
        });
    }
}

/// A real file, not a link to one. `symlink_metadata` does not follow, which
/// is the whole point: every place intake decides to *read* something asks this
/// first, so "symlinks are refused rather than followed" holds for content, not
/// just for the directory entry that names it.
fn is_regular(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file())
}

/// What a path in an intake directory has to look like to be an item of this
/// kind, and what its name, content path, and manifest-relative path are.
///
/// Skills follow the library layout: a directory holding `SKILL.md`, named for
/// the directory. Instructions are single `.md` files, named for the stem.
fn shape(kind: Kind, path: &Path, dir: &Path) -> Option<(String, PathBuf, String)> {
    let stem_or_name = |p: &Path, ext: bool| -> Option<String> {
        let s = if ext { p.file_stem() } else { p.file_name() };
        s.and_then(|s| s.to_str()).map(str::to_string)
    };
    match kind {
        Kind::Skill => {
            // `is_file()` follows links, so the marker file is checked with
            // `is_regular` first: a `SKILL.md` symlinked at `~/.ssh/id_rsa`
            // would otherwise be opened by the summarizer and its first line
            // printed by `status` — a read of an out-of-project file, before
            // any gate. The refusal has to cover the content, not just the
            // directory entry.
            if !path.is_dir() || !is_regular(&path.join("SKILL.md")) {
                return None;
            }
            let name = stem_or_name(path, false)?;
            let rel = format!("./{}/{}", kind.dir(), name);
            Some((name, path.to_path_buf(), rel))
        }
        Kind::Instruction => {
            if !is_regular(path) || path.extension().and_then(|e| e.to_str()) != Some("md") {
                return None;
            }
            let name = stem_or_name(path, true)?;
            let rel = format!("./{}/{}.md", kind.dir(), name);
            let _ = dir;
            Some((name, path.to_path_buf(), rel))
        }
    }
}

/// The absolute paths this manifest already declares for `kind`, so declared
/// content is never re-offered. Paths are compared by canonical identity, not
/// by string, because `./skills/x`, `skills/x`, and an absolute spelling of the
/// same directory are the same declaration.
fn declared_paths(kind: Kind, dir: &Path, manifest: &Manifest) -> Vec<PathBuf> {
    let anchor = |p: &str| -> PathBuf {
        let raw = Path::new(p);
        if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            dir.join(raw)
        }
    };
    match kind {
        Kind::Skill => manifest
            .skills
            .values()
            .filter_map(|s| s.path.as_deref())
            .map(anchor)
            .collect(),
        Kind::Instruction => manifest
            .instructions
            .values()
            .map(|i| anchor(&i.path))
            .collect(),
    }
}

/// Whether the manifest already has an entry under this name for this kind —
/// whatever that entry's source is.
fn declares_name(kind: Kind, manifest: &Manifest, name: &str) -> bool {
    match kind {
        Kind::Skill => manifest.skills.contains_key(name),
        Kind::Instruction => manifest.instructions.contains_key(name),
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    canon(a) == canon(b)
}

/// A one-line summary for the preview, from content that is assumed hostile:
/// bounded read, a bounded number of lines considered, and every byte that
/// survives passes through the shared one-line sanitizer.
fn summarize(kind: Kind, path: &Path) -> Option<String> {
    let file = match kind {
        Kind::Skill => path.join("SKILL.md"),
        Kind::Instruction => path.to_path_buf(),
    };
    let text = read_head(&file, SUMMARY_READ_BYTES)?;
    let mut in_frontmatter = false;
    for (i, raw) in text.lines().enumerate().take(40) {
        let line = raw.trim();
        if i == 0 && line == "---" {
            in_frontmatter = true;
            continue;
        }
        if in_frontmatter {
            if line == "---" {
                in_frontmatter = false;
                continue;
            }
            if let Some(rest) = line.strip_prefix("description:") {
                let v = rest.trim().trim_matches(['"', '\'']);
                if !v.is_empty() {
                    return Some(clean(v));
                }
            }
            continue;
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        return Some(clean(line));
    }
    None
}

/// Strip terminal escapes and control bytes, then cap the length. Both halves
/// matter: `sanitize_line` neutralizes the content, `truncate_chars` keeps a
/// pathological single line from filling the terminal.
fn clean(s: &str) -> String {
    crate::text::truncate_chars(&crate::text::sanitize_line(s), 100)
}

/// Read at most `max` bytes of a file, lossily. Truncating mid-UTF-8 is fine
/// here: the result is only ever summarized and sanitized, never re-parsed.
fn read_head(path: &Path, max: usize) -> Option<String> {
    use std::io::Read;
    if !is_regular(path) {
        return None;
    }
    let mut f = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; max];
    let n = f.read(&mut buf).ok()?;
    buf.truncate(n);
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// The provenance clock: git tracking plus this project's last recorded trust
/// grant (the P0.2 trust-mutation events).
struct ProvenanceClock {
    /// Every path git tracks under the intake directories, canonicalized.
    /// `None` when `base` is not a git work tree at all — there, "untracked"
    /// carries no information and only the grant timestamp is consulted.
    ///
    /// Collected in ONE git spawn rather than one per item: `status` is on this
    /// path and is required to feel instant.
    tracked: Option<Vec<PathBuf>>,
    /// Unix seconds of the most recent grant or regrant for this project.
    last_grant: Option<u64>,
}

impl ProvenanceClock {
    fn for_project(base: &Path) -> Self {
        Self {
            tracked: tracked_under_intake(base),
            last_grant: last_grant_ts(base),
        }
    }

    /// Classify one path.
    ///
    /// In a git work tree, git alone decides: untracked is the user's own work,
    /// tracked came with the project. The mtime rule is deliberately NOT
    /// consulted for tracked content, because git rewrites the mtime of every
    /// file a `pull`, `checkout`, or `rebase` lands — so "newer than the last
    /// review" would promote freshly pulled, remote-authored content to "your
    /// own work", the exact inversion the provenance split exists to prevent.
    ///
    /// Outside a work tree there is no tracking signal, so the grant clock is
    /// all there is: content modified since the last review is local work.
    /// A project with no grant history has no clock to compare against either,
    /// so it takes the full staged review — the conservative reading the
    /// strategy asks for.
    fn classify(&self, _base: &Path, path: &Path) -> Provenance {
        if let Some(tracked) = &self.tracked {
            // A skill is a directory: it counts as tracked if git knows about
            // anything inside it, so adding one new file to a committed skill
            // does not read as "git has never seen this".
            let canon = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
            return if tracked.iter().any(|t| t.starts_with(&canon)) {
                Provenance::Arrived("committed to this repository")
            } else {
                Provenance::LocallyAuthored("untracked in git")
            };
        }
        match (self.last_grant, mtime_secs(path)) {
            (Some(grant), Some(m)) if m > grant => {
                Provenance::LocallyAuthored("changed after this project's last review")
            }
            (Some(_), _) => Provenance::Arrived("committed, and unchanged since the last review"),
            (None, _) => Provenance::Arrived("this project has no review history yet"),
        }
    }
}

/// Every tracked path under the intake directories, canonicalized, in one git
/// call. `None` means "not a git work tree" — distinct from "a work tree that
/// tracks nothing here", which is an empty vec and makes everything local.
///
/// `-z` because filenames are hostile input: git quotes and escapes them in the
/// default output, and NUL-delimited output has no escaping to misparse.
fn tracked_under_intake(base: &Path) -> Option<Vec<PathBuf>> {
    let out = crate::gitx::run_raw(
        crate::gitx::Profile::Ingest,
        &[
            "ls-files",
            "-z",
            "--",
            ".agentstack/skills",
            ".agentstack/instructions",
            "skills",
            "instructions",
        ],
        Some(base),
    )
    .ok()?;
    if !out.success {
        // Non-zero here means "not a repository" (a valid repo with no matches
        // exits 0 with empty output), so the git signal is unavailable.
        return None;
    }
    Some(
        out.stdout
            .split('\0')
            .filter(|s| !s.is_empty())
            .map(|rel| {
                let p = base.join(rel);
                std::fs::canonicalize(&p).unwrap_or(p)
            })
            .collect(),
    )
}

/// Newest mtime in a subtree (or of a single file), in unix seconds. A skill is
/// a directory, and editing one file inside it is authoring the skill.
fn mtime_secs(path: &Path) -> Option<u64> {
    fn walk(path: &Path, depth: usize, budget: &mut usize) -> Option<u64> {
        if *budget == 0 {
            return None;
        }
        *budget -= 1;
        let meta = std::fs::symlink_metadata(path).ok()?;
        let own = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs());
        if !meta.is_dir() || depth == 0 {
            return own;
        }
        let mut newest = own;
        for e in std::fs::read_dir(path).ok()?.flatten() {
            if let Some(t) = walk(&e.path(), depth - 1, budget) {
                newest = Some(newest.map_or(t, |n: u64| n.max(t)));
            }
        }
        newest
    }
    // Bounded like every other read of dropped content: a deep or wide tree
    // stops contributing rather than turning a status call into a full walk.
    let mut budget = MAX_ENTRIES;
    walk(path, 8, &mut budget)
}

/// The most recent grant or regrant recorded for this project, in unix seconds.
///
/// Reads the P0.2 trust-mutation log. `Repin` is deliberately excluded: it
/// re-pins a digest without a human in the loop, so it is not a review the
/// provenance clock may date content against. The log is oldest-first, so the
/// last match is the newest.
fn last_grant_ts(base: &Path) -> Option<u64> {
    let key = agentstack_trust::key_for(base);
    agentstack_recorder::read_trust_all()
        .into_iter()
        .rfind(|e| {
            e.project == key && matches!(e.action, TrustAction::Grant | TrustAction::Regrant)
        })
        .map(|e| e.ts)
}

/// The one-line notice a command shows when intake content is waiting. `None`
/// when there is nothing to say — callers print nothing rather than "0 items".
pub fn notice(found: &Found) -> Option<String> {
    let items = &found.items;
    if items.is_empty() {
        return None;
    }
    let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).take(3).collect();
    let more = items.len().saturating_sub(names.len());
    let listed = if more > 0 {
        format!("{}, +{more} more", names.join(", "))
    } else {
        names.join(", ")
    };
    Some(format!(
        "{} not in your setup yet ({listed}) — `agentstack adopt` to review and add",
        super::commands::count(items.len(), "dropped file"),
    ))
}

/// Print the notice, if any, for a command that is not itself an intake
/// surface — `use` and `lock` mention dropped content in passing so the user
/// learns about it at the moment they are already touching the setup.
pub fn print_notice(dir: &Path, base: &Path, manifest: &Manifest) {
    use owo_colors::OwoColorize;
    let found = scan(dir, base, manifest);
    if let Some(text) = notice(&found) {
        println!("  {} {}", "·".dimmed(), text.dimmed());
    }
    for c in &found.collisions {
        println!(
            "  {} {}",
            "·".dimmed(),
            format!(
                "{} '{}' in {} is not adopted — that name is already declared",
                c.kind.noun(),
                c.name,
                c.rel_path
            )
            .dimmed()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    fn manifest_with(text: &str) -> Manifest {
        toml::from_str(&format!("version = 1\n{text}")).expect("test manifest parses")
    }

    fn drop_skill(dir: &Path, name: &str, body: &str) {
        let d = dir.join("skills").join(name);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("SKILL.md"), body).unwrap();
    }

    #[test]
    fn undeclared_skills_and_instructions_are_seen_declared_ones_are_not() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let dir = tmp.path();
        drop_skill(
            dir,
            "dropped",
            "---\ndescription: Does a thing\n---\n# Dropped\n",
        );
        drop_skill(dir, "already", "# Already\n");
        std::fs::create_dir_all(dir.join("instructions")).unwrap();
        std::fs::write(dir.join("instructions/house.md"), "House rules go here.\n").unwrap();

        let manifest = manifest_with("[skills.already]\npath = \"./skills/already\"\n");
        let items = scan(dir, dir, &manifest).items;

        let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["dropped", "house"],
            "declared content is not re-offered"
        );
        assert_eq!(items[0].kind, Kind::Skill);
        assert_eq!(items[0].rel_path, "./skills/dropped");
        assert_eq!(items[0].summary.as_deref(), Some("Does a thing"));
        assert_eq!(items[1].kind, Kind::Instruction);
        assert_eq!(items[1].rel_path, "./instructions/house.md");
        assert_eq!(items[1].summary.as_deref(), Some("House rules go here."));
    }

    /// The provenance split, witnessed the way the strategy specifies: one
    /// directory, two items differing only in provenance, different paths.
    /// Here the difference is the grant clock — one file predates the
    /// project's only recorded review, the other postdates it — with git out
    /// of the picture so the timestamp is the sole discriminator.
    #[test]
    fn same_directory_two_provenances_two_paths() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let dir = tmp.path();
        drop_skill(dir, "old", "# Old\n");
        drop_skill(dir, "new", "# New\n");

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        set_mtime(&dir.join("skills/old"), now - 10_000);
        set_mtime(&dir.join("skills/old/SKILL.md"), now - 10_000);

        let clock = ProvenanceClock {
            tracked: None,
            last_grant: Some(now - 5_000),
        };
        let old = clock.classify(dir, &dir.join("skills/old"));
        let new = clock.classify(dir, &dir.join("skills/new"));

        assert!(
            !old.is_local(),
            "content older than the last review is not local work"
        );
        assert!(
            new.is_local(),
            "content written since the last review is local work"
        );
        assert_ne!(old, new, "the two must take different paths");
    }

    #[test]
    fn no_review_history_means_full_review() {
        let tmp = assert_fs::TempDir::new().unwrap();
        drop_skill(tmp.path(), "fresh", "# Fresh\n");
        let clock = ProvenanceClock {
            tracked: None,
            last_grant: None,
        };
        let p = clock.classify(tmp.path(), &tmp.path().join("skills/fresh"));
        assert!(!p.is_local());
        assert_eq!(p.reason(), "this project has no review history yet");
    }

    /// Hostile input: an unusable name never becomes a manifest key, a symlink
    /// is not followed, and a control-sequence summary is neutralized.
    #[test]
    fn hostile_content_is_bounded_and_never_becomes_a_key() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let dir = tmp.path();
        drop_skill(dir, "../escape", "# no\n");
        drop_skill(dir, "ok", "\u{1b}[31mred\u{7} and\nmore\n");
        std::fs::create_dir_all(dir.join("skills/target")).unwrap();
        std::fs::write(dir.join("skills/target/SKILL.md"), "# T\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.join("skills/target"), dir.join("skills/link")).unwrap();

        let items = scan(dir, dir, &manifest_with("")).items;
        let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
        assert!(
            !names.iter().any(|n| n.contains("..")),
            "path-traversal name rejected: {names:?}"
        );
        assert!(
            !names.contains(&"link"),
            "symlinks are not followed: {names:?}"
        );

        let ok = items
            .iter()
            .find(|i| i.name == "ok")
            .expect("valid sibling still seen");
        let summary = ok.summary.as_deref().unwrap_or_default();
        assert!(
            !summary.contains('\u{1b}'),
            "escape sequences are stripped: {summary:?}"
        );
        assert!(
            !summary.contains('\u{7}'),
            "control bytes are stripped: {summary:?}"
        );
    }

    fn set_mtime(path: &Path, secs: u64) {
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs);
        let f = std::fs::File::options()
            .write(true)
            .open(path)
            .or_else(|_| std::fs::File::open(path))
            .unwrap();
        f.set_modified(t).unwrap();
    }
}
