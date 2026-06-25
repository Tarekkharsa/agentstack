//! OS keychain store, backed by the `keyring` crate (macOS Keychain, Windows
//! Credential Manager, Linux Secret Service). This is agentstack's own managed
//! secret store — where `agentstack secret set` writes — and a resolver link in
//! the chain.

use anyhow::{Context, Result};

use super::Resolver;

/// Service name under which all agentstack secrets are stored.
pub const SERVICE: &str = "agentstack";

/// Resolves `${NAME}` from the OS keychain (service `agentstack`, account
/// `NAME`).
pub struct KeychainResolver;

impl Resolver for KeychainResolver {
    fn resolve(&self, name: &str) -> Option<String> {
        entry(name).ok()?.get_password().ok()
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
        Err(e) => Err(e).with_context(|| format!("reading secret '{name}' from keychain")),
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
