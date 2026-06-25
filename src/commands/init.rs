//! `agentstack init` — never a blank page. Detect installed CLIs, import their
//! existing MCP servers into one manifest, and lift inline secrets into
//! `${REF}`s (stored in the keychain).

use std::path::Path;

use anyhow::{Context, Result};
use indexmap::IndexMap;
use owo_colors::OwoColorize;

use crate::adapter::{extract_servers, Registry};
use crate::cli::InitArgs;
use crate::discover::{lift_secrets, merge_servers};
use crate::manifest::load::MANIFEST_FILE;
use crate::manifest::model::{Manifest, Meta, Server, Targets};
use crate::secret::keychain;

pub fn run(args: &InitArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let dir = match manifest_dir {
        Some(d) => d.to_path_buf(),
        None => std::env::current_dir()?,
    };
    let manifest_path = dir.join(MANIFEST_FILE);
    if manifest_path.exists() && !args.force && !args.dry_run {
        anyhow::bail!(
            "{} already exists — use --force to overwrite or --dry-run to preview",
            manifest_path.display()
        );
    }

    let registry = Registry::load()?;

    // Discover + import.
    let mut detected: Vec<String> = Vec::new();
    let mut servers: IndexMap<String, Server> = IndexMap::new();
    let mut display_names: Vec<String> = Vec::new();

    for desc in registry.iter() {
        if !desc.detected() {
            continue;
        }
        detected.push(desc.id.clone());
        display_names.push(desc.display.clone());

        let Some(value) = desc.read_config_value()? else {
            continue;
        };
        let imported = extract_servers(desc, &value);
        let conflicts = merge_servers(&mut servers, imported);
        for c in conflicts {
            println!(
                "{} server '{c}' differs between CLIs — kept the first definition",
                "⚠".yellow()
            );
        }
    }

    if detected.is_empty() {
        println!("No supported CLIs detected on this machine. Nothing to import.");
        return Ok(());
    }

    println!(
        "{}  Detected {} CLI(s): {}",
        "🔍".dimmed(),
        detected.len(),
        display_names.join(" · ")
    );
    println!(
        "{}  Imported {} MCP server(s) from existing configs",
        "📥".dimmed(),
        servers.len()
    );

    // Lift inline secrets.
    let lifted = lift_secrets(&mut servers);
    if !lifted.is_empty() {
        let names: Vec<&str> = lifted.iter().map(|l| l.reference.as_str()).collect();
        println!(
            "{}  Lifted {} inline secret(s) → {}",
            "🔐".dimmed(),
            lifted.len(),
            names.join(", ")
        );
    }

    // Assemble the manifest.
    let manifest = Manifest {
        version: 1,
        meta: Meta { name: None },
        servers,
        skills: IndexMap::new(),
        profiles: IndexMap::new(),
        instructions: IndexMap::new(),
        targets: Targets {
            default: detected.clone(),
        },
        policy: Default::default(),
    };
    let toml_text = toml::to_string_pretty(&manifest).context("serializing manifest to TOML")?;

    if args.dry_run {
        println!("\n{} (preview — nothing written)\n", MANIFEST_FILE.bold());
        println!("{toml_text}");
        if !lifted.is_empty() {
            println!("Would store {} secret(s) in the keychain.", lifted.len());
        }
        return Ok(());
    }

    // Store lifted secrets (unless opted out).
    if !args.no_keychain {
        for l in &lifted {
            keychain::set(&l.reference, &l.value)
                .with_context(|| format!("storing '{}' in keychain", l.reference))?;
        }
    }

    std::fs::write(&manifest_path, &toml_text)
        .with_context(|| format!("writing {}", manifest_path.display()))?;

    println!("{}  Wrote {}", "✅".dimmed(), manifest_path.display());
    if !lifted.is_empty() && args.no_keychain {
        println!(
            "{} secret(s) referenced but not stored (--no-keychain). Run `agentstack secret set <NAME>`.",
            lifted.len()
        );
    }
    println!("\nNext: review the manifest, then `agentstack diff` to preview rendering.");
    Ok(())
}
