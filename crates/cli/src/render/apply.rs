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
    /// Whether the config file was readable on disk when this plan was
    /// computed. `existing == ""` alone cannot answer that — an absent file
    /// and an empty file both read as "": the first means "never rendered
    /// here", the second "a rendered file the manifest moved ahead of", and
    /// external UIs need the distinction without parsing diff hunk headers.
    pub existed_before: bool,
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
    /// Set when the project's trust state forbids delivering the server
    /// definitions this plan carries (see the gate in [`trust_refusal`]). The
    /// plan is still built so the caller can SHOW what is being withheld;
    /// [`TargetPlan::write`] refuses, so a plan in this state can never reach
    /// disk.
    pub refusal: Option<String>,
}

/// Render one `TargetPlan::failed` entry — already shaped as
/// `NAME (server 'X') — <root cause>` — into a full report line for `apply` and
/// `use`. Shared by both so their advice never drifts. The fix is naming the
/// secret to set: a read that *errored* (as opposed to a *missing* secret) is
/// not something a bare retry fixes, so we point at `agentstack secret set`
/// unconditionally rather than the old "may be set; retry" guess. The `${REF}`
/// name is the entry's first token — ref names never contain whitespace.
pub fn failed_secret_line(entry: &str) -> String {
    let name = entry.split_whitespace().next().unwrap_or(entry);
    format!("secret read failed {entry} ↳ run `agentstack secret set {name}`, then re-run")
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

    /// Plain (uncolored) diff for the web t3code — same secret redaction.
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
    ///
    /// The write choke point. The trust refusal is enforced HERE, not only at
    /// the call sites, so a caller that forgets to read `refusal` still cannot
    /// put an untrusted project's server command lines into a harness config.
    pub fn write(&self) -> Result<()> {
        if let Some(why) = &self.refusal {
            anyhow::bail!("{why}");
        }
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

/// The key names present in `content`'s managed section at (dotted)
/// `location` — e.g. every MCP server name a target's config already has,
/// before this render touches it.
///
/// `diff` uses this to tell "ours" from "someone else's": a name here that
/// isn't in [`TargetPlan::managed`] or [`TargetPlan::removed`] was never
/// written by us — hand-added directly in the file, or left by another
/// agentstack manifest — either way, `merge_json`/`merge_toml` silently
/// preserve it today (review finding H8: that preservation was real but
/// unlabeled). Best-effort like [`is_empty_shell`]: unparsable content or an
/// absent section returns empty rather than erroring, since a foreign or
/// malformed file must never fail a read-only `diff`.
pub fn section_keys(content: &str, location: &str, format: Format) -> Vec<String> {
    let value: Value = match format {
        Format::Json => match serde_json::from_str(content) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        },
        Format::Toml => {
            let Ok(t) = content.parse::<toml::Value>() else {
                return Vec::new();
            };
            match serde_json::to_value(t) {
                Ok(v) => v,
                Err(_) => return Vec::new(),
            }
        }
    };
    let mut cur = &value;
    for key in location.split('.') {
        let Some(obj) = cur.as_object() else {
            return Vec::new();
        };
        match obj.get(key) {
            Some(v) => cur = v,
            None => return Vec::new(),
        }
    }
    cur.as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default()
}

/// The trust gate for server delivery — the same rule `render::extensions`
/// states as "untrusted means inert" (invariant 3), and `render::hooks` applies
/// to `[hooks.*]`, applied to the third thing a rendered file makes executable.
///
/// A `[servers.*]` entry IS a command line (stdio) or an endpoint the harness
/// talks to (http). Once it is in a native MCP config, the harness spawns or
/// dials it ITSELF, at its own startup, outside agentstack — so none of the
/// launch-time gates that do consult trust (`session start`, the protected
/// `run`, `Gateway::from_frozen`, the MCP auto-project gate) is in the path.
/// The rendered file is the delivery, and it is therefore what the gate has to
/// hold. As with hooks, the gate is on the CONTENT's provenance, not on the
/// destination, so it is identical at project scope (`.mcp.json`) and global
/// scope (`~/.claude.json`) — the global case being the sharper half, since a
/// repository's command line lands where every project the user opens reads it.
///
/// Returns the refusal message, or `None` when there is nothing to refuse.
/// Deliberately NOT gated:
///   * an empty `managed` — a prune (or a plan that selects nothing here)
///     removes or re-emits bytes we already own, which is the inert direction
///     and must keep working for an untrusted project, exactly as extension and
///     hook pruning does;
///   * the machine manifest itself — `$AGENTSTACK_HOME/agentstack.toml` is the
///     user's own personal layer. It is deliberately undiscoverable as a
///     project (`manifest::discover_project_base`), so `trust` can never reach
///     it: its base resolves to `$HOME`, which no code path can put in the
///     trust store. Gating it would make machine-level servers permanently
///     unrenderable rather than merely pending a review no command can perform.
///     One spelling of the rule across the capability kinds — keep this in step
///     with `render::hooks::trust_refusal` and `render::extensions::render`.
///
/// `prior` is the third exemption and the only conditional one: a command that
/// WROTE the manifest bytes this project is now `Changed` by is judged against
/// the state that held before it wrote, because a command cannot be allowed to
/// refuse itself. See [`crate::render::PriorTrust`] for why that authorizes
/// nothing new, and why nothing is re-pinned afterwards.
fn trust_refusal(
    managed: &[String],
    project_dir: &Path,
    prior: crate::render::PriorTrust,
) -> Option<String> {
    if managed.is_empty() {
        return None;
    }
    if crate::util::paths::is_machine_home(project_dir) {
        return None;
    }
    let base = crate::manifest::project_root_of(project_dir);
    let why = prior.refusal_reason(&base)?;
    let names = managed
        .iter()
        .map(|n| format!("'{n}'"))
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "refusing to render MCP servers: project at {} {why} — review and \
         `agentstack trust .` before writing server definitions the harness \
         launches on its own ({names})",
        base.display()
    ))
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
// One over the limit, and the eighth is the trust answer this plan is judged
// against — a grouping struct would only rename the same eight facts.
#[allow(clippy::too_many_arguments)]
pub fn plan_target(
    manifest: &Manifest,
    desc: &AdapterDescriptor,
    resolver: &dyn Resolver,
    selection: &Selection,
    previously_managed: &[String],
    scope: Scope,
    project_dir: &Path,
    prior: crate::render::PriorTrust,
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
        prior,
    )
}

