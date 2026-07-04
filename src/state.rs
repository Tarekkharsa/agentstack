//! Managed-entry tracking: `~/.agentstack/state.json`.
//!
//! agentstack only ever touches entries it owns. The sidecar state file records,
//! per target, which servers/skills we manage and a content hash of what we last
//! wrote — so `apply` can prune entries that left the manifest, and `diff`/
//! `doctor` can detect hand-edits (drift) without polluting the target configs
//! with ownership markers. (Decision D4.)

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

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
    /// Top-level keys we own in this target's native settings file.
    #[serde(default)]
    pub managed_settings: Vec<String>,
    /// Hook names we compiled into this target's native hooks config.
    #[serde(default)]
    pub managed_hooks: Vec<String>,
    /// Hash of the file content we last wrote, as hex. Lets us tell "unchanged"
    /// from "edited on disk since our last write".
    #[serde(default)]
    pub last_hash: String,
    /// The manifest (canonical `agentstack.toml` path) that last recorded
    /// `managed_servers`. Global-scope keys are shared by every manifest on
    /// the machine, so this is what lets a prune tell "left *my* manifest"
    /// from "belongs to a *different* manifest" (see [`State::foreign_prunes`]).
    /// Absent in pre-schema state files — those load fine and get the field on
    /// their next apply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_manifest: Option<String>,
    /// Foreign-manifest server names a guarded write *kept* on disk (see
    /// [`State::foreign_prunes`]). They are present in the live config but no
    /// longer in `managed_servers` once the writing manifest records its own
    /// set — this list keeps them reachable so `apply --prune-foreign` still
    /// prunes them later and `doctor`/`diff` keep reporting the adopt-or-prune
    /// choice. Empty (default) in pre-schema state files.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kept_foreign: Vec<String>,
}

/// Identity of the manifest at `dir` for the cross-manifest prune guard: the
/// canonical path of its `agentstack.toml` (lexical fallback when the file
/// can't be canonicalized, e.g. it vanished mid-run).
pub fn manifest_identity(dir: &Path) -> String {
    let p = dir.join(crate::manifest::MANIFEST_FILE);
    p.canonicalize().unwrap_or(p).display().to_string()
}

