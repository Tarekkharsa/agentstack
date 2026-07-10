//! `agentstack sign` / `agentstack verify` — detached ed25519 signatures over
//! the project lockfile (ROADMAP Phase 4, the distribution primitive).
//!
//! `sign` derives a key from a fresh random seed, signs `agentstack.lock`,
//! writes the detached signature to `agentstack.lock.sig`, and prints the
//! public key to publish. `verify` checks the lockfile against a published
//! public key and that signature. Key distribution and a bundle registry are
//! out of scope until there are real users — this is the primitive they will
//! build on, and it lets a puller confirm a bundle's lockfile came from a key
//! they trust before the unchanged content-pinning + review flow runs.

use std::path::Path;

use anyhow::{Context, Result};
use owo_colors::OwoColorize;

use crate::cli::{SignArgs, VerifyArgs};

/// The detached-signature sidecar next to `agentstack.lock`.
const SIG_FILE: &str = "agentstack.lock.sig";

pub fn sign(args: &SignArgs, dir: Option<&Path>) -> Result<()> {
    let base = dir.map(Path::to_path_buf).unwrap_or_else(|| ".".into());
    let mdir = crate::manifest::resolve_manifest_dir(&base);

    // A fresh signing seed per invocation. The seed (the private key) is NOT
    // persisted here — this is the primitive; durable key management is the
    // deferred registry work. Re-run to rotate; publish the printed key.
    let mut seed = [0u8; 32];
    let src = crate::util::random_bytes();
    anyhow::ensure!(src.len() >= 32, "insufficient system randomness for a key");
    seed.copy_from_slice(&src[..32]);
    let (pubkey, signature) =
        agentstack_trust::sign::sign_lock(&mdir, &seed).context("signing agentstack.lock")?;

    let sig_path = mdir.join(SIG_FILE);
    std::fs::write(&sig_path, signature.to_hex())
        .with_context(|| format!("writing {}", sig_path.display()))?;

    println!(
        "{} signed {}",
        "✓".green(),
        mdir.join("agentstack.lock").display()
    );
    println!("  signature → {}", sig_path.display());
    println!("  public key: {}", pubkey.to_hex().bold());
    if args.print_key_only {
        // Nothing else; the key line above is the machine-readable output.
    } else {
        println!(
            "\n  Publish the public key; a puller verifies with:\n    {} --pubkey {}",
            "agentstack verify".bold(),
            pubkey.to_hex()
        );
    }
    Ok(())
}

pub fn verify(args: &VerifyArgs, dir: Option<&Path>) -> Result<()> {
    let base = dir.map(Path::to_path_buf).unwrap_or_else(|| ".".into());
    let mdir = crate::manifest::resolve_manifest_dir(&base);

    let pubkey = agentstack_trust::sign::PublicKey::from_hex(&args.pubkey)
        .context("--pubkey is not a 64-hex-char ed25519 public key")?;

    // The signature comes from --signature or the sidecar file.
    let sig_hex = match &args.signature {
        Some(s) => s.clone(),
        None => {
            let sig_path = mdir.join(SIG_FILE);
            std::fs::read_to_string(&sig_path).with_context(|| {
                format!("no --signature given and no {} to read", sig_path.display())
            })?
        }
    };
    let signature = agentstack_trust::sign::Signature::from_hex(&sig_hex)
        .context("signature is not 128 hex chars")?;

    let ok = agentstack_trust::sign::verify_lock(&mdir, &pubkey, &signature)
        .context("reading agentstack.lock")?;

    if ok {
        println!(
            "{} agentstack.lock signature is valid for this public key.",
            "✓".green()
        );
        Ok(())
    } else {
        // Exit nonzero so scripts/CI can gate on it.
        anyhow::bail!(
            "signature does NOT match agentstack.lock for this public key — the lockfile was changed or signed by a different key"
        )
    }
}
