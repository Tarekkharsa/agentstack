//! `<source>/library.toml` — one linked library source's capability index.
//!
//! A library source is any folder a project references capabilities from by
//! name, instead of copying capability files into each repo; several can be
//! linked at once, and `~/.agentstack/lib` is the one every machine starts with
//! (see `docs/design/linked-library-sources.md` and
//! `docs/reference.md#the-library-linked-source-folders`). This module is the
//! inert foundation: it models one source's index, loads/saves it, and merges
//! the linked sources into the ordered view every consumer reads
//! ([`Library::load_default`]). It performs **no resolution** — mapping a
//! project's `skills = ["name"]` reference to a library entry is the resolver's
//! job, on top of this.
//!
//! Skills ship in Phase 1; servers are modeled here for Phase 1b (the resolver
//! wiring lands in a later step); declarative `[hooks.*]` land in E3d — a hook's
//! reusable definition lives at `<lib_home>/hooks/<name>.toml`, exactly like a
//! server, and `agentstack add <name>` copies it into a project's `[hooks.<name>]`
//! table (hooks always render from the manifest, so the library is a source to
//! copy from, never a runtime indirection). The file is an index, not a scan
//! target: entries carry provenance and an integrity digest so `lib list`,
//! `explain`, and drift checks have metadata to work with.

use agentstack_core::digest::Sha256Hex;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::manifest::Skill;
use crate::store::Store;

/// Parse the one-line `description:` value from a `SKILL.md` YAML frontmatter
/// block — the leading `---` … `---` fence. Returns `None` when there's no
/// frontmatter or no `description:` key. This is the single shared parser for
/// every surface that shows skill descriptions: central-library search
/// (`agentstack search`), `lib list`, and the MCP loadable catalog
/// (`mcp_server`), which all call it rather than re-implementing it.
pub fn parse_frontmatter_description(md: &str) -> Option<String> {
    let rest = md.trim_start().strip_prefix("---")?;
    let end = rest.find("\n---")?;
    let mut lines = rest[..end].lines().peekable();
    while let Some(line) = lines.next() {
        // Only top-level keys: an indented `description:` belongs to some
        // nested structure, not the skill.
        if line.starts_with(char::is_whitespace) {
            continue;
        }
        let Some(v) = line.trim().strip_prefix("description:") else {
            continue;
        };
        let v = v.trim();
        // YAML block scalars (`|`, `>`, with optional chomp/indent
        // indicators like `|-` or `>2`) and empty values put the actual
        // text on the FOLLOWING indented lines. Third-party skills use
        // full YAML frontmatter — returning the literal "|" here made
        // perfectly described skills look undescribed.
        let is_block = matches!(v.chars().next(), Some('|' | '>'))
            && v[1..].chars().all(|c| matches!(c, '+' | '-' | '0'..='9'));
        if is_block || v.is_empty() {
            let mut parts: Vec<String> = Vec::new();
            while let Some(next) = lines.peek() {
                if !next.trim().is_empty() && !next.starts_with(char::is_whitespace) {
                    break; // next top-level key
                }
                let text = lines.next().unwrap_or_default().trim().to_string();
                if !text.is_empty() {
                    parts.push(text);
                }
            }
            if parts.is_empty() {
                return None;
            }
            // Fold to one logical line — every consumer (search, lib list,
            // the loadable index) wants a single description string. Sanitized
            // at this single parse chokepoint: the description is remote text
            // headed for terminals and agent context (design §A.2 #1).
            return Some(crate::text::sanitize_line(&parts.join(" ")));
        }
        return Some(crate::text::sanitize_line(
            v.trim_matches('"').trim_matches('\''),
        ));
    }
    None
}

/// Whether a skill directory's `SKILL.md` carries a non-empty frontmatter
/// `description:`. Search matching and an agent's decision to load both hinge
/// entirely on the description — `lib add` and `doctor` warn when it's
/// missing rather than let the skill go silently undiscoverable.
pub fn skill_has_description(dir: &std::path::Path) -> bool {
    std::fs::read_to_string(dir.join("SKILL.md"))
        .ok()
        .and_then(|text| parse_frontmatter_description(&text))
        .is_some_and(|d| !d.trim().is_empty())
}

pub const LIBRARY_FILE: &str = "library.toml";
/// Newest library-index schema version this build reads and writes. Anything
/// above it was written by a future agentstack; [`Library::load`] refuses it
/// instead of misinterpreting silently.
pub const SUPPORTED_LIBRARY_VERSION: u32 = 1;

/// The central library index. Lives at `<lib_home>/library.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Library {
    pub version: u32,
    /// Skills available in the central library, keyed by unique `name`.
    #[serde(default, rename = "skill")]
    pub skills: Vec<LibrarySkill>,
    /// MCP servers available in the central library, keyed by unique `name`
    /// (Phase 1b). The definition lives at `<lib_home>/servers/<name>.toml`.
    #[serde(default, rename = "server")]
    pub servers: Vec<LibraryServer>,
    /// Native harness extensions available in the central library (E3), keyed
    /// by unique `name`. A `path` source body lives under
    /// `<lib_home>/extensions/<name>/`; a `git` source is referenced (resolved
    /// through the shared store, like a git skill).
    #[serde(default, rename = "extension")]
    pub extensions: Vec<LibraryExtension>,
    /// Declarative lifecycle hooks available in the central library (E3d), keyed
    /// by unique `name`. The definition — a serialized `manifest::Hook` — lives at
    /// `<lib_home>/hooks/<name>.toml`, mirroring servers exactly.
    #[serde(default, rename = "hook")]
    pub hooks: Vec<LibraryHook>,
    /// Versioned capability **packages** available in the central library (W5,
    /// `docs/design/package-layer.md`), keyed by unique `name`. A `path` source
    /// body lives under `<lib_home>/packages/<name>/` with a `pack.toml` at its
    /// root; a `git` source is referenced (resolved through the shared store,
    /// like a git skill or extension).
    #[serde(default, rename = "package")]
    pub packages: Vec<LibraryPackage>,
    /// House-rule fragments available in a linked source
    /// (`docs/design/instruction-variants.md`), keyed by unique `name`. The
    /// body is a **directory** at `<source>/instructions/<name>/` holding an
    /// `instruction.toml` plus its markdown bodies — a directory because a
    /// fragment with per-(CLI, model) variants is several files, and the folder
    /// taxonomy already spells "has members" as a directory.
    ///
    /// Like servers and hooks, the entry carries no `path`: the body is always
    /// at the name, so [`Library::source_root`] is all a resolver needs and
    /// there is nothing for [`Library::absolutize_paths`] to rewrite.
    #[serde(default, rename = "instruction")]
    pub instructions: Vec<LibraryInstruction>,
    /// The linked sources this index was merged from, in precedence order
    /// (`docs/design/linked-library-sources.md`). In-memory only — it is a view
    /// over several `library.toml` files, never a field any of them carries.
    ///
    /// Empty for a single-file index read by [`Library::load`]; populated by
    /// [`Library::load_linked`], which every "the machine's library" caller
    /// reaches through [`Library::load_default`].
    #[serde(skip)]
    pub linked: LinkedView,
}

