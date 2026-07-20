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
