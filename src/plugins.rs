//! Read-only discovery of native harness plugins already installed on this
//! machine.
//!
//! Claude Code has its own plugin system: marketplaces (`known_marketplaces.json`)
//! and installed plugins (`installed_plugins.json`) cached under
//! `~/.claude/plugins/`. Codex exposes `codex plugin list --json` and stores
//! plugin state in `~/.codex/config.toml` plus a local cache. agentstack doesn't
//! manage these yet, but surfacing them makes the setup visible (the same first
//! step we took for skills).

use serde_json::Value;
use std::process::Command;

use crate::util::paths::expand_tilde;

/// One installed plugin, aggregated across the projects it's installed in.
#[derive(Debug, Clone)]
pub struct Plugin {
    /// Harness that owns this plugin (`claude-code`, `codex`, …).
    pub harness: String,
    pub name: String,
    pub marketplace: String,
    /// Install scope (e.g. `local`, `user`).
    pub scope: String,
    /// Basenames of the projects this plugin is installed in.
    pub projects: Vec<String>,
    pub version: Option<String>,
    /// Whether the owning harness reports the plugin enabled.
    pub enabled: Option<bool>,
    /// Human status string, e.g. `installed, enabled`.
    pub status: String,
    /// Source/path hint, when available.
    pub source: Option<String>,
}

/// A plugin marketplace Claude knows about.
#[derive(Debug, Clone)]
pub struct Marketplace {
    /// Harness that owns this marketplace (`claude-code`, `codex`, …).
    pub harness: String,
    pub name: String,
    /// Human source string, e.g. `github:anthropics/claude-plugins-official`.
    pub source: String,
}

