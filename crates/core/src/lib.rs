//! agentstack-core — the bundle format.
//!
//! Manifest model + layered loading, the lockfile with its content digests,
//! and the shared path/fs helpers. Depends on nothing else in the workspace;
//! everything security-relevant builds on these types.
//!
//! Manifest *validation* lives in the `cli` crate for now — it reaches into
//! the central library and resolver, which have not been extracted yet.

#![forbid(unsafe_code)]

pub mod lock;
pub mod manifest;
pub mod refs;
pub mod scope;
pub mod util;
