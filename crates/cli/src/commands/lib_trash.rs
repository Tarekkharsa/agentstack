//! The central library's trash (`~/.agentstack/lib/.trash/`).
//!
//! Removing a capability from the library used to delete its body outright.
//! People curate a library the way they curate a toolbox — dropping a skill or
//! an MCP server is routine — so a removal must be recoverable. Every
//! `lib remove*` path now *moves* the body here and records the dropped
//! `library.toml` entry beside it, and [`restore`] puts both back.
//!
//! Layout — one directory per removal:
//!
//! ```text
//! ~/.agentstack/lib/.trash/
//!   skill-pdf-1753574400/
//!     entry.toml        the TrashRecord: kind, name, when, and the index entry
//!     body/             the moved `lib/skills/pdf` directory   (dir bodies)
//!     body.toml         the moved `lib/servers/github.toml`    (file bodies)
//! ```
//!
//! The trash is machine-local: `lib sync` gitignores it, so a removal on one
//! machine does not ship a resurrection copy to another.
//!
//! Nothing here resolves, renders, or executes anything — it moves bytes and
//! rewrites the index. Trashed content is inert: it is out of `library.toml`,
//! so no resolver, render, or agent path can reach it until it is restored.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::library::{Library, LibraryExtension, LibraryHook, LibraryServer, LibrarySkill};

/// Directory name under `lib_home` holding the trash. Dot-prefixed so it never
/// collides with a capability kind directory (`skills/`, `servers/`, …).
pub const TRASH_DIR: &str = ".trash";
/// The per-removal metadata file.
pub const RECORD_FILE: &str = "entry.toml";
/// Schema version for [`TrashRecord`]. Bumped only on a breaking shape change;
/// [`list`] skips records it cannot read rather than failing the whole listing.
pub const RECORD_VERSION: u32 = 1;
/// The moved body's name inside a record directory: a directory body keeps
/// `body/`, a single-file body keeps `body.toml`.
const BODY_DIR: &str = "body";
const BODY_FILE: &str = "body.toml";

/// Which library collection a trashed entry came from. The string form is what
/// lands in `entry.toml` and in every user-facing line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Skill,
    Server,
    Extension,
    Hook,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Skill => "skill",
            Kind::Server => "server",
            Kind::Extension => "extension",
            Kind::Hook => "hook",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        match s {
            "skill" => Some(Kind::Skill),
            "server" => Some(Kind::Server),
            "extension" => Some(Kind::Extension),
            "hook" => Some(Kind::Hook),
            _ => None,
        }
    }
}

/// One removal's metadata: enough to put the entry back exactly as it was.
///
/// The index entry rides in the kind-specific field (`skill`/`server`/…) rather
/// than an enum, so a record written by a build that knows one more kind still
/// parses here — the unknown table is simply absent from every field we read.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrashRecord {
    pub version: u32,
    /// `skill` | `server` | `extension` | `hook`.
    pub kind: String,
    /// The library name the entry had (and will have again on restore).
    pub name: String,
    /// Unix seconds at removal.
    pub removed_at: u64,
    /// `body` (a moved directory) or `body.toml` (a moved file); absent when
    /// the entry had no local body (a git-backed skill references the shared
    /// store, which removal never touches).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// Where the body came from, absolute at removal time — shown by `lib trash`
    /// so a restore's destination is legible before it runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_origin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill: Option<LibrarySkill>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<LibraryServer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extension: Option<LibraryExtension>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hook: Option<LibraryHook>,
}

impl TrashRecord {
    pub fn kind(&self) -> Option<Kind> {
        Kind::parse(&self.kind)
    }
}

/// A trash entry as listed: its id (the directory name), where it lives, and
/// the parsed record.
#[derive(Debug, Clone)]
pub struct TrashedEntry {
    pub id: String,
    pub dir: PathBuf,
    pub record: TrashRecord,
}

impl TrashedEntry {
    /// The moved body inside this record directory, if it has one.
    pub fn body_path(&self) -> Option<PathBuf> {
        self.record.body.as_deref().map(|b| self.dir.join(b))
    }
}

/// `<lib_home>/.trash`.
pub fn trash_home(lib_home: &Path) -> PathBuf {
    lib_home.join(TRASH_DIR)
}

/// Unix seconds now. A clock before the epoch yields 0 rather than panicking —
/// the timestamp is for ordering and display, never for a security decision.
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A filesystem-safe slug for a library name. Library names are already
/// constrained, but the index is a hand-editable file: anything outside
/// `[A-Za-z0-9._-]` becomes `_` so a crafted name can never introduce a path
/// component (`/`, `..`) into the trash id.
fn slug(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    // Collapse any `..` the mapping left behind (a name like `../x` becomes
    // `_.._x`), so the id reads as one plain component everywhere it is shown.
    // Traversal is already impossible — `/` and `\` are gone — this keeps the
    // string honest.
    let mut s = s.trim_matches('.').to_string();
    while s.contains("..") {
        s = s.replace("..", "_");
    }
    if s.is_empty() {
        "entry".to_string()
    } else {
        s
    }
}

