//! Data-driven adapters: descriptors, the registry that loads them, and the
//! generic renderer that turns manifest servers into target-shaped values.

pub mod descriptor;
pub mod import;
pub mod registry;
pub mod render;

pub use descriptor::{AdapterDescriptor, Format};
pub use import::{extract_servers, extract_settings};
pub use registry::Registry;
pub use render::{render_server, Rendered};

use anyhow::{Context, Result};

use crate::util::paths;

impl AdapterDescriptor {
    /// Whether this CLI's binary is on `$PATH`.
    pub fn is_installed(&self) -> bool {
        self.detect.bin.as_deref().map(bin_on_path).unwrap_or(false)
    }

    /// Whether this CLI's config file exists.
    pub fn config_present(&self) -> bool {
        self.config
            .as_ref()
            .map(|c| paths::expand_tilde(&c.path).exists())
            .unwrap_or(false)
    }

    /// Detected = installed or already configured on this machine.
    pub fn detected(&self) -> bool {
        self.is_installed() || self.config_present()
    }

    /// Read and parse this CLI's config into a JSON-shaped value tree (TOML is
    /// converted to the same shape), or `None` if absent/empty.
    pub fn read_config_value(&self) -> Result<Option<serde_json::Value>> {
        let Some(config) = self.config.as_ref() else {
            return Ok(None);
        };
        let path = paths::expand_tilde(&config.path);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        if text.trim().is_empty() {
            return Ok(None);
        }
        let value = match config.format {
            Format::Json => serde_json::from_str(&text)
                .with_context(|| format!("parsing {}", path.display()))?,
            Format::Toml => {
                let tv: toml::Value =
                    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
                serde_json::to_value(tv).context("converting TOML to value tree")?
            }
        };
        Ok(Some(value))
    }

    /// Read and parse this CLI's native settings file (global scope) into a
    /// JSON-shaped value tree, or `None` if the CLI has no settings file or it
    /// is absent/empty.
    pub fn read_settings_value(
        &self,
        project_dir: &std::path::Path,
    ) -> Result<Option<serde_json::Value>> {
        let Some((path, format)) = self.settings_for(crate::scope::Scope::Global, project_dir)
        else {
            return Ok(None);
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        if text.trim().is_empty() {
            return Ok(None);
        }
        let value = match format {
            Format::Json => serde_json::from_str(&text)
                .with_context(|| format!("parsing {}", path.display()))?,
            Format::Toml => {
                let tv: toml::Value =
                    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
                serde_json::to_value(tv).context("converting TOML to value tree")?
            }
        };
        Ok(Some(value))
    }
}

/// A skill found already present in a CLI's skills directory.
#[derive(Debug, Clone)]
pub struct DiscoveredSkill {
    /// Directory name in the skills dir (the skill's name).
    pub name: String,
    /// The real source on disk (symlinks resolved to their target).
    pub source: std::path::PathBuf,
    /// True if the entry in the skills dir is a symlink (agentstack-style) vs a
    /// real directory living in the CLI's own folder.
    pub is_symlink: bool,
}

impl AdapterDescriptor {
    /// Scan this CLI's skills directory (for a scope) for skills already on
    /// disk: subdirectories containing a `SKILL.md`. Symlinks are resolved to
    /// their real source. Hidden entries (`.system`, …) are skipped.
    pub fn discover_skills(
        &self,
        scope: crate::scope::Scope,
        project_dir: &std::path::Path,
    ) -> Vec<DiscoveredSkill> {
        let Some(dir) = self.skills_dir_for(scope, project_dir) else {
            return Vec::new();
        };
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let path = entry.path();
            let is_symlink = std::fs::symlink_metadata(&path)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false);
            // Resolve through symlinks to the real directory.
            let source = std::fs::canonicalize(&path).unwrap_or(path);
            if !source.is_dir() || !source.join("SKILL.md").is_file() {
                continue;
            }
            out.push(DiscoveredSkill {
                name,
                source,
                is_symlink,
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }
}

/// Whether `bin` is found in any `$PATH` entry.
pub fn bin_on_path(bin: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| dir.join(bin).is_file())
}
