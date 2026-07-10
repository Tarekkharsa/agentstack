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
    /// `${REF}`s that did not resolve on this machine (no store has them).
    pub unresolved: Vec<String>,
    /// `${REF}`s a secret store errored on while reading (e.g. a keychain
    /// failure) — the secret may be set; the read failed. Blocks writes like
    /// `unresolved`, but is reported as a read failure, not a missing secret.
    pub failed: Vec<String>,
    /// Selected servers this target's config format can't represent (e.g. an
    /// HTTP server for the stdio-only Claude Desktop config). Skipped from the
    /// render rather than written as an empty entry; surfaced so the user knows
    /// to wire them up by the harness's other mechanism (e.g. in-app Connectors).
    pub skipped: Vec<String>,
    /// Every `${REF}` resolved into this render, as `(ref-name, value)`. Used
    /// ONLY to redact the human-facing diff/apply preview — `proposed` still
    /// holds the real resolved values, and that is what `write` persists.
    pub secrets: Vec<(String, String)>,
    /// Non-blocking notices about servers that DID render but lost a
    /// transport-neutral attribute this target can't express — today, a `cwd`
    /// dropped because the CLI's config has no working-directory key. Surfaced
    /// so the user knows the server may need a shell wrapper on that harness,
    /// rather than the field vanishing silently.
    pub warnings: Vec<String>,
}

impl TargetPlan {
    pub fn changed(&self) -> bool {
        diff::differs(&self.existing, &self.proposed)
    }

    /// Colored diff for the terminal, with resolved secret values redacted to
    /// their `${REF}` name so a preview never prints a credential in cleartext.
    pub fn diff(&self) -> String {
        diff::mask_secrets(&diff::render(&self.existing, &self.proposed), &self.secrets)
    }

    /// Plain (uncolored) diff for the web dashboard — same secret redaction.
    pub fn diff_plain(&self) -> String {
        diff::mask_secrets(
            &diff::render_plain(&self.existing, &self.proposed),
            &self.secrets,
        )
    }

    /// Hash of the content we would write (for state tracking / drift checks).
    pub fn proposed_hash(&self) -> String {
        crate::state::hash(&self.proposed)
    }

    /// Write the proposed config to disk, creating parent dirs as needed.
    pub fn write(&self) -> Result<()> {
        crate::util::atomic::write(&self.config_path, &self.proposed)
    }

    /// After a prune-to-zero at PROJECT scope, delete the config file when
    /// nothing but the empty managed section remains (`{"mcpServers": {}}`)
    /// — a husk that would sit untracked in the repo forever. Files carrying
    /// any other content are never touched. Returns whether it was removed.
    pub fn remove_if_empty_shell(&self, desc: &AdapterDescriptor) -> bool {
        if self.scope != Scope::Project || !self.managed.is_empty() {
            return false;
        }
        let Some(mcp) = desc.mcp.as_ref() else {
            return false;
        };
        // Only the format matters here; the path arg is unused for it.
        let Some((_, format)) = desc.config_for(self.scope, Path::new(".")) else {
            return false;
        };
        if is_empty_shell(&self.proposed, &mcp.location, format)
            && fs::remove_file(&self.config_path).is_ok()
        {
            return true;
        }
        false
    }
}

