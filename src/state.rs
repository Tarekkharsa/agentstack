//! Managed-entry tracking: `~/.agentstack/state.json`.
//!
//! agentstack only ever touches entries it owns. The sidecar state file records,
//! per target, which servers/skills we manage and a content hash of what we last
//! wrote — so `apply` can prune entries that left the manifest, and `diff`/
//! `doctor` can detect hand-edits (drift) without polluting the target configs
//! with ownership markers. (Decision D4.)

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::util::paths;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct State {
    #[serde(default = "one")]
    pub version: u32,
    #[serde(default)]
    pub targets: BTreeMap<String, TargetState>,
}

fn one() -> u32 {
    1
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TargetState {
    #[serde(default)]
    pub managed_servers: Vec<String>,
    #[serde(default)]
    pub managed_skills: Vec<String>,
    /// Hash of the file content we last wrote, as hex. Lets us tell "unchanged"
    /// from "edited on disk since our last write".
    #[serde(default)]
    pub last_hash: String,
}

/// State key for a target in a scope. Global keeps the bare id (backward
/// compatible); project appends `@project`.
pub fn target_key(id: &str, scope: crate::scope::Scope) -> String {
    match scope {
        crate::scope::Scope::Global => id.to_string(),
        crate::scope::Scope::Project => format!("{id}@project"),
    }
}

impl State {
    pub fn path() -> PathBuf {
        paths::agentstack_home().join("state.json")
    }

    /// Load state, returning a default (empty) state if the file is absent.
    pub fn load() -> Result<Self> {
        let path = Self::path();
        match fs::read_to_string(&path) {
            Ok(text) => {
                serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State {
                version: 1,
                ..Default::default()
            }),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        let mut text = serde_json::to_string_pretty(self)?;
        text.push('\n');
        fs::write(&path, text).with_context(|| format!("writing {}", path.display()))
    }

    /// Servers we previously managed under `key` (empty if never applied).
    pub fn managed_servers(&self, key: &str) -> Vec<String> {
        self.targets
            .get(key)
            .map(|t| t.managed_servers.clone())
            .unwrap_or_default()
    }

    /// Skills we previously materialized under `key`.
    pub fn managed_skills(&self, key: &str) -> Vec<String> {
        self.targets
            .get(key)
            .map(|t| t.managed_skills.clone())
            .unwrap_or_default()
    }

    /// Record servers + a content hash for `key`.
    pub fn record(&mut self, key: &str, managed_servers: Vec<String>, content: &str) {
        let entry = self.targets.entry(key.to_string()).or_default();
        entry.managed_servers = managed_servers;
        entry.last_hash = hash(content);
    }

    /// Record the materialized skill set for `key`.
    pub fn record_skills(&mut self, key: &str, managed_skills: Vec<String>) {
        let entry = self.targets.entry(key.to_string()).or_default();
        entry.managed_skills = managed_skills;
    }
}

/// Stable, dependency-free content hash (FNV-1a 64) rendered as hex. Used only
/// for change detection, not security.
pub fn hash(s: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_stable_and_distinguishes() {
        assert_eq!(hash("abc"), hash("abc"));
        assert_ne!(hash("abc"), hash("abd"));
    }

    #[test]
    fn record_updates_target() {
        let mut s = State::default();
        s.record("codex", vec!["kibana".into()], "content");
        assert_eq!(s.managed_servers("codex"), vec!["kibana".to_string()]);
        assert!(!s.targets["codex"].last_hash.is_empty());
    }
}
