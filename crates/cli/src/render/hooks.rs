//! Compile `[hooks.*]` into each hook-capable harness's native hooks config.
//!
//! agentstack owns the harness's whole hooks key (e.g. Claude Code's `hooks` in
//! settings.json): it is built entirely from the manifest, merged
//! non-destructively alongside other keys, and pruned when no hooks remain.
//! `${REF}`s in commands/args resolve per machine, like every other field.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::{json, Map, Value};

use crate::adapter::descriptor::{Format, HookShape};
use crate::adapter::render::substitute;
use crate::adapter::AdapterDescriptor;
use crate::manifest::{Hook, Manifest};
use crate::scope::Scope;
use crate::secret::Resolver;
use crate::util::diff;

use super::{merge_json, merge_toml};

/// The computed hooks change for one target CLI.
pub struct HooksPlan {
    pub id: String,
    pub display: String,
    pub scope: Scope,
    pub path: PathBuf,
    pub existing: String,
    pub proposed: String,
    /// Hook names we rendered into this target.
    pub managed: Vec<String>,
    pub unresolved: Vec<String>,
    /// Resolved secret values (`(ref-name, value)`) to redact from the diff
    /// preview. The real values stay in `proposed` and are what `write` persists.
    pub secrets: Vec<(String, String)>,
    /// Set when the project's trust state forbids delivering the manifest hooks
    /// this plan carries (see the gate in [`plan_hooks`]). The plan is still
    /// built so the caller can SHOW what is being withheld; [`HooksPlan::write`]
    /// refuses, so a plan in this state can never reach disk.
    pub refusal: Option<String>,
}

impl HooksPlan {
    pub fn changed(&self) -> bool {
        diff::differs(&self.existing, &self.proposed)
    }
    pub fn diff(&self) -> String {
        diff::mask_secrets(&diff::render(&self.existing, &self.proposed), &self.secrets)
    }
    /// The write choke point. The trust refusal is enforced HERE, not only at
    /// the call sites, so a caller that forgets to read `refusal` still cannot
    /// put an untrusted project's hook commands into a harness config.
    pub fn write(&self) -> Result<()> {
        if let Some(why) = &self.refusal {
            anyhow::bail!("{why}");
        }
        crate::util::atomic::write(&self.path, &self.proposed)
    }
}

/// Build the hooks plan for one target in a scope. `previously_managed` = did we
/// own this target's hooks last run (so an emptied set prunes the key). Returns
/// `None` when the CLI has no hooks destination for this scope.
///
/// `machine_hooks` are machine-layer entries (today: the `[guard]` hook when
/// enabled) rendered ALONGSIDE the manifest's — apply owns the whole hooks
/// key, so without this a global-scope apply would silently strip the guard
/// the user installed. The caller passes them only at global scope: machine
/// protection never lands in a repo's committed config.
pub fn plan_hooks(
    manifest: &Manifest,
    desc: &AdapterDescriptor,
    resolver: &dyn Resolver,
    previously_managed: bool,
    scope: Scope,
    project_dir: &Path,
    machine_hooks: &[(String, Hook)],
) -> Result<Option<HooksPlan>> {
    let Some((path, format)) = desc.hooks_for(scope, project_dir) else {
        return Ok(None);
    };
    // `hooks_for` returned Some, which it only does when `desc.hooks` is Some.
    let spec = desc
        .hooks
        .as_ref()
        .expect("hooks_for returned Some, so desc.hooks is Some");
    // The two layers stay separate until the gate below has judged them: only
    // the manifest's hooks are the project's content, and only they are gated.
    let mut declared: Vec<(&String, &Hook)> = manifest
        .hooks
        .iter()
        .filter(|(_, h)| h.targets.iter().any(|t| t == "*" || t == &desc.id))
        .collect();
    let mut machine: Vec<(&String, &Hook)> = machine_hooks
        .iter()
        .filter(|(_, h)| h.targets.iter().any(|t| t == "*" || t == &desc.id))
        .map(|(name, hook)| (name, hook))
        .collect();
    // Codex loads hooks from BOTH `config.toml [hooks]` (this renderer) AND
    // `~/.codex/hooks.json` (written by `guard guard install`). The guard hook
    // lives in hooks.json — a file `apply` never owns, so it survives without a
    // manifest — and Codex would fire it a second time if we also rendered it
    // here. Defer any guard hook to the hooks.json seam for Codex so it is
    // registered exactly once. (Every other CLI has a single hook destination.)
    if desc.id == "codex" {
        declared.retain(|(_, h)| !is_guard_hook(&h.command));
        machine.retain(|(_, h)| !is_guard_hook(&h.command));
    }
    let refusal = trust_refusal(&declared, project_dir);
    let mut selected = declared;
    selected.extend(machine);
    if selected.is_empty() && !previously_managed {
        return Ok(None);
    }

    let mut unresolved = Vec::new();
    let mut secrets: Vec<(String, String)> = Vec::new();
    let managed: Vec<String> = selected.iter().map(|(n, _)| (*n).clone()).collect();
    let existing = fs::read_to_string(&path).unwrap_or_default();

    let proposed = if selected.is_empty() {
        // Nothing declared anymore → prune the key we used to own.
        let removals = std::slice::from_ref(&spec.key);
        match format {
            Format::Json => merge_json::merge_top_level(&existing, &[], removals)?,
            Format::Toml => merge_toml::merge_top_level(&existing, &[], removals)?,
        }
    } else {
        let obj = match spec.shape {
            HookShape::Claude => {
                build_claude_hooks(&selected, resolver, &mut unresolved, &mut secrets)
            }
        };
        let entries = [(spec.key.clone(), obj)];
        match format {
            Format::Json => merge_json::merge_top_level(&existing, &entries, &[])?,
            Format::Toml => merge_toml::merge_top_level(&existing, &entries, &[])?,
        }
    };

    secrets.dedup();
    Ok(Some(HooksPlan {
        id: desc.id.clone(),
        display: desc.display.clone(),
        scope,
        path,
        existing,
        proposed,
        managed,
        unresolved,
        secrets,
        refusal,
    }))
}

