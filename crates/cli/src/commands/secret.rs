//! `agentstack secret set|get|rm|list` — manage secrets in the OS keychain and
//! audit which `${REF}`s the manifest needs on this machine.

use std::path::Path;

use agentstack_core::paint::OwoColorize;
use anyhow::{Context, Result};

use crate::cli::{SecretArgs, SecretCommand};
use crate::secret::keychain;

pub fn run(args: &SecretArgs, manifest_dir: Option<&Path>) -> Result<()> {
    match &args.command {
        SecretCommand::Set {
            name,
            value,
            env_file,
        } => set(name, value.as_deref(), *env_file, manifest_dir),
        SecretCommand::Get { name } => get(name),
        SecretCommand::Rm { name } => rm(name),
        SecretCommand::List => list(manifest_dir),
    }
}

fn set(name: &str, value: Option<&str>, env_file: bool, manifest_dir: Option<&Path>) -> Result<()> {
    let value = match value {
        Some(v) => v.to_string(),
        // Refuse before rpassword touches /dev/tty: in CI or a pipe the raw
        // failure is "Device not configured (os error 6)", which names
        // neither the cause nor the flags that solve it.
        None if !crate::util::confirm::is_interactive() => {
            anyhow::bail!(
                "secret set needs a terminal to prompt for the value\n\
                 \n  \
                 pass it inline:  agentstack secret set {name} --value <VALUE>{}\n  \
                 (inline values can land in shell history — prefer the prompt when you can)",
                if env_file { " --env-file" } else { "" }
            );
        }
        None => rpassword::prompt_password(format!("Value for {name}: "))
            .context("reading secret from prompt")?,
    };
    if value.is_empty() {
        anyhow::bail!(
            "refusing to store an empty value for '{name}' — pass a non-empty --value, or omit --value to be prompted"
        );
    }
    if env_file {
        // Write to the project `.env` next to the manifest (the same file init's
        // `--secrets env` path targets), and keep it out of git.
        let dir = crate::manifest::resolve_manifest_dir(&super::project_base(manifest_dir)?);
        crate::secret::env_file::write(&dir, &[(name.to_string(), value)])?;
        let project_root = crate::manifest::project_root_of(&dir);
        let is_git = project_root.join(".git").exists();
        if is_git {
            crate::secret::env_file::ensure_gitignored(&project_root, &dir, true)?;
        }
        println!(
            "{} stored '{name}' in {}/.env{}",
            "✓".green(),
            dir.display(),
            if is_git { " (gitignored)" } else { "" }
        );
        return Ok(());
    }
    keychain::set(name, &value).map_err(|e| anyhow::anyhow!(keychain_unavailable(name, &e)))?;
    println!("{} stored '{name}' in the OS keychain", "✓".green());
    Ok(())
}

/// What to say when the keychain refuses the write.
///
/// Two problems, one sentence each. The cause is taken from `root_cause()`
/// because keyring folds its platform text into its own Display, so rendering
/// the context chain printed the same sentence twice — which read as a stutter
/// rather than an error. And a machine with no keychain at all (headless CI, a
/// container, a Linux box with no secret service) is not a machine the user can
/// fix from here, so the message names the store that does work instead of
/// stopping at the failure.
///
/// Pure so the wording is testable without breaking a real keychain.
fn keychain_unavailable(name: &str, err: &anyhow::Error) -> String {
    format!(
        "could not store '{name}' in the OS keychain: {}\n\
         \n  \
         no keychain on this machine? keep it in the project's env file instead:\n    \
         agentstack secret set {name} --value <VALUE> --env-file",
        err.root_cause()
    )
}

fn get(name: &str) -> Result<()> {
    // A broken keychain (no default keychain, locked, headless CI) is a
    // machine problem, not a typo — name the cause once and both supported
    // stores, so the user isn't stranded on the keychain path.
    let value = keychain::get(name).map_err(|e| {
        anyhow::anyhow!(
            "'{name}' is not readable from the OS keychain ({})\n\
             \n  \
             store it in the keychain:   agentstack secret set {name}\n  \
             or in this project's .env:  agentstack secret set {name} --env-file",
            e.root_cause()
        )
    })?;
    match value {
        Some(v) => {
            println!("{v}");
            Ok(())
        }
        None => {
            anyhow::bail!(
                "no secret '{name}' in keychain — run `agentstack secret set {name}` to store one (or `--env-file` for a project .env)"
            );
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

    // Report *where* each ref resolves (shared with `explain` + t3code).
    let sources = crate::secret::SecretSources::detect(&ctx.dir);

    println!("Secrets referenced by the manifest:\n");
    let mut missing = 0;
    for name in &refs {
        match sources.source_of(name) {
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
        println!(
            "{} unresolved on this machine.",
            super::count(missing, "secret")
        );
    } else {
        println!("{} all secrets resolve.", "✓".green());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_keychain_refusal_says_it_once_and_names_the_store_that_works() {
        let err = anyhow::anyhow!("No such keychain exists.")
            .context("storing secret 'TOKEN' in keychain");
        let msg = keychain_unavailable("TOKEN", &err);
        assert_eq!(msg.matches("No such keychain exists.").count(), 1, "{msg}");
        assert!(msg.contains("--env-file"), "{msg}");
        assert!(
            msg.contains("agentstack secret set TOKEN --value <VALUE> --env-file"),
            "{msg}"
        );
    }
}
