//! `agentstack secret set|get|rm|list` — manage secrets in the OS keychain and
//! audit which `${REF}`s the manifest needs on this machine.

use std::path::Path;

use anyhow::{Context, Result};
use owo_colors::OwoColorize;

use crate::cli::{SecretArgs, SecretCommand};
use crate::secret::{
    keychain, DotEnvResolver, EnvResolver, KeychainResolver, Resolver, VarlockResolver,
};

pub fn run(args: &SecretArgs, manifest_dir: Option<&Path>) -> Result<()> {
    match &args.command {
        SecretCommand::Set { name, value } => set(name, value.as_deref()),
        SecretCommand::Get { name } => get(name),
        SecretCommand::Rm { name } => rm(name),
        SecretCommand::List => list(manifest_dir),
    }
}

fn set(name: &str, value: Option<&str>) -> Result<()> {
    let value = match value {
        Some(v) => v.to_string(),
        None => rpassword::prompt_password(format!("Value for {name}: "))
            .context("reading secret from prompt")?,
    };
    if value.is_empty() {
        anyhow::bail!("refusing to store an empty value for '{name}'");
    }
    keychain::set(name, &value)?;
    println!("{} stored '{name}' in keychain", "✓".green());
    Ok(())
}

fn get(name: &str) -> Result<()> {
    match keychain::get(name)? {
        Some(v) => {
            println!("{v}");
            Ok(())
        }
        None => {
            anyhow::bail!("no secret '{name}' in keychain");
        }
    }
}

fn rm(name: &str) -> Result<()> {
    if keychain::delete(name)? {
        println!("{} removed '{name}'", "✓".green());
    } else {
        println!("{} '{name}' was not in the keychain", "·".dimmed());
    }
    Ok(())
}

/// Show every `${REF}` the manifest needs and where (if anywhere) it resolves on
/// this machine. Values are never printed.
fn list(manifest_dir: Option<&Path>) -> Result<()> {
    let ctx = super::load(manifest_dir)?;
    let refs = ctx.loaded.manifest.referenced_secrets();
    if refs.is_empty() {
        println!("Manifest references no secrets.");
        return Ok(());
    }

    // Build per-source resolvers so we can report *where* each ref resolves.
    let env = EnvResolver;
    let varlock = VarlockResolver::detect(&ctx.dir);
    let keychain = KeychainResolver;
    let dotenv = DotEnvResolver::from_dir(&ctx.dir);

    println!("Secrets referenced by the manifest:\n");
    let mut missing = 0;
    for name in &refs {
        let source = if env.resolve(name).is_some() {
            Some("env")
        } else if varlock.as_ref().and_then(|v| v.resolve(name)).is_some() {
            Some("varlock")
        } else if keychain.resolve(name).is_some() {
            Some("keychain")
        } else if dotenv.as_ref().and_then(|d| d.resolve(name)).is_some() {
            Some(".env")
        } else {
            None
        };
        match source {
            Some(src) => println!("  {} {name:<20} resolved ({src})", "✓".green()),
            None => {
                println!(
                    "  {} {name:<20} not found ↳ agentstack secret set {name}",
                    "✗".red()
                );
                missing += 1;
            }
        }
    }
    println!();
    if missing > 0 {
        println!("{missing} secret(s) unresolved on this machine.");
    } else {
        println!("{} all secrets resolve.", "✓".green());
    }
    Ok(())
}
