//! The sandbox runtime — container lifecycle for `agentstack run --sandbox`
//! (Phase 2, ROADMAP item 2.1).
//!
//! The design is a clean seam: a synchronous [`Sandbox`] trait abstracts the
//! container backend, and the [`orchestrate::run`] driver takes a bundle
//! through create → stream → teardown while emitting flight-recorder events —
//! all backend-agnostic and unit-tested against a fake. The real Docker
//! backend ([`docker`], behind the default `docker` feature) implements the
//! trait via `bollard`; because that trait is *synchronous*, the async Docker
//! API is driven by a runtime the backend owns internally, so the
//! orchestration logic above the trait stays synchronous. The one place async
//! surfaces is [`lockdown`] (also `docker`-gated), which follows the egress
//! sidecar's logs on a spawned task while the main thread blocks on the
//! sandbox — still on the backend's own runtime, never leaking tokio to
//! callers. Build `--no-default-features` for the pure-logic core alone.
//!
//! Crate edges (CLAUDE.md): `core`, `policy`, `recorder` only.
//!
//! Honest scope for this increment: the backend-agnostic core is complete and
//! tested. The `bollard` backend is compile-verified but NOT yet behavior-
//! verified — it needs a real Docker daemon, which its integration tests gate
//! behind a liveness check (skipped where none is available, e.g. CI).

#![forbid(unsafe_code)]

pub mod orchestrate;
pub mod sandbox;
pub mod spec;

#[cfg(feature = "docker")]
pub mod docker;

#[cfg(feature = "docker")]
pub mod lockdown;

pub use orchestrate::run;
pub use sandbox::{Exit, Sandbox, SandboxHandle, Stream, StreamChunk};
pub use spec::{Mount, NetworkPolicy, SandboxSpec};

#[cfg(feature = "docker")]
pub use lockdown::{Lockdown, LockdownSink, GATEWAY_RELAY_PORT, PROXY_ALIAS, PROXY_BASE_PORT};

/// A runtime failure. Backend errors are wrapped as `Backend` so the
/// orchestration layer stays backend-agnostic.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("sandbox backend: {0}")]
    Backend(String),
    #[error("sandbox teardown failed: {0}")]
    Teardown(String),
}

pub type Result<T> = std::result::Result<T, RuntimeError>;
