//! What to run and how to confine it — the backend-agnostic description of a
//! sandbox, built by the caller (cli) from a trusted bundle and its compiled
//! policy, then handed to a [`Sandbox`](crate::Sandbox) backend.

use agentstack_policy::CompiledRuleset;
use serde::{Deserialize, Serialize};

/// A host directory made visible inside the container.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mount {
    /// Absolute path on the host.
    pub host: String,
    /// Absolute path inside the container.
    pub container: String,
    /// Read-only when true — the backend turns this into a `:ro` bind, so
    /// the kernel enforces it. The caller decides: the workspace is
    /// read-only unless the effective `[policy.filesystem]` write scope
    /// covers it (`CompiledRuleset::workspace_write_decision`).
    pub read_only: bool,
}

/// The container's network exposure. Phase 2's whole point is that a sandbox
/// has no direct route out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetworkPolicy {
    /// No network at all — the container cannot reach anything. The honest
    /// default until the egress proxy (2.2) exists to give it a single
    /// controlled route.
    None,
    /// The container's only route out is the egress proxy at this address
    /// (`host:port`), which enforces the compiled ruleset per host per server.
    /// Reserved for when the egress crate lands; not yet constructed.
    ProxyOnly { endpoint: String },
}

/// The full description of one sandboxed run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxSpec {
    /// Container image to run the agent CLI in.
    pub image: String,
    /// The command + args to exec inside the container (the harness launch).
    pub command: Vec<String>,
    /// Bind mounts. The first is conventionally the workspace (read-write).
    pub mounts: Vec<Mount>,
    /// The container's working directory (usually the workspace mount point).
    pub workdir: String,
    /// Environment for the process — already secret-resolved and policy-scoped
    /// by the caller (never a `${REF}` placeholder, never a denied secret).
    pub env: Vec<(String, String)>,
    /// Network exposure (see [`NetworkPolicy`]).
    pub network: NetworkPolicy,
    /// The effective (machine ∩ bundle) policy, carried so the egress proxy
    /// (2.2) can enforce it from inside the container boundary without
    /// re-deriving anything — the identical artifact the gateway consumes.
    pub ruleset: CompiledRuleset,
}

impl SandboxSpec {
    /// The workspace host path (the first mount), for the flight-recorder
    /// `SandboxStarted` event.
    pub fn workspace(&self) -> &str {
        self.mounts.first().map(|m| m.host.as_str()).unwrap_or("")
    }
}
