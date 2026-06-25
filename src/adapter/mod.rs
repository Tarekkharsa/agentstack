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
        paths::expand_tilde(&self.config.path).exists()
    }

    /// Detected = installed or already configured on this machine.
    pub fn detected(&self) -> bool {
        self.is_installed() || self.config_present()
    }

    /// Read and parse this CLI's config into a JSON-shaped value tree (TOML is
    /// converted to the same shape), or `None` if absent/empty.
    pub fn read_config_value(&self) -> Result<Option<serde_json::Value>> {
        let path = paths::expand_tilde(&self.config.path);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        if text.trim().is_empty() {
            return Ok(None);
        }
        let value = match self.config.format {
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

/// Whether `bin` is found in any `$PATH` entry.
pub fn bin_on_path(bin: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| dir.join(bin).is_file())
}
