//! `agentstack adopt` — pull hand-added servers AND hand-edited fields of
//! manifest-known servers from a target config back into the manifest, lifting
//! their inline secrets. The reverse of `apply`.
//!
//! Edited fields are detected by comparing each target's *rendered* form of a
//! server against its on-disk entry, both read back through the same adapter
//! lens ([`extract_servers`]) — so adapter transforms (cwd shell-wrapping,
//! renamed keys) can never masquerade as hand-edits.
//!
//! Uses the TOML merger to upsert `[servers.<name>]` tables into the existing
//! `agentstack.toml`, preserving its comments and formatting.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use indexmap::IndexMap;
use owo_colors::OwoColorize;
use serde_json::Value;

use crate::adapter::descriptor::Format;
use crate::adapter::extract_servers;
use crate::cli::AdoptArgs;
use crate::discover::lift_secrets;
use crate::manifest::Server;
use crate::render::{merge_toml, plan_target_with_servers, resolve_targets, ruleset_for};
use crate::scope::Scope;
use crate::secret::keychain;
use crate::util::diff;

pub fn run(args: &AdoptArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let ctx = super::load(manifest_dir)?;
    let manifest = &ctx.loaded.manifest;
    let scope = args.scope.unwrap_or_else(|| Scope::default_for(&ctx.dir));

    // Collect servers present on disk but absent from the manifest, plus
    // hand-edited fields of servers the manifest already knows.
    let mut collected: IndexMap<String, Server> = IndexMap::new();
    let target_ids = resolve_targets(manifest, &ctx.registry, &args.targets)?;
    // Detecting an edited field needs the rendered baseline, and rendering
    // resolves secrets under the effective policy — same gate as apply/diff.
    let ruleset = ruleset_for(manifest)?;

    for id in &target_ids {
        let Some(desc) = ctx.registry.get(id) else {
            continue;
        };
        let Some((config_path, format)) = desc.config_for(scope, &ctx.dir) else {
            continue;
        };
        let text = fs::read_to_string(&config_path).unwrap_or_default();
        if text.trim().is_empty() {
            continue;
        }
        let Some(value) = parse_config(&text, format) else {
            continue;
        };
        // What the manifest would put on this target's disk, read back through
        // the same adapter lens as the on-disk entries — the drift baseline.
        let rendered_by_name: IndexMap<String, Server> = plan_target_with_servers(
            desc,
            &ctx.resolver,
            &ruleset,
            &manifest.servers,
            &[],
            scope,
            &ctx.dir,
        )?
        .and_then(|plan| parse_config(&plan.proposed, format))
        .map(|v| extract_servers(desc, &v).into_iter().collect())
        .unwrap_or_default();
        for (name, server) in extract_servers(desc, &value) {
            match manifest.servers.get(&name) {
                None => {
                    if !collected.contains_key(&name) {
                        println!("  {} {name} (from {})", "+".green(), desc.display);
                        collected.insert(name, server);
                    }
                }
                Some(existing) => {
                    // Hand-edited fields: any value where this target's disk
                    // disagrees with the manifest's rendered form. Owned
                    // servers are skipped — their refresh loop is `apply`'s
                    // job (see render::owned), not adoption.
                    if existing.owner.is_none() {
                        if let Some(rendered) = rendered_by_name.get(&name) {
                            let mut updated = collected.get(&name).unwrap_or(existing).clone();
                            let fields =
                                adopt_changed_fields(&mut updated, rendered, &server, &desc.id);
                            if !fields.is_empty() {
                                println!(
                                    "  {} {name}: {} (from {})",
                                    "~".yellow(),
                                    fields.join(", "),
                                    desc.display
                                );
                                collected.insert(name.clone(), updated);
                            }
                        }
                    }
                    // Hand-added native keys (per-target extras) the manifest
                    // doesn't carry yet.
                    for (target, new_keys) in new_extras(existing, server) {
                        println!(
                            "  {} {name}: extra.{target} {{{}}} (from {})",
                            "~".yellow(),
                            new_keys.keys().cloned().collect::<Vec<_>>().join(", "),
                            desc.display
                        );
                        let merged = collected
                            .entry(name.clone())
                            .or_insert_with(|| existing.clone());
                        merged.extra.entry(target).or_default().extend(new_keys);
                    }
                }
            }
        }
    }

    if collected.is_empty() {
        println!("Nothing to adopt — every on-disk server already matches the manifest.");
        return Ok(());
    }

    // Lift inline secrets so the manifest stays commit-safe.
    let lifted = lift_secrets(&mut collected);

    // Insert into the existing manifest text, preserving comments.
    let entries: Vec<(String, Value)> = collected
        .iter()
        .map(|(n, s)| {
            let value = serde_json::to_value(s)
                .expect("an internal derive(Serialize) struct always serializes");
            (n.clone(), value)
        })
        .collect();
    let manifest_text = fs::read_to_string(&ctx.loaded.manifest_path)
        .with_context(|| format!("reading {}", ctx.loaded.manifest_path.display()))?;
    let new_text = merge_toml::merge(&manifest_text, "servers", &entries, true)?;

    println!(
        "\n{} {} server(s) to adopt into {}",
        "→".cyan(),
        collected.len(),
        ctx.loaded.manifest_path.display()
    );
    print!(
        "{}",
        diff::render(&manifest_text, &new_text)
            .lines()
            .map(|l| format!("  {l}\n"))
            .collect::<String>()
    );
    if !lifted.is_empty() {
        let names: Vec<&str> = lifted.iter().map(|l| l.reference.as_str()).collect();
        println!("  {} lifted secret(s): {}", "🔐".dimmed(), names.join(", "));
    }

    if args.write {
        if !args.no_keychain {
            for l in &lifted {
                keychain::set(&l.reference, &l.value)
                    .with_context(|| format!("storing '{}' in keychain", l.reference))?;
            }
        }
        crate::util::atomic::write(&ctx.loaded.manifest_path, &new_text)
            .with_context(|| format!("writing {}", ctx.loaded.manifest_path.display()))?;
        println!("\n{} adopted {} server(s).", "✓".green(), collected.len());
    } else {
        println!(
            "\nDry run. Re-run with {} to update the manifest.",
            "--write".bold()
        );
    }
    Ok(())
}

