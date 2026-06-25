//! `agentstack adapters list|show` — inspect the available CLI adapters.

use anyhow::{Context, Result};
use owo_colors::OwoColorize;

use crate::adapter::Registry;
use crate::cli::{AdaptersArgs, AdaptersCommand};

pub fn run(args: &AdaptersArgs) -> Result<()> {
    let registry = Registry::load()?;
    match &args.command {
        AdaptersCommand::List => list(&registry),
        AdaptersCommand::Show { id } => show(&registry, id),
    }
}

fn list(registry: &Registry) -> Result<()> {
    println!("Known adapters:\n");
    for desc in registry.iter() {
        let status = match (desc.is_installed(), desc.config_present()) {
            (true, _) => format!("{} installed", "✓".green()),
            (false, true) => format!("{} config present, binary not on PATH", "⚠".yellow()),
            (false, false) => format!("{} not detected", "·".dimmed()),
        };
        println!("  {:<14} {:<16} {status}", desc.id.bold(), desc.display);
    }
    Ok(())
}

fn show(registry: &Registry, id: &str) -> Result<()> {
    let desc = registry
        .get(id)
        .with_context(|| format!("no adapter '{id}' (try `agentstack adapters list`)"))?;
    println!("id:      {}", desc.id);
    println!("display: {}", desc.display);
    println!("config:  {} ({:?})", desc.config.path, desc.config.format);
    println!("mcp:     location='{}'", desc.mcp.location);
    println!("         secret_mode={:?}", desc.mcp.secret_mode);
    if let Some(s) = &desc.skills {
        println!("skills:  {}", s.dir);
    }
    Ok(())
}
