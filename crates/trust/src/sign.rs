//! Detached ed25519 signatures over a bundle's lockfile — the crypto
//! primitive Phase 4 distribution builds on (ARCHITECTURE Layer 2: "an
//! optional detached signature (ed25519 over the lockfile) enables registry
//! distribution later"). The lockfile digest is the bundle's identity, so a
//! signature over the lockfile bytes lets a puller verify a bundle came from a
//! publisher key it trusts, before the content-pinning + review flow runs
//! unchanged.
//!
//! This module is the primitive only: sign, verify, and hex-encoded key and
//! signature types. Key distribution and a bundle registry are deliberately
//! out of scope until the real-user distribution gate in `STRATEGY.md`.
//!
//! No RNG dependency: a signing key is derived from a 32-byte seed the caller
//! supplies (e.g. `core::util::random_bytes`), so this crate stays on its
//! rule-6 dependency list.

use agentstack_core::lock::LOCK_FILE;
use ed25519_dalek::{Signature as DalekSig, Signer, SigningKey, Verifier, VerifyingKey};

/// A hex-encoded ed25519 public key (32 bytes → 64 hex chars).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicKey([u8; 32]);

/// A hex-encoded ed25519 signature (64 bytes → 128 hex chars).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature([u8; 64]);

impl PublicKey {
    pub fn to_hex(&self) -> String {
        hex(&self.0)
    }
    pub fn from_hex(s: &str) -> Option<PublicKey> {
        Some(PublicKey(unhex::<32>(s)?))
    }
}

impl Signature {
    pub fn to_hex(&self) -> String {
        hex(&self.0)
    }
    pub fn from_hex(s: &str) -> Option<Signature> {
        Some(Signature(unhex::<64>(s)?))
    }
}

/// Sign `message` with a signing key derived from `seed`, returning the public
/// key (to publish) and the detached signature.
pub fn sign(seed: &[u8; 32], message: &[u8]) -> (PublicKey, Signature) {
    let key = SigningKey::from_bytes(seed);
    let sig = key.sign(message);
    (
        PublicKey(key.verifying_key().to_bytes()),
        Signature(sig.to_bytes()),
    )
}

/// Verify a detached `signature` over `message` against `public_key`. `false`
/// on any mismatch or a malformed key — fail closed.
pub fn verify(public_key: &PublicKey, message: &[u8], signature: &Signature) -> bool {
    let Ok(vk) = VerifyingKey::from_bytes(&public_key.0) else {
        return false;
    };
    vk.verify(message, &DalekSig::from_bytes(&signature.0))
        .is_ok()
}

/// Sign the lockfile bytes at `dir/agentstack.lock`.
pub fn sign_lock(
    dir: &std::path::Path,
    seed: &[u8; 32],
) -> std::io::Result<(PublicKey, Signature)> {
    let bytes = std::fs::read(dir.join(LOCK_FILE))?;
    Ok(sign(seed, &bytes))
}

/// Verify a detached signature over the lockfile bytes at
/// `dir/agentstack.lock`. `Ok(false)` when the signature doesn't match;
/// `Err` only when the lockfile can't be read.
pub fn verify_lock(
    dir: &std::path::Path,
    public_key: &PublicKey,
    signature: &Signature,
) -> std::io::Result<bool> {
    let bytes = std::fs::read(dir.join(LOCK_FILE))?;
    Ok(verify(public_key, &bytes, signature))
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn unhex<const N: usize>(s: &str) -> Option<[u8; N]> {
    let s = s.trim();
    if s.len() != N * 2 {
        return None;
    }
    let mut out = [0u8; N];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A fixed seed keeps the test deterministic; production seeds come from
    // core::util::random_bytes.
    const SEED: [u8; 32] = [7u8; 32];

    #[test]
    fn sign_then_verify_roundtrips() {
        let msg = b"agentstack.lock digest bytes";
        let (pk, sig) = sign(&SEED, msg);
        assert!(verify(&pk, msg, &sig), "a valid signature verifies");
    }

    #[test]
    fn a_tampered_message_fails_verification() {
        let (pk, sig) = sign(&SEED, b"original lockfile");
        assert!(!verify(&pk, b"tampered lockfile", &sig));
    }

    #[test]
    fn a_wrong_key_fails_verification() {
        let msg = b"lockfile";
        let (_pk, sig) = sign(&SEED, msg);
        let (other_pk, _) = sign(&[9u8; 32], msg);
        assert!(!verify(&other_pk, msg, &sig), "another key must not verify");
    }

    #[test]
    fn hex_roundtrips_and_rejects_junk() {
        let (pk, sig) = sign(&SEED, b"x");
        assert_eq!(PublicKey::from_hex(&pk.to_hex()), Some(pk));
        assert_eq!(Signature::from_hex(&sig.to_hex()), Some(sig));
        assert_eq!(PublicKey::from_hex("nothex"), None);
        assert_eq!(PublicKey::from_hex("ab"), None); // wrong length
        assert_eq!(Signature::from_hex("zz"), None);
    }

    #[test]
    fn lockfile_sign_and_verify_end_to_end() {
        let dir = assert_fs::TempDir::new().unwrap();
        std::fs::write(dir.path().join(LOCK_FILE), "version = 2\n").unwrap();
        let (pk, sig) = sign_lock(dir.path(), &SEED).unwrap();
        assert!(verify_lock(dir.path(), &pk, &sig).unwrap());

        // Editing the lockfile invalidates the signature.
        std::fs::write(dir.path().join(LOCK_FILE), "version = 2\n# edit\n").unwrap();
        assert!(!verify_lock(dir.path(), &pk, &sig).unwrap());
    }
}