/// Read all known native plugin inventories.
pub fn all_plugins() -> (Vec<Plugin>, Vec<Marketplace>) {
    let (mut plugins, mut marketplaces) = claude_plugins();
    let (codex_plugins, codex_marketplaces) = codex_plugins();
    plugins.extend(codex_plugins);
    marketplaces.extend(codex_marketplaces);
    plugins.sort_by(|a, b| {
        a.harness
            .cmp(&b.harness)
            .then(a.marketplace.cmp(&b.marketplace))
            .then(a.name.cmp(&b.name))
    });
    marketplaces.sort_by(|a, b| a.harness.cmp(&b.harness).then(a.name.cmp(&b.name)));
    (plugins, marketplaces)
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
        let mut source = None;
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
            if source.is_none() {
                source = inst
                    .get("installPath")
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }
        }
        out.push(Plugin {
            harness: "claude-code".into(),
            name: name.to_string(),
            marketplace: marketplace.to_string(),
            scope,
            projects,
            version,
            enabled: None,
            status: "installed".into(),
            source,
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
        let source = marketplace_source(entry);
        out.push(Marketplace {
            harness: "claude-code".into(),
            name: name.clone(),
            source,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn marketplace_source(entry: &Value) -> String {
    let Some(src) = entry.get("source") else {
        return String::new();
    };
    if let Some(s) = src.as_str() {
        return s.to_string();
    }
    let kind = src
        .get("source")
        .or_else(|| src.get("type"))
        .or_else(|| src.get("kind"))
        .and_then(Value::as_str)
        .unwrap_or("");
    for key in ["path", "root", "url", "repo"] {
        if let Some(value) = src.get(key).and_then(Value::as_str) {
            return if kind.is_empty() {
                value.to_string()
            } else {
                format!("{kind}:{value}")
            };
        }
    }
    kind.to_string()
}

/// Read Codex installed plugins + marketplaces. Prefer the Codex CLI JSON
/// surface because it reflects enabled/disabled state and marketplace roots.
pub fn codex_plugins() -> (Vec<Plugin>, Vec<Marketplace>) {
    let plugins = codex_plugins_from_cli().unwrap_or_else(codex_plugins_from_cache);
    let marketplaces = codex_marketplaces_from_cli().unwrap_or_default();
    (plugins, marketplaces)
}

fn codex_plugins_from_cli() -> Option<Vec<Plugin>> {
    let root = codex_json(&["plugin", "list", "--json"])?;
    let installed = root.get("installed")?.as_array()?;
    let mut out = Vec::new();
    for item in installed {
        out.push(codex_plugin_from_value(item));
    }
    out.sort_by(|a, b| a.marketplace.cmp(&b.marketplace).then(a.name.cmp(&b.name)));
    Some(out)
}

fn codex_plugin_from_value(item: &Value) -> Plugin {
    let name = item
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let marketplace = item
        .get("marketplaceName")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let enabled = item.get("enabled").and_then(Value::as_bool);
    let installed = item
        .get("installed")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let status = match (installed, enabled) {
        (true, Some(true)) => "installed, enabled",
        (true, Some(false)) => "installed, disabled",
        (true, None) => "installed",
        (false, _) => "not installed",
    }
    .to_string();
    let source = item
        .get("source")
        .and_then(|s| s.get("path").or_else(|| s.get("source")))
        .and_then(Value::as_str)
        .map(str::to_string);
    Plugin {
        harness: "codex".into(),
        name,
        marketplace,
        scope: item
            .get("installPolicy")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_ascii_lowercase(),
        projects: Vec::new(),
        version: item
            .get("version")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .map(str::to_string),
        enabled,
        status,
        source,
    }
}

fn codex_marketplaces_from_cli() -> Option<Vec<Marketplace>> {
    let root = codex_json(&["plugin", "marketplace", "list", "--json"])?;
    let arr = root.get("marketplaces")?.as_array()?;
    let mut out = Vec::new();
    for item in arr {
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let source = item
            .get("root")
            .or_else(|| item.get("source"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        out.push(Marketplace {
            harness: "codex".into(),
            name,
            source,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Some(out)
}

fn codex_plugins_from_cache() -> Vec<Plugin> {
    let cache = expand_tilde("~/.codex/plugins/cache");
    let mut out = Vec::new();
    let Ok(marketplaces) = std::fs::read_dir(cache) else {
        return out;
    };
    for mp in marketplaces.flatten() {
        let Ok(plugin_dirs) = std::fs::read_dir(mp.path()) else {
            continue;
        };
        for plugin_dir in plugin_dirs.flatten() {
            let Ok(version_dirs) = std::fs::read_dir(plugin_dir.path()) else {
                continue;
            };
            for version_dir in version_dirs.flatten() {
                let manifest = version_dir.path().join(".codex-plugin/plugin.json");
                let Some(root) = read_json(&manifest) else {
                    continue;
                };
                let name = root
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if name.is_empty() {
                    continue;
                }
                out.push(Plugin {
                    harness: "codex".into(),
                    name,
                    marketplace: mp.file_name().to_string_lossy().to_string(),
                    scope: "cache".into(),
                    projects: Vec::new(),
                    version: root
                        .get("version")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    enabled: None,
                    status: "cached".into(),
                    source: Some(version_dir.path().display().to_string()),
                });
            }
        }
    }
    out.sort_by(|a, b| a.marketplace.cmp(&b.marketplace).then(a.name.cmp(&b.name)));
    out
}

fn codex_json(args: &[&str]) -> Option<Value> {
    let out = Command::new("codex").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    serde_json::from_slice(&out.stdout).ok()
}

fn read_json(path: &std::path::Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_codex_plugin_json_entry() {
        let p = codex_plugin_from_value(&json!({
            "pluginId": "github@openai-curated",
            "name": "github",
            "marketplaceName": "openai-curated",
            "version": "1.2.3",
            "installed": true,
            "enabled": true,
            "source": { "source": "local", "path": "/tmp/github" },
            "installPolicy": "AVAILABLE"
        }));
        assert_eq!(p.harness, "codex");
        assert_eq!(p.name, "github");
        assert_eq!(p.marketplace, "openai-curated");
        assert_eq!(p.version.as_deref(), Some("1.2.3"));
        assert_eq!(p.enabled, Some(true));
        assert_eq!(p.status, "installed, enabled");
        assert_eq!(p.source.as_deref(), Some("/tmp/github"));
    }
}