/// State key for a target in a scope. Global keeps the bare id (backward
/// compatible); project appends `@project:<project root>` — per directory, so
/// activating one project never inherits (or prunes against) another
/// project's bookkeeping.
pub fn target_key(id: &str, scope: crate::scope::Scope, manifest_dir: &std::path::Path) -> String {
    match scope {
        crate::scope::Scope::Global => id.to_string(),
        crate::scope::Scope::Project => {
            let root = crate::manifest::project_root_of(manifest_dir);
            format!("{id}@project:{}", root.display())
        }
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

    /// Record servers + a content hash for `key`, stamped with the manifest
    /// that wrote them (`source` — see [`manifest_identity`]).
    pub fn record(&mut self, key: &str, managed_servers: Vec<String>, content: &str, source: &str) {
        let entry = self.targets.entry(key.to_string()).or_default();
        entry.managed_servers = managed_servers;
        entry.last_hash = hash(content);
        entry.source_manifest = Some(source.to_string());
    }

    /// Foreign-manifest server names a guarded write left on disk under
    /// `key` — in the live config but not in `managed_servers` (see
    /// [`TargetState::kept_foreign`]).
    pub fn kept_foreign(&self, key: &str) -> Vec<String> {
        self.targets
            .get(key)
            .map(|t| t.kept_foreign.clone())
            .unwrap_or_default()
    }

    /// Record the kept-foreign set for `key`. Guarded writes pass the names
    /// they kept; a `--prune-foreign` write passes an empty set once the
    /// entries are actually gone.
    pub fn record_kept_foreign(&mut self, key: &str, kept: Vec<String>) {
        let entry = self.targets.entry(key.to_string()).or_default();
        entry.kept_foreign = kept;
    }

    /// The manifest that last recorded `key`'s managed servers (None for
    /// state written before the field existed).
    pub fn manifest_source(&self, key: &str) -> Option<&str> {
        self.targets
            .get(key)?
            .source_manifest
            .as_deref()
            .filter(|s| !s.is_empty())
    }

    /// Cross-manifest prune guard. Global-scope keys are bare adapter ids
    /// shared by every manifest on the machine (project keys are per-root),
    /// so when `key`'s managed set was last recorded by a *different*
    /// manifest, entries that left the current selection are not this
    /// manifest's to prune — pruning them would silently delete another
    /// setup's servers. This filters those names out of `previously` (the
    /// render then leaves them on disk, untracked) and returns them for the
    /// caller to surface (keep: `adopt` · prune: `--prune-foreign`). Project
    /// scope, same-manifest state, and pre-schema state (no recorded source)
    /// pass through untouched.
    pub fn foreign_prunes(
        &self,
        key: &str,
        scope: crate::scope::Scope,
        manifest_dir: &Path,
        previously: &mut Vec<String>,
        still_selected: impl Fn(&str) -> bool,
    ) -> Vec<String> {
        if scope != crate::scope::Scope::Global {
            return Vec::new();
        }
        let Some(source) = self.manifest_source(key) else {
            return Vec::new();
        };
        if source == manifest_identity(manifest_dir) {
            return Vec::new();
        }
        let foreign: Vec<String> = previously
            .iter()
            .filter(|n| !still_selected(n))
            .cloned()
            .collect();
        previously.retain(|n| still_selected(n));
        foreign
    }

    /// Record the materialized skill set for `key`.
    pub fn record_skills(&mut self, key: &str, managed_skills: Vec<String>) {
        let entry = self.targets.entry(key.to_string()).or_default();
        entry.managed_skills = managed_skills;
    }

    /// Settings keys we previously owned in `key`'s native settings file.
    pub fn managed_settings(&self, key: &str) -> Vec<String> {
        self.targets
            .get(key)
            .map(|t| t.managed_settings.clone())
            .unwrap_or_default()
    }

    /// Record the set of settings keys we own for `key`.
    pub fn record_settings(&mut self, key: &str, managed_settings: Vec<String>) {
        let entry = self.targets.entry(key.to_string()).or_default();
        entry.managed_settings = managed_settings;
    }

    /// Hook names we compiled into `key`'s native hooks config.
    pub fn managed_hooks(&self, key: &str) -> Vec<String> {
        self.targets
            .get(key)
            .map(|t| t.managed_hooks.clone())
            .unwrap_or_default()
    }

    /// Record the set of hook names we own for `key`.
    pub fn record_hooks(&mut self, key: &str, managed_hooks: Vec<String>) {
        let entry = self.targets.entry(key.to_string()).or_default();
        entry.managed_hooks = managed_hooks;
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
        s.record(
            "codex",
            vec!["kibana".into()],
            "content",
            "/a/agentstack.toml",
        );
        assert_eq!(s.managed_servers("codex"), vec!["kibana".to_string()]);
        assert!(!s.targets["codex"].last_hash.is_empty());
        assert_eq!(s.manifest_source("codex"), Some("/a/agentstack.toml"));
    }

    /// Pre-schema state files (no `source_manifest`, no `kept_foreign`) must
    /// still load — the fields are additive.
    #[test]
    fn old_state_without_source_manifest_loads() {
        let s: State = serde_json::from_str(
            r#"{"version":1,"targets":{"codex":{"managed_servers":["kibana"],"last_hash":"aa"}}}"#,
        )
        .unwrap();
        assert_eq!(s.managed_servers("codex"), vec!["kibana".to_string()]);
        assert_eq!(s.manifest_source("codex"), None);
        assert!(s.kept_foreign("codex").is_empty());
    }

    /// `record()` re-records the managed set without clobbering the
    /// kept-foreign bookkeeping — the exact overwrite that made the
    /// `--prune-foreign` follow-up a one-shot before the field existed.
    #[test]
    fn kept_foreign_survives_record_and_clears_explicitly() {
        let mut s = State::default();
        s.record_kept_foreign("codex", vec!["kibana".into()]);
        s.record(
            "codex",
            vec!["beta".into()],
            "content",
            "/b/agentstack.toml",
        );
        assert_eq!(s.kept_foreign("codex"), vec!["kibana".to_string()]);
        // Empty lists serialize away (state.json stays tidy) …
        s.record_kept_foreign("codex", Vec::new());
        assert!(s.kept_foreign("codex").is_empty());
        let text = serde_json::to_string(&s).unwrap();
        assert!(!text.contains("kept_foreign"));
    }

    #[test]
    fn foreign_prunes_guards_only_cross_manifest_global_keys() {
        let dir_a = assert_fs::TempDir::new().unwrap();
        let dir_b = assert_fs::TempDir::new().unwrap();
        std::fs::write(dir_a.path().join("agentstack.toml"), "version = 1\n").unwrap();
        std::fs::write(dir_b.path().join("agentstack.toml"), "version = 1\n").unwrap();

        let mut s = State::default();
        s.record(
            "codex",
            vec!["kibana".into(), "figma".into(), "shared".into()],
            "content",
            &manifest_identity(dir_a.path()),
        );
        let selected = |n: &str| n == "shared";

        // A different manifest pruning globally: foreign entries are filtered
        // out of `previously` (kept on disk) and returned by name.
        let mut prev = s.managed_servers("codex");
        let foreign = s.foreign_prunes(
            "codex",
            crate::scope::Scope::Global,
            dir_b.path(),
            &mut prev,
            selected,
        );
        assert_eq!(foreign, vec!["kibana".to_string(), "figma".to_string()]);
        assert_eq!(prev, vec!["shared".to_string()]);

        // The recording manifest itself prunes freely.
        let mut prev = s.managed_servers("codex");
        let foreign = s.foreign_prunes(
            "codex",
            crate::scope::Scope::Global,
            dir_a.path(),
            &mut prev,
            selected,
        );
        assert!(foreign.is_empty());
        assert_eq!(prev.len(), 3);

        // Project scope is keyed per-root — never guarded.
        let mut prev = s.managed_servers("codex");
        let foreign = s.foreign_prunes(
            "codex",
            crate::scope::Scope::Project,
            dir_b.path(),
            &mut prev,
            selected,
        );
        assert!(foreign.is_empty());
        assert_eq!(prev.len(), 3);

        // Pre-schema state (no recorded source) keeps today's behavior.
        let mut s2 = State::default();
        s2.targets
            .entry("codex".into())
            .or_default()
            .managed_servers = vec!["kibana".into()];
        let mut prev = s2.managed_servers("codex");
        let foreign = s2.foreign_prunes(
            "codex",
            crate::scope::Scope::Global,
            dir_b.path(),
            &mut prev,
            selected,
        );
        assert!(foreign.is_empty());
        assert_eq!(prev, vec!["kibana".to_string()]);
    }
}
