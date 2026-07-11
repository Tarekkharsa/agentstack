//! The egress proxy's **enforcement core** — the pure, fully-tested decision
//! logic a forward proxy needs, with no sockets and no async (ROADMAP Phase 2
//! item 2, the hardest step in the system).
//!
//! What lives here, verifiable without a network or Docker:
//! - [`connect`]: parse the target host:port out of an HTTP `CONNECT` request.
//! - [`sni`]: extract the SNI hostname from a TLS ClientHello, so HTTPS is
//!   filtered by the name the client asked for — no TLS interception / MITM.
//! - [`decide`]: consult the compiled ruleset for one (server, host) pair,
//!   allow or block, and emit the recorder event — the same `CompiledRuleset`
//!   the gateway already enforces, serialized across the process boundary so
//!   the proxy never re-derives policy.
//!
//! - [`proxy`]: the async forward-proxy SERVER (tokio) that applies the guard
//!   to real connections — one [`ServerProxy`](proxy::ServerProxy) per MCP
//!   server (per-server identity = per-listener), tunnelling allowed CONNECTs
//!   and refusing blocked ones. tokio is confined to this crate (rule 6). It
//!   is testable on loopback (no Docker); wiring a *container* to route through
//!   it is the runtime's job (the one Docker-dependent piece).
//!
//! Everything here treats its input as hostile: a CONNECT line and a
//! ClientHello both come from inside an untrusted container, so the parsers
//! are bounds-checked and never panic, and the request head is size-capped.

#![forbid(unsafe_code)]

pub mod blocking;
pub mod bridge;
pub mod connect;
pub mod decide;
pub mod proxy;
pub mod sni;

pub use blocking::BlockingBridge;
pub use bridge::{EgressBridge, ProxyEndpoint};
pub use connect::{parse_connect_target, Target};
pub use decide::{Decision, EgressGuard};
pub use proxy::{EventSink, ServerProxy};
pub use sni::extract_sni;
