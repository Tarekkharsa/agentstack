//! `~/.agentstack/sources.toml` — the ordered list of **linked library
//! sources** (see `docs/design/linked-library-sources.md`).
//!
//! There is no longer *the* library. Any folder on the device can be linked as
//! a library source, several at once, and the order of the list is the
//! precedence order: the first source holding a capability of the requested
//! kind and name wins — `PATH` semantics.
//!
//! Two properties of this file are load-bearing rather than incidental:
//!
//! - **It is personal-layer state, never project state.** The list is absolute
//!   paths on one machine; a repository must not be able to add a source, or
//!   repository content could point resolution at a folder the user never
//!   linked (invariant 3). Nothing outside `AGENTSTACK_HOME` is ever consulted.
//! - **Absent means today's behaviour.** A missing file is not an empty list;
//!   it is the single implicit source `local` → [`paths::lib_home`]. A user who
//!   never links anything sees exactly the single central library they had
//!   before, and this file is only ever created by `agentstack lib link`.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::util::paths;

/// The link list's filename, directly under `AGENTSTACK_HOME`.
pub const SOURCES_FILE: &str = "sources.toml";

/// Newest link-list schema version this build reads and writes. Anything above
/// it was written by a future agentstack; [`Sources::load`] refuses it instead
/// of misinterpreting silently — same contract as the library index.
pub const SUPPORTED_SOURCES_VERSION: u32 = 1;

/// The name of the implicit first source, `~/.agentstack/lib`. It is a real
/// name, not a placeholder: `local:pdf` addresses the central library
/// explicitly even on a machine that has never linked a second folder.
pub const DEFAULT_SOURCE_NAME: &str = "local";

/// The separator between a source name and a capability name in a
/// fully-qualified reference: `team:sql-review`. Chosen because `:` cannot
/// occur in a capability name (the name contract admits only `[a-z0-9._-]`),
/// so the split needs no escaping rule.
pub const QUALIFIER: char = ':';

/// The prefix of the explicit library-reference spelling,
/// `lib:<source>/<name>`.
///
/// The same selection as `<source>:<name>`, written so the *origin* is
/// legible without knowing the link list: `central:rust-testing` reads like a
/// name with a colon in it, `lib:central/rust-testing` says out loud that this
/// capability comes from a linked library called `central`. It is the form
/// `agentstack add from` accepts, so one command line both selects a library
/// skill and states that it is one.
///
/// Deliberately NOT a second identity: [`capability_name`] returns the same
/// bare name for both spellings, so the lock key, the rendered directory and
/// the gateway name are untouched by which spelling a manifest uses.
pub const LIB_PREFIX: &str = "lib:";

/// One linked source, as recorded on disk. Position in the file is precedence.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceEntry {
    /// The name this source is addressed by in a qualified reference. Unique
    /// in the list, and validated by the ordinary capability name contract.
    pub name: String,
    /// The folder, stored `~`-relative when it is under the home directory so
    /// the file stays readable and portable between accounts.
    pub path: String,
    /// Optional one-line note the user attached when linking.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// The link list. An empty list means "not configured", which resolves to the
/// implicit default source rather than to nothing (see [`Sources::linked`]).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Sources {
    pub version: u32,
    #[serde(default, rename = "source")]
    pub sources: Vec<SourceEntry>,
}

impl Default for Sources {
    fn default() -> Self {
        Sources {
            version: SUPPORTED_SOURCES_VERSION,
            sources: Vec::new(),
        }
    }
}

/// A linked source resolved for use: its name and the absolute folder it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedSource {
    pub name: String,
    pub root: PathBuf,
    /// False for the implicit `local` source on a machine with no link list —
    /// surfaces so `lib sources` can say "the default" rather than imply the
    /// user configured it.
    pub explicit: bool,
}

impl Sources {
    /// `~/.agentstack/sources.toml`, honoring `AGENTSTACK_HOME`.
    pub fn path() -> PathBuf {
        paths::agentstack_home().join(SOURCES_FILE)
    }

    /// Load the link list. A missing file yields the default (empty) list —
    /// the machine simply has not linked anything beyond the central library.
    pub fn load() -> Result<Self> {
        let path = Self::path();
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let sources: Sources =
                    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
                crate::util::check_schema_version(
                    sources.version,
                    SUPPORTED_SOURCES_VERSION,
                    "library source list",
                    &path,
                )?;
                Ok(sources)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Sources::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// Best-effort load for surfaces that must not fail because the personal
    /// layer is unreadable. The error is reported rather than swallowed, and
    /// the machine falls back to the single central library — the same
    /// degradation `Library::load_default_or_warn` already performs.
    pub fn load_or_warn() -> Self {
        Self::load().unwrap_or_else(|e| {
            eprintln!("warning: library source list unavailable ({e:#}); using ~/.agentstack/lib");
            Sources::default()
        })
    }

    /// Write the list atomically. A torn write here would leave the machine
    /// with an unreadable source list and every project's names unresolvable,
    /// which is the one state neither a link nor an unlink can reason about.
    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let text = toml::to_string_pretty(self)?;
        crate::util::atomic::write(&path, &text)
            .with_context(|| format!("writing {}", path.display()))
    }

