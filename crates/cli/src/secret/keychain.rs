//! OS keychain store, backed by the `keyring` crate (macOS Keychain, Windows
//! Credential Manager, Linux Secret Service). This is agentstack's own managed
//! secret store — where `agentstack secret set` writes — and a resolver link in
//! the chain.

use anyhow::{Context, Result};

use super::{Lookup, Resolver};

/// Service name under which all agentstack secrets are stored.
pub const SERVICE: &str = "agentstack";

/// Resolves `${NAME}` from the OS keychain (service `agentstack`, account
/// `NAME`).
pub struct KeychainResolver;

impl Resolver for KeychainResolver {
    fn resolve(&self, name: &str) -> Option<String> {
        self.lookup(name).found()
    }

    fn lookup(&self, name: &str) -> Lookup {
        read_with_retry(|| get(name))
    }
}

/// A keychain read can fail transiently (the `security` daemon under load);
/// retry once, and report a persistent failure as [`Lookup::Failed`]. Reading
/// "error" as "not stored" is what used to block `apply` with a bogus
/// "unresolved secret" for a secret that is in the keychain.
fn read_with_retry(read: impl Fn() -> Result<Option<String>>) -> Lookup {
    if let Ok(outcome) = read() {
        return outcome.map_or(Lookup::Missing, |v| Lookup::Found(v.into()));
    }
    match read() {
        Ok(Some(v)) => Lookup::Found(v.into()),
        Ok(None) => Lookup::Missing,
        // Report the root cause only. anyhow's `{e:#}` walks every `source()`
        // and joins with ": ", but keyring/io errors already fold their
        // source's text into their own Display — so `{e:#}` prints the root
        // sentence twice ("… not found.: … not found.") behind two restated
        // context prefixes. `root_cause()` is the single actionable line; the
        // render layer supplies the secret name and store around it.
        Err(e) => Lookup::Failed(format!("keychain read failed: {}", e.root_cause())),
    }
}

fn entry(name: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, name).context("opening keychain entry")
}

/// Store a secret value (overwrites any existing one).
pub fn set(name: &str, value: &str) -> Result<()> {
    entry(name)?
        .set_password(value)
        .with_context(|| format!("storing secret '{name}' in keychain"))
}

/// Read a secret value, if present.
pub fn get(name: &str) -> Result<Option<String>> {
    match entry(name)?.get_password() {
        Ok(v) => Ok(Some(v)),
        Err(keyring::Error::NoEntry) => Ok(None),
        // Same dedup as `lookup` above: keyring's Display already embeds its
        // platform cause, so a plain `.context()` chain would print the root
        // sentence twice. Keep exactly two layers — our context over the bare
        // root — so `{e:#}` prints each once and `root_cause()` stays the
        // single platform sentence (flattening both into one string made
        // every downstream `root_cause()` re-print the name and store).
        Err(e) => {
            let e = anyhow::Error::new(e);
            let root = e.root_cause().to_string();
            Err(anyhow::anyhow!(root).context(format!("reading secret '{name}' from keychain")))
        }
    }
}

