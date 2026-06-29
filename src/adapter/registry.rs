//! Adapter registry: the descriptors embedded in the binary, plus any
//! user-supplied overrides/additions dropped in `~/.agentstack/adapters/`.

use std::collections::BTreeMap;
use std::fs;

use anyhow::{Context, Result};
use include_dir::{include_dir, Dir};

use super::descriptor::AdapterDescriptor;
use crate::util::paths;

/// Descriptors shipped inside the binary.
static EMBEDDED: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/adapters");

/// All known adapters, keyed by id. User descriptors override embedded ones
/// with the same id.
pub struct Registry {
    adapters: BTreeMap<String, AdapterDescriptor>,
}

impl Registry {
    /// Load embedded descriptors then layer user descriptors on top.
    pub fn load() -> Result<Self> {
        let mut adapters = BTreeMap::new();

        for file in EMBEDDED.files() {
            if !is_yaml(file.path().to_string_lossy().as_ref()) {
                continue;
            }
            let text = file
                .contents_utf8()
                .context("embedded adapter is not valid UTF-8")?;
            let desc: AdapterDescriptor = serde_yaml::from_str(text)
                .with_context(|| format!("parsing embedded adapter {}", file.path().display()))?;
            adapters.insert(desc.id.clone(), desc);
        }

        let user_dir = paths::user_adapters_dir();
        if user_dir.is_dir() {
            for entry in fs::read_dir(&user_dir)
                .with_context(|| format!("reading {}", user_dir.display()))?
            {
                let path = entry?.path();
                if !path.is_file() || !is_yaml(path.to_string_lossy().as_ref()) {
                    continue;
                }
                let text = fs::read_to_string(&path)
                    .with_context(|| format!("reading {}", path.display()))?;
                let desc: AdapterDescriptor = serde_yaml::from_str(&text)
                    .with_context(|| format!("parsing {}", path.display()))?;
                adapters.insert(desc.id.clone(), desc);
            }
        }

        Ok(Registry { adapters })
    }

    pub fn get(&self, id: &str) -> Option<&AdapterDescriptor> {
        self.adapters.get(id)
    }

    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.adapters.keys().map(String::as_str)
    }

    pub fn iter(&self) -> impl Iterator<Item = &AdapterDescriptor> {
        self.adapters.values()
    }
}

fn is_yaml(name: &str) -> bool {
    name.ends_with(".yaml") || name.ends_with(".yml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_adapters_load() {
        let reg = Registry::load().unwrap();
        assert!(reg.get("claude-code").is_some());
        assert!(reg.get("codex").is_some());
    }

    /// Every shipped harness adapter must parse and embed. Guards against a new
    /// descriptor regressing the schema (parse errors surface in `load`).
    #[test]
    fn all_shipped_adapters_present() {
        let reg = Registry::load().unwrap();
        for id in [
            "claude-code",
            "claude-desktop",
            "codex",
            "copilot-cli",
            "cursor",
            "gemini",
            "antigravity",
            "junie",
            "kiro",
            "opencode",
            "pi",
            "vscode",
            "windsurf",
        ] {
            assert!(reg.get(id).is_some(), "adapter {id} failed to load");
        }
    }
}
