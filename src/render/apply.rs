//! Orchestration: manifest + registry + resolver → a per-target render plan.
//!
//! Computing the plan is always read-only. Writing it to disk is a separate,
//! explicit step (`TargetPlan::write`) so `apply --dry-run` and `diff` can share
//! all the rendering logic without any risk of touching files.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use indexmap::IndexMap;
use serde_json::Value;

use crate::adapter::descriptor::Format;
use crate::adapter::{render_server, AdapterDescriptor, Registry};
use crate::library::Library;
use crate::manifest::{Manifest, Server};
use crate::resolve::{resolve_server, ResolvedServer};
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
    /// Selected servers this target's config format can't represent (e.g. an
    /// HTTP server for the stdio-only Claude Desktop config). Skipped from the
    /// render rather than written as an empty entry; surfaced so the user knows
    /// to wire them up by the harness's other mechanism (e.g. in-app Connectors).
    pub skipped: Vec<String>,
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
        crate::util::atomic::write(&self.config_path, &self.proposed)
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
    // Back-compat, inline-only server map (today's behavior). Callers not yet
    // wired for the central library keep this path.
    let names = selected_servers(manifest, selection)?;
    let servers: IndexMap<String, Server> = names
        .into_iter()
        .map(|n| {
            let s = manifest.servers[&n].clone();
            (n, s)
        })
        .collect();
    plan_target_with_servers(
        desc,
        resolver,
        &servers,
        previously_managed,
        scope,
        project_dir,
    )
}

/// Build the plan for one target from an already-resolved **effective server
/// map** (`name -> Server`, `${REF}` placeholders intact). This is the core
/// renderer: secret resolution happens *here*, via `render_server` + `resolver`,
/// never earlier. Library-aware callers build the map with [`effective_servers`]
/// and call this directly.
pub fn plan_target_with_servers(
    desc: &AdapterDescriptor,
    resolver: &dyn Resolver,
    servers: &IndexMap<String, Server>,
    previously_managed: &[String],
    scope: Scope,
    project_dir: &Path,
) -> Result<Option<TargetPlan>> {
    let Some((config_path, format)) = desc.config_for(scope, project_dir) else {
        return Ok(None);
    };
    let Some(mcp) = desc.mcp.as_ref() else {
        return Ok(None);
    };

    let mut entries: Vec<(String, Value)> = Vec::new();
    let mut unresolved: Vec<String> = Vec::new();
    let mut managed: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    for (name, server) in servers {
        let rendered = render_server(desc, server, resolver);
        // The adapter's format can't express this transport — skip it rather
        // than emit an empty `{}` entry into a real config file.
        if !rendered.representable {
            skipped.push(name.clone());
            continue;
        }
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
            merge_json::merge_with_removals(&existing, &mcp.location, &entries, &removed)?
        }
        Format::Toml => merge_toml::merge_with_removals(
            &existing,
            &mcp.location,
            &entries,
            &removed,
            mcp.headers_as_subtable,
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
        skipped,
    }))
}

/// Resolve a selection into an ordered list of server names that exist in the
/// manifest (inline only — used by the back-compat [`plan_target`]).
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

/// The raw (unfiltered) server names a selection asks for — library-only names
/// are kept so they can be resolved centrally by [`effective_servers`].
fn selection_names(manifest: &Manifest, selection: &Selection) -> Result<Vec<String>> {
    match selection {
        Selection::All => Ok(manifest.servers.keys().cloned().collect()),
        Selection::Profile(p) => {
            let profile = manifest
                .profiles
                .get(p)
                .with_context(|| format!("no profile named '{p}' in manifest"))?;
            Ok(profile.servers.clone())
        }
        Selection::Explicit(names) => Ok(names.clone()),
    }
}

/// Resolve a selection's server refs to full [`ResolvedServer`]s (definition +
/// origin + provenance + digest), inline-first then central library. An
/// unresolved ref is a hard error, so activation/render fails before any write.
/// `${REF}`s are preserved verbatim; no secret is resolved here.
pub fn resolve_active_servers(
    manifest: &Manifest,
    library: &Library,
    lib_home: &Path,
    selection: &Selection,
) -> Result<Vec<ResolvedServer>> {
    let mut out = Vec::new();
    for name in selection_names(manifest, selection)? {
        out.push(
            resolve_server(manifest, library, lib_home, &name)
                .with_context(|| format!("resolving server '{name}' for rendering"))?,
        );
    }
    Ok(out)
}

