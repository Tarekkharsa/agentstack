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

use std::collections::{BTreeMap, BTreeSet};

use agentstack_core::manifest::{
    egress_match, glob_to_match, normalize_host, Dimension, PatternMatch, RuleDenial,
};
use serde::{Deserialize, Serialize};

use crate::{Layer, PolicyDenial};

/// Bump ONLY on an incompatible semantic change. Additive fields ride on
/// serde defaults without a bump.
///
/// v2: egress patterns gained an optional `:port` suffix (`host:port` scopes to
/// that port). This REINTERPRETS the grammar — a v1 consumer would read
/// `!evil.example:443` as a hostname glob and fail to deny it — so the version
/// gate must make an older enforcer fail closed rather than misread a v2
/// ruleset.
///
/// v3: added `gateway_only_hosts` (D4) — the declared HTTP MCP upstream hosts a
/// lockdown container may reach only through the gateway relay. Unlike a field a
/// stale consumer can safely ignore (an fs rule it never consults), an older
/// sidecar that dropped this set would fail OPEN, serving the very direct route
/// the field exists to close. So this is a semantic-incompatibility bump: the
/// version gate must make an older enforcer fail closed, not silently omit it.
pub const RULESET_VERSION: u32 = 3;

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

    /// Deny-then-allow evaluation with a custom `matches(pattern)` (leading `!`
    /// already stripped for denies) that captures its own subject. Glob
    /// dimensions pass a `glob_to_match` closure via [`Guard::check`]; egress
    /// passes a port-aware one — the one grammar that can report `Malformed`,
    /// which fails the whole check CLOSED (a deny the grammar can't read must
    /// never be inert, and a broken allowlist must not half-work).
    fn check_with(
        &self,
        dimension: Dimension,
        matches: &impl Fn(&str) -> PatternMatch,
    ) -> Result<(), RuleDenial> {
        let malformed = |pattern: &str| RuleDenial {
            dimension,
            message: format!(
                "malformed {dimension} pattern \"{pattern}\" in the compiled ruleset — \
                 failing closed; fix the pattern in the manifest"
            ),
        };
        for d in &self.deny {
            match matches(d) {
                PatternMatch::Match => {
                    return Err(RuleDenial {
                        dimension,
                        message: format!("denied by {dimension} rule \"!{d}\""),
                    });
                }
                PatternMatch::Malformed => return Err(malformed(d)),
                PatternMatch::NoMatch => {}
            }
        }
        for allows in &self.allow_all_of {
            let mut hit = false;
            for a in allows {
                match matches(a) {
                    PatternMatch::Match => hit = true,
                    PatternMatch::Malformed => return Err(malformed(a)),
                    PatternMatch::NoMatch => {}
                }
            }
            if !hit {
                return Err(RuleDenial {
                    dimension,
                    message: format!("not in the {dimension} allowlist ({})", allows.join(", ")),
                });
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

    /// Machine layer first (its refusal carries `Layer::Machine`), then the
    /// bundle's — the same composition as `agentstack_policy::tool_decision`.
    pub fn check(&self, dimension: Dimension, subject: &str) -> Result<(), PolicyDenial> {
        self.check_with(dimension, &|pat| glob_to_match(pat, subject))
    }

    /// Machine-then-bundle check with a custom pattern matcher (egress uses it
    /// to scope by port). A machine refusal carries `Layer::Machine`, so the
    /// rendered error still names the layer — via `Display`, not string glue.
    pub fn check_with(
        &self,
        dimension: Dimension,
        matches: &impl Fn(&str) -> PatternMatch,
    ) -> Result<(), PolicyDenial> {
        self.machine
            .check_with(dimension, matches)
            .map_err(|denial| PolicyDenial {
                layer: Layer::Machine,
                denial,
            })?;
        self.bundle
            .check_with(dimension, matches)
            .map_err(|denial| PolicyDenial {
                layer: Layer::Bundle,
                denial,
            })
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
/// per-server). The `write` guard is enforced by the sandbox's workspace
/// mount via [`CompiledRuleset::workspace_write_decision`]; `read` scopes
/// remain informational while the only mount is the whole workspace (there
/// is no finer-grained mount for them to narrow yet). `deny` is the pure
/// blocklist from `[policy.filesystem] deny`: its two layers carry deny
/// entries only (no allow bounds), so the effective set is the union —
/// deny is monotonic, a bundle can only add. Consumed by the host-mode
/// hook guard via [`CompiledRuleset::fs_deny_decision`].
///
/// `deny` is an additive field, so no `RULESET_VERSION` bump: the only
/// cross-process consumer today is the egress sidecar, which reads the
/// egress dimension exclusively — an older sidecar ignoring an fs field it
/// never consults loses nothing. Revisit if fs rules ever cross the wire.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsRules {
    #[serde(default, skip_serializing_if = "Guard::is_empty")]
    pub read: Guard,
    #[serde(default, skip_serializing_if = "Guard::is_empty")]
    pub write: Guard,
    #[serde(default, skip_serializing_if = "Guard::is_empty")]
    pub deny: Guard,
}

impl FsRules {
    pub fn is_empty(&self) -> bool {
        self.read.is_empty() && self.write.is_empty() && self.deny.is_empty()
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
    /// Declared HTTP MCP upstream hosts a lockdown container may reach ONLY
    /// through the gateway relay, never by direct egress (D4). Each host is
    /// normalized (lowercased, trailing dot stripped); the `BTreeSet` keeps the
    /// artifact sorted + deduplicated (byte-deterministic wire form). Populated
    /// ONLY for lockdown runs — empty everywhere else, so it constrains nothing
    /// outside lockdown. Host-exact and port-agnostic.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub gateway_only_hosts: BTreeSet<String>,
}

impl Default for CompiledRuleset {
    fn default() -> Self {
        CompiledRuleset {
            version: RULESET_VERSION,
            defaults: Defaults::default(),
            servers: BTreeMap::new(),
            any: ServerRules::default(),
            filesystem: FsRules::default(),
            gateway_only_hosts: BTreeSet::new(),
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
    pub fn tool_decision(&self, server: &str, tool: &str) -> Result<(), PolicyDenial> {
        self.rules_for(server).tools.check(Dimension::Tools, tool)
    }

    /// Whether `server` may reach `host` (on `port`, if known) per the compiled
    /// `[policy.egress]`. `port` is `Some` at runtime (the proxy has the CONNECT
    /// port, so `host:port` patterns are enforced exactly) and `None` for the
    /// host-only write/spawn-time check (where the exact port is deferred to
    /// runtime — a `host:port` pattern still matches the host there).
    pub fn egress_decision(
        &self,
        server: &str,
        host: &str,
        port: Option<u16>,
    ) -> Result<(), PolicyDenial> {
        self.rules_for(server)
            .egress
            .check_with(Dimension::Egress, &|pat| egress_match(pat, host, port))
    }

    /// Whether `server` may resolve the secret named `reference` per the
    /// compiled `[policy.secrets]`. Enforced fail-closed at both substitution
    /// sites (gateway + adapter render).
    pub fn secret_decision(&self, server: &str, reference: &str) -> Result<(), PolicyDenial> {
        self.rules_for(server)
            .secrets
            .check(Dimension::Secrets, reference)
    }

    /// Whether the sandbox may mount the run's workspace read-WRITE, per the
    /// compiled `[policy.filesystem]` write scope. This is where the fs
    /// path-matching semantics live — the sandbox mount code just asks.
    ///
    /// Unlike the other dimensions this is DENY-by-default: a sandbox grants
    /// nothing the policy doesn't name, so with no write scope anywhere the
    /// workspace mounts read-only. With one, the scope must cover the
    /// workspace *root* — spelled `./` or `.` in a glob's eyes (so `"./**"`,
    /// `"./*"`, `"*"`, or a literal `"."`/`"./"` all grant it). A partial
    /// scope like `"src/**"` does NOT: the workspace is one all-or-nothing
    /// mount, and enforcement rounds DOWN (read-only), never up.
    ///
    /// The OR over the two root spellings is sound today because
    /// `compile` gives fs scopes no deny grammar (no `!` patterns, unlike the
    /// keyed dimensions) — if that ever changes, "any spelling passes" must
    /// be revisited, since a deny could match one spelling and not the other.
    pub fn workspace_write_decision(&self) -> Result<(), PolicyDenial> {
        if self.filesystem.write.is_empty() {
            // No authored rule was consulted here — this is the dimension's
            // own deny-by-default refusing, so it carries that layer rather
            // than pinning the refusal on the machine or the bundle.
            return Err(PolicyDenial {
                layer: Layer::DenyByDefault,
                denial: RuleDenial {
                    dimension: Dimension::FsWrite,
                    message: "no [policy.filesystem] write scope covers the workspace \
                         (sandbox workspace writes are deny-by-default)"
                        .to_string(),
                },
            });
        }
        let mut refusal: Option<PolicyDenial> = None;
        for subject in ["./", "."] {
            match self.filesystem.write.check(Dimension::FsWrite, subject) {
                Ok(()) => return Ok(()),
                // Keep the first refusal: `./` is the canonical spelling, so
                // its error is the one worth showing.
                Err(e) if refusal.is_none() => refusal = Some(e),
                Err(_) => {}
            }
        }
        // The loop always ran at least once over a non-empty guard, so a
        // refusal was recorded on every non-returning path.
        Err(refusal.expect("non-empty write guard yields a refusal per subject"))
    }

    /// Whether a path may be touched at all per `[policy.filesystem] deny`.
    /// The caller passes every spelling it knows for the path (relative,
    /// absolute, bare file name) and the path is denied if ANY spelling
    /// matches ANY deny glob — more spellings can only make the check
    /// stricter, the safe direction for a blocklist. Machine denies are
    /// checked first so the refusal names the layer.
    pub fn fs_deny_decision(&self, spellings: &[&str]) -> Result<(), PolicyDenial> {
        self.filesystem.deny.check_with(Dimension::FsDeny, &|pat| {
            if spellings
                .iter()
                .any(|s| agentstack_core::manifest::glob_match(pat, s))
            {
                PatternMatch::Match
            } else {
                PatternMatch::NoMatch
            }
        })
    }

    /// Whether ANY egress rule constrains `server`. Write-time checks use
    /// this to fail closed when a declared URL's host can't be statically
    /// determined (it contains a `${REF}`): an unconstrained server passes
    /// (allow-by-default), a constrained one must be verifiable.
    pub fn egress_constrained(&self, server: &str) -> bool {
        !self.rules_for(server).egress.is_empty()
    }

    /// Whether `host` is a declared MCP upstream that a lockdown container may
    /// reach only through the gateway relay — never by direct egress (D4).
    /// Evaluated BEFORE ordinary `[policy.egress]`, and it wins over any allow:
    /// this is structural confinement derived from the frozen run plan, not
    /// user-authored policy that a repo could re-open. Host-exact and
    /// port-agnostic (an upstream has no legitimate direct route on any port);
    /// the query host is normalized so a casing/trailing-dot variant can't
    /// slip the set. Always `false` when the set is empty, so this constrains
    /// nothing outside a lockdown run.
    pub fn is_gateway_only_host(&self, host: &str) -> bool {
        if self.gateway_only_hosts.is_empty() {
            return false;
        }
        self.gateway_only_hosts.contains(&normalize_host(host))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_gateway_only(hosts: &[&str]) -> CompiledRuleset {
        CompiledRuleset {
            gateway_only_hosts: hosts.iter().map(|h| h.to_string()).collect(),
            ..CompiledRuleset::default()
        }
    }

    #[test]
    fn gateway_only_is_host_exact_port_agnostic_and_normalized() {
        let rs = with_gateway_only(&["mcp.example.com"]);
        // The method takes no port, so the match is port-agnostic by
        // construction — an upstream has no legitimate direct route on any port.
        assert!(rs.is_gateway_only_host("mcp.example.com"));
        // A casing / trailing-root-dot variant must not slip the set: the query
        // host is normalized before the lookup.
        assert!(rs.is_gateway_only_host("MCP.Example.Com."));
        // A different declared host (or a general-egress host) is not fenced.
        assert!(!rs.is_gateway_only_host("api.example.com"));
    }

    #[test]
    fn empty_gateway_only_set_constrains_nothing() {
        // Sandbox / host paths carry no set — the check must be inert, never
        // block a host merely because the field exists.
        assert!(!CompiledRuleset::default().is_gateway_only_host("mcp.example.com"));
    }

    #[test]
    fn gateway_only_hosts_serialize_sorted_and_roundtrip() {
        // Byte-determinism: a BTreeSet serializes sorted, and the field
        // round-trips through the wire form the sidecar reads.
        let rs = with_gateway_only(&["b.example", "a.example", "b.example"]);
        let json = serde_json::to_string(&rs).unwrap();
        assert!(json.contains("[\"a.example\",\"b.example\"]"), "{json}");
        let back: CompiledRuleset = serde_json::from_str(&json).unwrap();
        assert_eq!(back, rs);
    }
}
