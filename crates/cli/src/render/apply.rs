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
    /// Policy refusals — a `${REF}` `[policy.secrets]` denies this server, or
    /// an HTTP server whose declared URL host fails `[policy.egress]`. The
    /// message names the rule and layer. Blocks writes fail-closed; an
    /// egress-refused server is also skipped from the render entirely.
    pub denied: Vec<String>,
    /// Selected servers skipped from this target's render, as
    /// `(name, reason)`: a transport its config format can't represent (e.g.
    /// an HTTP server for the stdio-only Claude Desktop config), or a server
    /// NAME the CLI itself refuses at startup (e.g. Codex's
    /// `^[a-zA-Z0-9_-]+$`). Skipped rather than written as an entry the CLI
    /// rejects on every launch; the reason is surfaced verbatim.
    pub skipped: Vec<(String, String)>,
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
    let ruleset = ruleset_for(manifest)?;
    plan_target_with_servers(
        desc,
        resolver,
        &ruleset,
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
    ruleset: &agentstack_policy::CompiledRuleset,
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
    let mut denied: Vec<String> = Vec::new();
    let mut managed: Vec<String> = Vec::new();
    let mut skipped: Vec<(String, String)> = Vec::new();
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
        // A server NAME this CLI refuses at its own startup (e.g. Codex's
        // ^[a-zA-Z0-9_-]+$) would render into a config that errors on every
        // launch — skip the entry and say exactly why, never write it.
        if let Some(charset) = mcp.name_charset {
            if !charset.permits(name) {
                skipped.push((
                    name.clone(),
                    format!(
                        "{} rejects this server name at startup ({}) — rename the server in the manifest",
                        desc.display,
                        charset.describe()
                    ),
                ));
                continue;
            }
        }
        // Write-time egress check (HTTP only): the DECLARED URL host must
        // pass the effective [policy.egress] before this server is rendered
        // into a live config. A host hidden behind a ${REF} can't be checked
        // statically — fail closed only when a rule actually constrains this
        // server (allow-by-default otherwise). Runtime egress filtering is
        // the Phase-2 proxy's job; this covers what is knowable at write time.
        if server.server_type == crate::manifest::ServerType::Http {
            if let Some(url) = &server.url {
                match declared_host(url) {
                    Some(host) => {
                        if let Err(rule) = ruleset.egress_decision(name, &host, None) {
                            denied.push(format!("server '{name}' declared host {host} — {rule}"));
                            continue;
                        }
                    }
                    None => {
                        if ruleset.egress_constrained(name) {
                            denied.push(format!(
                                "server '{name}' has an egress policy but its declared URL host can't be verified (it contains a ${{REF}} or is malformed)"
                            ));
                            continue;
                        }
                    }
                }
            }
        }
        // Per-server secret scoping: refs outside this server's effective
        // [policy.secrets] never reach any backing store (fail closed).
        let scoped = crate::secret::ScopedResolver::new(resolver, ruleset, name);
        let rendered = render_server(desc, server, &scoped);
        // The adapter's format can't express this transport — skip it rather
        // than emit an empty `{}` entry into a real config file.
        if !rendered.representable {
            skipped.push((
                name.clone(),
                format!(
                    "{} can't represent this server's transport (add it via the harness's own UI/connector)",
                    desc.display
                ),
            ));
            continue;
        }
        for u in rendered.unresolved {
            unresolved.push(format!("{u} (server '{name}')"));
        }
        for (f, why) in rendered.failed {
            failed.push(format!("{f} (server '{name}') — {why}"));
        }
        for (_, why) in rendered.denied {
            denied.push(why);
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

    // No-op trust rule: when we manage nothing and prune nothing, we own no
    // bytes in this file, so the plan must propose the existing content
    // verbatim. Otherwise the merge normalizes untouched configs — scaffolding
    // `{ "mcpServers": {} }` into an empty or `{}` file, or reformatting a
    // hand-written section — and apply/diff/doctor report phantom drift
    // ("0 change(s) pending"). Prunes still render so previously managed
    // entries can be removed.
    if managed.is_empty() && removed.is_empty() {
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
        denied,
        skipped,
        secrets,
        warnings,
    }))
}

/// Compile the effective (machine ∩ project) ruleset for a manifest — the
/// artifact every render-time policy check consults. Server names come from
/// the inline `[servers.*]` table; names either policy layer mentions are
/// folded in by `compile` itself, and anything else routes to the rename-
/// proof `any` bucket, so library-resolved names are covered either way.
pub fn ruleset_for(manifest: &Manifest) -> Result<agentstack_policy::CompiledRuleset> {
    let names: Vec<&str> = manifest.servers.keys().map(String::as_str).collect();
    let machine = crate::machine_policy::load()?;
    Ok(agentstack_policy::compile(
        &machine,
        &manifest.policy,
        &names,
    ))
}

/// The host of a DECLARED server URL, statically: scheme stripped, userinfo
/// dropped, port dropped. `None` when the URL isn't HTTP(S), has no host, or the
/// host segment contains a `${REF}` (not knowable at write time).
///
/// Delegates to the ONE shared extractor in `core` so the write-time egress
/// check here and the D4 gateway-only fence classifier read every URL
/// identically — divergent parsers were exactly the seam that let a host be
/// fenced one way and checked another.
pub(crate) fn declared_host(url: &str) -> Option<String> {
    agentstack_core::manifest::host_from_url(url)
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
        let plan = plan_target_with_servers(
            desc,
            &resolver,
            &Default::default(),
            &map,
            &[],
            Scope::Global,
            proj.path(),
        )
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
        let plan2 = plan_target_with_servers(
            desc,
            &empty,
            &Default::default(),
            &map,
            &[],
            Scope::Global,
            proj.path(),
        )
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
    fn plan_with_nothing_managed_proposes_existing_verbatim() {
        // Regression: doctor warned "0 change(s) pending" because a plan with
        // no servers still normalized an existing `{}` config into
        // `{ "mcpServers": {} }`, making changed() true with nothing to apply.
        let _g = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("HOME", home.path());
        std::env::set_var("AGENTSTACK_HOME", home.child(".agentstack").path());

        let reg = Registry::load().unwrap();
        let desc = reg.get("claude-code").unwrap();
        let proj = assert_fs::TempDir::new().unwrap();
        let (config_path, _) = desc.config_for(Scope::Global, proj.path()).unwrap();
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        fs::write(&config_path, "{}").unwrap();

        let plan = plan_target_with_servers(
            desc,
            &MapResolver::from([]),
            &Default::default(),
            &IndexMap::new(),
            &[],
            Scope::Global,
            proj.path(),
        )
        .unwrap()
        .unwrap();

        assert_eq!(plan.proposed, plan.existing, "no-op plan must not reformat");
        assert!(!plan.changed());

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
            let plan = plan_target_with_servers(
                desc,
                &chain,
                &Default::default(),
                &servers,
                &[],
                Scope::Global,
                proj.path(),
            )
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
        // A server with an explicit `targets = []` opts out of the direct
        // fan-out entirely, and `targets = ["<id>"]` scopes it to named
        // adapters. The `targets` field is honored inside
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
            &Default::default(),
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
            &Default::default(),
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
            &Default::default(),
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
            &Default::default(),
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
            &Default::default(),
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
