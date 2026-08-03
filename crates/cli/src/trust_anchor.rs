//! The trust anchor: the consent digest one live connection was authorized
//! against, re-verified from disk at every upstream dispatch (W2).
//!
//! # Why an anchor and not a flag
//!
//! Trust used to be decided once — when a gateway was built, or when a lease,
//! load, or session call re-checked it — and an already-spawned upstream then
//! stayed proxied until the next such call. That is the hole
//! `docs/design/automatic-delivery.md` §"Trust is checked at dispatch, from
//! the digest" names: revoking trust, editing the manifest out of band, or
//! swapping the lock wholesale (what a `git pull` or a branch switch actually
//! does) all happen *outside* AgentStack, so nothing in-process is notified
//! and a stale "yes" keeps a live path open.
//!
//! The anchor is the fix in its smallest form: capture the digest the
//! connection was authorized against, and compare the CURRENT digest to it
//! before every dispatch. It is a value, not a subscription — there is no
//! watcher to miss an event and no generation counter to go stale.
//!
//! # No cache, on purpose
//!
//! The contract permits a generation token as an accelerator but states it is
//! never authoritative. This codebase has no filesystem watcher, so a token
//! here could only ever be a guess that a file did not change — which is
//! exactly the guess that lets a `git pull` through. [`TrustAnchor::verify`]
//! therefore recomputes on every call: three small file reads and one
//! SHA-256. The contract explicitly blesses always-recompute.
//!
//! # Fail closed on uncertainty
//!
//! Every outcome that is not "the digest still matches AND the store still
//! says trusted" is a violation, including an unreadable manifest. An
//! inconclusive re-verification is a refusal, never a pass.

use std::path::{Path, PathBuf};

use agentstack_trust::TrustState;

/// Why a dispatch must be refused. Three parts, because they feed the
/// [`crate::seatbelt::Denial`] the user actually reads: a stable machine-facing
/// `state` tag for the evidence log, the sentence fragment that says what
/// happened, and the one command that fixes it.
pub struct Violation {
    /// Closed set, for the run-event log: `"revoked"`, `"changed"`,
    /// `"unreadable"`. Machine-authored — never repository content.
    ///
    /// The run event's own set is one wider: the lease/load refusals (W1) have
    /// no anchor to compare against and derive their tag from the current trust
    /// state, which adds `"untrusted"`. Only these three can come from *here*.
    pub state: &'static str,
    /// What happened, as a phrase that follows the attempted action.
    pub why: String,
    /// The one safe next step, naming the command.
    pub next_step: String,
}

/// The consent digest a live gateway (or eager serve loop) was authorized
/// against, plus the project root it was captured for.
///
/// Cheap to clone and cheap to hold: two owned `String`/`PathBuf` fields and
/// no borrow of the gateway, so a worker thread can carry one without tying
/// its lifetime to anything. (Owning the two values rather than borrowing the
/// project context is deliberate — the context is dropped long before the
/// first dispatch.)
#[derive(Debug, Clone)]
pub struct TrustAnchor {
    root: PathBuf,
    digest: String,
}

impl TrustAnchor {
    /// Bind an anchor to a digest the caller already computed. Private to the
    /// crate so the only way to get one is to have observed `Trusted` — the
    /// gateway computes the digest once and reuses it for both decisions.
    pub(crate) fn new(root: PathBuf, digest: String) -> TrustAnchor {
        TrustAnchor { root, digest }
    }

    /// Capture the anchor for `root` if — and only if — it is trusted right
    /// now. `None` for every other state, which is what keeps a never-trusted
    /// eager project behaving exactly as it did before W2.
    pub fn capture(root: &Path) -> Option<TrustAnchor> {
        let digest = agentstack_trust::digest_for(root)?;
        match agentstack_trust::check_digest(root, Some(&digest)) {
            TrustState::Trusted => Some(TrustAnchor::new(root.to_path_buf(), digest)),
            _ => None,
        }
    }

    /// The digest this connection was authorized against.
    pub fn digest(&self) -> &str {
        &self.digest
    }

