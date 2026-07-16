//! `agentstack.lock` — pins each capability's resolved source for reproducible,
//! SHA-256 checksum-verified installs (PLAN §9d, D12). Lives next to the
//! manifest.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const LOCK_FILE: &str = "agentstack.lock";
/// Newest lockfile schema version this build reads and writes. Anything above
/// it was written by a future agentstack; [`Lock::load`] refuses it instead of
/// misinterpreting silently.
///
/// v2 added `[[instruction]]` pins. The bump matters because TOML parsing here
/// is permissive (no `deny_unknown_fields`): a v1 binary reading a lock with
/// instruction pins would silently drop them — and write them away on its next
/// save. The version guard turns that silent unpinning into a loud refusal.
pub const SUPPORTED_LOCK_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Lock {
    pub version: u32,
    #[serde(default, rename = "skill")]
    pub skills: Vec<LockedSkill>,
    #[serde(default, rename = "server")]
    pub servers: Vec<LockedServer>,
    #[serde(default, rename = "instruction")]
    pub instructions: Vec<LockedInstruction>,
    /// D3 executable pins (locked-run contract §8). Additive `#[serde(default)]`
    /// fields at version 2, unlike the v1→v2 instruction-pin bump: a pre-D3 v2
    /// binary that rewrites these pins away changes the lock bytes, which flips
    /// the trust digest and forces re-review — and both the trust gate and
    /// strict locked verification block unpinned repo-relative executables, so
    /// silent unpinning cannot pass any gate downstream.
    #[serde(default, rename = "executable")]
    pub executables: Vec<LockedExecutable>,
}

impl Default for Lock {
    fn default() -> Self {
        Lock {
            version: SUPPORTED_LOCK_VERSION,
            skills: Vec::new(),
            servers: Vec::new(),
            instructions: Vec::new(),
            executables: Vec::new(),
        }
    }
}

/// A pinned MCP server: the SHA-256 of its **definition** (a `${REF}`-only
/// server table — never resolved secret values, never a provider-specific render
/// shape), so a fresh checkout resolves the same server. `source` is `"inline"`
/// or `"library"`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockedServer {
    pub name: String,
    pub source: String,
    pub checksum: String,
}

/// A pinned instruction fragment: the SHA-256 of the file's raw bytes.
/// Instructions are always local files declared by path in the manifest —
/// no source/git/rev fields apply. Machine-layer (user-layer) fragments are
/// never pinned; they aren't repo content.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockedInstruction {
    pub name: String,
    pub path: String,
    pub checksum: String,
}

/// How a D3 executable pin's digest was computed — the two families are not
/// interchangeable (see `agentstack_core::digest`).
///
/// (TS mental model: a string-literal union `"file" | "root"` with exhaustive
/// `match` at every consumer.)
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum ExecutableKind {
    /// One repository-relative file (a stdio `command` or interpreter-script
    /// `args` entry), pinned by `contained_file_digest` — raw file bytes.
    File,
    /// A declared integrity root (file or directory subtree), pinned by the
    /// symlink-rejecting, domain-separated `integrity_root_digest`.
    Root,
}

/// A pinned repository-local executable input (D3, contract §8): the
/// repo-relative path as declared in the manifest, which digest family pinned
/// it, and the content checksum. Identity is `(path, kind)` — the same path
/// may legitimately carry both a file pin (as an `args` entry) and a root pin
/// (as a declared root), and the two digests differ.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockedExecutable {
    pub path: String,
    pub kind: ExecutableKind,
    pub checksum: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockedSkill {
    pub name: String,
    /// `path` or `git`.
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    pub checksum: String,
}

impl Lock {
    pub fn path(dir: &Path) -> PathBuf {
        dir.join(LOCK_FILE)
    }

