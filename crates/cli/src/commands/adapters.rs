//! `agentstack adapters list|show|validate` — inspect the available CLI
//! adapters and check user-supplied descriptors.

use std::path::Path;

use anyhow::{bail, Context, Result};
use owo_colors::OwoColorize;

use crate::adapter::{AdapterDescriptor, AdapterSource, Registry};
use crate::cli::{AdaptersArgs, AdaptersCommand};

pub fn run(args: &AdaptersArgs) -> Result<()> {
    match &args.command {
        AdaptersCommand::Validate { file } => validate(file),
        AdaptersCommand::List => list(&Registry::load()?),
        AdaptersCommand::Show { id } => show(&Registry::load()?, id),
    }
}

/// A short tag naming where a descriptor came from (empty for plain built-ins).
fn origin_tag(registry: &Registry, desc: &AdapterDescriptor) -> String {
    match &desc.source {
        AdapterSource::BuiltIn => String::new(),
        AdapterSource::User(_) if registry.is_builtin(&desc.id) => {
            format!("  {}", "user override".magenta())
        }
        AdapterSource::User(_) => format!("  {}", "user".cyan()),
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
        println!(
            "  {:<14} {:<16} {status}{}",
            desc.id.bold(),
            desc.display,
            origin_tag(registry, desc)
        );
    }
    Ok(())
}

fn show(registry: &Registry, id: &str) -> Result<()> {
    let desc = registry
        .get(id)
        .with_context(|| format!("no adapter '{id}' (try `agentstack adapters list`)"))?;
    println!("id:      {}", desc.id);
    println!("display: {}", desc.display);
    match &desc.source {
        AdapterSource::BuiltIn => println!("source:  built-in"),
        AdapterSource::User(p) if registry.is_builtin(id) => {
            println!("source:  {} (overrides a built-in)", p.display())
        }
        AdapterSource::User(p) => println!("source:  {}", p.display()),
    }
    match &desc.config {
        Some(c) => println!("config:  {} ({:?})", c.path, c.format),
        None => println!("config:  (none — no MCP support)"),
    }
    if let Some(mcp) = &desc.mcp {
        println!("mcp:     location='{}'", mcp.location);
        println!("         secret_mode={:?}", mcp.secret_mode);
    }
    if let Some(s) = &desc.skills {
        println!("skills:  {}", s.dir);
    }
    Ok(())
}

/// Parse and sanity-check a user adapter descriptor file, so a user can catch a
/// broken descriptor before it silently fails to load from the adapters dir.
fn validate(file: &str) -> Result<()> {
    let path = Path::new(file);
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let desc: AdapterDescriptor = serde_yaml::from_str(&text)
        .with_context(|| format!("{} is not a valid adapter descriptor", path.display()))?;

    if desc.id.trim().is_empty() {
        bail!("adapter `id` is empty");
    }
    if desc.display.trim().is_empty() {
        bail!("adapter `display` is empty");
    }

    // A descriptor that declares no surface does nothing useful — warn, don't
    // fail (it still parses).
    let has_surface = desc.config.is_some()
        || desc.mcp.is_some()
        || desc.skills.is_some()
        || desc.instructions.is_some()
        || desc.settings.is_some()
        || desc.hooks.is_some()
        || desc.extensions.is_some();
    if !has_surface {
        println!(
            "{} '{}' parses but declares no surface (config/mcp/skills/instructions/settings/hooks) \
             — agentstack would have nothing to manage for it",
            "⚠".yellow(),
            desc.id
        );
    }

    println!(
        "{} '{}' ({}) is a valid adapter descriptor",
        "✓".green(),
        desc.id.bold(),
        desc.display
    );

    if Registry::load()?.is_builtin(&desc.id) {
        println!(
            "  {} id '{}' matches a built-in adapter — installing this to \
             ~/.agentstack/adapters/ would override it",
            "note:".dimmed(),
            desc.id
        );
    } else {
        println!(
            "  {} drop it in ~/.agentstack/adapters/{}.yaml to load it (no rebuild needed)",
            "next:".dimmed(),
            desc.id
        );
    }
    Ok(())
}
