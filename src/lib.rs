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
pub mod codemode;
pub mod commands;
pub mod consolidate;
pub mod dashboard;
pub mod discover;
pub mod footprint;
pub mod gateway;
pub mod history;
pub mod library;
pub mod lock;
pub mod manifest;
pub mod mcp;
pub mod mcp_server;
pub mod pi_packages;
pub mod plugin_recipes;
pub mod plugins;
pub mod provider;
pub mod render;
pub mod resolve;
pub mod runs;
pub mod scan;
pub mod scope;
pub mod secret;
pub mod session;
pub mod state;
pub mod store;
pub mod usage;
pub mod util;