    /// The ordered sources to resolve against. An empty list resolves to the
    /// single implicit `local` source, so a machine that never linked anything
    /// behaves exactly as it did before linked sources existed.
    pub fn linked(&self) -> Vec<LinkedSource> {
        if self.sources.is_empty() {
            return vec![LinkedSource {
                name: DEFAULT_SOURCE_NAME.to_string(),
                root: paths::lib_home(),
                explicit: false,
            }];
        }
        self.sources
            .iter()
            .map(|s| LinkedSource {
                name: s.name.clone(),
                root: paths::expand_tilde(&s.path),
                explicit: true,
            })
            .collect()
    }

    /// The first linked source — where library-first authoring and `init`'s
    /// import land. Always present: [`Sources::linked`] never returns empty.
    pub fn primary(&self) -> LinkedSource {
        self.linked()
            .into_iter()
            .next()
            .expect("linked() always yields at least the implicit source")
    }

    /// Add a source at the end of the list (or at the front with `first`).
    /// Refuses a duplicate name or a folder that is already linked: two names
    /// for one folder would make the collision report describe a folder
    /// shadowing itself.
    pub fn link(&mut self, name: &str, root: &Path, first: bool, note: Option<&str>) -> Result<()> {
        crate::text::validate_name(name).with_context(|| {
            format!(
                "refusing '{}' as a library source name",
                name.escape_debug()
            )
        })?;
        // The list starts implicit. The moment the user links a *second*
        // folder, the central library has to become an explicit entry too, or
        // linking would silently unlink it.
        if self.sources.is_empty() {
            let lib = paths::lib_home();
            if lib != root {
                self.sources.push(SourceEntry {
                    name: DEFAULT_SOURCE_NAME.to_string(),
                    path: tildify(&lib),
                    note: None,
                });
            }
        }
        if self.sources.iter().any(|s| s.name == name) {
            bail!("a library source named '{name}' is already linked — unlink it first, or pick another name");
        }
        if let Some(existing) = self
            .sources
            .iter()
            .find(|s| paths::expand_tilde(&s.path) == root)
        {
            bail!(
                "{} is already linked as '{}'",
                root.display(),
                existing.name
            );
        }
        let entry = SourceEntry {
            name: name.to_string(),
            path: tildify(root),
            note: note.map(crate::text::sanitize_line),
        };
        if first {
            self.sources.insert(0, entry);
        } else {
            self.sources.push(entry);
        }
        Ok(())
    }

    /// Remove a source by name. Returns whether anything was removed.
    pub fn unlink(&mut self, name: &str) -> bool {
        let before = self.sources.len();
        self.sources.retain(|s| s.name != name);
        self.sources.len() != before
    }

