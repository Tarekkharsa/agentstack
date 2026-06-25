//! Orchestration: manifest + registry + resolver → a per-target render plan.
//!
//! Computing the plan is always read-only. Writing it to disk is a separate,
//! explicit step (`TargetPlan::write`) so `apply --dry-run` and `diff` can share
//! all the rendering logic without any risk of touching files.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::Value;

use crate::adapter::descriptor::Format;
use crate::adapter::{render_server, AdapterDescriptor, Registry};
use crate::manifest::Manifest;
use crate::scope::Scope;
use crate::secret::Resolver;
use crate::util::diff;

use super::{merge_json, merge_toml};

/// The computed change for one target CLI.
pub struct TargetPlan {
    pub id: String,
    pub display: String,
    pub scope: Scope,
    pub config_path: PathBuf,
    pub existing: String,
    pub proposed: String,
    /// Names of the servers we rendered into this target.
    pub managed: Vec<String>,
    /// Names we previously managed but pruned this run (left the selection).
    pub removed: Vec<String>,
    /// `${REF}`s that did not resolve on this machine.
    pub unresolved: Vec<String>,
}

impl TargetPlan {
    pub fn changed(&self) -> bool {
        diff::differs(&self.existing, &self.proposed)
    }

    pub fn diff(&self) -> String {
        diff::render(&self.existing, &self.proposed)
    }

    /// Hash of the content we would write (for state tracking / drift checks).
    pub fn proposed_hash(&self) -> String {
        crate::state::hash(&self.proposed)
    }

    /// Write the proposed config to disk, creating parent dirs as needed.
    pub fn write(&self) -> Result<()> {
        if let Some(parent) = self.config_path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        fs::write(&self.config_path, &self.proposed)
            .with_context(|| format!("writing {}", self.config_path.display()))
    }
}

/// Which servers a run targets.
pub enum Selection {
    /// Every server in the manifest.
    All,
    /// A named profile's server list.
    Profile(String),
    /// An explicit set of server names (e.g. one CLI's enabled set).
    Explicit(Vec<String>),
}

/// Build the plan for one target id in a given scope. `previously_managed` are
/// the server names we wrote on the last `apply` (from state); any not in the
/// current selection are pruned. Returns `Ok(None)` when the target doesn't
/// support `scope` (e.g. project scope for a global-only CLI).
pub fn plan_target(
    manifest: &Manifest,
    desc: &AdapterDescriptor,
    resolver: &dyn Resolver,
    selection: &Selection,
    previously_managed: &[String],
    scope: Scope,
    project_dir: &Path,
) -> Result<Option<TargetPlan>> {
    let Some((config_path, format)) = desc.config_for(scope, project_dir) else {
        return Ok(None);
    };

    let names = selected_servers(manifest, selection)?;

    let mut entries: Vec<(String, Value)> = Vec::new();
    let mut unresolved: Vec<String> = Vec::new();
    let mut managed: Vec<String> = Vec::new();
    for name in &names {
        let server = &manifest.servers[name];
        let rendered = render_server(desc, server, resolver);
        for u in rendered.unresolved {
            unresolved.push(format!("{u} (server '{name}')"));
        }
        entries.push((name.clone(), rendered.value));
        managed.push(name.clone());
    }

    // Prune entries we used to own but no longer select.
    let removed: Vec<String> = previously_managed
        .iter()
        .filter(|n| !managed.contains(n))
        .cloned()
        .collect();

    let existing = fs::read_to_string(&config_path).unwrap_or_default();

    let proposed = match format {
        Format::Json => {
            merge_json::merge_with_removals(&existing, &desc.mcp.location, &entries, &removed)?
        }
        Format::Toml => merge_toml::merge_with_removals(
            &existing,
            &desc.mcp.location,
            &entries,
            &removed,
            desc.mcp.headers_as_subtable,
        )?,
    };

    Ok(Some(TargetPlan {
        id: desc.id.clone(),
        display: desc.display.clone(),
        scope,
        config_path,
        existing,
        proposed,
        managed,
        removed,
        unresolved,
    }))
}

/// Resolve a selection into an ordered list of server names that exist in the
/// manifest.
fn selected_servers(manifest: &Manifest, selection: &Selection) -> Result<Vec<String>> {
    match selection {
        Selection::All => Ok(manifest.servers.keys().cloned().collect()),
        Selection::Profile(p) => {
            let profile = manifest
                .profiles
                .get(p)
                .with_context(|| format!("no profile named '{p}' in manifest"))?;
            Ok(profile
                .servers
                .iter()
                .filter(|s| manifest.servers.contains_key(*s))
                .cloned()
                .collect())
        }
        Selection::Explicit(names) => Ok(names
            .iter()
            .filter(|s| manifest.servers.contains_key(*s))
            .cloned()
            .collect()),
    }
}

/// Decide which target ids to act on: explicit `--target` wins, else the
/// manifest's `[targets].default`, else every registered adapter.
pub fn resolve_targets(
    manifest: &Manifest,
    registry: &Registry,
    requested: &[String],
) -> Vec<String> {
    if !requested.is_empty() {
        return requested.to_vec();
    }
    if !manifest.targets.default.is_empty() {
        return manifest.targets.default.clone();
    }
    registry.ids().map(String::from).collect()
}