/// The effective server definitions for a selection: `name -> Server`, inline
/// winning over the central library. `${REF}` placeholders are preserved; secret
/// resolution is deferred to [`plan_target_with_servers`] at render time.
pub fn effective_servers(
    manifest: &Manifest,
    library: &Library,
    lib_home: &Path,
    selection: &Selection,
) -> Result<IndexMap<String, Server>> {
    Ok(
        resolve_active_servers(manifest, library, lib_home, selection)?
            .into_iter()
            .map(|r| (r.name, r.server))
            .collect(),
    )
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::LibraryServer;
    use crate::secret::MapResolver;
    use assert_fs::prelude::*;

    /// Write a library server definition and return its index entry.
    fn write_lib_server(lib_home: &assert_fs::TempDir, name: &str, url: &str, with_ref: bool) {
        let mut content = format!("type = \"http\"\nurl = \"{url}\"\n");
        if with_ref {
            content.push_str("\n[headers]\nAuthorization = \"Bearer ${TOKEN}\"\n");
        }
        lib_home
            .child(format!("servers/{name}.toml"))
            .write_str(&content)
            .unwrap();
    }

    fn server_entry(name: &str) -> LibraryServer {
        LibraryServer {
            name: name.into(),
            checksum: None,
            version: None,
            provenance: Some("consolidated".into()),
        }
    }

    #[test]
    fn effective_servers_inline_wins_and_library_resolves() {
        let lib_home = assert_fs::TempDir::new().unwrap();
        write_lib_server(&lib_home, "kibana", "https://central-kibana/mcp", false);
        write_lib_server(&lib_home, "figma", "https://central-figma/mcp", false);
        let mut library = Library::default();
        library.upsert_server(server_entry("kibana"));
        library.upsert_server(server_entry("figma"));

        // Inline kibana overrides the library; figma is library-only.
        let manifest: Manifest = toml::from_str(
            "version = 1\n[servers.kibana]\ntype = \"http\"\nurl = \"https://inline-kibana/mcp\"\n\
             [profiles.p]\nservers = [\"kibana\", \"figma\"]\n",
        )
        .unwrap();

        let map = effective_servers(
            &manifest,
            &library,
            lib_home.path(),
            &Selection::Profile("p".into()),
        )
        .unwrap();

        assert_eq!(
            map.get("kibana").unwrap().url.as_deref(),
            Some("https://inline-kibana/mcp")
        );
        assert_eq!(
            map.get("figma").unwrap().url.as_deref(),
            Some("https://central-figma/mcp")
        );
        let keys: Vec<&str> = map.keys().map(String::as_str).collect();
        assert_eq!(keys, vec!["kibana", "figma"], "selection order preserved");
    }

    #[test]
    fn effective_servers_unresolved_ref_fails() {
        let lib_home = assert_fs::TempDir::new().unwrap();
        let manifest: Manifest =
            toml::from_str("version = 1\n[profiles.p]\nservers = [\"ghost\"]\n").unwrap();
        let err = effective_servers(
            &manifest,
            &Library::default(),
            lib_home.path(),
            &Selection::Profile("p".into()),
        )
        .unwrap_err();
        assert!(err.to_string().contains("ghost"));
    }

    #[test]
    fn effective_servers_keeps_ref_intact() {
        let lib_home = assert_fs::TempDir::new().unwrap();
        write_lib_server(&lib_home, "kibana", "https://x/mcp", true);
        let mut library = Library::default();
        library.upsert_server(server_entry("kibana"));
        let manifest: Manifest =
            toml::from_str("version = 1\n[profiles.p]\nservers = [\"kibana\"]\n").unwrap();

        let map = effective_servers(
            &manifest,
            &library,
            lib_home.path(),
            &Selection::Profile("p".into()),
        )
        .unwrap();

        // The resolver never runs here — the ${REF} is returned verbatim.
        assert_eq!(
            map.get("kibana")
                .unwrap()
                .headers
                .get("Authorization")
                .map(String::as_str),
            Some("Bearer ${TOKEN}")
        );
    }

    #[test]
    fn plan_renders_library_server_and_resolves_ref_at_render() {
        let _g = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("HOME", home.path());
        std::env::set_var("AGENTSTACK_HOME", home.child(".agentstack").path());

        let lib_home = assert_fs::TempDir::new().unwrap();
        write_lib_server(&lib_home, "kibana", "https://x/mcp", true);
        let mut library = Library::default();
        library.upsert_server(server_entry("kibana"));
        let manifest: Manifest =
            toml::from_str("version = 1\n[profiles.p]\nservers = [\"kibana\"]\n").unwrap();
        let map = effective_servers(
            &manifest,
            &library,
            lib_home.path(),
            &Selection::Profile("p".into()),
        )
        .unwrap();

        let reg = Registry::load().unwrap();
        let desc = reg.get("claude-code").unwrap();
        let proj = assert_fs::TempDir::new().unwrap();

        // Secret present → resolved into the rendered config at render time.
        let resolver = MapResolver::from([("TOKEN", "secret123")]);
        let plan = plan_target_with_servers(desc, &resolver, &map, &[], Scope::Global, proj.path())
            .unwrap()
            .unwrap();
        assert!(plan.managed.contains(&"kibana".to_string()));
        assert!(
            plan.proposed.contains("secret123"),
            "ref resolved during render: {}",
            plan.proposed
        );
        assert!(plan.unresolved.is_empty());

        // Secret missing → reported unresolved (the caller blocks the write).
        let empty = MapResolver::from([]);
        let plan2 = plan_target_with_servers(desc, &empty, &map, &[], Scope::Global, proj.path())
            .unwrap()
            .unwrap();
        assert!(
            !plan2.unresolved.is_empty(),
            "missing ${{REF}} reported as unresolved"
        );

        std::env::remove_var("AGENTSTACK_HOME");
        std::env::remove_var("HOME");
    }
}
