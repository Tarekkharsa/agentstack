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

pub mod sign;

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use agentstack_core::lock::LOCK_FILE;
use agentstack_core::manifest::load::{LOCAL_FILE, MANIFEST_FILE};
use agentstack_core::util::paths;

const TRUST_DIGEST_DOMAIN: &[u8] = b"agentstack-trust-digest-v2\0";

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
    hasher.update(TRUST_DIGEST_DOMAIN);
    for segment in [&manifest, &local, &lock] {
        // Length prefixes make each file boundary unambiguous, like framing
        // three byte buffers before concatenating them in TypeScript.
        hasher.update((segment.len() as u64).to_le_bytes());
        hasher.update(segment);
    }
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
    fn digest_is_stable_for_identical_inputs() {
        let proj = project_with_manifest();

        assert_eq!(digest_for(proj.path()), digest_for(proj.path()));
    }

    #[test]
    fn digest_frames_manifest_and_local_as_distinct_segments() {
        let first = assert_fs::TempDir::new().unwrap();
        first
            .child(".agentstack/agentstack.toml")
            .write_binary(b"")
            .unwrap();
        first
            .child(".agentstack/agentstack.local.toml")
            .write_binary(b"\0")
            .unwrap();

        let second = assert_fs::TempDir::new().unwrap();
        second
            .child(".agentstack/agentstack.toml")
            .write_binary(b"\0")
            .unwrap();
        second
            .child(".agentstack/agentstack.local.toml")
            .write_binary(b"")
            .unwrap();

        assert_ne!(digest_for(first.path()), digest_for(second.path()));
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

    // ── Property test: the re-gate invariant (CLAUDE.md rule 4) ────────────
    // NEVER delete or weaken this test. It is the machine-checked form of
    // "any pinned byte changes → bundle re-gates": for ALL contents of the
    // pinned files, ALL choices of file, ALL byte positions, and ALL nonzero
    // bit patterns, flipping that one byte demotes Trusted to Changed.
    //
    // How proptest works, for the record: a `Strategy` is a value generator
    // (like fast-check arbitraries in the TS world). `proptest!` runs the
    // test body against many generated inputs, and when a case fails it
    // *shrinks* — re-runs with progressively simpler inputs (shorter files,
    // index 0, delta 1) and reports the minimal failing case instead of a
    // random haystack. `prop_flat_map` builds dependent generators: the
    // flip index must be generated *after* (and within) the chosen file's
    // length, so the second stage's ranges depend on the first stage's
    // output.

    use proptest::prelude::*;

    /// (manifest, local, lock, which file to corrupt, byte index, xor delta).
    /// All three files non-empty so every (which, idx) pair is valid; delta
    /// is drawn from 1..=255 so `byte ^ delta` is guaranteed to differ.
    fn pinned_surface() -> impl Strategy<Value = (Vec<u8>, Vec<u8>, Vec<u8>, usize, usize, u8)> {
        (
            prop::collection::vec(any::<u8>(), 1..256),
            prop::collection::vec(any::<u8>(), 1..256),
            prop::collection::vec(any::<u8>(), 1..256),
            0usize..3,
            1u8..=255u8,
        )
            .prop_flat_map(|(manifest, local, lock, which, delta)| {
                let len = [manifest.len(), local.len(), lock.len()][which];
                (
                    Just(manifest),
                    Just(local),
                    Just(lock),
                    Just(which),
                    0..len,
                    Just(delta),
                )
            })
    }

    proptest! {
        // Each case touches the real filesystem (tempdir + env var), so run
        // fewer, bigger cases than proptest's default 256.
        #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

        #[test]
        fn any_single_byte_flip_in_any_pinned_file_regates(
            (manifest, local, lock, which, idx, delta) in pinned_surface()
        ) {
            with_home(|_| {
                let proj = assert_fs::TempDir::new().unwrap();
                // digest_for hashes raw bytes — the files need not parse, so
                // the invariant holds over arbitrary (hostile) content.
                proj.child(".agentstack/agentstack.toml").write_binary(&manifest).unwrap();
                proj.child(".agentstack/agentstack.local.toml").write_binary(&local).unwrap();
                proj.child(".agentstack/agentstack.lock").write_binary(&lock).unwrap();

                trust(proj.path()).unwrap();
                prop_assert_eq!(check(proj.path()), TrustState::Trusted);

                let (name, bytes) = match which {
                    0 => ("agentstack.toml", manifest),
                    1 => ("agentstack.local.toml", local),
                    _ => ("agentstack.lock", lock),
                };
                let mut corrupted = bytes;
                corrupted[idx] ^= delta;
                proj.child(format!(".agentstack/{name}")).write_binary(&corrupted).unwrap();

                prop_assert_eq!(check(proj.path()), TrustState::Changed);
                Ok(())
            })?;
        }
    }
}
