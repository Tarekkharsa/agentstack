//! `~/.agentstack/lib/library.toml` — the central capability library index.
//!
//! The library is the single managed home a project references capabilities from
//! by name, instead of copying capability files into each repo (see
//! `docs/reference.md#the-central-library`). This module is the inert foundation: it models the
//! index and loads/saves it. It performs **no resolution** — mapping a project's
//! `skills = ["name"]` reference to a library entry is the resolver's job, added
//! on top of this in a later step.
//!
//! Skills ship in Phase 1; servers are modeled here for Phase 1b (the resolver
//! wiring lands in a later step); `hooks` remain future work. The file is an
//! index, not a scan target: entries carry provenance and an integrity digest so
//! `lib list`, `explain`, and drift checks have metadata to work with.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::manifest::Skill;
use crate::store::Store;
use crate::util::paths;

/// Parse the one-line `description:` value from a `SKILL.md` YAML frontmatter
/// block — the leading `---` … `---` fence. Returns `None` when there's no
/// frontmatter or no `description:` key. This is the single shared parser for
/// every surface that shows skill descriptions: central-library search
/// (`agentstack search`), `lib list`, and the MCP loadable catalog
/// (`mcp_server`), which all call it rather than re-implementing it.
pub fn parse_frontmatter_description(md: &str) -> Option<String> {
    let rest = md.trim_start().strip_prefix("---")?;
    let end = rest.find("\n---")?;
    for line in rest[..end].lines() {
        if let Some(v) = line.trim().strip_prefix("description:") {
            return Some(v.trim().trim_matches('"').trim_matches('\'').to_string());
        }
    }
    None
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
}

impl Default for Library {
    fn default() -> Self {
        Library {
            version: 1,
            skills: Vec::new(),
            servers: Vec::new(),
        }
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
    pub checksum: Option<String>,
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
        // Reuse the resolver's view of a library skill (path relative to
        // `<lib_home>/skills/`, or a cached git clone) without digesting — the
        // same shape `resolve_skill` builds for `SkillOrigin::Library`.
        let skill = Skill {
            path: self.path.clone(),
            git: self.git.clone(),
            rev: self.rev.clone(),
            subpath: self.subpath.clone(),
        };
        let dir = Store::default_store()
            .resolve_path_only(&skill, &lib_home.join("skills"))
            .ok()
            .flatten()?
            .path;
        let text = fs::read_to_string(dir.join("SKILL.md")).ok()?;
        parse_frontmatter_description(&text)
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
    pub checksum: Option<String>,
    /// Optional declared version for the entry (informational).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Where the entry came from (e.g. `"consolidated:<provider>"`, `"manual"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<String>,
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

    /// Load the index from the default library home (`~/.agentstack/lib`, honoring
    /// `AGENTSTACK_HOME`).
    pub fn load_default() -> Result<Self> {
        Self::load(&paths::lib_home())
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
    pub fn save(&self, lib_home: &Path) -> Result<()> {
        fs::create_dir_all(lib_home).with_context(|| format!("creating {}", lib_home.display()))?;
        let path = Self::path(lib_home);
        let text = toml::to_string_pretty(self)?;
        fs::write(&path, text).with_context(|| format!("writing {}", path.display()))
    }

    /// Look up a library skill by the name a project references it by.
    pub fn get(&self, name: &str) -> Option<&LibrarySkill> {
        self.skills.iter().find(|s| s.name == name)
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

    /// Look up a library server by the name a project references it by.
    pub fn get_server(&self, name: &str) -> Option<&LibraryServer> {
        self.servers.iter().find(|s| s.name == name)
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
            checksum: Some("deadbeef".into()),
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
            checksum: Some("cafe".into()),
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