/// Parse a target config's text into a JSON-shaped value tree, `None` when it
/// doesn't parse (an unreadable config is skipped, never adopted from).
fn parse_config(text: &str, format: Format) -> Option<Value> {
    match format {
        Format::Json => serde_json::from_str(text).ok(),
        Format::Toml => toml::from_str::<toml::Value>(text)
            .ok()
            .and_then(|tv| serde_json::to_value(tv).ok()),
    }
}

fn has_ref(s: &str) -> bool {
    !crate::secret::refs_in(s).is_empty()
}

/// Any string leaf carrying a `${REF}` (extras may nest).
fn value_has_ref(v: &Value) -> bool {
    match v {
        Value::String(s) => has_ref(s),
        Value::Array(a) => a.iter().any(value_has_ref),
        Value::Object(o) => o.values().any(value_has_ref),
        _ => false,
    }
}

/// Pull hand-edited fields of a manifest-known server into `entry` (the
/// manifest definition being updated): every canonical field — and every
/// per-target extra key the manifest already carries — where `disk` (the
/// on-disk entry) disagrees with `rendered` (the manifest's rendered form for
/// this target). Both sides went through the same adapter lens, so adapter
/// transforms compare equal and only real edits surface.
///
/// A rendered value still carrying a `${REF}` (unresolved secret) is skipped:
/// without the secret, equality with the disk literal can't be judged, and a
/// false diff would copy a stale literal over the reference. Fields where the
/// manifest's ref DID resolve compare against the resolved form, so a rotated
/// on-disk token is picked up (and re-lifted by `lift_secrets` afterwards).
///
/// Returns the labels of the adopted fields. Idempotent across targets: a
/// value `entry` already carries is never re-adopted or re-reported.
fn adopt_changed_fields(
    entry: &mut Server,
    rendered: &Server,
    disk: &Server,
    adapter_id: &str,
) -> Vec<String> {
    let mut changed = Vec::new();

    if rendered.server_type != disk.server_type && entry.server_type != disk.server_type {
        entry.server_type = disk.server_type;
        changed.push("type".to_string());
    }
    adopt_scalar(
        &mut entry.url,
        &rendered.url,
        &disk.url,
        "url",
        &mut changed,
    );
    adopt_scalar(
        &mut entry.command,
        &rendered.command,
        &disk.command,
        "command",
        &mut changed,
    );
    adopt_scalar(
        &mut entry.cwd,
        &rendered.cwd,
        &disk.cwd,
        "cwd",
        &mut changed,
    );
    if rendered.args != disk.args
        && entry.args != disk.args
        && !rendered.args.iter().any(|a| has_ref(a))
    {
        entry.args = disk.args.clone();
        changed.push("args".to_string());
    }
    adopt_map(
        &mut entry.headers,
        &rendered.headers,
        &disk.headers,
        "headers",
        &mut changed,
    );
    adopt_map(
        &mut entry.env,
        &rendered.env,
        &disk.env,
        "env",
        &mut changed,
    );

    // Per-target extras: value edits and removals of keys the manifest already
    // renders for this adapter. Hand-ADDED extra keys are `new_extras`' job.
    if let Some(rendered_extra) = rendered.extra.get(adapter_id) {
        let disk_extra = disk.extra.get(adapter_id);
        for (k, rv) in rendered_extra {
            if value_has_ref(rv) {
                continue;
            }
            match disk_extra.and_then(|d| d.get(k)) {
                Some(dv) if dv != rv => {
                    let slot = entry.extra.entry(adapter_id.to_string()).or_default();
                    if slot.get(k) != Some(dv) {
                        slot.insert(k.clone(), dv.clone());
                        changed.push(format!("extra.{adapter_id}.{k}"));
                    }
                }
                Some(_) => {}
                None => {
                    let removed = entry
                        .extra
                        .get_mut(adapter_id)
                        .is_some_and(|slot| slot.shift_remove(k).is_some());
                    if removed {
                        changed.push(format!("extra.{adapter_id}.{k} (removed)"));
                    }
                }
            }
        }
        if entry.extra.get(adapter_id).is_some_and(|m| m.is_empty()) {
            entry.extra.shift_remove(adapter_id);
        }
    }

    changed
}

