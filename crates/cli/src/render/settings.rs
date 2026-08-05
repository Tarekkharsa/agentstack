//! Render the manifest's per-CLI `[settings.<target>]` block into that CLI's
//! native settings file (Claude Code `settings.json`, Codex `config.toml`, …).
//!
//! We own a *set of top-level keys* (e.g. `permissions`, `env`): each is merged
//! non-destructively, unmanaged keys survive byte-for-byte, and keys we used to
//! own but dropped from the manifest are pruned. `${REF}`s in string values are
//! resolved per machine, exactly like server fields.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::{Map, Value};

use crate::adapter::descriptor::Format;
use crate::adapter::render::substitute;
use crate::adapter::AdapterDescriptor;
use crate::manifest::Manifest;
use crate::scope::Scope;
use crate::secret::Resolver;
use crate::util::diff;

use super::{merge_json, merge_toml};

/// The computed settings change for one target CLI.
pub struct SettingsPlan {
    pub id: String,
    pub display: String,
    pub scope: Scope,
    pub settings_path: PathBuf,
    pub existing: String,
    pub proposed: String,
    /// Top-level keys we rendered into this target's settings file.
    pub managed: Vec<String>,
    /// Keys we previously managed but pruned this run.
    pub removed: Vec<String>,
    /// `${REF}`s that did not resolve on this machine.
    pub unresolved: Vec<String>,
    /// Resolved secret values (`(ref-name, value)`) to redact from the diff
    /// preview. The real values stay in `proposed` and are what `write` persists.
    pub secrets: Vec<(String, String)>,
    /// The declared entries with their `${REF}`s resolved, in declaration order
    /// — exactly what was merged to produce `proposed`. Kept so a caller can ask
    /// the per-key question ([`SettingsPlan::drifted_keys`]) instead of only the
    /// whole-file one; never rendered anywhere, because a resolved value can
    /// carry a secret.
    resolved: Vec<(String, Value)>,
    /// Which merge dialect `settings_path` speaks. Needed to re-run the merge
    /// one key at a time.
    format: Format,
}

impl SettingsPlan {
    pub fn changed(&self) -> bool {
        diff::differs(&self.existing, &self.proposed)
    }

    /// The managed keys whose value in the live settings file is not what this
    /// plan would write — i.e. per-key drift, named.
    ///
    /// **Only keys we declare are examined, which is the whole point.**
    /// AgentStack owns a set of top-level keys and leaves the rest of the
    /// harness's file alone, so a user's unrelated edit elsewhere in that file
    /// must never read as drift. Asking the question one declared key at a time
    /// is what makes that structural rather than a promise.
    ///
    /// The comparison re-runs the real merge for a single entry and asks whether
    /// it would change anything. That is deliberate: it borrows the merge's own
    /// definition of "this key already holds this value" — including how each
    /// format spells nesting — instead of growing a second, subtly different
    /// notion of equality that could disagree with what `apply --write` does.
    /// A merge that errors (an existing settings file that no longer parses) is
    /// reported as drift on every declared key rather than silently as none:
    /// an unreadable destination is exactly when a caller must not be told
    /// "matches the pin".
    ///
    /// Returns key names only. Values are never included — they are resolved,
    /// and a resolved value can be a secret.
    pub fn drifted_keys(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (key, value) in &self.resolved {
            let one = [(key.clone(), value.clone())];
            let merged = match self.format {
                Format::Json => merge_json::merge_top_level(&self.existing, &one, &[]),
                Format::Toml => merge_toml::merge_top_level(&self.existing, &one, &[]),
            };
            match merged {
                Ok(text) if !diff::differs(&self.existing, &text) => {}
                _ => out.push(key.clone()),
            }
        }
        out
    }
    pub fn diff(&self) -> String {
        diff::mask_secrets(&diff::render(&self.existing, &self.proposed), &self.secrets)
    }
    pub fn write(&self) -> Result<()> {
        crate::util::atomic::write(&self.settings_path, &self.proposed)
    }
}

/// Build the settings plan for one target in a scope. Returns `Ok(None)` when
/// the CLI has no settings file for this scope (the common case for CLIs we
/// haven't mapped settings for yet). `previously_managed` are the keys we wrote
/// last run (from state); any no longer present are pruned.
pub fn plan_settings(
    manifest: &Manifest,
    desc: &AdapterDescriptor,
    resolver: &dyn Resolver,
    previously_managed: &[String],
    scope: Scope,
    project_dir: &Path,
) -> Result<Option<SettingsPlan>> {
    // The scope a CLI keeps its settings in is the CLI's choice, not ours.
    // Most declare one file and it is the global one; asking for the project
    // scope there used to return `None`, so a manifest's `[settings.<id>]`
    // was silently dropped and `apply` still reported the target in sync.
    // Fall back to the place the descriptor DOES declare: the keys reach the
    // file the CLI actually reads, and every surface prints that path, so the
    // wider scope is stated rather than assumed.
    let Some((settings_path, format)) = desc
        .settings_for(scope, project_dir)
        .or_else(|| desc.settings_for(Scope::Global, project_dir))
    else {
        return Ok(None);
    };
    // Nothing declared for this target and nothing previously managed → no-op.
    let declared = manifest.settings.get(&desc.id);
    if declared.is_none() && previously_managed.is_empty() {
        return Ok(None);
    }

    let mut unresolved: Vec<String> = Vec::new();
    let mut secrets: Vec<(String, String)> = Vec::new();
    let mut entries: Vec<(String, Value)> = Vec::new();
    let mut managed: Vec<String> = Vec::new();
    if let Some(Value::Object(obj)) = declared {
        for (k, v) in obj {
            let resolved = resolve_value(v, resolver, &mut unresolved, &mut secrets);
            entries.push((k.clone(), resolved));
            managed.push(k.clone());
        }
    }
    secrets.dedup();

    let removed: Vec<String> = previously_managed
        .iter()
        .filter(|k| !managed.contains(k))
        .cloned()
        .collect();

    let existing = fs::read_to_string(&settings_path).unwrap_or_default();
    let proposed = match format {
        Format::Json => merge_json::merge_top_level(&existing, &entries, &removed)?,
        Format::Toml => merge_toml::merge_top_level(&existing, &entries, &removed)?,
    };

    Ok(Some(SettingsPlan {
        id: desc.id.clone(),
        display: desc.display.clone(),
        scope,
        settings_path,
        existing,
        proposed,
        managed,
        removed,
        unresolved,
        secrets,
        resolved: entries,
        format,
    }))
}

/// Recursively resolve `${REF}`s in any string leaf of a settings value.
fn resolve_value(
    v: &Value,
    resolver: &dyn Resolver,
    unresolved: &mut Vec<String>,
    secrets: &mut Vec<(String, String)>,
) -> Value {
    match v {
        Value::String(s) => Value::String(substitute(s, resolver, false, unresolved, secrets)),
        Value::Array(arr) => Value::Array(
            arr.iter()
                .map(|e| resolve_value(e, resolver, unresolved, secrets))
                .collect(),
        ),
        Value::Object(obj) => {
            let mut out = Map::new();
            for (k, val) in obj {
                out.insert(k.clone(), resolve_value(val, resolver, unresolved, secrets));
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}