impl Default for Library {
    fn default() -> Self {
        Library {
            version: 1,
            skills: Vec::new(),
            servers: Vec::new(),
            extensions: Vec::new(),
            hooks: Vec::new(),
            packages: Vec::new(),
            instructions: Vec::new(),
            linked: LinkedView::default(),
        }
    }
}

/// The directory every library instruction body lives under, in every source.
pub const INSTRUCTIONS_DIR: &str = "instructions";

/// The declaration file at the root of one library instruction body. Carries
/// the base `path` and the `[[variant]]` array — the SAME grammar the manifest
/// uses, parsed by the same serde types, so there is one grammar, one
/// hostile-input gate and one precedence function.
pub const INSTRUCTION_FILE: &str = "instruction.toml";

/// One house-rule fragment installed in a linked library source.
///
/// Deliberately thin, mirroring [`LibraryHook`]: the body is addressed by name
/// (`<source>/instructions/<name>/`), so there is no source/path/git axis to
/// record — a fragment is prose, not a fetchable artifact.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LibraryInstruction {
    /// The name a project references this fragment by. Unique within a source.
    pub name: String,
    /// One-line human description for `lib list`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Where the entry came from (`"manual"`, `"init"`, …). Informational.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<String>,
}

impl LibraryInstruction {
    /// The directory holding this fragment's bodies under `root` (a source
    /// root, not the `instructions/` subdir — the subdir is this kind's own
    /// constant and never a caller's to remember).
    pub fn body_dir(&self, root: &Path) -> PathBuf {
        root.join(INSTRUCTIONS_DIR).join(&self.name)
    }

    /// One-line description for display. Signature mirrors
    /// [`LibrarySkill::description`] so `lib list` renders every kind through
    /// the same row shape.
    pub fn description(&self, _lib_home: &Path) -> Option<String> {
        self.description.clone()
    }
}

/// The capability kinds a linked source can hold. Used to key precedence and
/// collision lookups uniformly, so the five kinds cannot drift apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Skill,
    Server,
    Extension,
    Hook,
    Package,
    Instruction,
}

impl Kind {
    /// The word surfaces use for this kind, singular.
    pub fn noun(self) -> &'static str {
        match self {
            Kind::Skill => "skill",
            Kind::Server => "server",
            Kind::Extension => "extension",
            Kind::Hook => "hook",
            Kind::Package => "package",
            Kind::Instruction => "house rule",
        }
    }
}

/// One linked source's own index, with its root.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceIndex {
    /// The name this source is addressed by in a `<source>:<name>` reference.
    pub name: String,
    /// The folder holding `library.toml` and the capability taxonomy.
    pub root: PathBuf,
    /// This source's index, on its own. Path entries belonging to a **non-**
    /// primary source are absolutized against `root` when the view is built,
    /// so every consumer that already passes the primary `lib_home` resolves
    /// them correctly without a second base parameter.
    pub library: Library,
}

impl SourceIndex {
    fn holds(&self, kind: Kind, name: &str) -> bool {
        match kind {
            Kind::Skill => self.library.skills.iter().any(|e| e.name == name),
            Kind::Server => self.library.servers.iter().any(|e| e.name == name),
            Kind::Extension => self.library.extensions.iter().any(|e| e.name == name),
            Kind::Hook => self.library.hooks.iter().any(|e| e.name == name),
            Kind::Package => self.library.packages.iter().any(|e| e.name == name),
            Kind::Instruction => self.library.instructions.iter().any(|e| e.name == name),
        }
    }
}

/// One name held by more than one linked source: who wins, who is shadowed.
/// Built once at merge time so no surface has to recompute it — and so no
/// surface can show a capability the user will not actually get.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collision {
    pub kind: Kind,
    pub name: String,
    /// The source that satisfies the bare name.
    pub winner: String,
    /// The sources holding the same name, later in the order.
    pub shadowed: Vec<String>,
}

impl Collision {
    /// The reference that pins the *shadowed* copy — the one thing a user
    /// reading a collision report actually needs.
    pub fn qualified_shadowed(&self) -> String {
        format!(
            "{}{}{}",
            self.shadowed
                .first()
                .map(String::as_str)
                .unwrap_or_default(),
            crate::sources::QUALIFIER,
            self.name
        )
    }
}

/// The ordered linked sources behind a merged [`Library`], plus the collisions
/// the merge found. Empty means "this index is one file", which is the shape
/// every library-management command still works in.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LinkedView {
    pub sources: Vec<SourceIndex>,
    pub collisions: Vec<Collision>,
}