/// One optional scalar field of [`adopt_changed_fields`]' contract: follow
/// disk when it disagrees with the rendered form, unless the rendered value
/// still carries an unresolved `${REF}` or `entry` already has the disk value.
fn adopt_scalar(
    entry: &mut Option<String>,
    rendered: &Option<String>,
    disk: &Option<String>,
    label: &str,
    changed: &mut Vec<String>,
) {
    if rendered == disk
        || rendered.as_deref().is_some_and(has_ref)
        || entry.as_deref() == disk.as_deref()
    {
        return;
    }
    *entry = disk.clone();
    changed.push(label.to_string());
}

/// Per-key map (headers/env) counterpart of [`adopt_scalar`]: edited and added
/// keys follow disk; a key the render carries but disk dropped is removed.
fn adopt_map(
    entry: &mut IndexMap<String, String>,
    rendered: &IndexMap<String, String>,
    disk: &IndexMap<String, String>,
    prefix: &str,
    changed: &mut Vec<String>,
) {
    for (k, dv) in disk {
        match rendered.get(k) {
            Some(rv) if rv == dv || has_ref(rv) => continue,
            _ => {}
        }
        if entry.get(k) == Some(dv) {
            continue;
        }
        entry.insert(k.clone(), dv.clone());
        changed.push(format!("{prefix}.{k}"));
    }
    for (k, rv) in rendered {
        if disk.contains_key(k) || has_ref(rv) {
            continue;
        }
        if entry.shift_remove(k).is_some() {
            changed.push(format!("{prefix}.{k} (removed)"));
        }
    }
}

