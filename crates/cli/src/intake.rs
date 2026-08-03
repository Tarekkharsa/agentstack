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
    /// What the name is ALREADY declared as — `git@abc123`, `library`, or a
    /// declared path. The refusal is more useful when it names the thing being
    /// protected, not just the fact of the conflict.
    pub declared_as: String,
    /// The real difference between the bytes already approved under this name
    /// and the bytes that were dropped, capped for the terminal. Empty when the
    /// approved bytes are not recorded (never locked, or pinned before
    /// snapshots existed) — in which case the refusal degrades to naming the
    /// declaration, which is still more than it said before.
    ///
    /// **This informs; it does not unlock anything.** The drop is still refused
    /// by default, and nothing here creates a path to replace the declaration.
    pub diff: Vec<String>,
}

/// What the colliding name is already declared as, and how the dropped bytes
/// differ from the bytes approved under it.
///
/// Read-only and best-effort in both halves: a manifest without the entry, an
/// unlocked declaration, or a missing snapshot each degrade to a less specific
/// answer rather than failing the scan. Intake runs on every ordinary command,
/// so nothing here may bail — and repository content is hostile input, so the
/// diff comes back through the same capped, sanitized renderer the re-gate
/// card uses.
fn collision_detail(
    kind: Kind,
    manifest: &crate::manifest::Manifest,
    dir: &Path,
    name: &str,
    dropped: &Path,
) -> (String, Vec<String>) {
    let declared_as = match kind {
        Kind::Skill => match manifest.skills.get(name) {
            Some(s) => match (&s.git, &s.rev, &s.path) {
                (Some(url), rev, _) => {
                    let short = rev
                        .as_deref()
                        .map(|r| r[..r.len().min(7)].to_string())
                        .unwrap_or_else(|| "unpinned".to_string());
                    format!("git@{short} ({url})")
                }
                (None, _, Some(p)) => format!("path {p}"),
                _ => "a library skill".to_string(),
            },
            None => "already declared".to_string(),
        },
        Kind::Instruction => match manifest.instructions.get(name) {
            Some(i) => crate::instructions::declared_label(name, i),
            None => "already declared".to_string(),
        },
    };

    // The approved bytes, if this declaration was ever pinned. Without a lock
    // entry there is no snapshot to compare against and the refusal simply
    // names the declaration.
    let Ok(lock) = crate::lock::Lock::load(dir) else {
        return (declared_as, Vec::new());
    };
    let pin = match kind {
        Kind::Skill => lock.get(name).map(|e| e.checksum.hex().to_string()),
        Kind::Instruction => lock
            .get_instruction(name)
            .map(|e| e.checksum.hex().to_string()),
    };
    let Some(pin) = pin else {
        return (declared_as, Vec::new());
    };
    let store = crate::store::Store::default_store();
    // An instruction is a single file; the snapshot layout is a directory, so
    // compare the dropped file against the snapshot's directory either way —
    // `diff_against_pin` walks both sides and is tolerant of shape.
    let diff = crate::regate::diff_against_pin(store.root(), &pin, dropped);
    let lines = crate::regate::render_lines(&diff, crate::regate::DIFF_LINE_CAP);
    // `NoSnapshot` renders its own honest sentence; keep it, it is true here
    // too — we know what it is declared as but not what its bytes were.
    (declared_as, lines)
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
    // The intake directory itself — and every ancestor up to the manifest dir —
    // must be real. A repo shipping `.agentstack/skills -> /elsewhere` would
    // otherwise have its "dropped files" read from outside the project entirely,
    // and per-entry symlink checks never see it: the escape happened above them.
    // Canonicalizing both sides catches the root being a link, an ancestor being
    // one, and `..` segments, in one comparison.
    if !contained_in(dir, &root) {
        return;
    }
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
            let (declared_as, diff) = collision_detail(kind, manifest, dir, &name, &content);
            out.collisions.push(Collision {
                kind,
                name,
                rel_path: rel,
                declared_as,
                diff,
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

/// Whether `path` really lives under `parent` once every link and `..` in both
/// is resolved. Used to confirm an intake directory is inside the project
/// rather than a doorway out of it.
fn contained_in(parent: &Path, path: &Path) -> bool {
    let (Ok(parent), Ok(path)) = (std::fs::canonicalize(parent), std::fs::canonicalize(path))
    else {
        return false;
    };
    path.starts_with(&parent)
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
        // Every body a fragment declares, base and per-(CLI, model) variants
        // alike: a variant body IS declared content, and leaving it out of this
        // set made the funnel report a declared variant as a stray drop.
        Kind::Instruction => manifest
            .instructions
            .values()
            .flat_map(|i| {
                i.path
                    .iter()
                    .chain(i.variants.iter().map(|v| &v.path))
                    .map(|p| anchor(p))
                    .collect::<Vec<_>>()
            })
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

/// The provenance clock: what arrived through governed intake, plus git
/// tracking. (The mtime-vs-last-grant fallback is gone — see `classify`.)
struct ProvenanceClock {
    /// Every path git tracks under the intake directories, canonicalized.
    /// `None` when `base` is not a git work tree at all.
    ///
    /// Collected in ONE git spawn rather than one per item: `status` is on this
    /// path and is required to feel instant.
    tracked: Option<Vec<PathBuf>>,
    /// Digests of content that arrived through `receive` / `add from` on this
    /// machine (F3). Checked FIRST: received bytes are usually untracked in
    /// git — that is precisely how they laundered into "your own work".
    received: std::collections::HashSet<String>,
}

impl ProvenanceClock {
    fn for_project(base: &Path) -> Self {
        Self {
            tracked: tracked_under_intake(base),
            received: received_digests(),
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
        // Received content first (F3): bytes that arrived through `receive` /
        // `add from` are a stranger's work whatever git says about them —
        // adopt lands them untracked, which is exactly how they used to earn
        // the "your own work" label and the compressed path.
        if !self.received.is_empty() && subtree_has_received(path, &self.received) {
            return Provenance::Arrived("arrived through receive/add from");
        }
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
        // Outside a git work tree there is no authorship signal a hostile
        // process cannot forge. The old fallback promoted anything whose
        // mtime postdated the last grant — and `touch` is free to any process
        // with filesystem access (F3), while a failed git query silently
        // landed here too (F17). No signal means no compression: the content
        // still flows, it just takes the full staged review.
        Provenance::Arrived("no git history to attest who authored this")
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

/// The machine-local ledger of content digests that arrived through
/// governed intake (`receive` / `add from`) — the F3 wire. Lives under
/// `AGENTSTACK_HOME`, never inside a project: repository content is hostile
/// input, and a ledger a clone could ship would let a repo relabel things.
/// (A repo APPENDING to it could only downgrade labels toward the full
/// review — the safe direction — but it cannot, because it never leaves the
/// user's home.) Digest-keyed so a rename cannot launder received bytes.
fn received_path() -> PathBuf {
    crate::util::paths::agentstack_home().join("received.jsonl")
}

/// Append one received file's digest. Best-effort by design: a failed append
/// must never fail an adopt — it only costs the label its extra precision,
/// and the content still takes the full review by every other rule.
pub fn record_received(bytes: &[u8]) {
    use std::io::Write;
    let path = received_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let line = format!(
        "{{\"sha256\":\"{}\"}}\n",
        agentstack_core::digest::sha256_hex(bytes)
    );
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| f.write_all(line.as_bytes()));
}

/// Every digest the ledger holds. Malformed lines are skipped — the ledger is
/// our own machine-local file, but parsing it defensively costs nothing.
fn received_digests() -> std::collections::HashSet<String> {
    let Ok(text) = std::fs::read_to_string(received_path()) else {
        return Default::default();
    };
    text.lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter_map(|v| v.get("sha256").and_then(|s| s.as_str()).map(str::to_string))
        .collect()
}

/// Does any regular file under `path` (or `path` itself) hash to a received
/// digest? Bounded like every other read of dropped content, symlinks never
/// followed.
fn subtree_has_received(path: &Path, received: &std::collections::HashSet<String>) -> bool {
    fn walk(path: &Path, received: &std::collections::HashSet<String>, budget: &mut usize) -> bool {
        if *budget == 0 {
            return false;
        }
        *budget -= 1;
        let Ok(meta) = path.symlink_metadata() else {
            return false;
        };
        if meta.file_type().is_symlink() {
            return false;
        }
        if meta.is_file() {
            return std::fs::read(path)
                .map(|b| received.contains(&agentstack_core::digest::sha256_hex(&b)))
                .unwrap_or(false);
        }
        if meta.is_dir() {
            if let Ok(rd) = std::fs::read_dir(path) {
                for e in rd.flatten() {
                    if walk(&e.path(), received, budget) {
                        return true;
                    }
                }
            }
        }
        false
    }
    let mut budget = MAX_ENTRIES;
    walk(path, received, &mut budget)
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
        "{} not in your setup yet ({listed}) — `agentstack yes` to review and take live",
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
                "{} '{}' in {} is not adopted — that name is declared as {}; the drop would replace it",
                c.kind.noun(),
                crate::text::sanitize_line(&c.name),
                crate::text::sanitize_line(&c.rel_path),
                crate::text::sanitize_line(&c.declared_as)
            )
            .dimmed()
        );
        for line in &c.diff {
            println!("    {}", line.dimmed());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    /// F3 witness: outside a git work tree there is NO compressed path — the
    /// old mtime-vs-last-grant fallback promoted anything a hostile process
    /// could `touch`. The tamper here is the timestamp itself: a fresh mtime
    /// (the forgeable signal) must no longer buy the "your own work" label.
    #[test]
    fn without_git_a_fresh_mtime_never_reads_as_local_work() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let dir = tmp.path();
        drop_skill(dir, "fresh", "# Fresh\n");

        let clock = ProvenanceClock {
            tracked: None,
            received: Default::default(),
        };
        let p = clock.classify(dir, &dir.join("skills/fresh"));
        assert!(
            !p.is_local(),
            "a forgeable mtime bought the compressed path: {p:?}"
        );
        assert_eq!(p.reason(), "no git history to attest who authored this");
    }

    /// F3 witness: bytes recorded as received are a stranger's work whatever
    /// git would say — the tamper is the laundering route itself (adopt lands
    /// received files untracked, which used to read as "your own work").
    #[test]
    fn received_bytes_never_read_as_local_work() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let dir = tmp.path();
        drop_skill(dir, "gift", "# From a stranger\n");

        let mut received = std::collections::HashSet::new();
        received.insert(agentstack_core::digest::sha256_hex(b"# From a stranger\n"));
        // `tracked: Some(vec![])` = a git work tree that tracks nothing here,
        // i.e. the exact state in which the file would read "untracked in
        // git" and take the compressed path.
        let clock = ProvenanceClock {
            tracked: Some(Vec::new()),
            received,
        };
        let p = clock.classify(dir, &dir.join("skills/gift"));
        assert!(
            !p.is_local(),
            "received bytes laundered to local work: {p:?}"
        );
        assert_eq!(p.reason(), "arrived through receive/add from");
    }

    #[test]
    fn no_git_means_full_review() {
        let tmp = assert_fs::TempDir::new().unwrap();
        drop_skill(tmp.path(), "fresh", "# Fresh\n");
        let clock = ProvenanceClock {
            tracked: None,
            received: Default::default(),
        };
        let p = clock.classify(tmp.path(), &tmp.path().join("skills/fresh"));
        assert!(!p.is_local());
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
}