/// The id a removal *would* get: `<kind>-<slug>-<unix seconds>`, with a `-2`,
/// `-3`, … suffix if that directory already exists (two removals of the same
/// name within one second).
fn allocate_id(lib_home: &Path, kind: Kind, name: &str, at: u64) -> String {
    let base = format!("{}-{}-{}", kind.as_str(), slug(name), at);
    let home = trash_home(lib_home);
    if !home.join(&base).exists() {
        return base;
    }
    for n in 2..1000 {
        let candidate = format!("{base}-{n}");
        if !home.join(&candidate).exists() {
            return candidate;
        }
    }
    format!("{base}-{}", now_secs())
}

/// What a removal will put in the trash, without doing it — the preview half,
/// so `--write`-less runs and the panel's consent card can name the destination.
pub fn plan_destination(lib_home: &Path, kind: Kind, name: &str) -> PathBuf {
    trash_home(lib_home).join(allocate_id(lib_home, kind, name, now_secs()))
}

/// Move `body` (a directory or a file, may be `None`) plus `record` into the
/// trash and return the created entry. The caller drops the `library.toml`
/// entry itself — this module owns the trash, not the index.
///
/// Moves are `rename` first, with a copy-then-delete fallback for the
/// cross-device case (a library home on a different volume than its bodies).
pub fn stash(
    lib_home: &Path,
    kind: Kind,
    name: &str,
    body: Option<&Path>,
    mut record: TrashRecord,
) -> Result<TrashedEntry> {
    let at = now_secs();
    let id = allocate_id(lib_home, kind, name, at);
    let dir = trash_home(lib_home).join(&id);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating trash directory {}", dir.display()))?;

    record.version = RECORD_VERSION;
    record.kind = kind.as_str().to_string();
    record.name = name.to_string();
    record.removed_at = at;
    record.body = None;
    record.body_origin = None;

    if let Some(src) = body {
        if src.exists() {
            let is_dir = src.is_dir();
            let dest = dir.join(if is_dir { BODY_DIR } else { BODY_FILE });
            move_path(src, &dest)?;
            record.body = Some(if is_dir { BODY_DIR } else { BODY_FILE }.to_string());
            record.body_origin = Some(src.display().to_string());
        }
    }

    let text = toml::to_string_pretty(&record).context("serializing the trash record")?;
    crate::util::atomic::write(&dir.join(RECORD_FILE), &text)
        .with_context(|| format!("writing {}", dir.join(RECORD_FILE).display()))?;

    Ok(TrashedEntry { id, dir, record })
}

/// Move a file or directory, falling back to copy + delete when `rename` cannot
/// cross a device boundary.
fn move_path(src: &Path, dest: &Path) -> Result<()> {
    if std::fs::rename(src, dest).is_ok() {
        return Ok(());
    }
    if src.is_dir() {
        crate::util::fsx::copy_dir_all_following_symlinks(src, dest)
            .with_context(|| format!("copying {} to {}", src.display(), dest.display()))?;
        std::fs::remove_dir_all(src).with_context(|| format!("removing {}", src.display()))?;
    } else {
        std::fs::copy(src, dest)
            .with_context(|| format!("copying {} to {}", src.display(), dest.display()))?;
        std::fs::remove_file(src).with_context(|| format!("removing {}", src.display()))?;
    }
    Ok(())
}