/// The per-target extras in `imported` (a server extracted from a live config)
/// that `existing` (the manifest entry) doesn't carry yet — the adoptable
/// delta for an already-managed server.
fn new_extras(existing: &Server, imported: Server) -> IndexMap<String, IndexMap<String, Value>> {
    imported
        .extra
        .into_iter()
        .filter_map(|(target, fields)| {
            let have = existing.extra.get(&target);
            let fresh: IndexMap<String, Value> = fields
                .into_iter()
                .filter(|(k, _)| have.map_or(true, |h| !h.contains_key(k)))
                .collect();
            (!fresh.is_empty()).then_some((target, fresh))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::prelude::*;

    fn server(toml_str: &str) -> Server {
        toml::from_str(toml_str).unwrap()
    }

    #[test]
    fn new_extras_reports_only_missing_keys() {
        // Manifest entry already carries one codex extra; the live config adds
        // startup_timeout_sec (hand-tuned) and repeats the one we have.
        let existing = server("type = \"stdio\"\ncommand = \"npx\"\n[extra.codex]\nnote = \"x\"");
        let imported = server(
            "type = \"stdio\"\ncommand = \"npx\"\n\
             [extra.codex]\nnote = \"x\"\nstartup_timeout_sec = 20",
        );
        let delta = new_extras(&existing, imported);
        assert_eq!(delta.len(), 1);
        assert_eq!(delta["codex"].len(), 1);
        assert_eq!(delta["codex"]["startup_timeout_sec"], serde_json::json!(20));

        // Nothing new → empty delta (adopt stays a no-op).
        let same = server("type = \"stdio\"\ncommand = \"npx\"\n[extra.codex]\nnote = \"x\"");
        assert!(new_extras(&existing, same).is_empty());
    }

    #[test]
    fn new_extras_never_touches_existing_values() {
        // A key present in both keeps the manifest's value: it is not part of
        // new_extras' delta. Edited values of manifest-known keys are
        // adopt_changed_fields' job (rendered-vs-disk, so an unresolved
        // ${REF} can't be clobbered by its stale literal).
        let existing =
            server("type = \"stdio\"\ncommand = \"npx\"\n[extra.codex]\nstartup_timeout_sec = 120");
        let imported =
            server("type = \"stdio\"\ncommand = \"npx\"\n[extra.codex]\nstartup_timeout_sec = 20");
        assert!(new_extras(&existing, imported).is_empty());
    }

    #[test]
    fn hand_edited_url_adopts_and_next_apply_is_a_noop() {
        // The reference.md promise: "edited on disk since last apply → adopt
        // pulls it into the manifest." Repro of the bug this guards against:
        // apply, hand-edit the url in .mcp.json, adopt — the manifest must
        // pick up the edit so the next apply no longer reverts it.
        let _g = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("HOME", home.path());
        std::env::set_var("AGENTSTACK_HOME", home.child(".agentstack").path());

        let proj = assert_fs::TempDir::new().unwrap();
        proj.child("agentstack.toml")
            .write_str(
                "version = 1\n\n# docs server for the team\n[servers.docs]\ntype = \"http\"\n\
                 url = \"https://docs.example/mcp\"\ntargets = [\"claude-code\"]\n",
            )
            .unwrap();

        // `apply --scope project --write`, then a hand-edit of the url.
        let reg = crate::adapter::Registry::load().unwrap();
        let desc = reg.get("claude-code").unwrap();
        let resolver = crate::secret::MapResolver::default();
        let manifest: crate::manifest::Manifest =
            toml::from_str(&fs::read_to_string(proj.child("agentstack.toml").path()).unwrap())
                .unwrap();
        plan_target_with_servers(
            desc,
            &resolver,
            &Default::default(),
            &manifest.servers,
            &[],
            Scope::Project,
            proj.path(),
        )
        .unwrap()
        .unwrap()
        .write()
        .unwrap();
        let mcp_path = proj.child(".mcp.json");
        let edited = fs::read_to_string(mcp_path.path())
            .unwrap()
            .replace("https://docs.example/mcp", "https://docs-eu.example/mcp");
        fs::write(mcp_path.path(), &edited).unwrap();

        // Adopt pulls the edited url back into the manifest…
        let args = crate::cli::AdoptArgs {
            targets: vec!["claude-code".into()],
            scope: Some(Scope::Project),
            write: true,
            no_keychain: true,
        };
        run(&args, Some(proj.path())).unwrap();
        let manifest_text = fs::read_to_string(proj.child("agentstack.toml").path()).unwrap();
        assert!(
            manifest_text.contains("https://docs-eu.example/mcp"),
            "{manifest_text}"
        );
        assert!(
            !manifest_text.contains("url = \"https://docs.example/mcp\""),
            "{manifest_text}"
        );
        assert!(
            manifest_text.contains("# docs server for the team"),
            "comments above the server table survive: {manifest_text}"
        );

        // …and the next apply proposes no change: the hand-edit survives.
        let manifest: crate::manifest::Manifest = toml::from_str(&manifest_text).unwrap();
        let plan = plan_target_with_servers(
            desc,
            &resolver,
            &Default::default(),
            &manifest.servers,
            &[],
            Scope::Project,
            proj.path(),
        )
        .unwrap()
        .unwrap();
        std::env::remove_var("AGENTSTACK_HOME");
        std::env::remove_var("HOME");
        assert!(!plan.changed(), "apply must be a no-op:\n{}", plan.diff());
    }
}