/// True when `content` is exactly an empty managed section at (dotted)
/// `location` and nothing else — e.g. `{"mcpServers": {}}`.
fn is_empty_shell(content: &str, location: &str, format: Format) -> bool {
    let value: Value = match format {
        Format::Json => match serde_json::from_str(content) {
            Ok(v) => v,
            Err(_) => return false,
        },
        Format::Toml => {
            let Ok(t) = content.parse::<toml::Value>() else {
                return false;
            };
            match serde_json::to_value(t) {
                Ok(v) => v,
                Err(_) => return false,
            }
        }
    };
    let mut cur = &value;
    for key in location.split('.') {
        let Some(obj) = cur.as_object() else {
            return false;
        };
        // Any sibling next to the managed chain means real user content.
        if obj.len() != 1 {
            return false;
        }
        match obj.get(key) {
            Some(v) => cur = v,
            None => return false,
        }
    }
    cur.as_object().is_some_and(|o| o.is_empty())
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
    let mut failed: Vec<String> = Vec::new();
    let mut managed: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    let mut secrets: Vec<(String, String)> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    for (name, server) in servers {
        // Definition-level target scoping (`[servers.X] targets = [...]`).
        // Every plan — apply, diff, doctor drift, use, dashboard — flows
        // through here, so the scoping can't disagree between commands. A
        // previously managed server that no longer applies falls into the
        // prune set below, like any other deselection.
        if !server.applies_to(&desc.id) {
            continue;
        }
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
        for (f, why) in rendered.failed {
            failed.push(format!("{f} (server '{name}') — {why}"));
        }
        secrets.extend(rendered.secrets);
        // A stdio `cwd` this target's config can't express natively is instead
        // auto-wrapped by the renderer into a shell that `cd`s there (see
        // `render_server`), so it's no longer dropped and needs no warning.
        // The only remaining gap is the (practically unreachable) case where
        // the manifest has no `command` to wrap around — nothing to rewrite,
        // so cwd really is dropped and the user still needs to know.
        if server.server_type == crate::manifest::ServerType::Stdio
            && server.cwd.is_some()
            && mcp.fields.cwd.is_none()
            && !(mcp.fields.command.is_some() && server.command.is_some())
        {
            warnings.push(name.clone());
        }
        entries.push((name.clone(), rendered.value));
        managed.push(name.clone());
    }
    secrets.dedup();

    // Prune entries we used to own but no longer select.
    let removed: Vec<String> = previously_managed
        .iter()
        .filter(|n| !managed.contains(n))
        .cloned()
        .collect();

    let existing = fs::read_to_string(&config_path).unwrap_or_default();

    let mut proposed = match format {
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

    // First-run trust rule: selecting no servers for a target that has no config
    // yet must be a true no-op. Otherwise JSON adapters create empty scaffolds
    // like `{ "mcpServers": {} }`, which looks like a write blast radius even
    // though no capability was configured. Prunes still render so previously
    // managed entries can be removed.
    if managed.is_empty() && removed.is_empty() && existing.trim().is_empty() {
        proposed = existing.clone();
    }

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
        failed,
        skipped,
        secrets,
        warnings,
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

    #[test]
    fn one_resolution_per_ref_per_run_across_targets() {
        // The observed bug: apply resolved KIBANA_TOKEN fresh for every
        // target × server; a transient keychain failure partway through the
        // run made the same secret "unresolved" for the last targets only.
        // The chain must read each distinct ref once per run, so every target
        // sees the same value — even if the store turns flaky afterwards.
        use std::cell::Cell;
        use std::rc::Rc;

        /// Succeeds only on the very first read, then fails forever.
        struct FirstReadOnly {
            calls: Rc<Cell<usize>>,
        }
        impl crate::secret::Resolver for FirstReadOnly {
            fn resolve(&self, name: &str) -> Option<String> {
                self.lookup(name).found()
            }
            fn lookup(&self, _name: &str) -> crate::secret::Lookup {
                self.calls.set(self.calls.get() + 1);
                if self.calls.get() == 1 {
                    crate::secret::Lookup::Found("secret123".into())
                } else {
                    crate::secret::Lookup::Failed("keychain read failed: flaky".into())
                }
            }
        }

        let calls = Rc::new(Cell::new(0));
        let chain = crate::secret::Chain::new(vec![Box::new(FirstReadOnly {
            calls: calls.clone(),
        })]);

        let manifest: Manifest = toml::from_str(
            r#"
            version = 1
            [servers.kibana_mcp]
            type = "http"
            url = "https://x/mcp"
            headers = { Authorization = "Bearer ${KIBANA_TOKEN}" }
            "#,
        )
        .unwrap();
        let servers: IndexMap<String, Server> = manifest.servers.clone();

        let _g = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("HOME", home.path());
        let reg = Registry::load().unwrap();
        let proj = assert_fs::TempDir::new().unwrap();

        // Same run (same chain), several targets — as `apply --write` does.
        for target in ["claude-code", "opencode", "codex"] {
            let desc = reg.get(target).unwrap();
            let plan =
                plan_target_with_servers(desc, &chain, &servers, &[], Scope::Global, proj.path())
                    .unwrap()
                    .unwrap();
            assert!(
                plan.unresolved.is_empty() && plan.failed.is_empty(),
                "{target} must reuse the first resolution, got unresolved={:?} failed={:?}",
                plan.unresolved,
                plan.failed
            );
            assert!(plan.proposed.contains("secret123"), "{}", plan.proposed);
        }
        assert_eq!(calls.get(), 1, "one store read per distinct ref per run");
        std::env::remove_var("HOME");
    }

    #[test]
    fn server_targets_scope_the_fanout_and_prune_stale_entries() {
        // The adopted-plugin duplication bug: recipe-owned servers (adopted
        // from a native plugin, `targets = []`) fanned out to every target,
        // configuring the same server twice on the harness whose plugin
        // already provides it. The `targets` field scopes the fan-out inside
        // plan_target_with_servers, so apply/diff/doctor/use all agree.
        let manifest: Manifest = toml::from_str(
            r#"
            version = 1

            [servers.everywhere]
            type = "http"
            url = "https://x/mcp"

            [servers.claude-scoped]
            type = "http"
            url = "https://y/mcp"
            targets = ["claude-code"]

            [servers.github-github]
            type = "http"
            url = "https://api.githubcopilot.com/mcp/"
            targets = []
            "#,
        )
        .unwrap();
        let servers: IndexMap<String, Server> = manifest.servers.clone();

        let _g = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("HOME", home.path());
        let reg = Registry::load().unwrap();
        let proj = assert_fs::TempDir::new().unwrap();
        let resolver = MapResolver::default();

        // claude-code gets the wildcard server and the one scoped to it.
        let claude = plan_target_with_servers(
            reg.get("claude-code").unwrap(),
            &resolver,
            &servers,
            &[],
            Scope::Global,
            proj.path(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(claude.managed, vec!["everywhere", "claude-scoped"]);

        // codex gets only the wildcard server; the recipe-owned entry a
        // pre-scoping apply wrote there is pruned like any deselection.
        let codex = plan_target_with_servers(
            reg.get("codex").unwrap(),
            &resolver,
            &servers,
            &["github-github".to_string()],
            Scope::Global,
            proj.path(),
        )
        .unwrap()
        .unwrap();
        std::env::remove_var("HOME");
        assert_eq!(codex.managed, vec!["everywhere"]);
        assert_eq!(codex.removed, vec!["github-github"]);
        assert!(
            !codex.proposed.contains("githubcopilot"),
            "{}",
            codex.proposed
        );
    }

    #[test]
    fn cwd_renders_for_capable_target_and_auto_wraps_for_incapable_one() {
        let manifest: Manifest = toml::from_str(
            r#"
            version = 1

            [servers.tldraw]
            type = "stdio"
            command = "node"
            args = ["dist/index.js"]
            cwd = "/srv/tldraw"
            "#,
        )
        .unwrap();
        let servers: IndexMap<String, Server> = manifest.servers.clone();

        let _g = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("HOME", home.path());
        let reg = Registry::load().unwrap();
        let proj = assert_fs::TempDir::new().unwrap();
        let resolver = MapResolver::default();

        // Codex expresses cwd natively: it lands in the config, no warning.
        let codex = plan_target_with_servers(
            reg.get("codex").unwrap(),
            &resolver,
            &servers,
            &[],
            Scope::Global,
            proj.path(),
        )
        .unwrap()
        .unwrap();
        assert!(codex.proposed.contains("/srv/tldraw"), "{}", codex.proposed);
        assert!(codex.warnings.is_empty());

        // Claude Code has no cwd key: instead of dropping it, the server is
        // auto-wrapped in a shell that `cd`s there first — no warning needed
        // since the cwd is still honored.
        let claude = plan_target_with_servers(
            reg.get("claude-code").unwrap(),
            &resolver,
            &servers,
            &[],
            Scope::Global,
            proj.path(),
        )
        .unwrap()
        .unwrap();
        std::env::remove_var("HOME");
        assert_eq!(claude.managed, vec!["tldraw"]);
        assert!(
            claude.proposed.contains("/srv/tldraw"),
            "{}",
            claude.proposed
        );
        assert!(claude.proposed.contains("\"sh\""), "{}", claude.proposed);
        assert!(claude.warnings.is_empty(), "{:?}", claude.warnings);
    }

    #[test]
    fn codex_extras_survive_apply_and_match_hand_edited_config() {
        // The exact loss this guards against: a hand-added startup_timeout_sec
        // on a Codex npx server used to be dropped by every `apply --write`.
        let existing = r#"model = "gpt-5.5"

[mcp_servers.miro]
command = "npx"
args = ["-y", "@mirohq/mcp-server"]
# npx fetches from the registry on cold cache — must not block CLI startup
startup_timeout_sec = 20
"#;
        let manifest: Manifest = toml::from_str(
            r#"
            version = 1

            [servers.miro]
            type = "stdio"
            command = "npx"
            args = ["-y", "@mirohq/mcp-server"]

            [servers.miro.extra.codex]
            startup_timeout_sec = 20
            "#,
        )
        .unwrap();

        let _g = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("HOME", home.path());
        home.child(".codex/config.toml")
            .write_str(existing)
            .unwrap();

        let reg = Registry::load().unwrap();
        let desc = reg.get("codex").unwrap();
        let servers: IndexMap<String, Server> = manifest.servers.clone();
        let proj = assert_fs::TempDir::new().unwrap();
        let plan = plan_target_with_servers(
            desc,
            &MapResolver::default(),
            &servers,
            &[],
            Scope::Global,
            proj.path(),
        )
        .unwrap()
        .unwrap();
        std::env::remove_var("HOME");

        // The rendered entry carries the extra key…
        assert!(
            plan.proposed.contains("startup_timeout_sec = 20"),
            "{}",
            plan.proposed
        );
        // …and re-parses to the same server table as the hand-edited config
        // (comments inside a managed table are rewritten; the key is not lost).
        let a: toml::Value = plan.proposed.parse().unwrap();
        let b: toml::Value = existing.parse().unwrap();
        assert_eq!(a["mcp_servers"]["miro"], b["mcp_servers"]["miro"]);
    }

    #[test]
    fn empty_shell_detection() {
        // Pure husk → empty shell.
        assert!(is_empty_shell(
            "{\n  \"mcpServers\": {}\n}",
            "mcpServers",
            Format::Json
        ));
        // A remaining server → not empty.
        assert!(!is_empty_shell(
            "{\"mcpServers\": {\"x\": {}}}",
            "mcpServers",
            Format::Json
        ));
        // Sibling user content → never delete.
        assert!(!is_empty_shell(
            "{\"mcpServers\": {}, \"inputs\": []}",
            "mcpServers",
            Format::Json
        ));
        // TOML husk (empty table) and non-husk.
        assert!(is_empty_shell(
            "[mcp_servers]\n",
            "mcp_servers",
            Format::Toml
        ));
        assert!(!is_empty_shell(
            "[mcp_servers.x]\ncommand = \"npx\"\n",
            "mcp_servers",
            Format::Toml
        ));
        // Unparseable → never delete.
        assert!(!is_empty_shell("not json", "mcpServers", Format::Json));
    }
}
