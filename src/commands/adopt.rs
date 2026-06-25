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
            if !manifest.servers.contains_key(&name) && !collected.contains_key(&name) {
                println!("  {} {name} (from {})", "+".green(), desc.display);
                collected.insert(name, server);
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
        .map(|(n, s)| (n.clone(), serde_json::to_value(s).unwrap()))
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