/// The trust gate for hook delivery — the same rule `render::extensions` states
/// as "untrusted means inert" (invariant 3), applied to the other executable
/// capability kind.
///
/// A `[hooks.*]` entry is a command the harness runs in its own process at full
/// user permission (`docs/ENFORCEMENT.md`, Hooks), so it is executable content:
/// a project that is untrusted, or whose consent surface changed since it was
/// trusted, delivers ZERO of it. The gate is on the CONTENT's provenance, not on
/// the destination, so it holds identically at project scope
/// (`.claude/settings.json`) and at global scope (`~/.claude/settings.json`) —
/// `apply --scope global` on an untrusted repo was the sharper half of the hole
/// this closes.
///
/// Returns the refusal message, or `None` when there is nothing to refuse.
/// Deliberately NOT gated:
///   * an empty `declared` — a prune (or a machine-hooks-only plan) removes or
///     re-emits bytes we already own, which is the inert direction and must
///     keep working for an untrusted project, exactly as extension pruning does;
///   * machine-layer hooks (today the `[guard]` hook) — they are the user's own
///     machine configuration, never the project's content, so they ride along
///     either way and a refused project cannot strip them;
///   * the machine manifest itself — `$AGENTSTACK_HOME/agentstack.toml` is that
///     same personal layer. It is deliberately undiscoverable as a project
///     (`manifest::discover_project_base`), so `trust` can never reach it: its
///     base resolves to `$HOME`, which no code path can put in the trust store.
///     Gating it would make machine-level hooks permanently unrenderable rather
///     than merely pending a review no command can perform. Same exemption
///     shape the trust walk already gives machine-layer instruction fragments
///     (`render::instructions`).
fn trust_refusal(declared: &[(&String, &Hook)], project_dir: &Path) -> Option<String> {
    if declared.is_empty() {
        return None;
    }
    if crate::util::paths::is_machine_home(project_dir) {
        return None;
    }
    let base = crate::manifest::project_root_of(project_dir);
    let why = match crate::trust::check(&base) {
        crate::trust::TrustState::Trusted => return None,
        crate::trust::TrustState::Untrusted => "is not trusted",
        crate::trust::TrustState::Changed => "changed since it was trusted",
    };
    let names = declared
        .iter()
        .map(|(n, _)| format!("'{n}'"))
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "refusing to render hooks: project at {} {why} — review and \
         `agentstack trust .` before rendering hook commands the harness runs \
         at full user permission ({names})",
        base.display()
    ))
}

/// Does this hook command invoke `agentstack guard check`? Recognizes the
/// guard by its subcommand so the Codex renderer can defer it to the
/// `hooks.json` seam (see `plan_hooks`). Matches the marker the guard
/// installer's own `value_mentions_guard` uses.
fn is_guard_hook(command: &str) -> bool {
    command.contains("guard check --protocol")
}

/// Claude form: `{ Event: [ { matcher?, hooks: [ {type:"command", command, …} ] } ] }`.
pub(crate) fn build_claude_hooks(
    selected: &[(&String, &Hook)],
    resolver: &dyn Resolver,
    unresolved: &mut Vec<String>,
    secrets: &mut Vec<(String, String)>,
) -> Value {
    let mut events: Map<String, Value> = Map::new();
    for (_, h) in selected {
        let mut handler = Map::new();
        handler.insert("type".into(), json!("command"));
        handler.insert(
            "command".into(),
            json!(substitute(&h.command, resolver, false, unresolved, secrets)),
        );
        if !h.args.is_empty() {
            let args: Vec<Value> = h
                .args
                .iter()
                .map(|a| json!(substitute(a, resolver, false, unresolved, secrets)))
                .collect();
            handler.insert("args".into(), Value::Array(args));
        }
        if let Some(t) = h.timeout {
            handler.insert("timeout".into(), json!(t));
        }

        let mut entry = Map::new();
        if let Some(m) = &h.matcher {
            entry.insert("matcher".into(), json!(m));
        }
        entry.insert("hooks".into(), Value::Array(vec![Value::Object(handler)]));

        events
            .entry(h.event.clone())
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .expect("the entry was just inserted as Value::Array")
            .push(Value::Object(entry));
    }
    Value::Object(events)
}
