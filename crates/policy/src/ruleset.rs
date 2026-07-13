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

use agentstack_core::manifest::{egress_match, glob_match};
use serde::{Deserialize, Serialize};

/// Bump ONLY on an incompatible semantic change. Additive fields ride on
/// serde defaults without a bump.
///
/// v2: egress patterns gained an optional `:port` suffix (`host:port` scopes to
/// that port). This REINTERPRETS the grammar — a v1 consumer would read
/// `!evil.example:443` as a hostname glob and fail to deny it — so the version
/// gate must make an older enforcer fail closed rather than misread a v2
/// ruleset.
pub const RULESET_VERSION: u32 = 2;

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
    /// dimensions pass a `glob_match` closure via [`Guard::check`]; egress
    /// passes a port-aware one.
    fn check_with(&self, dimension: &str, matches: &impl Fn(&str) -> bool) -> Result<(), String> {
        for d in &self.deny {
            if matches(d) {
                return Err(format!("denied by {dimension} rule \"!{d}\""));
            }
        }
        for allows in &self.allow_all_of {
            if !allows.iter().any(|a| matches(a)) {
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
        self.check_with(dimension, &|pat| glob_match(pat, subject))
    }

    /// Machine-then-bundle check with a custom pattern matcher (egress uses it
    /// to scope by port). Machine refusals still name their layer.
    pub fn check_with(
        &self,
        dimension: &str,
        matches: &impl Fn(&str) -> bool,
    ) -> Result<(), String> {
        self.machine
            .check_with(dimension, matches)
            .map_err(|r| format!("{r} (machine policy — ~/.agentstack/agentstack.toml)"))?;
        self.bundle.check_with(dimension, matches)
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
    ) -> Result<(), String> {
        self.rules_for(server)
            .egress
            .check_with("[policy.egress]", &|pat| egress_match(pat, host, port))
    }

    /// Whether `server` may resolve the secret named `reference` per the
    /// compiled `[policy.secrets]`. Enforced fail-closed at both substitution
    /// sites (gateway + adapter render).
    pub fn secret_decision(&self, server: &str, reference: &str) -> Result<(), String> {
        self.rules_for(server)
            .secrets
            .check("[policy.secrets]", reference)
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
    pub fn workspace_write_decision(&self) -> Result<(), String> {
        if self.filesystem.write.is_empty() {
            return Err("no [policy.filesystem] write scope covers the workspace \
                 (sandbox workspace writes are deny-by-default)"
                .to_string());
        }
        let mut refusal = String::new();
        for subject in ["./", "."] {
            match self
                .filesystem
                .write
                .check("[policy.filesystem] write", subject)
            {
                Ok(()) => return Ok(()),
                // Keep the first refusal: `./` is the canonical spelling, so
                // its error is the one worth showing.
                Err(e) if refusal.is_empty() => refusal = e,
                Err(_) => {}
            }
        }
        Err(refusal)
    }

    /// Whether a path may be touched at all per `[policy.filesystem] deny`.
    /// The caller passes every spelling it knows for the path (relative,
    /// absolute, bare file name) and the path is denied if ANY spelling
    /// matches ANY deny glob — more spellings can only make the check
    /// stricter, the safe direction for a blocklist. Machine denies are
    /// checked first so the refusal names the layer.
    pub fn fs_deny_decision(&self, spellings: &[&str]) -> Result<(), String> {
        self.filesystem
            .deny
            .check_with("[policy.filesystem] deny", &|pat| {
                spellings.iter().any(|s| glob_match(pat, s))
            })
    }

    /// Whether ANY egress rule constrains `server`. Write-time checks use
    /// this to fail closed when a declared URL's host can't be statically
    /// determined (it contains a `${REF}`): an unconstrained server passes
    /// (allow-by-default), a constrained one must be verifiable.
    pub fn egress_constrained(&self, server: &str) -> bool {
        !self.rules_for(server).egress.is_empty()
    }
}
