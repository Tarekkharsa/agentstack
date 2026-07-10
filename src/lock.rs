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
pub const SUPPORTED_LOCK_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Lock {
    pub version: u32,
    #[serde(default, rename = "skill")]
    pub skills: Vec<LockedSkill>,
    #[serde(default, rename = "server")]
    pub servers: Vec<LockedServer>,
}

impl Default for Lock {
    fn default() -> Self {
        Lock {
            version: 1,
            skills: Vec::new(),
            servers: Vec::new(),
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
        match fs::read_to_string(&path) {
            Ok(text) => {
                let lock: Lock =
                    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
                crate::util::check_schema_version(
                    lock.version,
                    SUPPORTED_LOCK_VERSION,
                    "lockfile",
                    &path,
                )?;
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

    pub fn get_server(&self, name: &str) -> Option<&LockedServer> {
        self.servers.iter().find(|s| s.name == name)
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
        fs::write(Lock::path(dir.path()), "version = 1\n").unwrap();
        assert!(Lock::load(dir.path()).is_ok());

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
}
