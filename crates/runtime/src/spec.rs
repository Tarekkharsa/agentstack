//! What to run and how to confine it — the backend-agnostic description of a
//! sandbox, built by the caller (cli) from a trusted bundle and its compiled
//! policy, then handed to a [`Sandbox`](crate::Sandbox) backend.

use agentstack_policy::CompiledRuleset;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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
    /// Today this is the host-process proxy reached via `host.docker.internal`:
    /// it gates the agent's *configured* egress (HTTPS_PROXY), but a container
    /// that ignored the env could still reach the open internet directly.
    ProxyOnly { endpoint: String },
    /// True no-direct-route lockdown: the container is attached ONLY to this
    /// internal Docker network, which has no host route and no internet. Its
    /// sole reachable peer is the egress-proxy sidecar the lockdown module
    /// stands up on the same network — so ignoring `HTTPS_PROXY` reaches
    /// nothing. The network is created and torn down per run by
    /// `runtime::docker::Lockdown`.
    Lockdown { network: String },
}

/// Kernel/container hardening controls. The default preserves existing agent
/// CLI sandbox behavior; hostile generated-code execution opts into
/// [`SandboxSecurity::hardened_executor`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SandboxSecurity {
    pub user: Option<String>,
    pub read_only_root: bool,
    pub memory_bytes: Option<i64>,
    pub nano_cpus: Option<i64>,
    pub pids_limit: Option<i64>,
    pub drop_all_capabilities: bool,
    pub no_new_privileges: bool,
    pub tmpfs: BTreeMap<String, String>,
}

impl SandboxSecurity {
    pub fn hardened_executor() -> Self {
        Self {
            user: Some("65532:65532".into()),
            read_only_root: true,
            memory_bytes: Some(128 * 1024 * 1024),
            nano_cpus: Some(1_000_000_000),
            pids_limit: Some(32),
            drop_all_capabilities: true,
            no_new_privileges: true,
            tmpfs: BTreeMap::from([(
                "/tmp".into(),
                "rw,noexec,nosuid,nodev,size=16m,mode=1777".into(),
            )]),
        }
    }
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
    /// Container hardening independent of policy interpretation.
    pub security: SandboxSecurity,
}

impl SandboxSpec {
    /// The workspace host path (the first mount), for the flight-recorder
    /// `SandboxStarted` event.
    pub fn workspace(&self) -> &str {
        self.mounts.first().map(|m| m.host.as_str()).unwrap_or("")
    }
}