/// Why a harness has no MCP **server destination** in a given scope.
///
/// [`plan_target_with_servers`] answers `Ok(None)` for two structurally
/// different situations, and collapsing them is what let `apply` throw a whole
/// harness away. A harness can lack a place to write SERVERS while still
/// declaring instructions, settings, hooks, skills and extensions — those route
/// rendered and have no other way to arrive. "No server destination" is
/// therefore never "nothing to do": it skips the servers leg only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoServerLane {
    /// The harness has no MCP channel at all, in any scope — it is a file-only
    /// tool by design (Pi). Every other lane still applies, in both scopes its
    /// descriptor declares.
    NoMcpChannel,
    /// The harness DOES speak MCP but declares no server config file for this
    /// particular scope (e.g. Claude Desktop has no project config). Only this
    /// scope's servers leg is absent.
    NoConfigInScope,
}

impl NoServerLane {
    /// A truthful one-line reason, for the message a command prints when it
    /// skips the servers leg. Deliberately states what is skipped (servers) and
    /// what is not (everything else), so the line can never again read as
    /// "this harness gets nothing".
    pub fn sentence(self, display: &str, scope: Scope) -> String {
        match self {
            NoServerLane::NoMcpChannel => format!(
                "{display} has no MCP channel — no servers to write; \
                 instructions, settings, hooks, skills and extensions still apply"
            ),
            NoServerLane::NoConfigInScope => format!(
                "{display} declares no {scope}-scope server config — no servers to write here; \
                 its other {scope}-scope files still apply"
            ),
        }
    }
}