    /// Reorder the list to exactly `order`. Every linked source must be named
    /// once: a partial order would silently decide the precedence of the names
    /// the user left out, which is the failure this whole rule exists to avoid.
    pub fn reorder(&mut self, order: &[String]) -> Result<()> {
        let current = self.linked();
        if order.len() != current.len() {
            bail!(
                "reorder expects every linked source exactly once ({} linked: {})",
                current.len(),
                current
                    .iter()
                    .map(|s| s.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        for name in order {
            if !current.iter().any(|s| &s.name == name) {
                bail!("'{}' is not a linked library source", name.escape_debug());
            }
            if order.iter().filter(|n| *n == name).count() > 1 {
                bail!("'{}' is named twice in the new order", name.escape_debug());
            }
        }
        // Materialize the implicit source before reordering, so the written
        // file says what the user just chose rather than staying implicit.
        if self.sources.is_empty() {
            self.sources = current
                .iter()
                .map(|s| SourceEntry {
                    name: s.name.clone(),
                    path: tildify(&s.root),
                    note: None,
                })
                .collect();
        }
        let mut reordered = Vec::with_capacity(self.sources.len());
        for name in order {
            if let Some(pos) = self.sources.iter().position(|s| &s.name == name) {
                reordered.push(self.sources.remove(pos));
            }
        }
        self.sources = reordered;
        Ok(())
    }
}

/// Store a path `~`-relative when it is under the home directory. Cosmetic for
/// the file, but it keeps `sources.toml` readable and survives a home that
/// moves (a synced dotfiles directory, a renamed account).
fn tildify(path: &Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(rest) = path.strip_prefix(&home) {
            if rest.as_os_str().is_empty() {
                return "~".to_string();
            }
            return format!("~/{}", rest.display());
        }
    }
    path.display().to_string()
}

/// Split an explicit `lib:<source>/<name>` reference into
/// `(source, capability)`. `None` for anything else, including a malformed
/// `lib:` reference — a caller that wants to *report* the malformation asks
/// for the prefix itself ([`LIB_PREFIX`]) and then finds this returning
/// `None`.
pub fn split_lib_reference(reference: &str) -> Option<(&str, &str)> {
    let rest = reference.strip_prefix(LIB_PREFIX)?;
    let (source, name) = rest.split_once('/')?;
    // One segment each: `lib:a/b/c` names nothing this resolver can address,
    // and silently taking `a` + `b/c` would invent a path where the reference
    // grammar has none.
    if source.is_empty() || name.is_empty() || name.contains('/') || source.contains(QUALIFIER) {
        return None;
    }
    Some((source, name))
}

/// Split a fully-qualified reference into `(source, capability)`. `None` for a
/// bare reference — the common case, which resolves through the ordered list.
///
/// Both qualified spellings land here — `<source>:<name>` and the explicit
/// `lib:<source>/<name>` — so every resolver, validator and reporter that
/// already asked this question understands the new form without being told.
pub fn split_reference(reference: &str) -> Option<(&str, &str)> {
    if let Some(split) = split_lib_reference(reference) {
        return Some(split);
    }
    let (source, name) = reference.split_once(QUALIFIER)?;
    if source.is_empty() || name.is_empty() {
        return None;
    }
    Some((source, name))
}

/// The capability's own name in a reference: the part after any `<source>:`.
/// The qualifier is a **selector**, never part of the identity — the lock key,
/// the rendered directory, and the gateway's name for a capability are all this
/// bare name, whichever source it came from.
pub fn capability_name(reference: &str) -> &str {
    split_reference(reference).map_or(reference, |(_, name)| name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The explicit spelling selects exactly what the colon spelling selects,
    /// and names the same capability — a second spelling, never a second
    /// identity.
    #[test]
    fn the_lib_spelling_is_the_colon_spelling_said_out_loud() {
        assert_eq!(
            split_reference("lib:central/rust-testing"),
            Some(("central", "rust-testing"))
        );
        assert_eq!(
            split_reference("central:rust-testing"),
            Some(("central", "rust-testing"))
        );
        assert_eq!(capability_name("lib:central/rust-testing"), "rust-testing");
        assert_eq!(capability_name("central:rust-testing"), "rust-testing");
    }

    /// A malformed `lib:` reference must not decay into the colon grammar and
    /// resolve as a source literally named `lib` — that would turn a typo into
    /// a lookup in a source the user never linked.
    #[test]
    fn a_malformed_lib_reference_names_nothing() {
        for bad in [
            "lib:central",
            "lib:/rust-testing",
            "lib:central/",
            "lib:central/nested/name",
            "lib:",
        ] {
            assert_eq!(split_lib_reference(bad), None, "{bad}");
        }
        // `lib:central` still splits under the colon grammar — a source really
        // named `lib` is legal — but it is NOT the library spelling.
        assert_eq!(split_reference("lib:central"), Some(("lib", "central")));
    }

    #[test]
    fn an_absent_list_is_the_single_central_library() {
        let sources = Sources::default();
        let linked = sources.linked();
        assert_eq!(linked.len(), 1);
        assert_eq!(linked[0].name, DEFAULT_SOURCE_NAME);
        assert!(!linked[0].explicit);
    }

    #[test]
    fn linking_the_first_extra_folder_materializes_the_central_library() {
        let _guard = agentstack_core::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", home.path());

        let mut sources = Sources::default();
        sources
            .link("team", &home.path().join("team"), false, None)
            .unwrap();
        // `local` first, `team` second — linking must never silently drop the
        // library the machine already had.
        assert_eq!(
            sources
                .sources
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            vec!["local", "team"]
        );

        std::env::remove_var("AGENTSTACK_HOME");
    }

    #[test]
    fn a_duplicate_name_or_folder_is_refused() {
        let _guard = agentstack_core::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", home.path());

        let mut sources = Sources::default();
        let team = home.path().join("team");
        sources.link("team", &team, false, None).unwrap();
        assert!(sources
            .link("team", &home.path().join("other"), false, None)
            .is_err());
        let err = sources
            .link("team2", &team, false, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("already linked as 'team'"), "{err}");

        std::env::remove_var("AGENTSTACK_HOME");
    }

    #[test]
    fn reorder_requires_the_whole_list() {
        let _guard = agentstack_core::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", home.path());

        let mut sources = Sources::default();
        sources
            .link("team", &home.path().join("team"), false, None)
            .unwrap();
        assert!(sources.reorder(&["team".to_string()]).is_err());
        sources
            .reorder(&["team".to_string(), "local".to_string()])
            .unwrap();
        assert_eq!(sources.sources[0].name, "team");

        std::env::remove_var("AGENTSTACK_HOME");
    }

    #[test]
    fn a_qualified_reference_splits_and_a_bare_one_does_not() {
        assert_eq!(
            split_reference("team:sql-review"),
            Some(("team", "sql-review"))
        );
        assert_eq!(split_reference("sql-review"), None);
        assert_eq!(split_reference(":x"), None);
        assert_eq!(capability_name("team:sql-review"), "sql-review");
        assert_eq!(capability_name("sql-review"), "sql-review");
    }
}