impl LinkedView {
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    /// The source a reference resolves in, and the capability's bare name.
    ///
    /// A qualified reference resolves **only** in the source it names — no
    /// fall-through, or the explicit form would be weaker than the implicit
    /// one. A bare reference takes the first source in order that holds it.
    pub fn find<'v, 'r>(
        &'v self,
        kind: Kind,
        reference: &'r str,
    ) -> Option<(&'v SourceIndex, &'r str)> {
        if let Some((source, name)) = crate::sources::split_reference(reference) {
            let index = self.sources.iter().find(|s| s.name == source)?;
            return index.holds(kind, name).then_some((index, name));
        }
        self.sources
            .iter()
            .find(|s| s.holds(kind, reference))
            .map(|s| (s, reference))
    }

    /// Whether `source` is a linked source name at all — so an unresolved
    /// qualified reference can say "no such source" rather than "no such
    /// capability", which are different mistakes with different fixes.
    pub fn has_source(&self, source: &str) -> bool {
        self.sources.iter().any(|s| s.name == source)
    }

    /// The linked source names, in precedence order.
    pub fn source_names(&self) -> Vec<&str> {
        self.sources.iter().map(|s| s.name.as_str()).collect()
    }

    /// The collisions involving one name, whatever the kind.
    pub fn collisions_for(&self, name: &str) -> Vec<&Collision> {
        self.collisions.iter().filter(|c| c.name == name).collect()
    }
}

/// The library subdirectory holding package bodies: `<lib_home>/packages/`.
/// One constant so the index, the resolver, and any future `lib add-package`
/// cannot disagree about where a body lives.
pub const PACKAGES_DIR: &str = "packages";

/// The manifest file at the root of every package body, whatever its source.
/// The same name and the same grammar the git pack rail already publishes —
/// see [`crate::provider::gitpack::PackToml`].
pub const PACK_FILE: &str = "pack.toml";

/// One versioned capability package installed in the central library (W5).
///
/// **The on-disk shape, and why it is this one.** A package is a *directory
/// body* — `<lib_home>/packages/<name>/pack.toml` plus member bodies at paths
/// relative to that root — mirroring [`LibraryExtension`] rather than
/// [`LibraryServer`]. Three reasons, in `docs/design/package-layer.md`
/// §"The on-disk shape of a package in the library": it is literally the same
/// artifact `pack.toml` already describes, so the git pack rail's parser,
/// name-contract gate and content scan are reused rather than duplicated; the
/// folder taxonomy already spells "has members" as a directory
/// (`skills/<name>/`, `extensions/<name>/`) and "is one definition" as a file
/// (`servers/<name>.toml`, `hooks/<name>.toml`); and indexing it exactly like
/// the four kinds that came before is what "a first-class package index" means.
///
/// The `checksum` covers the package's `pack.toml` — its *boundary*, not its
/// member bodies. Member bytes are digested individually at lock time, one pin
/// each, which is the thing that makes the compact reference safe; a single
/// roll-up digest here would let one member's bytes move inside an unchanged
/// package digest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LibraryPackage {
    /// The name a toolset references this package by. Unique within the library.
    pub name: String,
    /// `path` or `git`.
    pub source: String,
    /// For `source = "path"`: location of the body, relative to
    /// `<lib_home>/packages/` (or absolute).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// For `source = "git"`: the source URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<String>,
    /// Pinned git revision (git sources only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    /// For `source = "git"`: the package's directory within the repo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subpath: Option<String>,
    /// SHA-256 of the package's `pack.toml` — the boundary digest. Optional
    /// until the entry has been resolved and hashed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checksum: Option<Sha256Hex>,
    /// The package's declared version. Unlike the other kinds' informational
    /// `version`, this one is **load-bearing**: it is recorded verbatim in the
    /// lock's package pin, so an upgrade diff has a version axis to report.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// One-line human description, stored in the index (a package's own
    /// description lives in `pack.toml`, which the index caches here so
    /// `lib list` need not read every body).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Where the entry came from (e.g. `"git:<url>"`, `"manual"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<String>,
}

impl LibraryPackage {
    /// The directory holding this package's body, if it is locally readable
    /// right now: path sources resolve under `<lib_home>/packages/…`; git
    /// sources only if already cached in the shared store (no network, no
    /// fetch, no digest). Mirrors [`LibrarySkill::body_dir`] exactly, including
    /// its "not installed" vs "installed but unreadable" distinction.
    pub fn body_dir(&self, lib_home: &Path) -> Option<PathBuf> {
        let as_skill = Skill {
            path: self.path.clone(),
            git: self.git.clone(),
            rev: self.rev.clone(),
            subpath: self.subpath.clone(),
        };
        Some(
            Store::default_store()
                .resolve_path_only(&as_skill, &lib_home.join(PACKAGES_DIR), None)
                .ok()
                .flatten()?
                .path,
        )
    }

    /// One-line description for display, straight from the stored index field.
    /// Signature mirrors [`LibrarySkill::description`] so `lib list` renders
    /// every kind through the same row shape.
    pub fn description(&self, _lib_home: &Path) -> Option<String> {
        self.description.clone()
    }
}

/// One skill installed in the central library. Mirrors the lockfile's
/// `LockedSkill` shape (`source`/`path`/`git`/`rev`/`checksum`) so the resolver
/// can pass integrity straight through to a project's `agentstack.lock`, and adds
/// library-only metadata (`version`, `provenance`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LibrarySkill {
    /// The name a project references this skill by. Unique within the library.
    pub name: String,
    /// `path` or `git`.
    pub source: String,
    /// For `source = "path"`: location of the skill body, relative to
    /// `<lib_home>/skills/` (or absolute).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// For `source = "git"`: the source URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<String>,
    /// Pinned git revision (git sources only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    /// For `source = "git"`: the skill's directory within the repo (subdir
    /// layouts). `None`/absent means the repo root holds `SKILL.md`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subpath: Option<String>,
    /// SHA-256 of the skill content. Optional until the entry has been resolved
    /// and hashed; the resolver populates it and records it in project locks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checksum: Option<Sha256Hex>,
    /// Optional declared version for the entry (informational).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Where the entry came from (e.g. `"consolidated"`, `"catalog:<pack>"`,
    /// `"manual"`). Informational; surfaced by `explain`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<String>,
}

