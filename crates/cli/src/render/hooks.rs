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
}

impl HooksPlan {
    pub fn changed(&self) -> bool {
        diff::differs(&self.existing, &self.proposed)
    }
    pub fn diff(&self) -> String {
        diff::mask_secrets(&diff::render(&self.existing, &self.proposed), &self.secrets)
    }
    pub fn write(&self) -> Result<()> {
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
    let mut selected: Vec<(&String, &Hook)> = manifest
        .hooks
        .iter()
        .filter(|(_, h)| h.targets.iter().any(|t| t == "*" || t == &desc.id))
        .collect();
    for (name, hook) in machine_hooks {
        if hook.targets.iter().any(|t| t == "*" || t == &desc.id) {
            selected.push((name, hook));
        }
    }
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
    }))
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
