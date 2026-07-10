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

/// The outcome of one reference lookup. `Failed` is a backing store erroring
/// while reading (e.g. a keychain timeout) — distinct from `Missing` so callers
/// don't misreport a transient read failure as "secret not set".
#[derive(Clone, Debug, PartialEq)]
pub enum Lookup {
    Found(String),
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
    pub fn found(self) -> Option<String> {
        match self {
            Lookup::Found(v) => Some(v),
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
            Some(v) => Lookup::Found(v),
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