    pub fn load(dir: &Path) -> Result<Self> {
        let path = Self::path(dir);
        // Bounded: a cloned repo's lockfile is hostile input (rule 7).
        match crate::util::read_to_string_bounded(&path, crate::util::MAX_CONFIG_BYTES) {
            Ok(text) => {
                let lock: Lock =
                    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
                crate::util::check_schema_version(
                    lock.version,
                    SUPPORTED_LOCK_VERSION,
                    "lockfile",
                    &path,
                )?;
                // Normalize the in-memory version so struct equality (and the
                // callers' "no-op if unchanged" save checks) stays honest: an
                // untouched older lock is never rewritten just to bump its
                // version, but any genuine write upgrades the file.
                let mut lock = lock;
                lock.version = SUPPORTED_LOCK_VERSION;
                Ok(lock)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Lock::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        let path = Self::path(dir);
        let text = toml::to_string_pretty(self)?;
        fs::write(&path, text).with_context(|| format!("writing {}", path.display()))
    }

    pub fn get(&self, name: &str) -> Option<&LockedSkill> {
        self.skills.iter().find(|s| s.name == name)
    }

    /// Insert or replace a skill entry, keeping entries sorted by name.
    pub fn upsert(&mut self, entry: LockedSkill) {
        if let Some(existing) = self.skills.iter_mut().find(|s| s.name == entry.name) {
            *existing = entry;
        } else {
            self.skills.push(entry);
        }
        self.skills.sort_by(|a, b| a.name.cmp(&b.name));
    }

    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.skills.len();
        self.skills.retain(|s| s.name != name);
        self.skills.len() != before
    }

    /// Drop locked skills whose name is no longer in `keep`.
    pub fn retain_names(&mut self, keep: &[String]) {
        self.skills.retain(|s| keep.contains(&s.name));
    }

    pub fn get_instruction(&self, name: &str) -> Option<&LockedInstruction> {
        self.instructions.iter().find(|i| i.name == name)
    }

    /// Insert or replace an instruction entry, keeping entries sorted by name.
    pub fn upsert_instruction(&mut self, entry: LockedInstruction) {
        if let Some(existing) = self.instructions.iter_mut().find(|i| i.name == entry.name) {
            *existing = entry;
        } else {
            self.instructions.push(entry);
        }
        self.instructions.sort_by(|a, b| a.name.cmp(&b.name));
    }

    /// Drop pinned instructions whose name is no longer in `keep` — stale pins
    /// for removed fragments are pruned by `agentstack lock`.
    pub fn retain_instruction_names(&mut self, keep: &[String]) {
        self.instructions.retain(|i| keep.contains(&i.name));
    }

    pub fn get_server(&self, name: &str) -> Option<&LockedServer> {
        self.servers.iter().find(|s| s.name == name)
    }

    pub fn get_executable(&self, path: &str, kind: ExecutableKind) -> Option<&LockedExecutable> {
        self.executables
            .iter()
            .find(|e| e.path == path && e.kind == kind)
    }

    /// Insert or replace an executable pin, keeping entries sorted by
    /// `(path, kind)`.
    pub fn upsert_executable(&mut self, entry: LockedExecutable) {
        if let Some(existing) = self
            .executables
            .iter_mut()
            .find(|e| e.path == entry.path && e.kind == entry.kind)
        {
            *existing = entry;
        } else {
            self.executables.push(entry);
        }
        self.executables
            .sort_by(|a, b| (&a.path, a.kind).cmp(&(&b.path, b.kind)));
    }

    /// Drop executable pins no longer in `keep` — stale pins for paths a
    /// re-lock no longer derives from the manifest are pruned, so the lock
    /// never carries dead pins that mask a renamed payload.
    pub fn retain_executables(&mut self, keep: &[(String, ExecutableKind)]) {
        self.executables
            .retain(|e| keep.iter().any(|(p, k)| *p == e.path && *k == e.kind));
    }

    /// Insert or replace a server entry, keeping entries sorted by name.
    pub fn upsert_server(&mut self, entry: LockedServer) {
        if let Some(existing) = self.servers.iter_mut().find(|s| s.name == entry.name) {
            *existing = entry;
        } else {
            self.servers.push(entry);
        }
        self.servers.sort_by(|a, b| a.name.cmp(&b.name));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_checks_the_lock_schema_version() {
        let dir = assert_fs::TempDir::new().unwrap();

        // No lockfile → empty default (unchanged).
        assert_eq!(Lock::load(dir.path()).unwrap(), Lock::default());

        // The current version loads.
        fs::write(
            Lock::path(dir.path()),
            format!("version = {SUPPORTED_LOCK_VERSION}\n"),
        )
        .unwrap();
        assert!(Lock::load(dir.path()).is_ok());

        // An older (v1, pre-instruction-pins) lock still loads, and its
        // in-memory version normalizes to the current one so a genuine write
        // upgrades the file.
        fs::write(Lock::path(dir.path()), "version = 1\n").unwrap();
        assert_eq!(
            Lock::load(dir.path()).unwrap().version,
            SUPPORTED_LOCK_VERSION
        );

        // A future version is refused, not silently misread.
        fs::write(Lock::path(dir.path()), "version = 99\n").unwrap();
        let err = Lock::load(dir.path()).unwrap_err().to_string();
        assert!(err.contains("lockfile version 99"), "{err}");
        assert!(err.contains("upgrade agentstack"), "{err}");

        // A lockfile with no version fails deserialization (required field).
        fs::write(Lock::path(dir.path()), "[[skill]]\n").unwrap();
        assert!(Lock::load(dir.path()).is_err());
    }

    #[test]
    fn upsert_and_roundtrip() {
        let mut lock = Lock::default();
        lock.upsert(LockedSkill {
            name: "b".into(),
            source: "path".into(),
            path: Some("./b".into()),
            git: None,
            rev: None,
            checksum: "deadbeef".into(),
        });
        lock.upsert(LockedSkill {
            name: "a".into(),
            source: "git".into(),
            path: None,
            git: Some("https://x".into()),
            rev: Some("abc".into()),
            checksum: "cafe".into(),
        });
        // Sorted by name.
        assert_eq!(lock.skills[0].name, "a");
        let text = toml::to_string_pretty(&lock).unwrap();
        let parsed: Lock = toml::from_str(&text).unwrap();
        assert_eq!(parsed.skills, lock.skills);
        assert!(text.contains("[[skill]]"));
    }

    #[test]
    fn server_upsert_sorts_and_roundtrips() {
        let mut lock = Lock::default();
        lock.upsert_server(LockedServer {
            name: "kibana".into(),
            source: "library".into(),
            checksum: "cafe".into(),
        });
        lock.upsert_server(LockedServer {
            name: "figma".into(),
            source: "inline".into(),
            checksum: "beef".into(),
        });
        assert_eq!(lock.servers[0].name, "figma", "sorted by name");
        assert_eq!(lock.get_server("kibana").unwrap().source, "library");

        let text = toml::to_string_pretty(&lock).unwrap();
        assert!(text.contains("[[server]]"));
        let parsed: Lock = toml::from_str(&text).unwrap();
        assert_eq!(parsed.servers, lock.servers);
    }

    #[test]
    fn executable_upsert_sorts_roundtrips_and_retains() {
        let mut lock = Lock::default();
        lock.upsert_executable(LockedExecutable {
            path: "tools".into(),
            kind: ExecutableKind::Root,
            checksum: "cafe".into(),
        });
        lock.upsert_executable(LockedExecutable {
            path: "scripts/run.sh".into(),
            kind: ExecutableKind::File,
            checksum: "beef".into(),
        });
        assert_eq!(lock.executables[0].path, "scripts/run.sh", "sorted by path");

        // Identity is (path, kind): the same path carries both pin kinds.
        lock.upsert_executable(LockedExecutable {
            path: "tools".into(),
            kind: ExecutableKind::File,
            checksum: "f00d".into(),
        });
        assert_eq!(lock.executables.len(), 3);
        assert_eq!(
            lock.get_executable("tools", ExecutableKind::File)
                .unwrap()
                .checksum,
            "f00d"
        );
        assert_eq!(
            lock.get_executable("tools", ExecutableKind::Root)
                .unwrap()
                .checksum,
            "cafe"
        );

        // Upsert replaces in place, keyed by both path and kind.
        lock.upsert_executable(LockedExecutable {
            path: "tools".into(),
            kind: ExecutableKind::Root,
            checksum: "0000".into(),
        });
        assert_eq!(lock.executables.len(), 3);
        assert_eq!(
            lock.get_executable("tools", ExecutableKind::Root)
                .unwrap()
                .checksum,
            "0000"
        );

        let text = toml::to_string_pretty(&lock).unwrap();
        assert!(text.contains("[[executable]]"));
        assert!(text.contains("kind = \"root\""));
        assert!(text.contains("kind = \"file\""));
        let parsed: Lock = toml::from_str(&text).unwrap();
        assert_eq!(parsed.executables, lock.executables);

        // Prune to the derived set.
        lock.retain_executables(&[("tools".to_string(), ExecutableKind::Root)]);
        assert_eq!(lock.executables.len(), 1);
        assert!(lock.get_executable("tools", ExecutableKind::Root).is_some());
    }

    #[test]
    fn pre_d3_lock_without_executables_parses_to_empty() {
        // Ruling: additive #[serde(default)] fields, no version bump — an
        // existing v2 lock with no [[executable]] entries must load with an
        // empty pin set (the trust gate and strict verification decide what an
        // absent pin means; parsing never invents one).
        let parsed: Lock =
            toml::from_str(&format!("version = {SUPPORTED_LOCK_VERSION}\n")).unwrap();
        assert!(parsed.executables.is_empty());
    }

    #[test]
    fn instruction_upsert_sorts_roundtrips_and_retains() {
        let mut lock = Lock::default();
        lock.upsert_instruction(LockedInstruction {
            name: "style".into(),
            path: "./instructions/style.md".into(),
            checksum: "cafe".into(),
        });
        lock.upsert_instruction(LockedInstruction {
            name: "house".into(),
            path: "./instructions/house.md".into(),
            checksum: "beef".into(),
        });
        assert_eq!(lock.instructions[0].name, "house", "sorted by name");

        // Upsert replaces in place.
        lock.upsert_instruction(LockedInstruction {
            name: "house".into(),
            path: "./instructions/house.md".into(),
            checksum: "f00d".into(),
        });
        assert_eq!(lock.get_instruction("house").unwrap().checksum, "f00d");

        let text = toml::to_string_pretty(&lock).unwrap();
        assert!(text.contains("[[instruction]]"));
        let parsed: Lock = toml::from_str(&text).unwrap();
        assert_eq!(parsed.instructions, lock.instructions);

        // Prune to the declared set.
        lock.retain_instruction_names(&["style".to_string()]);
        assert!(lock.get_instruction("house").is_none());
        assert!(lock.get_instruction("style").is_some());
    }
}
