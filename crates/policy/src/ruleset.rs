//! The compiled ruleset — the flat, canonical, serializable artifact every
//! enforcer consumes.
//!
//! [`compile`](crate::compile) collapses (machine ∩ bundle) across all policy
//! dimensions into this one value. The in-process gateway reads it today; the
//! Phase-2 egress proxy and sandbox runtime receive the *identical* artifact
//! serialized across the process boundary. The two-layer merge logic lives in
//! exactly one place — compile — never in a consumer.
//!
//! Two rulings, load-bearing enough to write down:
//!
//! - **Not trust-digest-relevant, by construction.** The ruleset derives from
//!   the bundle policy (pinned, in-tree) AND the machine policy
//!   (`~/.agentstack/agentstack.toml` — outside every repo, unpinned by
//!   design). One input is not in the pinned bundle, so this artifact must
//!   never feed the trust digest: that would create a second, machine-varying
//!   source of trust truth. Its byte-determinism (BTreeMap + sorted lists)
//!   serves reproducibility and Phase-2 wire integrity, not trust gating.
//! - **Fail closed on unknown versions.** A consumer that reads a `version`
//!   greater than it understands must deny everything, not guess — this is
//!   the enforcement artifact, not advisory config.
//!
//! The encoding is deliberately lossless rather than clever: glob allowlists
//! from different layers/keys cannot be folded into one equivalent list
//! (machine `get_*` ∧ bundle `*_file` has no single-glob form), so [`Guard`]
//! keeps each layer's rules and ANDs them at check time. Denies union exactly
//! (deny is monotonic).

use std::collections::BTreeMap;

use agentstack_core::manifest::glob_match;
use serde::{Deserialize, Serialize};

/// Bump ONLY on an incompatible semantic change. Additive fields ride on
/// serde defaults without a bump.
pub const RULESET_VERSION: u32 = 1;

/// Default action when no rule constrains a subject. Uniform allow-by-default
/// across dimensions (maintainer ruling): least privilege is an explicit
/// machine opt-in (`"*" = ["!*"]` then allowlist), not a per-dimension special
/// case. Self-describing in the artifact so Phase-2 consumers need no
/// out-of-band knowledge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    #[default]
    Allow,
    Deny,
}

/// Per-dimension defaults. All `Allow` in v1 (see [`Action`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Defaults {
    #[serde(default)]
    pub tools: Action,
    #[serde(default)]
    pub egress: Action,
    #[serde(default)]
    pub secrets: Action,
    #[serde(default)]
    pub filesystem: Action,
}

/// One layer's folded rules for one server and dimension: denies from its
/// named key and its `"*"` key unioned; each key's non-empty allowlist kept
/// as an independent bound in `allow_all_of` (a subject must match at least
/// one glob in EVERY inner list — AND across lists, OR within one).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayerRules {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow_all_of: Vec<Vec<String>>,
}

impl LayerRules {
    pub fn is_empty(&self) -> bool {
        self.deny.is_empty() && self.allow_all_of.is_empty()
    }

    fn check(&self, dimension: &str, subject: &str) -> Result<(), String> {
        for d in &self.deny {
            if glob_match(d, subject) {
                return Err(format!("denied by {dimension} rule \"!{d}\""));
            }
        }
        for allows in &self.allow_all_of {
            if !allows.iter().any(|a| glob_match(a, subject)) {
                return Err(format!(
                    "not in the {dimension} allowlist ({})",
                    allows.join(", ")
                ));
            }
        }
        Ok(())
    }
}

/// The lossless (machine ∩ bundle) encoding for one server and dimension.
/// Machine rules are kept separate from bundle rules so a machine refusal
/// still names its layer in the error — the audit trail (and the malicious-
/// repo demo) depend on "machine denies win *and say so*".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Guard {
    #[serde(default, skip_serializing_if = "LayerRules::is_empty")]
    pub machine: LayerRules,
    #[serde(default, skip_serializing_if = "LayerRules::is_empty")]
    pub bundle: LayerRules,
}