/// Every readable trash entry, newest first. A directory without a parseable
/// `entry.toml` (hand-edited, half-written, or from a future schema) is skipped
/// rather than failing the listing — the rest of the trash stays usable.
pub fn list(lib_home: &Path) -> Result<Vec<TrashedEntry>> {
    let home = trash_home(lib_home);
    let Ok(read) = std::fs::read_dir(&home) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for entry in read.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(dir.join(RECORD_FILE)) else {
            continue;
        };
        let Ok(record) = toml::from_str::<TrashRecord>(&text) else {
            continue;
        };
        if record.version > RECORD_VERSION {
            continue;
        }
        let id = entry.file_name().to_string_lossy().to_string();
        out.push(TrashedEntry { id, dir, record });
    }
    out.sort_by(|a, b| {
        b.record
            .removed_at
            .cmp(&a.record.removed_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(out)
}

/// Look one entry up by its exact id.
pub fn get(lib_home: &Path, id: &str) -> Result<TrashedEntry> {
    list(lib_home)?
        .into_iter()
        .find(|e| e.id == id)
        .with_context(|| format!("no trash entry '{id}' — run `agentstack lib trash` to list them"))
}

/// What a restore did (or, on a dry run, would do).
#[derive(Debug)]
pub struct RestoreOutcome {
    pub id: String,
    pub kind: String,
    pub name: String,
    /// Where the body goes back to (`None` when the entry had no local body).
    pub body_dest: Option<PathBuf>,
    pub written: bool,
}

/// Put a trashed entry back: its `library.toml` row and, if it had one, its
/// body at the original location. Refuses when the name is taken again (a
/// re-add after the removal) unless `replace` — restoring must never silently
/// overwrite a newer capability with the same name.
pub fn restore(lib_home: &Path, id: &str, replace: bool, write: bool) -> Result<RestoreOutcome> {
    let entry = get(lib_home, id)?;
    let kind = entry
        .record
        .kind()
        .with_context(|| format!("trash entry '{id}' has an unknown kind"))?;
    let name = entry.record.name.clone();

    let mut library = Library::load(lib_home)?;
    let taken = match kind {
        Kind::Skill => library.get(&name).is_some(),
        Kind::Server => library.get_server(&name).is_some(),
        Kind::Extension => library.get_extension(&name).is_some(),
        Kind::Hook => library.get_hook(&name).is_some(),
    };
    if taken && !replace {
        bail!(
            "'{name}' is already in the central library again — pass --replace to overwrite it \
             with the trashed copy"
        );
    }

    // Where the body belongs. Derived from the *kind*, not from the recorded
    // origin path, so a hand-edited record can never write outside the library.
    let body_dest = entry.body_path().map(|_| match kind {
        Kind::Skill => lib_home.join("skills").join(&name),
        Kind::Extension => lib_home.join("extensions").join(&name),
        Kind::Server => lib_home.join("servers").join(format!("{name}.toml")),
        Kind::Hook => lib_home.join("hooks").join(format!("{name}.toml")),
    });

    if write {
        if let (Some(src), Some(dest)) = (entry.body_path(), body_dest.as_ref()) {
            if dest.exists() {
                if !replace {
                    bail!(
                        "{} already exists — pass --replace to overwrite it",
                        dest.display()
                    );
                }
                if dest.is_dir() {
                    std::fs::remove_dir_all(dest)
                        .with_context(|| format!("removing {}", dest.display()))?;
                } else {
                    std::fs::remove_file(dest)
                        .with_context(|| format!("removing {}", dest.display()))?;
                }
            }
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            move_path(&src, dest)?;
        }

        match kind {
            Kind::Skill => {
                let e = entry
                    .record
                    .skill
                    .clone()
                    .context("trash entry has no skill index row")?;
                library.upsert(e);
            }
            Kind::Server => {
                let e = entry
                    .record
                    .server
                    .clone()
                    .context("trash entry has no server index row")?;
                library.upsert_server(e);
            }
            Kind::Extension => {
                let e = entry
                    .record
                    .extension
                    .clone()
                    .context("trash entry has no extension index row")?;
                library.upsert_extension(e);
            }
            Kind::Hook => {
                let e = entry
                    .record
                    .hook
                    .clone()
                    .context("trash entry has no hook index row")?;
                library.upsert_hook(e);
            }
        }
        library.save(lib_home)?;
        // The record directory is now empty of meaning — drop it so a restored
        // entry cannot be restored twice.
        std::fs::remove_dir_all(&entry.dir)
            .with_context(|| format!("removing {}", entry.dir.display()))?;
    }

    Ok(RestoreOutcome {
        id: entry.id,
        kind: kind.as_str().to_string(),
        name,
        body_dest,
        written: write,
    })
}

/// What an empty did (or would do).
#[derive(Debug)]
pub struct EmptyOutcome {
    /// The entries deleted (or that would be).
    pub entries: Vec<TrashedEntry>,
    pub written: bool,
}

/// Permanently delete one trash entry (`id = Some`) or the whole trash. This is
/// the only path in the library that destroys content — it deletes exclusively
/// inside `<lib_home>/.trash/<id>`, never a live capability.
pub fn empty(lib_home: &Path, id: Option<&str>, write: bool) -> Result<EmptyOutcome> {
    let entries = match id {
        Some(id) => vec![get(lib_home, id)?],
        None => list(lib_home)?,
    };
    if write {
        for e in &entries {
            std::fs::remove_dir_all(&e.dir)
                .with_context(|| format!("removing {}", e.dir.display()))?;
        }
    }
    Ok(EmptyOutcome {
        entries,
        written: write,
    })
}

/// A blank record for `stash` to fill in — callers set only the kind-specific
/// index row.
pub fn blank_record() -> TrashRecord {
    TrashRecord {
        version: RECORD_VERSION,
        kind: String::new(),
        name: String::new(),
        removed_at: 0,
        body: None,
        body_origin: None,
        skill: None,
        server: None,
        extension: None,
        hook: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill(name: &str) -> LibrarySkill {
        LibrarySkill {
            name: name.to_string(),
            source: "path".into(),
            path: Some(name.to_string()),
            git: None,
            rev: None,
            subpath: None,
            checksum: None,
            version: None,
            provenance: Some("manual".into()),
        }
    }

    /// A stash moves the body (it is gone from its old home, present in the
    /// trash) and records the index row alongside it.
    #[test]
    fn stash_moves_the_body_and_records_the_entry() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let lib = tmp.path();
        let body = lib.join("skills").join("pdf");
        std::fs::create_dir_all(&body).unwrap();
        std::fs::write(body.join("SKILL.md"), "# pdf").unwrap();

        let mut record = blank_record();
        record.skill = Some(skill("pdf"));
        let entry = stash(lib, Kind::Skill, "pdf", Some(&body), record).unwrap();

        assert!(!body.exists(), "the body left its old home");
        assert!(entry.dir.join("body/SKILL.md").exists(), "body moved in");
        assert!(entry.dir.join(RECORD_FILE).exists());
        assert_eq!(entry.record.name, "pdf");
        assert_eq!(entry.record.body.as_deref(), Some("body"));
    }

    /// Restore is the exact inverse: the row is back in `library.toml`, the body
    /// is back under `lib/skills/<name>`, and the trash entry is consumed.
    #[test]
    fn restore_puts_the_entry_and_body_back() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let lib = tmp.path();
        let body = lib.join("skills").join("pdf");
        std::fs::create_dir_all(&body).unwrap();
        std::fs::write(body.join("SKILL.md"), "# pdf").unwrap();
        Library::default().save(lib).unwrap();

        let mut record = blank_record();
        record.skill = Some(skill("pdf"));
        let entry = stash(lib, Kind::Skill, "pdf", Some(&body), record).unwrap();

        let outcome = restore(lib, &entry.id, false, true).unwrap();
        assert!(outcome.written);
        assert!(body.join("SKILL.md").exists(), "body restored");
        assert!(!entry.dir.exists(), "trash entry consumed");
        assert!(Library::load(lib).unwrap().get("pdf").is_some());
    }

    /// A restore never overwrites a live capability that took the name back.
    #[test]
    fn restore_refuses_when_the_name_is_taken_again() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let lib = tmp.path();
        let mut record = blank_record();
        record.skill = Some(skill("pdf"));
        let entry = stash(lib, Kind::Skill, "pdf", None, record).unwrap();

        let mut library = Library::default();
        library.upsert(skill("pdf"));
        library.save(lib).unwrap();

        let err = restore(lib, &entry.id, false, false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("--replace"), "{err}");
    }

    /// A dry run mutates nothing — neither the index nor the trash.
    #[test]
    fn restore_dry_run_changes_nothing() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let lib = tmp.path();
        Library::default().save(lib).unwrap();
        let mut record = blank_record();
        record.skill = Some(skill("pdf"));
        let entry = stash(lib, Kind::Skill, "pdf", None, record).unwrap();

        let outcome = restore(lib, &entry.id, false, false).unwrap();
        assert!(!outcome.written);
        assert!(entry.dir.exists(), "trash entry still there");
        assert!(Library::load(lib).unwrap().get("pdf").is_none());
    }

    /// A name that tries to escape its directory is slugged, so the trash id
    /// stays a single path component.
    #[test]
    fn ids_never_carry_a_path_component() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let lib = tmp.path();
        let mut record = blank_record();
        record.server = Some(LibraryServer {
            name: "../../etc/x".into(),
            checksum: None,
            version: None,
            provenance: None,
        });
        let entry = stash(lib, Kind::Server, "../../etc/x", None, record).unwrap();

        assert!(!entry.id.contains('/'), "{}", entry.id);
        assert!(!entry.id.contains(".."), "{}", entry.id);
        assert!(entry.dir.starts_with(trash_home(lib)));
        assert!(trash_home(lib).is_dir(), "everything stayed under .trash");
    }

    /// Emptying deletes trash content only.
    #[test]
    fn empty_removes_trash_entries() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let lib = tmp.path();
        let mut record = blank_record();
        record.skill = Some(skill("pdf"));
        let entry = stash(lib, Kind::Skill, "pdf", None, record).unwrap();

        let preview = empty(lib, None, false).unwrap();
        assert_eq!(preview.entries.len(), 1);
        assert!(entry.dir.exists(), "dry run kept it");

        let done = empty(lib, None, true).unwrap();
        assert_eq!(done.entries.len(), 1);
        assert!(!entry.dir.exists());
        assert!(list(lib).unwrap().is_empty());
    }
}
