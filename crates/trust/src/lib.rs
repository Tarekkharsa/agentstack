//! direnv-style trust store for zero-files auto mode.
//!
//! A globally registered `agentstack mcp --auto-project` bridge discovers
//! whatever manifest the current project carries. Auto-loading that manifest's
//! servers would let any cloned repo spawn stdio commands and receive secrets —
//! so discovery is gated: a project's runtime surface stays control-plane-only
//! until a human runs `agentstack trust`, and trust is pinned to the content
//! digest of the manifest layers plus `agentstack.lock` (which pins the
//! definition digests of library-referenced servers). Change any of them (a
//! `git pull`, say) and the project must be re-trusted, exactly like `direnv
//! allow`.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use agentstack_core::lock::LOCK_FILE;
use agentstack_core::manifest::load::{LOCAL_FILE, MANIFEST_FILE};
use agentstack_core::util::paths;

/// Where trust decisions live: `~/.agentstack/trust.toml`.
pub fn store_path() -> PathBuf {
    paths::agentstack_home().join("trust.toml")
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct TrustStore {
    /// Canonical project base dir → the trust decision for it.
    #[serde(default)]
    pub trusted: IndexMap<String, TrustEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustEntry {
    /// `sha256:<hex>` over the manifest (+ local overlay + lockfile) at trust
    /// time.
    pub digest: String,
    pub trusted_at: u64,
}

/// Where a project stands with the zero-files bridge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustState {
    /// Trusted and the manifest is byte-identical to what was trusted.
    Trusted,
    /// Trusted once, but the manifest changed since — re-review + re-trust.
    Changed,
    /// Never trusted on this machine.
    Untrusted,
}

impl TrustStore {
    pub fn load() -> TrustStore {
        let Ok(text) = std::fs::read_to_string(store_path()) else {
            return TrustStore::default();
        };
        toml::from_str(&text).unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let text = toml::to_string_pretty(self).context("serializing trust store")?;
        agentstack_core::util::atomic::write(&store_path(), &text)
    }
}

/// The trust key for a project: its canonicalized base dir (the dir holding
/// `.agentstack/` or a legacy root manifest — NOT the manifest dir itself).
pub fn key_for(base: &Path) -> String {
    std::fs::canonicalize(base)
        .unwrap_or_else(|_| base.to_path_buf())
        .display()
        .to_string()
}

/// Content digest of the consent surface at `base`: the manifest layers
/// (`agentstack.toml` plus the `agentstack.local.toml` overlay, both of which
/// declare runnable servers) and `agentstack.lock`, which pins the definition
/// digests of library-referenced servers the gateway will serve. Re-pinning
/// the lock changes what a name ref runs, so it re-gates the project exactly
/// like a manifest edit. `None` when there is no manifest.
pub fn digest_for(base: &Path) -> Option<String> {
    let dir = agentstack_core::manifest::resolve_manifest_dir(base);
    let manifest = std::fs::read(dir.join(MANIFEST_FILE)).ok()?;
    let local = std::fs::read(dir.join(LOCAL_FILE)).unwrap_or_default();
    let lock = std::fs::read(dir.join(LOCK_FILE)).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(&manifest);
    hasher.update([0u8]);
    hasher.update(&local);
    hasher.update([0u8]);
    hasher.update(&lock);
    Some(format!("sha256:{:x}", hasher.finalize()))
}

/// Where `base` stands right now (digest recomputed against the store).
pub fn check(base: &Path) -> TrustState {
    let store = TrustStore::load();
    let Some(entry) = store.trusted.get(&key_for(base)) else {
        return TrustState::Untrusted;
    };
    match digest_for(base) {
        Some(d) if d == entry.digest => TrustState::Trusted,
        // Manifest gone or rewritten since trust — either way, re-review.
        _ => TrustState::Changed,
    }
}

/// Record trust for `base` at its current manifest digest. Errors when there is
/// no manifest to pin.
pub fn trust(base: &Path) -> Result<String> {
    let digest = digest_for(base)
        .with_context(|| format!("no agentstack manifest under {}", base.display()))?;
    let mut store = TrustStore::load();
    store.trusted.insert(
        key_for(base),
        TrustEntry {
            digest: digest.clone(),
            trusted_at: now_secs(),
        },
    );
    store.save()?;
    Ok(digest)
}

/// Remove trust for `base`. Returns whether an entry existed.
pub fn revoke(base: &Path) -> Result<bool> {
    let mut store = TrustStore::load();
    let existed = store.trusted.shift_remove(&key_for(base)).is_some();
    if existed {
        store.save()?;
    }
    Ok(existed)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::prelude::*;

    fn with_home<T>(f: impl FnOnce(&assert_fs::TempDir) -> T) -> T {
        let _guard = agentstack_core::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", home.path());
        let out = f(&home);
        std::env::remove_var("AGENTSTACK_HOME");
        out
    }

    fn project_with_manifest() -> assert_fs::TempDir {
        let proj = assert_fs::TempDir::new().unwrap();
        proj.child(".agentstack/agentstack.toml")
            .write_str("version = 1\n[servers.x]\ntype = \"http\"\nurl = \"https://x/mcp\"\n")
            .unwrap();
        proj
    }

    #[test]
    fn trust_then_check_then_change_then_revoke() {
        with_home(|_| {
            let proj = project_with_manifest();
            assert_eq!(check(proj.path()), TrustState::Untrusted);

            trust(proj.path()).unwrap();
            assert_eq!(check(proj.path()), TrustState::Trusted);

            // Any manifest edit invalidates trust (direnv semantics).
            proj.child(".agentstack/agentstack.toml")
                .write_str("version = 1\n[servers.evil]\ntype = \"stdio\"\ncommand = \"sh\"\n")
                .unwrap();
            assert_eq!(check(proj.path()), TrustState::Changed);

            // Re-trusting the new content restores it; revoking clears it.
            trust(proj.path()).unwrap();
            assert_eq!(check(proj.path()), TrustState::Trusted);
            assert!(revoke(proj.path()).unwrap());
            assert_eq!(check(proj.path()), TrustState::Untrusted);
        });
    }

    #[test]
    fn local_overlay_participates_in_the_digest() {
        with_home(|_| {
            let proj = project_with_manifest();
            trust(proj.path()).unwrap();
            // The gitignored overlay also declares servers — adding one must
            // invalidate trust too.
            proj.child(".agentstack/agentstack.local.toml")
                .write_str("[servers.local]\ntype = \"stdio\"\ncommand = \"sh\"\n")
                .unwrap();
            assert_eq!(check(proj.path()), TrustState::Changed);
        });
    }

    #[test]
    fn lockfile_participates_in_the_digest() {
        with_home(|_| {
            let proj = project_with_manifest();
            trust(proj.path()).unwrap();
            // The lock pins the library server definitions the gateway will
            // run — re-pinning changes the runtime surface, so it re-gates
            // exactly like a manifest edit.
            proj.child(".agentstack/agentstack.lock")
                .write_str(
                    "version = 1\n[[server]]\nname = \"kibana\"\nsource = \"library\"\nchecksum = \"sha256:aaa\"\n",
                )
                .unwrap();
            assert_eq!(check(proj.path()), TrustState::Changed);
        });
    }

    #[test]
    fn no_manifest_means_no_digest_and_trust_errors() {
        with_home(|_| {
            let empty = assert_fs::TempDir::new().unwrap();
            assert!(digest_for(empty.path()).is_none());
            assert!(trust(empty.path()).is_err());
            assert_eq!(check(empty.path()), TrustState::Untrusted);
        });
    }
}