impl Guard {
    pub fn is_empty(&self) -> bool {
        self.machine.is_empty() && self.bundle.is_empty()
    }

    /// Machine layer first (its refusal names the layer), then the bundle's —
    /// the same composition as `agentstack_policy::tool_decision`.
    pub fn check(&self, dimension: &str, subject: &str) -> Result<(), String> {
        self.machine
            .check(dimension, subject)
            .map_err(|r| format!("{r} (machine policy — ~/.agentstack/agentstack.toml)"))?;
        self.bundle.check(dimension, subject)
    }
}

/// All per-server dimensions for one server identity.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerRules {
    #[serde(default, skip_serializing_if = "Guard::is_empty")]
    pub tools: Guard,
    #[serde(default, skip_serializing_if = "Guard::is_empty")]
    pub egress: Guard,
    #[serde(default, skip_serializing_if = "Guard::is_empty")]
    pub secrets: Guard,
}

impl ServerRules {
    fn is_empty(&self) -> bool {
        self.tools.is_empty() && self.egress.is_empty() && self.secrets.is_empty()
    }
}

/// Filesystem scopes are bundle-global (a sandbox mount is per-run, not
/// per-server). Carried verbatim in Phase 1 — the path-matching semantics are
/// deliberately NOT implemented until Phase 2's mount code has something
/// concrete to match against.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsRules {
    #[serde(default, skip_serializing_if = "Guard::is_empty")]
    pub read: Guard,
    #[serde(default, skip_serializing_if = "Guard::is_empty")]
    pub write: Guard,
}

impl FsRules {
    pub fn is_empty(&self) -> bool {
        self.read.is_empty() && self.write.is_empty()
    }
}

/// The wire contract between the policy engine and every enforcer.
///
/// `servers` holds one entry per server identity named by the trusted bundle
/// OR by either policy layer (a machine rule naming a server the bundle
/// doesn't declare must not silently degrade to the `any` bucket). A lookup
/// for anything else falls back to `any`, which carries exactly the folded
/// `"*"` rules — rename-proofing preserved by construction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompiledRuleset {
    pub version: u32,
    #[serde(default)]
    pub defaults: Defaults,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub servers: BTreeMap<String, ServerRules>,
    #[serde(default, skip_serializing_if = "ServerRules::is_empty")]
    pub any: ServerRules,
    #[serde(default, skip_serializing_if = "FsRules::is_empty")]
    pub filesystem: FsRules,
}

impl Default for CompiledRuleset {
    fn default() -> Self {
        CompiledRuleset {
            version: RULESET_VERSION,
            defaults: Defaults::default(),
            servers: BTreeMap::new(),
            any: ServerRules::default(),
            filesystem: FsRules::default(),
        }
    }
}

impl CompiledRuleset {
    fn rules_for(&self, server: &str) -> &ServerRules {
        self.servers.get(server).unwrap_or(&self.any)
    }

    /// The effective firewall decision for one tool call — identical
    /// semantics to `tool_decision(machine, bundle, server, tool)` (property-
    /// tested equivalence in lib.rs).
    pub fn tool_decision(&self, server: &str, tool: &str) -> Result<(), String> {
        self.rules_for(server).tools.check("[policy.tools]", tool)
    }

    /// Whether `server` may reach `host` per the compiled `[policy.egress]`.
    /// Phase 1 enforces this at write/spawn time against a server's DECLARED
    /// URL host only; runtime egress filtering is the Phase-2 proxy's job.
    pub fn egress_decision(&self, server: &str, host: &str) -> Result<(), String> {
        self.rules_for(server).egress.check("[policy.egress]", host)
    }

    /// Whether `server` may resolve the secret named `reference` per the
    /// compiled `[policy.secrets]`. Enforced fail-closed at both substitution
    /// sites (gateway + adapter render).
    pub fn secret_decision(&self, server: &str, reference: &str) -> Result<(), String> {
        self.rules_for(server)
            .secrets
            .check("[policy.secrets]", reference)
    }
}
