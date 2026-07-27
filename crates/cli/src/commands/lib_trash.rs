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
//!
//! **`entry.toml` is hostile input** (review finding F22). It is machine-local
//! state, not repository content, so a bad record is not a privilege boundary
//! being crossed — but it is still a file a hand edit, a half-written removal,
//! or a future schema can shape, and rule 7 applies to all of it. [`restore`]
//! therefore validates the *whole* record — name is a plain component, body is
//! one of the two known slots, the index row exists and agrees with the name,
//! and the derived destination canonically resolves inside `lib_home` — before
//! it moves a single byte. An earlier version derived the destination from the
//! kind and treated that as containment; the name was joined on verbatim, and
//! a record named `../../../escaped` restored outside the library.
//!
//! The mutations are ordered so every one of them can be walked back: the body
//! moves first (undoable by moving it back), anything it replaces is set aside
//! rather than deleted, and the index is saved last. A failure at any point
//! leaves the trash entry intact and the library as it was.

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

/// Reject a name that is not a single plain path component.
///
/// `restore` derives its destination from the *kind* and this name. Deriving
/// the directory from the kind was assumed to make the destination safe, but
/// the name is joined onto it verbatim, so a record naming `../../../escaped`
/// put the body outside `lib/` entirely (review finding F22). `entry.toml` is
/// machine-local state a hand edit or a half-written record can shape, which
/// rule 7 says to parse defensively — so the name is checked, not trusted.
///
/// The rule is about *components*, not substrings. `..` is rejected because it
/// is the parent-directory component; a name like `my..skill` contains those
/// two characters but is one ordinary component and a legal library name, so
/// rejecting it would break real names for no safety gain. Backslash is
/// rejected explicitly: on Unix it is an ordinary character that
/// [`Path::components`] would happily fold into one component, but these names
/// also become Windows paths.
pub fn is_plain_component(name: &str) -> bool {
    use std::path::Component;
    if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains('\0') {
        return false;
    }
    let mut components = Path::new(name).components();
    // Exactly one component, and it must be a Normal one — `.` parses as
    // CurDir and `..` as ParentDir, so both fall out here without a string
    // comparison.
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

fn plain_component(name: &str, what: &str) -> Result<()> {
    if !is_plain_component(name) {
        bail!(
            "refusing to restore: the trash record's {what} '{name}' is not a plain name — \
             it must be one path component: no '/', no '\\', and not '.' or '..'. This record \
             was not written by `lib remove` (or was edited by hand); delete it with \
             `agentstack lib trash --empty <id>`"
        );
    }
    Ok(())
}

/// Confirm `dest` really lands inside `root` after symlinks are resolved.
///
/// [`plain_component`] rejects traversal in the name; this catches the other
/// route — a `lib/skills` that is (or contains) a symlink pointing elsewhere,
/// where a perfectly plain name still resolves outside the library. Belt and
/// braces on purpose: the containment claim in this module's docs should hold
/// because it is checked, not because the inputs looked well-formed.
fn contained(root: &Path, dest: &Path) -> Result<()> {
    // The destination itself does not exist yet; canonicalize the nearest
    // existing ancestor and re-append what is missing.
    let mut existing = dest.to_path_buf();
    let mut rest = Vec::new();
    while !existing.exists() {
        let Some(parent) = existing.parent().map(|p| p.to_path_buf()) else {
            break;
        };
        if let Some(name) = existing.file_name() {
            rest.push(name.to_os_string());
        }
        existing = parent;
    }
    let base = existing
        .canonicalize()
        .with_context(|| format!("resolving {}", existing.display()))?;
    let mut resolved = base;
    for part in rest.into_iter().rev() {
        resolved.push(part);
    }
    let root = root
        .canonicalize()
        .with_context(|| format!("resolving {}", root.display()))?;
    if !resolved.starts_with(&root) {
        bail!(
            "refusing to restore: {} resolves to {}, which is outside the library at {} — \
             the trash can only ever put bytes back inside the library",
            dest.display(),
            resolved.display(),
            root.display()
        );
    }
    Ok(())
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

    // Deliberately NOT refused here on an unsafe name. Removing a corrupt
    // index entry is a cleanup path that has to keep working, and such an
    // entry never has a body to move — `valid_lib_name` already declines to
    // target a file for it, so nothing leaves the library either way. The
    // record it leaves is inert and `restore` refuses it by name with a
    // pointer at `lib trash --empty`.
    //
    // Describe the move before making it. The record is written first, with
    // the body field already set, so a failure mid-move leaves a record that
    // names a body — recoverable and visible in `lib trash` — instead of a
    // moved body with no record at all, which `list` would skip and no path
    // could reach (review finding F22).
    let mut moved: Option<(PathBuf, PathBuf)> = None;
    if let Some(src) = body {
        if src.exists() {
            let is_dir = src.is_dir();
            let slot = if is_dir { BODY_DIR } else { BODY_FILE };
            record.body = Some(slot.to_string());
            record.body_origin = Some(src.display().to_string());
            moved = Some((src.to_path_buf(), dir.join(slot)));
        }
    }

    let text = toml::to_string_pretty(&record).context("serializing the trash record")?;
    crate::util::atomic::write(&dir.join(RECORD_FILE), &text)
        .with_context(|| format!("writing {}", dir.join(RECORD_FILE).display()))?;

    if let Some((src, dest)) = &moved {
        if let Err(e) = move_path(src, dest) {
            // Nothing moved, so the record describes a body that is not there.
            // Drop the whole record directory rather than leave that behind.
            let _ = std::fs::remove_dir_all(&dir);
            return Err(e);
        }
    }

    Ok(TrashedEntry { id, dir, record })
}

/// Undo a [`stash`] that has not been committed to by an index save.
///
/// A removal is two steps — move the body here, then drop the index row and
/// save. If the save fails, the body has already moved and the index still
/// names it: the capability is now invisible to the library and present only
/// in the trash, which is neither removed nor intact. This puts it back.
///
/// Best-effort by construction: it runs on an error path, and the error that
/// sent us here is the one worth reporting. It returns whether the body made
/// it home so the caller can say which of the two situations the user is in.
pub fn unstash(entry: &TrashedEntry, origin: Option<&Path>) -> bool {
    let restored = match (entry.body_path(), origin) {
        (Some(src), Some(dest)) if src.exists() => move_path(&src, dest).is_ok(),
        // Nothing was moved, so there is nothing to put back.
        _ => true,
    };
    if restored {
        let _ = std::fs::remove_dir_all(&entry.dir);
    }
    restored
}

/// Move whatever currently occupies `dest` aside, into `staging`, and return
/// where it went (`None` when there was nothing there).
///
/// `--replace` used to `remove_dir_all` the live entry before moving the
/// trashed one in, which made a failure halfway through destroy the copy the
/// user already had. Setting it aside instead means the mutation is undoable
/// right up until the index save succeeds.
fn displace_existing(dest: &Path, staging: &Path) -> Result<Option<PathBuf>> {
    if !dest.exists() {
        return Ok(None);
    }
    let aside = staging.join("replaced");
    if aside.exists() {
        let _ = std::fs::remove_dir_all(&aside);
        let _ = std::fs::remove_file(&aside);
    }
    move_path(dest, &aside)
        .with_context(|| format!("setting {} aside before restoring over it", dest.display()))?;
    Ok(Some(aside))
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

    // ── validate the WHOLE record before touching anything ───────────────
    // Every check that can refuse this restore runs here, before the first
    // byte moves. It used to be interleaved with the mutation — the body was
    // moved, and only then was the index row looked for — so a record missing
    // its row failed with the body already gone from the trash and absent from
    // the index (review finding F22).
    let kind = entry
        .record
        .kind()
        .with_context(|| format!("trash entry '{id}' has an unknown kind"))?;
    let name = entry.record.name.clone();
    plain_component(&name, "name")?;

    // The body field names a fixed location inside the record directory. It is
    // an enum written by `stash`, not a path — treat anything else as a record
    // we did not write.
    if let Some(b) = entry.record.body.as_deref() {
        if b != BODY_DIR && b != BODY_FILE {
            bail!(
                "refusing to restore: the trash record's body '{b}' is not one of '{BODY_DIR}' \
                 or '{BODY_FILE}' — this record was not written by `lib remove`"
            );
        }
    }

    // Take the index row now, and require it to agree with the record's name.
    // A row naming something else would restore the body under one name and
    // the index entry under another.
    let row_name = match kind {
        Kind::Skill => entry.record.skill.as_ref().map(|e| e.name.clone()),
        Kind::Server => entry.record.server.as_ref().map(|e| e.name.clone()),
        Kind::Extension => entry.record.extension.as_ref().map(|e| e.name.clone()),
        Kind::Hook => entry.record.hook.as_ref().map(|e| e.name.clone()),
    }
    .with_context(|| {
        format!(
            "trash entry '{id}' has no {} index row — it cannot be put back",
            kind.as_str()
        )
    })?;
    if row_name != name {
        bail!(
            "refusing to restore: the record names '{name}' but its {} index row names \
             '{row_name}' — a restore must put one capability back under one name",
            kind.as_str()
        );
    }

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

    // Where the body belongs: derived from the kind, with a validated name,
    // and then CHECKED to land inside the library. The derivation alone was
    // the old containment argument and it was not sufficient.
    let body_dest = entry.body_path().map(|_| match kind {
        Kind::Skill => lib_home.join("skills").join(&name),
        Kind::Extension => lib_home.join("extensions").join(&name),
        Kind::Server => lib_home.join("servers").join(format!("{name}.toml")),
        Kind::Hook => lib_home.join("hooks").join(format!("{name}.toml")),
    });
    if let Some(dest) = body_dest.as_ref() {
        contained(lib_home, dest)?;
        if dest.is_symlink() {
            bail!(
                "refusing to restore: {} is a symlink — restoring would write through it",
                dest.display()
            );
        }
        if dest.exists() && !replace {
            bail!(
                "{} already exists — pass --replace to overwrite it",
                dest.display()
            );
        }
    }

    // Stage the index change in memory. Nothing is on disk yet, so a failure
    // here is free.
    match kind {
        Kind::Skill => library.upsert(entry.record.skill.clone().expect("checked above")),
        Kind::Server => library.upsert_server(entry.record.server.clone().expect("checked above")),
        Kind::Extension => {
            library.upsert_extension(entry.record.extension.clone().expect("checked above"))
        }
        Kind::Hook => library.upsert_hook(entry.record.hook.clone().expect("checked above")),
    }

    // The record says it has a body; the body must actually be there. Checked
    // with the rest of the validation, BEFORE anything is displaced — a record
    // pointing at a body that is gone used to set the live entry aside first
    // and only then discover it had nothing to put in its place, stranding the
    // user's copy under `.trash/<id>/replaced`.
    if let Some(src) = entry.body_path() {
        if !src.exists() {
            bail!(
                "refusing to restore: the record names a body at {} but it is not there — \
                 the trash entry is incomplete. Inspect it, or drop it with \
                 `agentstack lib trash --empty {id}`",
                src.display()
            );
        }
    }

    if write {
        // ── mutate ───────────────────────────────────────────────────────
        // Order matters: the body moves first because it is the only step
        // that can be undone by moving it back. If saving the index then
        // fails, the body returns to the trash and the entry is untouched —
        // better a restore that did nothing than a body outside any index.
        //
        // Every failure after the displacement puts the displaced copy back.
        // The closure exists so there is ONE place that can return an error
        // here, and one place that undoes the displacement — an early `?`
        // between the two is what stranded the replacement before.
        if let (Some(src), Some(dest)) = (entry.body_path(), body_dest.as_ref()) {
            let replaced = displace_existing(dest, &entry.dir)?;

            let outcome = (|| -> Result<()> {
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)
                        .with_context(|| format!("creating {}", parent.display()))?;
                }
                move_path(&src, dest)?;
                if let Err(e) = library.save(lib_home) {
                    // Put our body back in the trash before propagating, so
                    // the entry stays whole and restorable.
                    let _ = move_path(dest, &src);
                    return Err(e);
                }
                Ok(())
            })();

            if let Err(e) = outcome {
                if let Some(prev) = &replaced {
                    let _ = move_path(prev, dest);
                }
                return Err(e).with_context(|| {
                    format!("restoring '{name}' — it was left in the trash, unchanged")
                });
            }

            // The index is saved and the body is home. Only now is the
            // replaced copy (if any) genuinely superseded.
            if let Some(prev) = replaced {
                let _ = if prev.is_dir() {
                    std::fs::remove_dir_all(&prev)
                } else {
                    std::fs::remove_file(&prev)
                };
            }
        } else {
            library.save(lib_home)?;
        }

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

    /// F22: stashing a corrupt index name still works — that is the cleanup
    /// path — but the record it leaves is inert, and `restore` refuses it by
    /// name rather than acting on it. The pair is what makes the entry safe:
    /// it can be listed and emptied, never put back.
    #[test]
    fn a_stashed_unsafe_name_is_inert_and_never_restorable() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let lib = tmp.path();
        let mut record = blank_record();
        record.server = Some(LibraryServer {
            name: "../../etc/x".into(),
            checksum: None,
            version: None,
            provenance: None,
        });
        // No body: `valid_lib_name` refuses to target a file for such a name,
        // so a removal never has bytes to move here.
        let entry = stash(lib, Kind::Server, "../../etc/x", None, record).unwrap();
        assert!(!entry.id.contains('/'), "{}", entry.id);
        assert!(entry.dir.starts_with(trash_home(lib)));

        let err = restore(lib, &entry.id, false, true).unwrap_err();
        assert!(err.to_string().contains("is not a plain name"), "{err:#}");
        assert!(err.to_string().contains("--empty"), "says how to clear it");

        // And it can be cleared.
        empty(lib, Some(&entry.id), true).unwrap();
        assert!(!entry.dir.exists());
    }

    /// F22: the two name rules are ONE rule. A name the trash would refuse to
    /// restore must never have had a file targeted for it in the first place.
    #[test]
    fn the_removal_and_restore_name_rules_agree() {
        for name in ["../../etc/x", "..", ".", "a/b", "", "a\\b", "/abs", "a\0b"] {
            assert!(
                !is_plain_component(name),
                "'{name}' must be rejected by the shared rule"
            );
        }
        // The rule is about path COMPONENTS, not substrings. `..` between
        // other characters is an ordinary name, and rejecting it would break
        // legal library names for no gain — it cannot traverse anywhere.
        for name in [
            "pdf", "my-skill", "a.b", "x_1", "a..b", "..hidden", "v1..v2",
        ] {
            assert!(is_plain_component(name), "'{name}' should be allowed");
        }
    }

    /// F22 follow-up: a name containing `..` as characters is legal, and the
    /// whole round trip works for it — the grammar fix is not just a predicate
    /// change, the body genuinely lands back in the right place.
    #[test]
    fn a_name_with_embedded_dots_round_trips() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let lib = tmp.path().to_path_buf();
        let live = lib.join("skills/a..b");
        std::fs::create_dir_all(&live).unwrap();
        std::fs::write(live.join("SKILL.md"), "# dots").unwrap();

        let mut record = blank_record();
        record.skill = Some(skill("a..b"));
        let entry = stash(&lib, Kind::Skill, "a..b", Some(&live), record).unwrap();
        assert!(!live.exists(), "body moved to the trash");

        restore(&lib, &entry.id, false, true).unwrap();
        assert_eq!(
            std::fs::read_to_string(live.join("SKILL.md")).unwrap(),
            "# dots",
            "body came back to lib/skills/a..b"
        );
        assert!(Library::load(&lib).unwrap().get("a..b").is_some());
    }

    /// The id builder stays defensive independently of that refusal: it is the
    /// last line between a name and a path component in the trash layout.
    #[test]
    fn ids_never_carry_a_path_component() {
        for name in ["../../etc/x", "..", "a/b", "....", "/abs"] {
            let s = slug(name);
            assert!(!s.contains('/'), "{name} -> {s}");
            assert!(!s.contains('\\'), "{name} -> {s}");
            assert!(!s.contains(".."), "{name} -> {s}");
            assert!(!s.is_empty(), "{name} -> {s}");
        }
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

    /// F22 witness — the reproduction from the review, verbatim: a hand-written
    /// record naming `../../../escaped` used to restore a skill body OUTSIDE
    /// `lib/` and exit successfully.
    #[test]
    fn restore_refuses_a_record_whose_name_escapes_the_library() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let lib = tmp.path().join("lib");
        std::fs::create_dir_all(&lib).unwrap();

        // Hand-build the trash entry the way a crafted `entry.toml` would be.
        let dir = trash_home(&lib).join("skill-x-1");
        std::fs::create_dir_all(dir.join("body")).unwrap();
        std::fs::write(dir.join("body/SKILL.md"), "# pwned").unwrap();
        let mut record = blank_record();
        record.kind = "skill".into();
        record.name = "../../../escaped".into();
        record.body = Some("body".into());
        record.skill = Some(skill("../../../escaped"));
        std::fs::write(
            dir.join(RECORD_FILE),
            toml::to_string_pretty(&record).unwrap(),
        )
        .unwrap();

        let err = restore(&lib, "skill-x-1", false, true).unwrap_err();
        assert!(err.to_string().contains("is not a plain name"), "{err:#}");

        // Nothing moved, anywhere.
        assert!(
            dir.join("body/SKILL.md").exists(),
            "body stayed in the trash"
        );
        assert!(
            !tmp.path().parent().unwrap().join("escaped").exists(),
            "nothing was written outside the library"
        );
        assert!(Library::load(&lib)
            .unwrap()
            .get("../../../escaped")
            .is_none());
    }

    /// F22 witness — the other traversal route: a plain name whose destination
    /// resolves outside the library through a symlinked kind directory.
    #[test]
    #[cfg(unix)]
    fn restore_refuses_when_the_destination_resolves_outside_the_library() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let lib = tmp.path().join("lib");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, lib.join("skills")).unwrap();

        let dir = trash_home(&lib).join("skill-pdf-1");
        std::fs::create_dir_all(dir.join("body")).unwrap();
        std::fs::write(dir.join("body/SKILL.md"), "# pdf").unwrap();
        let mut record = blank_record();
        record.kind = "skill".into();
        record.name = "pdf".into();
        record.body = Some("body".into());
        record.skill = Some(skill("pdf"));
        std::fs::write(
            dir.join(RECORD_FILE),
            toml::to_string_pretty(&record).unwrap(),
        )
        .unwrap();

        let err = restore(&lib, "skill-pdf-1", false, true).unwrap_err();
        assert!(err.to_string().contains("outside the library"), "{err:#}");
        assert!(!outside.join("pdf").exists(), "nothing landed outside");
    }

    /// F22 witness — a record with no index row is refused BEFORE the body
    /// moves. It used to move the body first and fail afterwards, leaving the
    /// bytes in neither the trash nor the index.
    #[test]
    fn restore_refuses_a_record_with_no_index_row_without_moving_the_body() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let lib = tmp.path().join("lib");
        std::fs::create_dir_all(&lib).unwrap();

        let dir = trash_home(&lib).join("skill-pdf-1");
        std::fs::create_dir_all(dir.join("body")).unwrap();
        std::fs::write(dir.join("body/SKILL.md"), "# pdf").unwrap();
        let mut record = blank_record();
        record.kind = "skill".into();
        record.name = "pdf".into();
        record.body = Some("body".into());
        // record.skill deliberately left None.
        std::fs::write(
            dir.join(RECORD_FILE),
            toml::to_string_pretty(&record).unwrap(),
        )
        .unwrap();

        let err = restore(&lib, "skill-pdf-1", false, true).unwrap_err();
        assert!(err.to_string().contains("no skill index row"), "{err:#}");
        assert!(
            dir.join("body/SKILL.md").exists(),
            "the body must still be in the trash"
        );
        assert!(!lib.join("skills/pdf").exists());
    }

    /// F22 witness — an index row naming something other than the record is a
    /// refusal, not a restore that splits the two apart.
    #[test]
    fn restore_refuses_when_the_index_row_names_something_else() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let lib = tmp.path().join("lib");
        std::fs::create_dir_all(&lib).unwrap();
        let dir = trash_home(&lib).join("skill-pdf-1");
        std::fs::create_dir_all(&dir).unwrap();
        let mut record = blank_record();
        record.kind = "skill".into();
        record.name = "pdf".into();
        record.skill = Some(skill("something-else"));
        std::fs::write(
            dir.join(RECORD_FILE),
            toml::to_string_pretty(&record).unwrap(),
        )
        .unwrap();

        let err = restore(&lib, "skill-pdf-1", false, true).unwrap_err();
        assert!(err.to_string().contains("index row names"), "{err:#}");
    }

    /// F22 witness — a body field that is not one of the two known slots is
    /// refused rather than joined onto the record directory as a path.
    #[test]
    fn restore_refuses_an_unknown_body_slot() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let lib = tmp.path().join("lib");
        std::fs::create_dir_all(&lib).unwrap();
        let dir = trash_home(&lib).join("skill-pdf-1");
        std::fs::create_dir_all(&dir).unwrap();
        let mut record = blank_record();
        record.kind = "skill".into();
        record.name = "pdf".into();
        record.body = Some("../../../etc/passwd".into());
        record.skill = Some(skill("pdf"));
        std::fs::write(
            dir.join(RECORD_FILE),
            toml::to_string_pretty(&record).unwrap(),
        )
        .unwrap();

        let err = restore(&lib, "skill-pdf-1", false, true).unwrap_err();
        assert!(err.to_string().contains("is not one of"), "{err:#}");
    }

    /// F22 witness — `--replace` sets the live entry aside instead of deleting
    /// it, so the copy the user already had survives a failure partway through.
    #[test]
    fn replace_preserves_the_live_entry_until_the_index_is_saved() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let lib = tmp.path().join("lib");
        let live = lib.join("skills/pdf");
        std::fs::create_dir_all(&live).unwrap();
        std::fs::write(live.join("SKILL.md"), "# live").unwrap();

        let mut record = blank_record();
        record.skill = Some(skill("pdf"));
        let entry = stash(&lib, Kind::Skill, "pdf", Some(&live), record).unwrap();

        // Re-add a different body under the same name, then restore over it.
        std::fs::create_dir_all(&live).unwrap();
        std::fs::write(live.join("SKILL.md"), "# newer").unwrap();
        let mut library = Library::load(&lib).unwrap();
        library.upsert(skill("pdf"));
        library.save(&lib).unwrap();

        restore(&lib, &entry.id, true, true).unwrap();
        assert_eq!(
            std::fs::read_to_string(live.join("SKILL.md")).unwrap(),
            "# live",
            "the trashed copy is back"
        );
        assert!(!entry.dir.exists(), "the trash entry is consumed");
        assert!(
            !lib.join("skills/replaced").exists(),
            "no staging leftovers in the library"
        );
    }

    /// F22 follow-up — a record naming a body that is not there must be
    /// refused BEFORE the live entry is displaced. It used to set the user's
    /// copy aside first and only then fail, stranding it under
    /// `.trash/<id>/replaced`.
    #[test]
    fn a_missing_body_is_refused_before_the_live_entry_is_displaced() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let lib = tmp.path().to_path_buf();
        let live = lib.join("skills/pdf");
        std::fs::create_dir_all(&live).unwrap();
        std::fs::write(live.join("SKILL.md"), "# the live copy").unwrap();
        let mut library = Library::load(&lib).unwrap();
        library.upsert(skill("pdf"));
        library.save(&lib).unwrap();

        // A record that claims a body it does not have.
        let dir = trash_home(&lib).join("skill-pdf-1");
        std::fs::create_dir_all(&dir).unwrap();
        let mut record = blank_record();
        record.kind = "skill".into();
        record.name = "pdf".into();
        record.body = Some("body".into());
        record.skill = Some(skill("pdf"));
        std::fs::write(
            dir.join(RECORD_FILE),
            toml::to_string_pretty(&record).unwrap(),
        )
        .unwrap();

        let err = restore(&lib, "skill-pdf-1", true, true).unwrap_err();
        assert!(err.to_string().contains("is not there"), "{err:#}");

        // The live copy is still exactly where it was — not under .trash.
        assert_eq!(
            std::fs::read_to_string(live.join("SKILL.md")).unwrap(),
            "# the live copy"
        );
        assert!(
            !dir.join("replaced").exists(),
            "nothing was displaced into the trash entry"
        );
    }

    /// F22 follow-up — `unstash` is the removal path's rollback: the body goes
    /// back where it came from and the trash record is dropped, so a failed
    /// removal leaves no half-state.
    #[test]
    fn unstash_puts_the_body_back_and_drops_the_record() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let lib = tmp.path().to_path_buf();
        let live = lib.join("skills/pdf");
        std::fs::create_dir_all(&live).unwrap();
        std::fs::write(live.join("SKILL.md"), "# body").unwrap();

        let mut record = blank_record();
        record.skill = Some(skill("pdf"));
        let entry = stash(&lib, Kind::Skill, "pdf", Some(&live), record).unwrap();
        assert!(!live.exists(), "stash moved it out");

        assert!(unstash(&entry, Some(&live)), "rollback reported success");
        assert_eq!(
            std::fs::read_to_string(live.join("SKILL.md")).unwrap(),
            "# body",
            "body is back at its origin"
        );
        assert!(!entry.dir.exists(), "trash record dropped");
    }
}
