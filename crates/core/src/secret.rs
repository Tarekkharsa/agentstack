//! The secret-resolution *contract* — and only the contract.
//!
//! core owns the pure signature (this trait, its outcome enum, and the
//! map-backed test resolver); the mechanisms and their I/O — keychain,
//! varlock, env, dotenv, chains, caches — stay in the `cli` crate. Nothing
//! in core calls `resolve`, so core's hostile-input parsing surface does not
//! grow by hosting the signature. It lives here because the `${REF}`
//! lifecycle spans crates: `refs` owns the syntax, adapters consume the
//! interface at render time, and the Phase 2 runtime needs the same
//! interface for env-injection — with no `adapters` edge in the pinned
//! crate graph, the shared base is the only home that doesn't force
//! duplication or a forbidden edge.

use std::collections::HashMap;
use std::fmt;

/// A resolved secret value, wrapped so it can't leak through `Debug`.
///
/// The reason this type exists: `Lookup` derives `Debug` (it flows through
/// caches, error paths, and test asserts), and a bare `String` inside it means
/// a single stray `{:?}` or `dbg!` prints a live credential in cleartext. This
/// newtype's manual `Debug` prints `«redacted»` instead, so the carrier stays
/// `Debug`-able while the value never is. The raw bytes come out ONLY through
/// the explicit [`expose`](SecretValue::expose) / [`into_inner`] accessors — a
/// grep for those names is the audit of every place a secret is read in the
/// clear. Deliberately NO `Display`, `Serialize`, or `Deref`: each would be a
/// silent exposure path (rule 5 — secrets never serialize).
///
/// (For the TS-minded: this is a branded type — the compiler forces you to
/// call `.expose()` to unwrap it, so "I am handling a secret here" is visible
/// at every use site instead of a plain string that looks like any other.)
#[derive(Clone)]
pub struct SecretValue(String);

impl SecretValue {
    /// The raw value, for the point where it is genuinely needed (rendering
    /// into config, env injection). Every call is a place a secret is exposed.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Consume the wrapper, yielding the owned raw value.
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl From<String> for SecretValue {
    fn from(s: String) -> Self {
        SecretValue(s)
    }
}

impl From<&str> for SecretValue {
    fn from(s: &str) -> Self {
        SecretValue(s.to_string())
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The whole point of the type: the value is never rendered.
        f.write_str("SecretValue(«redacted»)")
    }
}

impl PartialEq for SecretValue {
    /// Constant-time so equality can't leak the value through a timing side
    /// channel. No runtime path compares two secrets today (the enum is only
    /// compared against unit variants), but a secret type with an early-return
    /// `==` is a footgun; `core::util::ct_eq` already exists for exactly this.
    fn eq(&self, other: &Self) -> bool {
        crate::util::ct_eq(self.0.as_bytes(), other.0.as_bytes())
    }
}

impl Eq for SecretValue {}

/// The outcome of one reference lookup. `Failed` is a backing store erroring
/// while reading (e.g. a keychain timeout) — distinct from `Missing` so callers
/// don't misreport a transient read failure as "secret not set".
#[derive(Clone, Debug, PartialEq)]
pub enum Lookup {
    /// The resolved value, wrapped so `Debug` can't print it (see
    /// [`SecretValue`]).
    Found(SecretValue),
    /// No store has this name.
    Missing,
    /// A store errored while reading; the message names the store and cause.
    Failed(String),
    /// `[policy.secrets]` refuses this reference for the requesting server —
    /// the ref never reaches any backing store. Distinct from `Missing`
    /// because a policy denial must never masquerade as "secret not set":
    /// misleading diagnostics at a security boundary are a bug. The message
    /// names the rule and layer. Fails closed like `Missing` (rule 5): block
    /// the write/run, never emit a resolved value or drop the placeholder.
    Denied(String),
}

impl Lookup {
    /// The exposed value when found, else `None`. This is an explicit
    /// exposure point — `resolve` returns the raw value by contract, so the
    /// wrapper is unwrapped here rather than propagated.
    pub fn found(self) -> Option<String> {
        match self {
            Lookup::Found(v) => Some(v.into_inner()),
            _ => None,
        }
    }
}

/// Anything that can turn a reference name into its secret value.
pub trait Resolver {
    fn resolve(&self, name: &str) -> Option<String>;

    /// Error-aware lookup. The default can't tell a read failure from a miss;
    /// resolvers with fallible backends (keychain) override it.
    fn lookup(&self, name: &str) -> Lookup {
        match self.resolve(name) {
            Some(v) => Lookup::Found(v.into()),
            None => Lookup::Missing,
        }
    }
}

/// In-memory resolver for tests and deterministic rendering.
#[derive(Default)]
pub struct MapResolver {
    vars: HashMap<String, String>,
}

impl<const N: usize> From<[(&str, &str); N]> for MapResolver {
    fn from(pairs: [(&str, &str); N]) -> Self {
        MapResolver {
            vars: pairs
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }
}

impl Resolver for MapResolver {
    fn resolve(&self, name: &str) -> Option<String> {
        self.vars.get(name).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reason the type exists: neither the wrapper nor the `Lookup` that
    /// carries it may render the value through `Debug`. NEVER weaken this —
    /// it is the witness for "secrets never leak via `{:?}`".
    #[test]
    fn debug_never_prints_the_secret() {
        let secret = "hunter2-super-secret";
        let wrapped = SecretValue::from(secret);
        assert!(!format!("{wrapped:?}").contains(secret));
        assert!(format!("{wrapped:?}").contains("redacted"));
        // …and through the enum that derives Debug and gets cached/logged.
        let found = Lookup::Found(SecretValue::from(secret));
        let rendered = format!("{found:?}");
        assert!(
            !rendered.contains(secret),
            "Lookup Debug leaked: {rendered}"
        );
        // The non-secret variants still carry their diagnostic text.
        let failed = Lookup::Failed("keychain timeout".into());
        assert!(format!("{failed:?}").contains("keychain timeout"));
    }

    /// The value is still reachable through the explicit accessors — the
    /// wrapper hides it from `Debug`, not from the code that needs it.
    #[test]
    fn value_is_reachable_only_through_explicit_accessors() {
        let s = SecretValue::from("v");
        assert_eq!(s.expose(), "v");
        assert_eq!(s.clone().into_inner(), "v");
        assert_eq!(
            Lookup::Found(SecretValue::from("v")).found().as_deref(),
            Some("v")
        );
        assert_eq!(Lookup::Missing.found(), None);
    }
}
