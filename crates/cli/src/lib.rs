//! agentstack — one portable manifest, every agent CLI.
//!
//! Library surface so both the `agentstack` binary and the integration tests
//! can drive the same code. The module layout mirrors the data flow:
//! [`manifest`] (source of truth) → [`secret`] (resolve `${REF}`s) →
//! [`adapter`] (per-CLI descriptors + generic render) → [`render`]
//! (non-destructive merge into native config).

pub mod adapter;
pub mod calllog;
pub mod catalog;
pub mod cli;
pub mod codemode;
pub mod commands;
pub mod consolidate;
pub mod dashboard;
pub mod discover;
pub mod footprint;
pub mod gateway;
pub mod history;
pub mod library;
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
pub mod transcripts;
pub mod trust;
pub mod usage;
pub use agentstack_core::util;