impl LibrarySkill {
    /// Best-effort one-line description from this skill's `SKILL.md` frontmatter.
    ///
    /// `lib_home` is the central-library root: path sources read directly from
    /// `<lib_home>/skills/…`; git sources read from the shared store **only if
    /// already cached** (no network, no fetch, no content digest). Any miss — a
    /// git source not yet installed, a missing/blank `SKILL.md`, or no
    /// `description:` key — yields `None`, and callers render a placeholder
    /// rather than failing. Reading at call time is deliberate: the library is
    /// small (~a dozen skills) so `search` and `lib list` stay cheap.
    pub fn description(&self, lib_home: &Path) -> Option<String> {
        let text = fs::read_to_string(self.body_dir(lib_home)?.join("SKILL.md")).ok()?;
        parse_frontmatter_description(&text)
    }

    /// The on-disk directory holding this skill's body, if it's locally
    /// readable right now: path sources resolve under `<lib_home>/skills/…`;
    /// git sources only if already cached in the shared store (no network,
    /// no fetch, no content digest). Lets callers distinguish "not installed"
    /// from "installed but undescribed".
    pub fn body_dir(&self, lib_home: &Path) -> Option<PathBuf> {
        // Reuse the resolver's view of a library skill (path relative to
        // `<lib_home>/skills/`, or a cached git clone) without digesting — the
        // same shape `resolve_skill` builds for `SkillOrigin::Library`.
        let skill = Skill {
            path: self.path.clone(),
            git: self.git.clone(),
            rev: self.rev.clone(),
            subpath: self.subpath.clone(),
        };
        Some(
            Store::default_store()
                .resolve_path_only(&skill, &lib_home.join("skills"), None)
                .ok()
                .flatten()?
                .path,
        )
    }
}

/// One MCP server installed in the central library (Phase 1b). The reusable
/// definition — a serialized `manifest::Server` with `${REF}` secrets only,
/// never plaintext — lives at `<lib_home>/servers/<name>.toml`; this index entry
/// records its identity, integrity digest, and provenance.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LibraryServer {
    /// The name a project references this server by. Unique within the library.
    pub name: String,
    /// SHA-256 of the server definition file (`servers/<name>.toml`). Optional
    /// until the entry has been written and hashed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checksum: Option<Sha256Hex>,
    /// Optional declared version for the entry (informational).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Where the entry came from (e.g. `"consolidated:<provider>"`, `"manual"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<String>,
}

/// One declarative lifecycle hook installed in the central library (E3d). The
/// reusable definition — a serialized `manifest::Hook` with `${REF}` secrets
/// only, never plaintext — lives at `<lib_home>/hooks/<name>.toml`; this index
/// entry records its identity, integrity digest, and provenance. Identical in
/// shape to [`LibraryServer`]: a hook is a flat definition file, not a directory
/// body, so there is no `source`/`path`/`git` — the body is always the one file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LibraryHook {
    /// The name a project references this hook by. Unique within the library.
    pub name: String,
    /// SHA-256 of the hook definition file (`hooks/<name>.toml`). Optional until
    /// the entry has been written and hashed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checksum: Option<String>,
    /// Optional declared version for the entry (informational).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Where the entry came from (e.g. `"file:<path>"`, `"manifest:<dir>"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<String>,
}

/// One native harness extension installed in the central library (E3). Mirrors
/// [`LibrarySkill`]'s `source`/`path`/`git`/`rev`/`subpath`/`checksum` shape so
/// the resolver can pin a library-origin extension exactly like a skill, plus
/// the one adapter it `target`s (extension code is harness-specific, so a
/// library extension carries its target the way `[extensions.*]` entries do).
///
/// A `path` source body is copied into `<lib_home>/extensions/<name>/`; a `git`
/// source stays in the shared store, referenced by this entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LibraryExtension {
    /// The name a project references this extension by. Unique within the library.
    pub name: String,
    /// `path` or `git`.
    pub source: String,
    /// The one adapter id this extension's code is written against (`pi`,
    /// `opencode`, …). Singular, never `"*"` — extension code is harness-specific.
    pub target: String,
    /// For `source = "path"`: location of the body, relative to
    /// `<lib_home>/extensions/` (or absolute).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// For `source = "git"`: the source URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<String>,
    /// Pinned git revision (git sources only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    /// For `source = "git"`: the extension's directory within the repo. Git
    /// extension bodies are always digested at a subpath (a checkout's `.git`
    /// can never be part of a reproducible pin), so this is effectively required.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subpath: Option<String>,
    /// SHA-256 of the extension content (the strict integrity-root digest, not
    /// the lenient skill digest). Optional until the entry has been resolved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checksum: Option<String>,
    /// One-line human description. Extensions carry no `SKILL.md`, so — unlike
    /// skills, whose description is read from the body at display time — this is
    /// stored in the index directly (from `lib add-extension --description`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional declared version for the entry (informational).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Where the entry came from (e.g. `"path:<src>"`, `"git:<url>"`,
    /// `"manual"`). Informational; surfaced by `lib list`/`explain`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<String>,
}

impl LibraryExtension {
    /// One-line description for display, straight from the stored index field.
    /// Signature mirrors [`LibrarySkill::description`] (which reads the body) so
    /// `lib list` can render both kinds through the same row shape.
    pub fn description(&self, _lib_home: &Path) -> Option<String> {
        self.description.clone()
    }
}

impl Library {
    /// The index path for a given library home directory.
    pub fn path(lib_home: &Path) -> PathBuf {
        lib_home.join(LIBRARY_FILE)
    }

