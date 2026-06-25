//! `agentstack export` / `import` — a portable, age-encrypted bundle of the
//! manifest (+ lockfile, + optionally secrets) for moving a setup to a new
//! machine (PLAN §9, §9e). Passphrase-protected; nothing readable at rest.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::Path;

use age::secrecy::Secret;
use anyhow::{bail, Context, Result};
use owo_colors::OwoColorize;
use serde::{Deserialize, Serialize};

use crate::cli::{ExportArgs, ImportArgs};
use crate::lock::{Lock, LOCK_FILE};
use crate::manifest::load::MANIFEST_FILE;
use crate::secret::{keychain, Chain, Resolver};

#[derive(Serialize, Deserialize)]
struct Bundle {
    version: u32,
    manifest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lock: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    secrets: BTreeMap<String, String>,
}

pub fn run_export(args: &ExportArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let ctx = super::load(manifest_dir)?;
    let manifest = std::fs::read_to_string(&ctx.loaded.manifest_path)
        .with_context(|| format!("reading {}", ctx.loaded.manifest_path.display()))?;

    let lock_path = Lock::path(&ctx.dir);
    let lock = std::fs::read_to_string(&lock_path).ok();

    let mut secrets = BTreeMap::new();
    if args.secrets {
        let chain = Chain::default_for_dir(&ctx.dir);
        for name in ctx.loaded.manifest.referenced_secrets() {
            if let Some(v) = chain.resolve(&name) {
                secrets.insert(name, v);
            }
        }
    }

    let bundle = Bundle {
        version: 1,
        manifest,
        lock,
        secrets,
    };
    let plaintext = toml::to_string(&bundle)?;

    let passphrase = read_passphrase(args.passphrase.as_deref(), true)?;
    let encrypted = encrypt(plaintext.as_bytes(), &passphrase)?;
    std::fs::write(&args.output, &encrypted)
        .with_context(|| format!("writing {}", args.output.display()))?;

    let secret_note = if args.secrets {
        format!(" + {} secret(s)", bundle.secrets.len())
    } else {
        String::new()
    };
    println!(
        "{} wrote {} (manifest{}{})",
        "✓".green(),
        args.output.display(),
        if bundle.lock.is_some() { " + lock" } else { "" },
        secret_note
    );
    Ok(())
}

pub fn run_import(args: &ImportArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let dir = match manifest_dir {
        Some(d) => d.to_path_buf(),
        None => std::env::current_dir()?,
    };
    let manifest_path = dir.join(MANIFEST_FILE);
    if manifest_path.exists() && !args.force {
        bail!(
            "{} already exists — use --force to overwrite",
            manifest_path.display()
        );
    }

    let encrypted =
        std::fs::read(&args.file).with_context(|| format!("reading {}", args.file.display()))?;
    let passphrase = read_passphrase(args.passphrase.as_deref(), false)?;
    let plaintext = decrypt(&encrypted, &passphrase)?;
    let bundle: Bundle =
        toml::from_str(&String::from_utf8_lossy(&plaintext)).context("parsing bundle")?;

    std::fs::write(&manifest_path, &bundle.manifest)
        .with_context(|| format!("writing {}", manifest_path.display()))?;
    let mut wrote = vec![MANIFEST_FILE.to_string()];
    if let Some(lock) = &bundle.lock {
        std::fs::write(dir.join(LOCK_FILE), lock)?;
        wrote.push(LOCK_FILE.to_string());
    }
    if !args.no_keychain {
        for (name, value) in &bundle.secrets {
            keychain::set(name, value)?;
        }
    }

    println!("{} imported {}", "✓".green(), wrote.join(" + "));
    if !bundle.secrets.is_empty() {
        let action = if args.no_keychain {
            "skipped (--no-keychain)"
        } else {
            "→ keychain"
        };
        println!("  {} secret(s) {action}", bundle.secrets.len());
    }
    println!("\nNext: `agentstack install` then `agentstack doctor`.");
    Ok(())
}

fn read_passphrase(provided: Option<&str>, confirm: bool) -> Result<String> {
    if let Some(p) = provided {
        return Ok(p.to_string());
    }
    let p = rpassword::prompt_password("Passphrase: ").context("reading passphrase")?;
    if p.is_empty() {
        bail!("passphrase must not be empty");
    }
    if confirm {
        let again = rpassword::prompt_password("Confirm passphrase: ")?;
        if again != p {
            bail!("passphrases do not match");
        }
    }
    Ok(p)
}

fn encrypt(plaintext: &[u8], passphrase: &str) -> Result<Vec<u8>> {
    let encryptor = age::Encryptor::with_user_passphrase(Secret::new(passphrase.to_string()));
    let mut out = Vec::new();
    let mut writer = encryptor
        .wrap_output(&mut out)
        .context("initializing encryption")?;
    writer.write_all(plaintext)?;
    writer.finish()?;
    Ok(out)
}

fn decrypt(encrypted: &[u8], passphrase: &str) -> Result<Vec<u8>> {
    let decryptor = match age::Decryptor::new(encrypted).context("reading bundle")? {
        age::Decryptor::Passphrase(d) => d,
        _ => bail!("bundle is not passphrase-encrypted"),
    };
    let mut reader = decryptor
        .decrypt(&Secret::new(passphrase.to_string()), None)
        .map_err(|_| anyhow::anyhow!("decryption failed (wrong passphrase?)"))?;
    let mut out = Vec::new();
    reader.read_to_end(&mut out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let enc = encrypt(b"hello bundle", "pw123").unwrap();
        assert_ne!(enc, b"hello bundle");
        let dec = decrypt(&enc, "pw123").unwrap();
        assert_eq!(dec, b"hello bundle");
        assert!(decrypt(&enc, "wrong").is_err());
    }
}