    /// Re-verify NOW, from disk. `Ok(())` only when the current consent bytes
    /// digest to the anchored value *and* the store still records a trust
    /// grant for that digest.
    ///
    /// The two failures are distinguished on purpose: "the bytes moved" and
    /// "the yes was withdrawn" have the same command as their fix but very
    /// different meanings to a reader, and a denial that misnames what
    /// happened is one the user cannot act on.
    pub fn verify(&self) -> Result<(), Violation> {
        let dir = crate::text::sanitize_line(&self.root.display().to_string());
        let next_step = format!(
            "review it and run `agentstack trust {dir}`, then reopen the lease \
             or restart the MCP connection"
        );
        // Uncertainty is a refusal. A manifest that cannot be read cannot be
        // shown to be the one that was consented to, and "probably unchanged"
        // is not a security property.
        let Some(current) = agentstack_trust::digest_for(&self.root) else {
            return Err(Violation {
                state: "unreadable",
                why: "could not re-verify trust from this project's manifest and lock \
                      (fail closed)"
                    .to_string(),
                next_step,
            });
        };
        if current != self.digest {
            return Err(Violation {
                state: "changed",
                why: "this project's manifest or lockfile changed since this connection \
                      was authorized"
                    .to_string(),
                next_step,
            });
        }
        // Same bytes, so the only thing that can have moved is the store: the
        // human withdrew the yes.
        if agentstack_trust::check_digest(&self.root, Some(&current)) != TrustState::Trusted {
            return Err(Violation {
                state: "revoked",
                why: "trust for this project was revoked since this connection was authorized"
                    .to_string(),
                next_step,
            });
        }
        Ok(())
    }

    /// The violation rendered in the same voice as the auto-project trust
    /// note, so a refused skill load reads identically however the refusal was
    /// reached. `None` when the anchor still verifies.
    pub fn note(&self) -> Option<String> {
        let v = self.verify().err()?;
        Some(format!(
            "This project's trust no longer holds: {} — its MCP servers are not proxied \
             and no skill is loaded until it is re-trusted. {}.",
            v.why, v.next_step
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(base: &Path, body: &str) {
        std::fs::create_dir_all(base.join(".agentstack")).unwrap();
        std::fs::write(base.join(".agentstack/agentstack.toml"), body).unwrap();
    }

    /// The three violations the contract names, each identified as itself.
    #[test]
    fn the_anchor_names_revoked_changed_and_unreadable_apart() {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", home.path());
        let tmp = assert_fs::TempDir::new().unwrap();
        let base = tmp.path().to_path_buf();
        project(&base, "version = 1\n");
        agentstack_trust::trust_unreviewed(&base).unwrap();

        let anchor = TrustAnchor::capture(&base).expect("trusted → an anchor");
        assert!(
            anchor.verify().is_ok(),
            "unchanged and trusted → no refusal"
        );
        assert!(anchor.note().is_none());

        // Bytes moved out of band.
        std::fs::write(
            base.join(".agentstack/agentstack.toml"),
            "version = 1\n# edit\n",
        )
        .unwrap();
        let v = anchor
            .verify()
            .expect_err("an out-of-band edit must refuse");
        assert_eq!(v.state, "changed");
        assert!(v.next_step.contains("agentstack trust"), "{}", v.next_step);

        // Bytes back, yes withdrawn.
        std::fs::write(base.join(".agentstack/agentstack.toml"), "version = 1\n").unwrap();
        assert!(anchor.verify().is_ok(), "restored bytes verify again");
        std::fs::remove_file(agentstack_trust::store_path()).unwrap();
        assert_eq!(
            anchor.verify().expect_err("revoked must refuse").state,
            "revoked"
        );

        // Unreadable is a refusal, not a pass.
        std::fs::remove_file(base.join(".agentstack/agentstack.toml")).unwrap();
        assert_eq!(
            anchor.verify().expect_err("unreadable must refuse").state,
            "unreadable"
        );

        std::env::remove_var("AGENTSTACK_HOME");
    }

    /// An untrusted project yields no anchor at all — the pre-W2 behaviour for
    /// eager, consent-by-invocation projects is preserved by construction.
    #[test]
    fn an_untrusted_project_has_no_anchor() {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", home.path());
        let tmp = assert_fs::TempDir::new().unwrap();
        project(tmp.path(), "version = 1\n");
        assert!(TrustAnchor::capture(tmp.path()).is_none());
        std::env::remove_var("AGENTSTACK_HOME");
    }
}
