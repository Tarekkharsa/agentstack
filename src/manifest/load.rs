//! Layered manifest loading.
//!
//! The shared, committed `agentstack.toml` is deep-merged with an optional,
//! gitignored `agentstack.local.toml` overlay (per-machine servers, path
//! differences, target subsets). The merge happens at the [`toml::Value`] level
//! so the overlay can touch any field without the model knowing about it.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::model::Manifest;

pub const MANIFEST_FILE: &str = "agentstack.toml";
pub const LOCAL_FILE: &str = "agentstack.local.toml";

/// Result of a layered load, keeping the resolved manifest plus provenance.
pub struct LoadedManifest {
    pub manifest: Manifest,
    pub manifest_path: PathBuf,
    pub local_path: Option<PathBuf>,
}

/// Load `agentstack.toml` from `dir`, deep-merging `agentstack.local.toml` over
/// it when present.
pub fn load_from_dir(dir: &Path) -> Result<LoadedManifest> {
    let manifest_path = dir.join(MANIFEST_FILE);
    let base_text = fs::read_to_string(&manifest_path).with_context(|| {
        format!(
            "no manifest at {} — run `agentstack init` to create one",
            manifest_path.display()
        )
    })?;
    let mut base: toml::Value = toml::from_str(&base_text)
        .with_context(|| format!("parsing {}", manifest_path.display()))?;

    let local_path = dir.join(LOCAL_FILE);
    let local_path = if local_path.exists() {
        let local_text = fs::read_to_string(&local_path)
            .with_context(|| format!("reading {}", local_path.display()))?;
        let overlay: toml::Value = toml::from_str(&local_text)
            .with_context(|| format!("parsing {}", local_path.display()))?;
        merge_value(&mut base, overlay);
        Some(local_path)
    } else {
        None
    };

    let manifest: Manifest = base
        .try_into()
        .context("manifest does not match the expected schema")?;

    Ok(LoadedManifest {
        manifest,
        manifest_path,
        local_path,
    })
}

/// Deep-merge `overlay` into `base`. Tables merge key-by-key (recursively);
/// every other value (including arrays) is replaced wholesale by the overlay.
fn merge_value(base: &mut toml::Value, overlay: toml::Value) {
    match (base, overlay) {
        (toml::Value::Table(base_tbl), toml::Value::Table(overlay_tbl)) => {
            for (k, v) in overlay_tbl {
                match base_tbl.get_mut(&k) {
                    Some(existing) => merge_value(existing, v),
                    None => {
                        base_tbl.insert(k, v);
                    }
                }
            }
        }
        (base_slot, overlay_val) => {
            *base_slot = overlay_val;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_adds_and_overrides() {
        let mut base: toml::Value = toml::from_str(
            r#"
            version = 1
            [servers.kibana]
            type = "http"
            url = "https://old"
            "#,
        )
        .unwrap();
        let overlay: toml::Value = toml::from_str(
            r#"
            [servers.kibana]
            url = "https://new"
            [servers.local-only]
            type = "stdio"
            command = "node"
            "#,
        )
        .unwrap();
        merge_value(&mut base, overlay);
        let m: Manifest = base.try_into().unwrap();
        assert_eq!(m.servers["kibana"].url.as_deref(), Some("https://new"));
        assert!(m.servers.contains_key("local-only"));
    }
}
