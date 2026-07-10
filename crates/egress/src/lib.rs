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
//! What does NOT live here yet (the supervised 2.2 async increment): the
//! forward-proxy SERVER itself — the tokio accept loop, byte tunnelling for
//! allowed connections, per-server proxy identity for attribution, and DNS
//! routing/filtering. That transport is where tokio/hyper (rule 6: confined to
//! this crate) will land; it needs real containers + network traffic to
//! develop against, so it is deliberately not blind-built here. This core is
//! what that server will call for every connection.
//!
//! Everything here treats its input as hostile: a CONNECT line and a
//! ClientHello both come from inside an untrusted container, so the parsers
//! are bounds-checked and never panic.

#![forbid(unsafe_code)]

pub mod connect;
pub mod decide;
pub mod sni;

pub use connect::{parse_connect_target, Target};
pub use decide::{Decision, EgressGuard};
pub use sni::extract_sni;