    /// Load the index from an explicit library home. A missing file yields an
    /// empty default library (the library simply hasn't been populated yet).
    pub fn load(lib_home: &Path) -> Result<Self> {
        let path = Self::path(lib_home);
        match fs::read_to_string(&path) {
            Ok(text) => {
                let library: Library =
                    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
                crate::util::check_schema_version(
                    library.version,
                    SUPPORTED_LIBRARY_VERSION,
                    "library index",
                    &path,
                )?;
                Ok(library)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Library::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// Load the machine's library: every linked source, merged in precedence
    /// order (`docs/design/linked-library-sources.md`).
    ///
    /// On a machine that never linked a folder this is exactly
    /// `Self::load(&paths::lib_home())` plus a one-source [`LinkedView`], so
    /// the single-library setup is unchanged down to the bytes of every entry.
    pub fn load_default() -> Result<Self> {
        Self::load_linked(&crate::sources::Sources::load()?.linked())
    }

    /// Merge the ordered linked sources into one index.
    ///
    /// **First match wins** — `PATH` semantics. A later source holding a name
    /// an earlier one already holds contributes nothing to the merged vectors
    /// and is recorded as a [`Collision`] instead, so the shadowed copy is
    /// always reportable and never silently gone.
    ///
    /// A source that cannot be read is reported and skipped rather than
    /// failing the whole machine: one unreadable linked folder must not take
    /// every other source's capabilities offline with it.
    pub fn load_linked(sources: &[crate::sources::LinkedSource]) -> Result<Self> {
        let mut indexes: Vec<SourceIndex> = Vec::with_capacity(sources.len());
        // The one root whose entries may stay in their recorded relative form:
        // `~/.agentstack/lib`, because that is the base every existing consumer
        // passes as `lib_home`. Not "the first source" — a user who links their
        // own folder first must not thereby make the central library's paths
        // resolve against the wrong root.
        let relative_root = crate::util::paths::lib_home();
        for source in sources {
            let mut library = match Self::load(&source.root) {
                Ok(l) => l,
                Err(e) => {
                    eprintln!(
                        "warning: library source '{}' at {} is unreadable ({e:#}); skipping it",
                        source.name,
                        source.root.display()
                    );
                    continue;
                }
            };
            // Every other source is rewritten to absolute paths. Leaving
            // `lib_home`'s own entries alone is what keeps a single-library
            // machine byte-identical — nothing about its lock entries or
            // rendered artifacts moves.
            if source.root != relative_root {
                library.absolutize_paths(&source.root);
            }
            indexes.push(SourceIndex {
                name: source.name.clone(),
                root: source.root.clone(),
                library,
            });
        }

        let mut merged = Library::default();
        let mut collisions: Vec<Collision> = Vec::new();
        for index in &indexes {
            for entry in &index.library.skills {
                if merged.skills.iter().any(|e| e.name == entry.name) {
                    note_collision(&mut collisions, &indexes, Kind::Skill, &entry.name);
                } else {
                    merged.skills.push(entry.clone());
                }
            }
            for entry in &index.library.servers {
                if merged.servers.iter().any(|e| e.name == entry.name) {
                    note_collision(&mut collisions, &indexes, Kind::Server, &entry.name);
                } else {
                    merged.servers.push(entry.clone());
                }
            }
            for entry in &index.library.extensions {
                if merged.extensions.iter().any(|e| e.name == entry.name) {
                    note_collision(&mut collisions, &indexes, Kind::Extension, &entry.name);
                } else {
                    merged.extensions.push(entry.clone());
                }
            }
            for entry in &index.library.hooks {
                if merged.hooks.iter().any(|e| e.name == entry.name) {
                    note_collision(&mut collisions, &indexes, Kind::Hook, &entry.name);
                } else {
                    merged.hooks.push(entry.clone());
                }
            }
            for entry in &index.library.packages {
                if merged.packages.iter().any(|e| e.name == entry.name) {
                    note_collision(&mut collisions, &indexes, Kind::Package, &entry.name);
                } else {
                    merged.packages.push(entry.clone());
                }
            }
            for entry in &index.library.instructions {
                if merged.instructions.iter().any(|e| e.name == entry.name) {
                    note_collision(&mut collisions, &indexes, Kind::Instruction, &entry.name);
                } else {
                    merged.instructions.push(entry.clone());
                }
            }
        }
        merged.linked = LinkedView {
            sources: indexes,
            collisions,
        };
        Ok(merged)
    }

    /// Rewrite this index's `path` entries to absolute paths under `root`.
    ///
    /// A merged view is consumed by callers that pass the *primary* library
    /// home as the base directory. An entry from another source would resolve
    /// against the wrong root, so its path is made root-independent here —
    /// once, at merge time — instead of threading a per-entry base through
    /// every resolver signature. Git-sourced entries need nothing: they resolve
    /// through the content store, which has no notion of a library root.
    fn absolutize_paths(&mut self, root: &Path) {
        fn absolutize(path: &mut Option<String>, anchor: PathBuf) {
            if let Some(p) = path {
                let candidate = PathBuf::from(&*p);
                if candidate.is_relative() {
                    *p = anchor.join(candidate).to_string_lossy().into_owned();
                }
            }
        }
        for skill in &mut self.skills {
            if skill.source == "path" {
                absolutize(&mut skill.path, root.join("skills"));
            }
        }
        for extension in &mut self.extensions {
            if extension.source == "path" {
                absolutize(&mut extension.path, root.join("extensions"));
            }
        }
        for package in &mut self.packages {
            if package.source == "path" {
                absolutize(&mut package.path, root.join(PACKAGES_DIR));
            }
        }
    }

    /// The library root a reference's definition file lives under, for the two
    /// kinds whose body is a bare file with no `path` field (servers and
    /// hooks). Falls back to `lib_home`, which is what a single-source machine
    /// and every management command already pass.
    pub fn source_root(&self, kind: Kind, lib_home: &Path, reference: &str) -> PathBuf {
        match self.linked.find(kind, reference) {
            Some((index, _)) => index.root.clone(),
            None => lib_home.to_path_buf(),
        }
    }

    /// Best-effort load for surfaces that degrade to inline-only resolution
    /// rather than failing (the gateway, rendering). The error — an unreadable
    /// index, or one written by a newer schema — is reported on stderr instead
    /// of being swallowed, so a version-incompatible library says "upgrade
    /// agentstack" rather than masquerading as name refs that don't resolve.
    pub fn load_default_or_warn() -> Self {
        Self::load_default().unwrap_or_else(|e| {
            eprintln!(
                "warning: central library unavailable ({e:#}); resolving inline servers only"
            );
            Library::default()
        })
    }

    /// Write the index to a library home, creating the directory if needed.
    /// Write the index. Atomic (write-temp-then-rename), because a removal
    /// moves a body to the trash and then saves this file: a torn write here
    /// leaves the body in the trash and the index unreadable, which is the one
    /// state neither the removal nor a restore can reason about.
    pub fn save(&self, lib_home: &Path) -> Result<()> {
        fs::create_dir_all(lib_home).with_context(|| format!("creating {}", lib_home.display()))?;
        let path = Self::path(lib_home);
        let text = toml::to_string_pretty(self)?;
        crate::util::atomic::write(&path, &text)
            .with_context(|| format!("writing {}", path.display()))
    }

    /// Look up a library skill by the reference a project uses — a bare name
    /// resolved through the source order, or a `<source>:<name>` reference
    /// that resolves only in the source it names.
    pub fn get(&self, reference: &str) -> Option<&LibrarySkill> {
        if let Some((index, name)) = self.linked.find(Kind::Skill, reference) {
            return index.library.skills.iter().find(|s| s.name == name);
        }
        // No linked view (a single-file index read by `load`) — the flat
        // vector is the whole library. A qualified reference finds nothing
        // here, which is correct: there is no source list to name.
        self.linked
            .is_empty()
            .then(|| self.skills.iter().find(|s| s.name == reference))
            .flatten()
    }

    /// Insert or replace a skill entry, keeping entries sorted by name.
    pub fn upsert(&mut self, entry: LibrarySkill) {
        if let Some(existing) = self.skills.iter_mut().find(|s| s.name == entry.name) {
            *existing = entry;
        } else {
            self.skills.push(entry);
        }
        self.skills.sort_by(|a, b| a.name.cmp(&b.name));
    }

    /// Remove a skill entry by name. Returns whether anything was removed.
    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.skills.len();
        self.skills.retain(|s| s.name != name);
        self.skills.len() != before
    }

    /// Look up a library server by the reference a project uses. Same
    /// precedence and qualification rules as [`Library::get`].
    pub fn get_server(&self, reference: &str) -> Option<&LibraryServer> {
        if let Some((index, name)) = self.linked.find(Kind::Server, reference) {
            return index.library.servers.iter().find(|s| s.name == name);
        }
        self.linked
            .is_empty()
            .then(|| self.servers.iter().find(|s| s.name == reference))
            .flatten()
    }

    /// Insert or replace a server entry, keeping entries sorted by name.
    pub fn upsert_server(&mut self, entry: LibraryServer) {
        if let Some(existing) = self.servers.iter_mut().find(|s| s.name == entry.name) {
            *existing = entry;
        } else {
            self.servers.push(entry);
        }
        self.servers.sort_by(|a, b| a.name.cmp(&b.name));
    }

    /// Remove a server entry by name. Returns whether anything was removed.
    pub fn remove_server(&mut self, name: &str) -> bool {
        let before = self.servers.len();
        self.servers.retain(|s| s.name != name);
        self.servers.len() != before
    }

    /// Look up a library extension by the reference a project uses. Same
    /// precedence and qualification rules as [`Library::get`].
    pub fn get_extension(&self, reference: &str) -> Option<&LibraryExtension> {
        if let Some((index, name)) = self.linked.find(Kind::Extension, reference) {
            return index.library.extensions.iter().find(|e| e.name == name);
        }
        self.linked
            .is_empty()
            .then(|| self.extensions.iter().find(|e| e.name == reference))
            .flatten()
    }

    /// Insert or replace an extension entry, keeping entries sorted by name.
    pub fn upsert_extension(&mut self, entry: LibraryExtension) {
        if let Some(existing) = self.extensions.iter_mut().find(|e| e.name == entry.name) {
            *existing = entry;
        } else {
            self.extensions.push(entry);
        }
        self.extensions.sort_by(|a, b| a.name.cmp(&b.name));
    }

    /// Remove an extension entry by name. Returns whether anything was removed.
    pub fn remove_extension(&mut self, name: &str) -> bool {
        let before = self.extensions.len();
        self.extensions.retain(|e| e.name != name);
        self.extensions.len() != before
    }

    /// Look up a library hook by the reference a project uses. Same precedence
    /// and qualification rules as [`Library::get`].
    pub fn get_hook(&self, reference: &str) -> Option<&LibraryHook> {
        if let Some((index, name)) = self.linked.find(Kind::Hook, reference) {
            return index.library.hooks.iter().find(|h| h.name == name);
        }
        self.linked
            .is_empty()
            .then(|| self.hooks.iter().find(|h| h.name == reference))
            .flatten()
    }

    /// Insert or replace a hook entry, keeping entries sorted by name.
    pub fn upsert_hook(&mut self, entry: LibraryHook) {
        if let Some(existing) = self.hooks.iter_mut().find(|h| h.name == entry.name) {
            *existing = entry;
        } else {
            self.hooks.push(entry);
        }
        self.hooks.sort_by(|a, b| a.name.cmp(&b.name));
    }

    /// Remove a hook entry by name. Returns whether anything was removed.
    pub fn remove_hook(&mut self, name: &str) -> bool {
        let before = self.hooks.len();
        self.hooks.retain(|h| h.name != name);
        self.hooks.len() != before
    }

    /// Look up a library package by the reference a toolset uses. Same
    /// precedence and qualification rules as [`Library::get`].
    pub fn get_package(&self, reference: &str) -> Option<&LibraryPackage> {
        if let Some((index, name)) = self.linked.find(Kind::Package, reference) {
            return index.library.packages.iter().find(|p| p.name == name);
        }
        self.linked
            .is_empty()
            .then(|| self.packages.iter().find(|p| p.name == reference))
            .flatten()
    }

    /// Insert or replace a package entry, keeping entries sorted by name.
    pub fn upsert_package(&mut self, entry: LibraryPackage) {
        if let Some(existing) = self.packages.iter_mut().find(|p| p.name == entry.name) {
            *existing = entry;
        } else {
            self.packages.push(entry);
        }
        self.packages.sort_by(|a, b| a.name.cmp(&b.name));
    }

    /// Remove a package entry by name. Returns whether anything was removed.
    pub fn remove_package(&mut self, name: &str) -> bool {
        let before = self.packages.len();
        self.packages.retain(|p| p.name != name);
        self.packages.len() != before
    }

    /// Look up a house-rule fragment by reference. Same shape as
    /// [`Self::get_package`]: a linked view resolves through the ordered
    /// sources (first match wins, a qualified reference only in its own
    /// source), and a bare single-file index falls back to its own list.
    pub fn get_instruction(&self, reference: &str) -> Option<&LibraryInstruction> {
        if let Some((index, name)) = self.linked.find(Kind::Instruction, reference) {
            return index.library.instructions.iter().find(|i| i.name == name);
        }
        self.linked
            .is_empty()
            .then(|| self.instructions.iter().find(|i| i.name == reference))
            .flatten()
    }

    /// Insert or replace a house-rule entry, keeping entries sorted by name.
    pub fn upsert_instruction(&mut self, entry: LibraryInstruction) {
        if let Some(existing) = self.instructions.iter_mut().find(|i| i.name == entry.name) {
            *existing = entry;
        } else {
            self.instructions.push(entry);
        }
        self.instructions.sort_by(|a, b| a.name.cmp(&b.name));
    }

    /// Remove a house-rule entry by name. Returns whether anything was removed.
    pub fn remove_instruction(&mut self, name: &str) -> bool {
        let before = self.instructions.len();
        self.instructions.retain(|i| i.name != name);
        self.instructions.len() != before
    }
}

/// Record one shadowed name, once, with every source that holds it.
///
/// Called when the merge meets a name a previous source already contributed.
/// It recomputes the full winner/shadowed set from the indexes rather than
/// accumulating it pairwise, so three sources holding one name produce one
/// collision with two shadowed entries — not two half-truths.
fn note_collision(out: &mut Vec<Collision>, indexes: &[SourceIndex], kind: Kind, name: &str) {
    if out.iter().any(|c| c.kind == kind && c.name == name) {
        return;
    }
    let mut holders = indexes
        .iter()
        .filter(|i| i.holds(kind, name))
        .map(|i| i.name.clone());
    let Some(winner) = holders.next() else {
        return;
    };
    out.push(Collision {
        kind,
        name: name.to_string(),
        winner,
        shadowed: holders.collect(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill(name: &str) -> LibrarySkill {
        LibrarySkill {
            name: name.into(),
            source: "path".into(),
            path: Some(name.into()),
            git: None,
            rev: None,
            subpath: None,
            checksum: None,
            version: None,
            provenance: Some("consolidated".into()),
        }
    }

    #[test]
    fn frontmatter_description_parses() {
        let md = "---\nname: pdf\ndescription: Fill and merge PDFs.\n---\nbody";
        assert_eq!(
            parse_frontmatter_description(md).as_deref(),
            Some("Fill and merge PDFs.")
        );
        assert_eq!(parse_frontmatter_description("no frontmatter"), None);
    }

    /// Third-party skills use full YAML frontmatter: block scalars (`|`,
    /// `>`, chomped variants) put the text on the following indented lines.
    /// The parser folds them to one line; the old behavior returned the
    /// literal "|", making described skills look undescribed everywhere.
    #[test]
    fn frontmatter_description_parses_block_scalars() {
        let folded = "---\nname: t\ndescription: >\n  Teach a topic\n  interactively.\nargument-hint: \"x\"\n---\nbody";
        assert_eq!(
            parse_frontmatter_description(folded).as_deref(),
            Some("Teach a topic interactively.")
        );
        let literal = "---\ndescription: |-\n  Builds agents.\n\n  Use when: asked.\n---\nbody";
        assert_eq!(
            parse_frontmatter_description(literal).as_deref(),
            Some("Builds agents. Use when: asked.")
        );
        // An empty block is still no description; an indented `description:`
        // inside nested structure is not the skill's.
        assert_eq!(
            parse_frontmatter_description("---\ndescription: |\nnext: k\n---\nbody"),
            None
        );
        assert_eq!(
            parse_frontmatter_description("---\nmeta:\n  description: nested\n---\nbody"),
            None
        );
    }

    #[test]
    fn skill_description_reads_path_body() {
        let dir = assert_fs::TempDir::new().unwrap();
        // A path skill body lives at `<lib_home>/skills/<path>/SKILL.md`.
        let body = dir.path().join("skills/quokka-lint");
        fs::create_dir_all(&body).unwrap();
        fs::write(
            body.join("SKILL.md"),
            "---\nname: quokka-lint\ndescription: Lint quokka configs.\n---\nbody\n",
        )
        .unwrap();

        let entry = skill("quokka-lint");
        assert_eq!(
            entry.description(dir.path()).as_deref(),
            Some("Lint quokka configs.")
        );

        // A skill whose body is absent degrades to None (no panic).
        assert_eq!(skill("ghost").description(dir.path()), None);
    }

    #[test]
    fn missing_file_loads_empty_default() {
        let dir = assert_fs::TempDir::new().unwrap();
        let lib = Library::load(dir.path()).unwrap();
        assert_eq!(lib, Library::default());
        assert!(lib.skills.is_empty());
    }

    #[test]
    fn load_checks_the_library_schema_version() {
        let dir = assert_fs::TempDir::new().unwrap();

        // The current version loads.
        fs::write(Library::path(dir.path()), "version = 1\n").unwrap();
        assert!(Library::load(dir.path()).is_ok());

        // A future version is refused, not silently misread.
        fs::write(Library::path(dir.path()), "version = 99\n").unwrap();
        let err = Library::load(dir.path()).unwrap_err().to_string();
        assert!(err.contains("library index version 99"), "{err}");
        assert!(err.contains("upgrade agentstack"), "{err}");

        // An index with no version fails deserialization (required field).
        fs::write(Library::path(dir.path()), "[[skill]]\n").unwrap();
        assert!(Library::load(dir.path()).is_err());
    }

    #[test]
    fn upsert_sorts_and_replaces() {
        let mut lib = Library::default();
        lib.upsert(skill("b"));
        lib.upsert(skill("a"));
        assert_eq!(lib.skills[0].name, "a");
        // Replace, not duplicate.
        let mut updated = skill("a");
        updated.version = Some("0.2.0".into());
        lib.upsert(updated);
        assert_eq!(lib.skills.len(), 2);
        assert_eq!(lib.get("a").unwrap().version.as_deref(), Some("0.2.0"));
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = assert_fs::TempDir::new().unwrap();
        let mut lib = Library::default();
        lib.upsert(LibrarySkill {
            name: "sql-review".into(),
            source: "git".into(),
            path: None,
            git: Some("https://example.com/skills.git".into()),
            rev: Some("abc123".into()),
            subpath: None,
            checksum: Some(Sha256Hex::of(b"deadbeef")),
            version: Some("0.1.0".into()),
            provenance: Some("catalog:sql-pack".into()),
        });
        lib.save(dir.path()).unwrap();

        let text = fs::read_to_string(Library::path(dir.path())).unwrap();
        assert!(text.contains("[[skill]]"));

        let parsed = Library::load(dir.path()).unwrap();
        assert_eq!(parsed, lib);
    }

    #[test]
    fn remove_reports_whether_present() {
        let mut lib = Library::default();
        lib.upsert(skill("a"));
        assert!(lib.remove("a"));
        assert!(!lib.remove("a"));
    }

    // ---------- servers (Phase 1b) ----------

    fn server(name: &str) -> LibraryServer {
        LibraryServer {
            name: name.into(),
            checksum: Some(Sha256Hex::of(b"cafe")),
            version: None,
            provenance: Some("consolidated:codex".into()),
        }
    }

    #[test]
    fn server_upsert_sorts_and_replaces() {
        let mut lib = Library::default();
        lib.upsert_server(server("kibana"));
        lib.upsert_server(server("figma"));
        assert_eq!(lib.servers[0].name, "figma");
        // Replace, not duplicate.
        let mut updated = server("kibana");
        updated.version = Some("2".into());
        lib.upsert_server(updated);
        assert_eq!(lib.servers.len(), 2);
        assert_eq!(
            lib.get_server("kibana").unwrap().version.as_deref(),
            Some("2")
        );
    }

    #[test]
    fn server_remove_reports_whether_present() {
        let mut lib = Library::default();
        lib.upsert_server(server("kibana"));
        assert!(lib.remove_server("kibana"));
        assert!(!lib.remove_server("kibana"));
    }

    // ---------- extensions (E3) ----------

    #[test]
    fn extension_upsert_sorts_replaces_and_roundtrips() {
        let dir = assert_fs::TempDir::new().unwrap();
        let mut lib = Library::default();
        lib.upsert_extension(LibraryExtension {
            name: "checkpoint".into(),
            source: "path".into(),
            target: "pi".into(),
            path: Some("checkpoint".into()),
            git: None,
            rev: None,
            subpath: None,
            checksum: Some("cafe".into()),
            description: Some("Checkpoint".into()),
            version: None,
            provenance: Some("path:/src".into()),
        });
        lib.upsert_extension(LibraryExtension {
            name: "audit".into(),
            source: "git".into(),
            target: "opencode".into(),
            path: None,
            git: Some("https://example.com/x.git".into()),
            rev: Some("abc".into()),
            subpath: Some("ext".into()),
            checksum: Some("beef".into()),
            description: None,
            version: None,
            provenance: Some("git:https://example.com/x.git".into()),
        });
        // Sorted by name.
        assert_eq!(lib.extensions[0].name, "audit");
        // Replace, not duplicate.
        let mut updated = lib.get_extension("checkpoint").unwrap().clone();
        updated.version = Some("0.2.0".into());
        lib.upsert_extension(updated);
        assert_eq!(lib.extensions.len(), 2);

        lib.save(dir.path()).unwrap();
        let text = fs::read_to_string(Library::path(dir.path())).unwrap();
        assert!(text.contains("[[extension]]"));
        let parsed = Library::load(dir.path()).unwrap();
        assert_eq!(parsed, lib);

        assert!(lib.remove_extension("audit"));
        assert!(!lib.remove_extension("audit"));
    }

    #[test]
    fn skills_and_servers_roundtrip_together() {
        let dir = assert_fs::TempDir::new().unwrap();
        let mut lib = Library::default();
        lib.upsert(skill("sql-review"));
        lib.upsert_server(server("kibana"));
        lib.save(dir.path()).unwrap();

        let text = fs::read_to_string(Library::path(dir.path())).unwrap();
        assert!(text.contains("[[skill]]"));
        assert!(text.contains("[[server]]"));

        let parsed = Library::load(dir.path()).unwrap();
        assert_eq!(parsed, lib);
        assert!(parsed.get_server("kibana").is_some());
        assert!(parsed.get("sql-review").is_some());
    }
}
