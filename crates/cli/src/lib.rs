//! agentstack — one portable manifest, every agent CLI.
//!
//! Library surface so both the `agentstack` binary and the integration tests
//! can drive the same code. The module layout mirrors the data flow:
//! [`manifest`] (source of truth) → [`secret`] (resolve `${REF}`s) →
//! [`adapter`] (per-CLI descriptors + generic render) → [`render`]
//! (non-destructive merge into native config).

// Unsafe is denied crate-wide; the sole sanctioned exception is `sys`, which
// concentrates every libc / raw-fd / pre_exec call behind safe wrappers so the
// entire unsafe surface is one greppable file (CLAUDE.md rule 1). `deny`, not
// `forbid`, because `forbid` can't be locally downgraded by this `#[allow]`.
#![deny(unsafe_code)]

// TODO(phase-1): shim — migrate callers to agentstack_adapters:: and drop.
pub use agentstack_adapters as adapter;
// TODO(phase-1): shim — migrate callers to agentstack_recorder:: and drop.
pub use agentstack_recorder as calllog;
pub mod catalog;
pub mod cli;
pub mod codemode;
pub mod commands;
pub mod consolidate;
pub mod dashboard;
pub mod discover;
pub mod execution;
pub mod footprint;
pub mod gateway;
pub mod gateway_http;
pub mod grant;
pub mod guard;
pub mod history;
pub mod library;
pub mod machine_policy;
// TODO(phase-1): re-export shims — migrate callers to agentstack_core:: paths
// and drop these, so the crate graph (not cli) is what exposes core types.
pub use agentstack_core::lock;
pub mod manifest;
pub mod mcp;
pub mod mcp_server;
pub mod pi_packages;
pub mod plugin_recipes;
pub mod plugins;
pub mod provider;
pub mod proxy;
pub mod render;
pub mod resolve;
pub mod runs;
pub mod scan;
pub use agentstack_core::scope;
pub mod secret;
pub mod session;
pub mod state;
pub mod store;
#[allow(unsafe_code)]
pub(crate) mod sys;
pub mod transcripts;
// TODO(phase-1): shim — migrate callers to agentstack_trust:: and drop.
pub use agentstack_trust as trust;
pub mod usage;
pub mod verify;
pub use agentstack_core::util;
