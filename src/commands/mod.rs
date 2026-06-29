//! Command implementations and shared setup.

pub mod adapters;
pub mod add;
pub mod adopt;
pub mod apply;
pub mod bootstrap;
pub mod bundle;
pub mod consolidate;
pub mod diff;
pub mod doctor;
pub mod explain;
pub mod hook;
pub mod init;
pub mod install;
pub mod instructions;
pub mod plugins;
pub mod remove;
pub mod restore;
pub mod runs;
pub mod search;
pub mod secret;
pub mod session;
pub mod stats;
pub mod upgrade;
pub mod use_profile;

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::adapter::Registry;
use crate::manifest::{self, LoadedManifest};
use crate::secret::Chain;

/// Everything a command needs after loading: the resolved manifest, the adapter
/// registry, and a secret resolver scoped to the manifest directory.
pub struct Context {
    pub dir: PathBuf,
    pub loaded: LoadedManifest,
    pub registry: Registry,
    pub resolver: Chain,
}

/// Resolve the manifest directory (explicit `--manifest-dir` or cwd) and load
/// everything a command needs.
pub fn load(manifest_dir: Option<&Path>) -> Result<Context> {
    let dir = match manifest_dir {
        Some(d) => d.to_path_buf(),
        None => std::env::current_dir()?,
    };
    let loaded = manifest::load_from_dir(&dir)?;
    let registry = Registry::load()?;
    let resolver = Chain::default_for_dir(&dir);
    Ok(Context {
        dir,
        loaded,
        registry,
        resolver,
    })
}