/// Whether a secret is stored, **without reading its value**.
///
/// macOS gates the *data* of a keychain item behind a per-application ACL —
/// that is the "agentstack wants to use your confidential information" dialog.
/// Item *attributes* are not gated, so an attribute-only query answers
/// "is it there?" without ever prompting.
///
/// This matters because an ACL grant ("Always Allow") is bound to the calling
/// binary's code identity, and a locally built `agentstack` is ad-hoc signed:
/// its cdhash changes on every `cargo build`, which silently invalidates every
/// previous grant. A status path that read *values* therefore re-prompted, once
/// per referenced secret, after every rebuild.
///
/// Status/provenance callers (doctor, `secret list`, `explain`, the panel
/// snapshot) only ever need existence and must come through here —
/// [`KeychainProbe`] is the type that enforces it. [`get`] stays for the
/// callers that genuinely need the value (render / apply / run), where the
/// prompt is the honest cost of using the secret.
pub fn exists(name: &str) -> Result<bool> {
    #[cfg(target_os = "macos")]
    {
        use security_framework::item::{ItemClass, ItemSearchOptions};

        /// `errSecItemNotFound` — a clean miss, not a failure.
        const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

        let found = ItemSearchOptions::new()
            .class(ItemClass::generic_password())
            .service(SERVICE)
            .account(name)
            // Required, and deliberately *attributes*, not data:
            // `SecItemCopyMatching` returns NULL when no `kSecReturn*` is
            // requested, which the binding reports as an empty result — a hit
            // would be indistinguishable from a miss. Attributes are the
            // cheapest non-empty return and stay clear of `kSecReturnData`,
            // the one that triggers the ACL prompt.
            .load_attributes(true)
            .limit(1)
            .search();
        match found {
            Ok(hits) => Ok(!hits.is_empty()),
            Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(false),
            // Same root-cause-only shape as `get`: keyring/security-framework
            // errors already fold their platform sentence into Display.
            Err(e) => Err(anyhow::anyhow!(e.to_string())
                .context(format!("checking secret '{name}' in keychain"))),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        // Windows Credential Manager and the Linux Secret Service don't gate
        // reads behind a per-application consent prompt, so the ordinary read
        // is free of the problem this function exists to avoid. One backend
        // keeps existence semantics identical across platforms.
        get(name).map(|v| v.is_some())
    }
}

/// Presence-only view of the keychain, for paths that report *where* a `${REF}`
/// resolves rather than resolving it.
///
/// The type is the witness: it exposes no way to obtain a value, so a status
/// path that holds one of these cannot trigger the keychain consent prompt,
/// however it is later edited. Swapping this for [`KeychainResolver`] in
/// [`crate::secret::SecretSources`] is what would reintroduce the bug.
pub struct KeychainProbe;

impl KeychainProbe {
    /// `true` when the keychain holds this ref. A read error reports as absent
    /// here — provenance is advisory, and the resolving path (`Chain`) is where
    /// a failed read is surfaced as `Failed` rather than `Missing`.
    pub fn contains(&self, name: &str) -> bool {
        exists(name).unwrap_or(false)
    }
}

/// Delete a secret. Returns `true` if something was removed, `false` if it was
/// already absent.
pub fn delete(name: &str) -> Result<bool> {
    match entry(name)?.delete_password() {
        Ok(()) => Ok(true),
        Err(keyring::Error::NoEntry) => Ok(false),
        Err(e) => Err(e).with_context(|| format!("deleting secret '{name}' from keychain")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn retry_recovers_from_one_transient_failure() {
        let calls = Cell::new(0);
        let out = read_with_retry(|| {
            calls.set(calls.get() + 1);
            if calls.get() == 1 {
                anyhow::bail!("security daemon timed out")
            }
            Ok(Some("v".to_string()))
        });
        assert_eq!(out, Lookup::Found("v".into()));
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn persistent_failure_reports_failed_not_missing() {
        let calls = Cell::new(0);
        let out = read_with_retry(|| {
            calls.set(calls.get() + 1);
            anyhow::bail!("security daemon timed out")
        });
        let Lookup::Failed(msg) = out else {
            panic!("expected Failed, got {out:?}");
        };
        assert!(msg.contains("keychain read failed"), "{msg}");
        assert!(msg.contains("security daemon timed out"), "{msg}");
        assert_eq!(calls.get(), 2, "exactly one retry");
    }

    /// A miss must come back as `Ok(false)`, not an error — this is the arm
    /// `SecretSources` reads as "not in the keychain". It also exercises the
    /// real platform query: a malformed one fails with `errSecParam` (-50)
    /// rather than `errSecItemNotFound`, and would surface here as `Err`.
    /// Safe to run anywhere — reading attributes of an item that doesn't exist
    /// touches no ACL and prompts for nothing.
    #[test]
    fn absent_secret_probes_as_not_found() {
        let out = exists("AGENTSTACK_TEST_SECRET_THAT_DOES_NOT_EXIST");
        assert!(matches!(out, Ok(false)), "{out:?}");
    }

    /// The end-to-end witness: an item written through `set` (i.e. by the same
    /// `keyring` backend `agentstack secret set` uses) is visible to the
    /// attribute-only probe. Ignored by default because it writes to and
    /// deletes from the developer's real login keychain — run explicitly with
    /// `cargo test -p agentstack keychain_roundtrip -- --ignored`.
    #[test]
    #[ignore = "touches the real OS keychain"]
    fn set_secret_is_visible_to_the_probe_keychain_roundtrip() {
        const NAME: &str = "AGENTSTACK_PROBE_SELFTEST";
        set(NAME, "value").expect("set");
        assert!(
            exists(NAME).expect("probe"),
            "written secret must probe true"
        );
        delete(NAME).expect("delete");
        assert!(
            !exists(NAME).expect("probe after delete"),
            "deleted secret must probe false"
        );
    }

    #[test]
    fn genuine_not_found_is_missing_without_retry() {
        let calls = Cell::new(0);
        let out = read_with_retry(|| {
            calls.set(calls.get() + 1);
            Ok(None)
        });
        assert_eq!(out, Lookup::Missing);
        assert_eq!(calls.get(), 1, "a clean miss is not retried");
    }
}
