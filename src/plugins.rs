//! Read-only discovery of Claude Code plugins already installed on this machine.
//!
//! Claude Code has its own plugin system: marketplaces (`known_marketplaces.json`)
//! and installed plugins (`installed_plugins.json`) cached under
//! `~/.claude/plugins/`. agentstack doesn't manage these yet, but surfacing them
//! makes the setup visible (the same first step we took for skills).
//!
//! Codex also has a `~/.codex/plugins/` tree, but its on-disk format is
//! different and undocumented, so it is intentionally not read here.

use serde_json::Value;

use crate::util::paths::expand_tilde;

/// One installed plugin, aggregated across the projects it's installed in.
#[derive(Debug, Clone)]
pub struct Plugin {
    pub name: String,
    pub marketplace: String,
    /// Install scope (e.g. `local`, `user`).
    pub scope: String,
    /// Basenames of the projects this plugin is installed in.
    pub projects: Vec<String>,
    pub version: Option<String>,
}

/// A plugin marketplace Claude knows about.
#[derive(Debug, Clone)]
pub struct Marketplace {
    pub name: String,
    /// Human source string, e.g. `github:anthropics/claude-plugins-official`.
    pub source: String,
}

/// Read Claude Code's installed plugins + known marketplaces (empty if absent).
pub fn claude_plugins() -> (Vec<Plugin>, Vec<Marketplace>) {
    let base = expand_tilde("~/.claude/plugins");
    let plugins = read_installed(&base.join("installed_plugins.json"));
    let marketplaces = read_marketplaces(&base.join("known_marketplaces.json"));
    (plugins, marketplaces)
}

fn read_installed(path: &std::path::Path) -> Vec<Plugin> {
    let Some(root) = read_json(path) else {
        return Vec::new();
    };
    let Some(map) = root.get("plugins").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (key, installs) in map {
        // Key is "name@marketplace".
        let (name, marketplace) = key.split_once('@').unwrap_or((key.as_str(), ""));
        let installs = installs.as_array().cloned().unwrap_or_default();
        let mut projects = Vec::new();
        let mut scope = String::new();
        let mut version = None;
        for inst in &installs {
            if let Some(p) = inst.get("projectPath").and_then(Value::as_str) {
                let base = p.rsplit('/').next().unwrap_or(p).to_string();
                if !projects.contains(&base) {
                    projects.push(base);
                }
            }
            if scope.is_empty() {
                scope = inst
                    .get("scope")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
            }
            if version.is_none() {
                version = inst
                    .get("version")
                    .and_then(Value::as_str)
                    .filter(|v| *v != "unknown")
                    .map(str::to_string);
            }
        }
        out.push(Plugin {
            name: name.to_string(),
            marketplace: marketplace.to_string(),
            scope,
            projects,
            version,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn read_marketplaces(path: &std::path::Path) -> Vec<Marketplace> {
    let Some(root) = read_json(path) else {
        return Vec::new();
    };
    let Some(map) = root.as_object() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (name, entry) in map {
        let src = entry.get("source");
        let kind = src
            .and_then(|s| s.get("source"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let repo = src
            .and_then(|s| s.get("repo"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let source = if repo.is_empty() {
            kind.to_string()
        } else {
            format!("{kind}:{repo}")
        };
        out.push(Marketplace {
            name: name.clone(),
            source,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn read_json(path: &std::path::Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}
