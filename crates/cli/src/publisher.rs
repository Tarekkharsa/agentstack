//! Publisher identity: who signed a bundle, and whose signatures you recognize.
//!
//! Phase 4, "sharing is signing". Two files under `~/.agentstack/`, both
//! machine-local and neither ever shared:
//!
//! - `publisher.key` — this machine's 32-byte signing seed, mode 0600. The one
//!   secret here. Present only if you have ever shared something.
//! - `publishers.json` — public keys you have chosen to recognize, each with
//!   the label you gave it. Public data; losing it costs you recognition, not
//!   safety.
//!
//! # What a signature is for, and what it is not for
//!
//! A signature answers exactly one question: *did the bytes in front of me come
//! from the holder of this key, unchanged?* It does not say the content is
//! safe, and it is emphatically not a second way to say yes. A bundle from a
//! publisher you recognize still opens the review card; what changes is that
//! the card can lead with "signed by Dana (recognized)" instead of making you
//! establish provenance yourself. That is the same job [`crate::recognition`]
//! does for content you have approved before, and it is deliberately built to
//! the same shape: **it shortens the reading, never the decision.**
//!
//! An unrecognized or invalid signature is not an error either. It is a fact
//! printed on the card, and the full review stands.
//!
//! # Fingerprints, because nobody compares 64 hex characters
//!
//! A key is shown as its first 16 hex characters in groups of four
//! (`3f2a-91c0-de44-1b07`). Short enough to read down a phone line, long
//! enough that finding a second key with the same prefix is not something an
//! attacker does casually. The full key is always available and is what is
//! actually compared — the fingerprint is for humans, never for verification.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use agentstack_trust::sign::{PublicKey, Signature};

/// This machine's signing seed. Absent until the first `share`.
fn key_path() -> std::path::PathBuf {
    agentstack_core::util::paths::agentstack_home().join("publisher.key")
}

/// Public keys this machine recognizes.
fn known_path() -> std::path::PathBuf {
    agentstack_core::util::paths::agentstack_home().join("publishers.json")
}

/// A publisher whose signature this machine recognizes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Known {
    /// Full public key, hex. This is what verification compares.
    pub key: String,
    /// What the user called them. Display only — never matched on, because a
    /// label is chosen by the local user and an attacker who could influence
    /// it could impersonate by name.
    pub label: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct KnownFile {
    #[serde(default)]
    publishers: Vec<Known>,
}

/// Human-readable form of a public key: first 16 hex chars, in fours.
///
/// Never used to decide anything — [`recognize`] compares full keys. Shortening
/// a key for comparison is how fingerprint collisions turn into impersonation;
/// shortening it for display is just courtesy.
pub fn fingerprint(key: &PublicKey) -> String {
    let hex = key.to_hex();
    hex.as_bytes()
        .chunks(4)
        .take(4)
        .map(|c| String::from_utf8_lossy(c).to_string())
        .collect::<Vec<_>>()
        .join("-")
}

/// This machine's signing seed, creating one on first use.
///
/// Written 0600 through the same `write_private` the `.env` path uses — a
/// signing seed at the ambient umask is readable by every local account, which
/// would let any of them publish as you.
pub fn signing_seed() -> Result<[u8; 32]> {
    let path = key_path();
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let bytes = hex_to_32(existing.trim())
            .with_context(|| format!("{} is not a valid signing key", path.display()))?;
        return Ok(bytes);
    }
    let fresh = agentstack_core::util::random_bytes();
    let mut seed = [0u8; 32];
    // `random_bytes` is the same source the rest of the product uses for
    // unpredictable values; take exactly 32 and fail rather than pad, because a
    // short seed would silently weaken every signature made with it.
    anyhow::ensure!(
        fresh.len() >= 32,
        "could not gather 32 bytes of randomness for a signing key"
    );
    seed.copy_from_slice(&fresh[..32]);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    agentstack_core::util::atomic::write_private(&path, &hex_of(&seed))
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(seed)
}

/// This machine's public key, if it has ever signed anything. Never creates
/// one — a read-only surface asking "who am I?" should not mint an identity as
/// a side effect.
pub fn public_key() -> Option<PublicKey> {
    let raw = std::fs::read_to_string(key_path()).ok()?;
    let seed = hex_to_32(raw.trim()).ok()?;
    Some(agentstack_trust::sign::sign(&seed, b"").0)
}

/// The publishers this machine recognizes.
///
/// A missing or unreadable file yields an empty list, not an error: the
/// failure posture is "recognize nobody", which costs the user a shortened
/// card and nothing else. Failing closed in the other direction — treating an
/// unreadable index as a reason to block — would turn a corrupt cache into an
/// outage.
pub fn known() -> Vec<Known> {
    let Ok(raw) = std::fs::read_to_string(known_path()) else {
        return Vec::new();
    };
    serde_json::from_str::<KnownFile>(&raw)
        .map(|f| f.publishers)
        .unwrap_or_default()
}

/// Record a publisher as recognized. Replaces the label if the key is known.
pub fn remember(key: &PublicKey, label: &str) -> Result<()> {
    let mut all = known();
    let hex = key.to_hex();
    match all.iter_mut().find(|k| k.key == hex) {
        Some(existing) => existing.label = label.to_string(),
        None => all.push(Known {
            key: hex,
            label: label.to_string(),
        }),
    }
    let path = known_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let body = serde_json::to_string_pretty(&KnownFile { publishers: all })?;
    agentstack_core::util::atomic::write(&path, &body)
        .with_context(|| format!("writing {}", path.display()))
}