/// Whether this harness has somewhere to write MCP servers in `scope`, and if
/// not, the true reason. `None` means a server destination exists (the normal
/// case) — mirroring exactly the condition under which
/// [`plan_target_with_servers`] returns `Ok(Some(_))` for a well-formed
/// descriptor, so a caller can branch on the reason instead of guessing one.
pub fn no_server_lane(
    desc: &AdapterDescriptor,
    scope: Scope,
    project_dir: &Path,
) -> Option<NoServerLane> {
    // Order matters: a harness with no `mcp` block has no MCP channel at all,
    // which is a stronger and more useful statement than "not in this scope".
    if desc.mcp.is_none() {
        return Some(NoServerLane::NoMcpChannel);
    }
    if desc.config_for(scope, project_dir).is_none() {
        return Some(NoServerLane::NoConfigInScope);
    }
    None
}

/// Build the plan for one target from an already-resolved **effective server
/// map** (`name -> Server`, `${REF}` placeholders intact). This is the core
/// renderer: secret resolution happens *here*, via `render_server` + `resolver`,
/// never earlier. Library-aware callers build the map with [`effective_servers`]
/// and call this directly.
///
/// `prior` is the trust state as of the calling command's START —
/// [`crate::render::PriorTrust::STRICT`] unless the caller captured one before
/// writing its own manifest bytes.
// One over the limit, and the eighth is the trust answer this plan is judged
// against — a grouping struct would only rename the same eight facts.
#[allow(clippy::too_many_arguments)]
pub fn plan_target_with_servers(
    desc: &AdapterDescriptor,
    resolver: &dyn Resolver,
    ruleset: &agentstack_policy::CompiledRuleset,
    servers: &IndexMap<String, Server>,
    previously_managed: &[String],
    scope: Scope,
    project_dir: &Path,
    prior: crate::render::PriorTrust,
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
        // Every plan — apply, diff, doctor drift, use, t3code — flows
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

    // A failed read (almost always "no such file") and an empty file must stay
    // distinguishable — `existed_before` carries that bit to diff/doctor.
    let (existing, existed_before) = match fs::read_to_string(&config_path) {
        Ok(content) => (content, true),
        Err(_) => (String::new(), false),
    };

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

    let refusal = trust_refusal(&managed, project_dir, prior);
    Ok(Some(TargetPlan {
        id: desc.id.clone(),
        display: desc.display.clone(),
        scope,
        config_path,
        existing,
        existed_before,
        proposed,
        managed,
        removed,
        unresolved,
        failed,
        denied,
        skipped,
        secrets,
        warnings,
        refusal,
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
                .with_context(|| format!("no toolset named '{p}' in manifest"))?;
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
        // Every server the manifest NAMES, not only the ones it defines inline
        // — see [`Manifest::declared_server_names`]. Reading `[servers]` alone
        // made a bare `apply` a silent no-op on exactly the manifest `init`
        // writes by default: everything in the library, `[servers]` empty, so
        // the selection came out empty and the run reported "no servers
        // selected" over six of them.
        Selection::All => Ok(manifest.declared_server_names()),
        Selection::Profile(p) => {
            let profile = manifest
                .profiles
                .get(p)
                .with_context(|| format!("no toolset named '{p}' in manifest"))?;
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
/// manifest's `[targets].default`, else the CLIs actually detected on this
/// machine — a hand-written manifest without `[targets]` should not create
/// `.cursor/`, `.junie/`, … config dirs for tools the user doesn't have. Only
/// when NOTHING is detected (e.g. a CI box rendering configs for the team)
/// does the fallback widen to every registered adapter, preserving the old
/// behavior where it was the only useful one.
///
/// An explicitly requested id that isn't a registered adapter is an ERROR
/// (with a did-you-mean), exactly as `--target`'s help promises — a typo'd
/// `--target codx` must never become a successful no-op that passes CI.
/// Manifest-sourced `[targets]` entries are NOT gated here: each command
/// reports those per target so `doctor` can diagnose a broken manifest
/// instead of dying on it.
///
/// Detection is asked of `project_dir`, not of the machine: a repo whose only
/// setup is a project-scope `.mcp.json` fans out to the CLIs that file belongs
/// to, instead of falling through to "nothing detected → every adapter in the
/// catalog".
pub fn resolve_targets(
    manifest: &Manifest,
    registry: &Registry,
    requested: &[String],
    project_dir: &std::path::Path,
) -> anyhow::Result<Vec<String>> {
    if !requested.is_empty() {
        for id in requested {
            if registry.get(id).is_none() {
                let hint = closest_id(registry, id)
                    .map(|c| format!(" — did you mean '{c}'?"))
                    .unwrap_or_default();
                anyhow::bail!(
                    "unknown CLI '{id}'{hint} (`agentstack x adapters list` shows all ids)"
                );
            }
        }
        return Ok(requested.to_vec());
    }
    if !manifest.targets.default.is_empty() {
        return Ok(manifest.targets.default.clone());
    }
    let detected: Vec<String> = registry
        .iter()
        .filter(|d| d.detected_in(project_dir))
        .map(|d| d.id.clone())
        .collect();
    Ok(if detected.is_empty() {
        registry.ids().map(String::from).collect()
    } else {
        detected
    })
}

/// The registered id closest to `input` by edit distance, when it's close
/// enough (≤ 2 edits, or ≤ 3 for longer ids) to be a plausible typo.
fn closest_id<'r>(registry: &'r Registry, input: &str) -> Option<&'r str> {
    let cap = if input.len() > 6 { 3 } else { 2 };
    registry
        .ids()
        .map(|id| (edit_distance(input, id), id))
        .filter(|(d, _)| *d <= cap)
        .min_by_key(|(d, _)| *d)
        .map(|(_, id)| id)
}

/// Classic two-row Levenshtein — ids are short, so O(a·b) is nothing.
fn edit_distance(a: &str, b: &str) -> usize {
    let b_chars: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b_chars.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b_chars.iter().enumerate() {
            let sub = prev[j] + usize::from(ca != *cb);
            cur.push(sub.min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    *prev.last().expect("row is never empty")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::LibraryServer;
    use crate::secret::MapResolver;
    use assert_fs::prelude::*;

    // DX witness: a typo'd `--target` is an ERROR with a did-you-mean, never a
    // successful no-op (the old "unknown adapter — skipping" + exit 0 could
    // pass CI while skipping the intended CLI).
    #[test]
    fn unknown_requested_target_errors_with_suggestion() {
        let registry = Registry::load().expect("built-in registry loads");
        let manifest: Manifest = toml::from_str("version = 1").expect("minimal manifest");
        let err = resolve_targets(
            &manifest,
            &registry,
            &["codx".to_string()],
            std::path::Path::new("."),
        )
        .expect_err("typo must not silently no-op");
        let msg = format!("{err:#}");
        assert!(msg.contains("did you mean 'codex'"), "{msg}");
        assert!(msg.contains("adapters list"), "{msg}");
    }

    #[test]
    fn failed_secret_line_names_the_fix_and_says_the_cause_once() {
        // Shape of a real `failed` entry after render + keychain root_cause.
        let entry = "FAKE_TOKEN_XYZ (server 'github') — keychain read failed: \
                     A default keychain could not be found.";
        let line = failed_secret_line(entry);

        // Names the secret and the correct fix (not the old "retry" guess).
        assert!(line.contains("FAKE_TOKEN_XYZ"), "{line}");
        assert!(
            line.contains("agentstack secret set FAKE_TOKEN_XYZ"),
            "{line}"
        );
        assert!(line.contains("then re-run"), "{line}");
        assert!(!line.contains("retry"), "old advice removed: {line}");
        assert!(!line.contains("may be set"), "old advice removed: {line}");

        // The root cause is stated exactly once — no doubled sentence.
        assert_eq!(
            line.matches("A default keychain could not be found.")
                .count(),
            1,
            "{line}"
        );
    }

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
            crate::render::PriorTrust::STRICT,
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
            crate::render::PriorTrust::STRICT,
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
            crate::render::PriorTrust::STRICT,
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
                crate::render::PriorTrust::STRICT,
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
            crate::render::PriorTrust::STRICT,
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
            crate::render::PriorTrust::STRICT,
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
            crate::render::PriorTrust::STRICT,
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
            crate::render::PriorTrust::STRICT,
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
            crate::render::PriorTrust::STRICT,
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

    #[test]
    fn section_keys_lists_json_and_toml_entries() {
        let mut keys = section_keys(
            "{\"mcpServers\": {\"postgres\": {}, \"handadded\": {}}}",
            "mcpServers",
            Format::Json,
        );
        keys.sort();
        assert_eq!(keys, vec!["handadded".to_string(), "postgres".to_string()]);

        let toml_keys = section_keys(
            "[mcp_servers.postgres]\ncommand = \"npx\"\n",
            "mcp_servers",
            Format::Toml,
        );
        assert_eq!(toml_keys, vec!["postgres".to_string()]);
    }

    // Invariant 8 witness: "no server destination" and "nothing to do" are
    // different facts, and the reason a command prints must be the true one.
    // Pi has no MCP channel BY DESIGN and still declares instructions,
    // settings, skills and extensions in BOTH scopes — so the servers leg is
    // the only thing absent, and the old "no global scope" line was false.
    #[test]
    fn no_mcp_channel_is_not_no_scope_and_never_means_nothing_to_do() {
        let reg = Registry::load().unwrap();
        let pi = reg.get("pi").expect("pi adapter is registered");
        let proj = assert_fs::TempDir::new().unwrap();

        for scope in [Scope::Global, Scope::Project] {
            assert_eq!(
                no_server_lane(pi, scope, proj.path()),
                Some(NoServerLane::NoMcpChannel),
                "pi has no MCP channel in {scope} scope"
            );
            let msg = NoServerLane::NoMcpChannel.sentence(&pi.display, scope);
            assert!(msg.contains("no MCP channel"), "{msg}");
            assert!(msg.contains("still apply"), "{msg}");
            assert!(
                !msg.contains("skipping"),
                "the harness is not skipped: {msg}"
            );
        }

        // …and the descriptor really does declare the other lanes in both
        // scopes, which is what makes the old message a false claim.
        assert!(pi.settings_for(Scope::Global, proj.path()).is_some());
        assert!(pi.instructions.is_some());
        assert!(pi.skills.is_some());
    }

    // A harness that DOES have an mcp block is unaffected — and the
    // scope-specific absence keeps its own, still-true reason.
    #[test]
    fn mcp_harness_has_a_server_lane_and_scope_absence_says_so() {
        let _g = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("HOME", home.path());
        let reg = Registry::load().unwrap();
        let proj = assert_fs::TempDir::new().unwrap();

        let claude = reg.get("claude-code").unwrap();
        assert_eq!(no_server_lane(claude, Scope::Global, proj.path()), None);
        assert_eq!(no_server_lane(claude, Scope::Project, proj.path()), None);

        // Claude Desktop speaks MCP but declares no project config file.
        let desktop = reg.get("claude-desktop").unwrap();
        assert_eq!(no_server_lane(desktop, Scope::Global, proj.path()), None);
        assert_eq!(
            no_server_lane(desktop, Scope::Project, proj.path()),
            Some(NoServerLane::NoConfigInScope)
        );
        let msg = NoServerLane::NoConfigInScope.sentence(&desktop.display, Scope::Project);
        assert!(msg.contains("project-scope server config"), "{msg}");

        std::env::remove_var("HOME");
    }

    #[test]
    fn section_keys_is_empty_for_absent_section_or_unparseable_content() {
        assert!(section_keys("{}", "mcpServers", Format::Json).is_empty());
        assert!(section_keys("not json", "mcpServers", Format::Json).is_empty());
    }
}
