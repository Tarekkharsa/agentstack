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
/// Preferred subdirectory holding the manifest and all agentstack-managed files
/// (lock, `skills/`, `instructions/`, `.env`). A repo opts in by placing its
/// manifest here; the legacy root layout is still discovered for back-compat.
pub const MANIFEST_SUBDIR: &str = ".agentstack";

/// Resolve the directory that holds an *existing* manifest, given a project/base
/// dir. Prefers `<base>/.agentstack/` (the new layout) when it actually contains
/// a manifest, otherwise falls back to the legacy root `<base>/`. When neither
/// has a manifest, returns the legacy root so callers' "no manifest" errors point
/// at the conventional path.
pub fn resolve_manifest_dir(base: &Path) -> PathBuf {
    let nested = base.join(MANIFEST_SUBDIR);
    if nested.join(MANIFEST_FILE).exists() {
        nested
    } else {
        base.to_path_buf()
    }
}

/// Resolve the directory where a *new* manifest should be created. Keeps using a
/// legacy root manifest if one already exists there; otherwise prefers the new
/// `<base>/.agentstack/` layout.
pub fn new_manifest_dir(base: &Path) -> PathBuf {
    if base.join(MANIFEST_FILE).exists() {
        base.to_path_buf()
    } else {
        base.join(MANIFEST_SUBDIR)
    }
}

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
    fn resolve_prefers_nested_then_falls_back_to_root() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let base = tmp.path();

        // Nothing yet → legacy root (so "no manifest" errors point at the root).
        assert_eq!(resolve_manifest_dir(base), base.to_path_buf());
        // New manifests are created under `.agentstack/`.
        assert_eq!(new_manifest_dir(base), base.join(MANIFEST_SUBDIR));

        // Legacy root manifest present → both resolve to root.
        fs::write(base.join(MANIFEST_FILE), "version = 1\n").unwrap();
        assert_eq!(resolve_manifest_dir(base), base.to_path_buf());
        assert_eq!(new_manifest_dir(base), base.to_path_buf());

        // `.agentstack/` manifest present → preferred over a missing root one.
        let tmp2 = assert_fs::TempDir::new().unwrap();
        let base2 = tmp2.path();
        let nested = base2.join(MANIFEST_SUBDIR);
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join(MANIFEST_FILE), "version = 1\n").unwrap();
        assert_eq!(resolve_manifest_dir(base2), nested);
        assert_eq!(new_manifest_dir(base2), nested);
    }

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
