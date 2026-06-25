//! agentstack — one portable manifest, every agent CLI.
//!
//! Library surface so both the `agentstack` binary and the integration tests
//! can drive the same code. The module layout mirrors the data flow:
//! [`manifest`] (source of truth) → [`secret`] (resolve `${REF}`s) →
//! [`adapter`] (per-CLI descriptors + generic render) → [`render`]
//! (non-destructive merge into native config).

pub mod adapter;
pub mod catalog;
pub mod cli;
pub mod commands;
pub mod dashboard;
pub mod discover;
pub mod lock;
pub mod manifest;
pub mod mcp;
pub mod render;
pub mod scope;
pub mod secret;
pub mod state;
pub mod store;
pub mod usage;
pub mod util;
