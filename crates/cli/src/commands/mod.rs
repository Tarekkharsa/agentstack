//! Command implementations and shared setup.

pub mod adapters;
pub mod add;
pub mod adopt;
pub mod analyze;
pub mod apply;
pub mod audit;
pub mod bootstrap;
pub mod bundle;
pub mod codemode;
pub mod connect;
pub mod consolidate;
pub mod diff;
pub mod doctor;
pub mod explain;
pub mod guard;
pub mod hook;
pub mod init;
pub mod install;
pub mod instructions;
pub mod lib;
pub mod lock;
pub mod locked;
pub mod optimize;
pub mod overview;
pub mod pack;
pub mod plugins;
pub mod proxy;
pub mod remove;
pub mod report;
pub mod restore;
pub mod runs;
pub mod sandbox;
pub mod search;
pub mod secret;
pub mod self_cmd;
pub mod session;
pub mod settings;
pub mod setup;
pub mod stats;
pub mod trust;
pub mod upgrade;
pub mod use_profile;
pub mod verify_cmd;

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

/// The central-library inputs a command needs for library-aware validation and
/// server resolution: the loaded index, its home dir, and a content store. A
/// missing/unreadable library yields an empty one (inline-only fallback).
pub struct LibraryCtx {
    pub library: crate::library::Library,
    pub lib_home: PathBuf,
    pub store: crate::store::Store,
}

impl Context {
    /// Load the central-library inputs for this command.
    pub fn library_ctx(&self) -> LibraryCtx {
        LibraryCtx {
            library: crate::library::Library::load_default_or_warn(),
            lib_home: crate::util::paths::lib_home(),
            store: crate::store::Store::default_store(),
        }
    }
}

impl LibraryCtx {
    /// Borrow these inputs as a [`crate::manifest::ValidateCtx`] for library-aware
    /// validation, anchored at `manifest_dir`.
    pub fn validate_ctx<'a>(&'a self, manifest_dir: &'a Path) -> crate::manifest::ValidateCtx<'a> {
        crate::manifest::ValidateCtx {
            manifest_dir,
            library: &self.library,
            lib_home: &self.lib_home,
            store: &self.store,
        }
    }
}

/// Resolve the manifest directory (explicit `--manifest-dir` or cwd) and load
/// everything a command needs.
pub fn load(manifest_dir: Option<&Path>) -> Result<Context> {
    let base = match manifest_dir {
        Some(d) => d.to_path_buf(),
        None => std::env::current_dir()?,
    };
    // Prefer the `.agentstack/` layout, falling back to a legacy root manifest.
    let dir = manifest::resolve_manifest_dir(&base);
    let mut loaded = manifest::load_from_dir(&dir)?;
    // The machine-level manifest's [instructions] merge in beneath every
    // project load (instructions only — never servers/skills; see
    // manifest::merge_user_layer for the whole contract).
    manifest::merge_user_layer(&mut loaded);
    let registry = Registry::load()?;
    let resolver = Chain::default_for_dir(&dir);
    Ok(Context {
        dir,
        loaded,
        registry,
        resolver,
    })
}
