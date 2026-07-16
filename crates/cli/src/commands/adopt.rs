//! `agentstack adopt` — pull hand-added servers from a target config back into
//! the manifest, lifting their inline secrets. The reverse of `apply`.
//!
//! Uses the TOML merger to insert `[servers.<name>]` tables into the existing
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
use crate::render::{merge_toml, resolve_targets};
use crate::scope::Scope;
use crate::secret::keychain;
use crate::util::diff;

pub fn run(args: &AdoptArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let ctx = super::load(manifest_dir)?;
    let manifest = &ctx.loaded.manifest;
    let scope = args.scope.unwrap_or(Scope::Global);

    // Collect servers present on disk but absent from the manifest.
    let mut collected: IndexMap<String, Server> = IndexMap::new();
    let target_ids = resolve_targets(manifest, &ctx.registry, &args.targets);

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
        let value: Value = match format {
            Format::Json => match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(_) => continue,
            },
            Format::Toml => match toml::from_str::<toml::Value>(&text) {
                Ok(tv) => serde_json::to_value(tv).unwrap_or(Value::Null),
                Err(_) => continue,
            },
        };
        for (name, server) in extract_servers(desc, &value) {
            match manifest.servers.get(&name) {
                None => {
                    if !collected.contains_key(&name) {
                        println!("  {} {name} (from {})", "+".green(), desc.display);
                        collected.insert(name, server);
                    }
                }
                // Already managed: adopt hand-added native keys (per-target
                // extras) the manifest doesn't carry yet. Surgical — canonical
                // fields keep the manifest as their source of truth.
                Some(existing) => {
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
        println!("Nothing to adopt — every on-disk server is already in the manifest.");
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
        // the delta even when the live config disagrees (that's canonical
        // drift, resolved by apply — not silently adopted).
        let existing =
            server("type = \"stdio\"\ncommand = \"npx\"\n[extra.codex]\nstartup_timeout_sec = 120");
        let imported =
            server("type = \"stdio\"\ncommand = \"npx\"\n[extra.codex]\nstartup_timeout_sec = 20");
        assert!(new_extras(&existing, imported).is_empty());
    }
}