/// Is this key one the user has chosen to recognize? Full-key comparison.
pub fn recognize(key: &PublicKey) -> Option<Known> {
    let hex = key.to_hex();
    known().into_iter().find(|k| k.key == hex)
}

/// What a receiver was able to establish about a bundle's signature.
///
/// Deliberately three states rather than a bool. "Unsigned" and "signed by
/// someone you have never recognized" are different situations with the same
/// consequence (full review), and collapsing them would lose the difference on
/// the card — the second one has a next step, the first does not.
#[derive(Debug, Clone, PartialEq)]
pub enum Provenance {
    /// No signature at all.
    Unsigned,
    /// A signature that does not verify against the bundle's own bytes. The
    /// loudest of the three: it means the content changed after signing, or
    /// was never signed by the key it claims.
    Invalid,
    /// Verified against the bundle's bytes. `known` is `Some` when the user has
    /// recognized this publisher before.
    Verified {
        key: PublicKey,
        known: Option<Known>,
    },
}

impl Provenance {
    /// Whether the signature actually holds — the only state a HEADLESS
    /// accept may lean on (F13). Unsigned and invalid are both false:
    /// "missing" and "broken" differ on the card, but neither is something
    /// an unattended `--yes` can vouch for.
    pub fn verifies(&self) -> bool {
        matches!(self, Provenance::Verified { .. })
    }

    /// The one line a review card leads with, in the reader's terms.
    ///
    /// Mirrors [`crate::recognition::line`] on purpose, including its most
    /// important property: it returns text that shortens the READING, and it
    /// is never consulted by anything that decides. There is no variant here
    /// that means "you can skip the review".
    pub fn card_line(&self) -> String {
        match self {
            Provenance::Unsigned => {
                "unsigned source — nothing vouches for where these bytes came from; \
                 review carefully"
                    .to_string()
            }
            Provenance::Invalid => {
                "SIGNATURE DOES NOT MATCH these bytes — they changed after signing, or \
                 were not signed by the key they claim. Review this as untrusted."
                    .to_string()
            }
            Provenance::Verified { key, known: None } => format!(
                "signed by {} — a publisher you have not recognized before. \
                 If you know whose key this is: agentstack more publisher trust {} --label <name>",
                fingerprint(key),
                key.to_hex()
            ),
            Provenance::Verified {
                key,
                known: Some(k),
            } => format!(
                "signed by {} ({}) — signature checks out against these exact bytes",
                k.label,
                fingerprint(key)
            ),
        }
    }

    /// Whether the card may lead with the short recognition line rather than
    /// making the reader establish provenance themselves.
    ///
    /// This is the ONLY thing a signature buys, and the name says what it
    /// affects: the card's length, not its outcome. Nothing in the activation
    /// path may call this.
    pub fn shortens_the_card(&self) -> bool {
        matches!(self, Provenance::Verified { known: Some(_), .. })
    }
}

/// Verify `signature` over `message` and classify the result.
pub fn assess(
    message: &[u8],
    key: Option<&PublicKey>,
    signature: Option<&Signature>,
) -> Provenance {
    let (Some(key), Some(signature)) = (key, signature) else {
        return Provenance::Unsigned;
    };
    if !agentstack_trust::sign::verify(key, message, signature) {
        return Provenance::Invalid;
    }
    Provenance::Verified {
        key: key.clone(),
        known: recognize(key),
    }
}

fn hex_of(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_to_32(s: &str) -> Result<[u8; 32]> {
    anyhow::ensure!(s.len() == 64, "expected 64 hex characters, got {}", s.len());
    let mut out = [0u8; 32];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16)
            .map_err(|_| anyhow::anyhow!("not hexadecimal"))?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fingerprint_is_for_reading_not_for_comparing() {
        let (key, _) = agentstack_trust::sign::sign(&[7u8; 32], b"x");
        let fp = fingerprint(&key);
        assert_eq!(fp.len(), 19, "four groups of four, three separators: {fp}");
        assert!(fp.contains('-'));
        // The full key is longer than what is displayed — the display form is
        // strictly lossy, which is why nothing may compare it.
        assert!(key.to_hex().len() > fp.replace('-', "").len());
    }

    #[test]
    fn an_unverifiable_signature_is_named_rather_than_ignored() {
        let (key, sig) = agentstack_trust::sign::sign(&[1u8; 32], b"the original bytes");
        let p = assess(b"different bytes", Some(&key), Some(&sig));
        assert_eq!(p, Provenance::Invalid);
        assert!(p.card_line().contains("DOES NOT MATCH"));
        assert!(!p.shortens_the_card(), "a bad signature must not shorten");
    }

    #[test]
    fn no_provenance_state_shortens_the_card_without_recognition() {
        let (key, sig) = agentstack_trust::sign::sign(&[2u8; 32], b"bytes");
        // Verified but unrecognized: correct signature, unknown publisher.
        let p = Provenance::Verified {
            key: key.clone(),
            known: None,
        };
        assert!(
            !p.shortens_the_card(),
            "a valid signature from a stranger is still a stranger"
        );
        assert!(
            p.card_line().contains("publisher trust"),
            "and it must say how to recognize them, or the state is a dead end"
        );
        assert!(!Provenance::Unsigned.shortens_the_card());
        assert!(!Provenance::Invalid.shortens_the_card());
        // Sanity: the verification itself works, so the assertions above are
        // about recognition rather than about a broken signature.
        assert!(agentstack_trust::sign::verify(&key, b"bytes", &sig));
    }
}
