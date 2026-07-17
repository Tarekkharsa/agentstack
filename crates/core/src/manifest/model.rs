//! Portable manifest data model (`agentstack.toml`).
//!
//! This is the single source of truth a user authors. It contains NO secret
//! literals — only `${REF}` references that are resolved per-machine at render
//! time. See [`crate::manifest::load`] for the layered load + overlay merge.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// Top-level manifest, deserialized from `agentstack.toml`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Manifest {
    /// Schema version. Currently always `1`.
    pub version: u32,

    #[serde(default, skip_serializing_if = "Meta::is_empty")]
    pub meta: Meta,

    /// MCP servers, keyed by the name used everywhere else (profiles, configs).
    #[serde(default)]
    pub servers: IndexMap<String, Server>,

    /// Skills (portable `SKILL.md` directories), keyed by name.
    #[serde(default)]
    pub skills: IndexMap<String, Skill>,

    /// Named bundles for selective loading.
    #[serde(default)]
    pub profiles: IndexMap<String, Profile>,

    /// Portable instruction fragments compiled into each harness's CLAUDE.md /
    /// AGENTS.md.
    #[serde(default)]
    pub instructions: IndexMap<String, Instruction>,

    /// Native per-CLI settings (permissions, feature flags). Keyed by adapter
    /// id (e.g. `claude-code`); each value is an object merged non-destructively
    /// into that CLI's settings file.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub settings: IndexMap<String, serde_json::Value>,

    /// Lifecycle hooks compiled into each hook-capable harness's native config.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub hooks: IndexMap<String, Hook>,

    /// Native harness extensions (executable add-on code, e.g. pi's `.ts`
    /// extensions), keyed by name. Each targets exactly ONE adapter and is
    /// pinned strictly in the lock (D6; docs/design/extensions-capability.md).
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub extensions: IndexMap<String, Extension>,

    /// Shareable plugin recipes compiled into native Claude Code / Codex plugin
    /// packages and repo marketplaces.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub plugins: IndexMap<String, PluginRecipe>,

    /// Where `apply` writes by default and which adapters are in play.
    #[serde(default)]
    pub targets: Targets,

    /// Optional governance: required/forbidden capabilities + source allowlist.
    #[serde(default, skip_serializing_if = "Policy::is_empty")]
    pub policy: Policy,

    /// Host-mode hook guard (`agentstack guard`) configuration. Only the
    /// MACHINE manifest's `[guard]` is consulted — a repo manifest can never
    /// enable, disable, or widen the guard (same trust posture as `[policy]`,
    /// but guard-specific knobs are host concerns, so they live outside it).
    #[serde(default, skip_serializing_if = "GuardConfig::is_empty")]
    pub guard: GuardConfig,

    /// Machine-owned experimental runtime features. Repository manifests are
    /// parsed for portability but never consulted as authority for these flags.
    #[serde(default, skip_serializing_if = "ExperimentalConfig::is_empty")]
    pub experimental: ExperimentalConfig,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ExperimentalConfig {
    /// Advertise and permit the isolated `tools_execute` MCP primitive.
    #[serde(default)]
    pub tools_execute: bool,
    /// Optional machine-owned ceilings. Requests may only narrow these.
    #[serde(default, skip_serializing_if = "ExperimentalExecuteLimits::is_empty")]
    pub tools_execute_limits: ExperimentalExecuteLimits,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ExperimentalExecuteLimits {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_calls: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_bytes: Option<usize>,
}

impl ExperimentalExecuteLimits {
    pub fn is_empty(&self) -> bool {
        self.timeout_ms.is_none() && self.max_calls.is_none() && self.max_output_bytes.is_none()
    }
}

impl ExperimentalConfig {
    pub fn is_empty(&self) -> bool {
        !self.tools_execute && self.tools_execute_limits.is_empty()
    }
}

/// `[guard]` — the machine-level destructive-command guard wired into each
/// agent CLI as a pre-tool-use hook. Enforcement is cooperative (the CLI must
/// honor its own hook protocol), aimed at accidents, not malice — the
/// kernel-enforced story is `run --sandbox`/`--lockdown`.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GuardConfig {
    /// Master switch. `None` (absent) means the guard is not configured;
    /// `Some(false)` is an explicit opt-out that also prunes installed hooks
    /// on the next `guard install`/`apply`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Absolute path prefixes (after `~` expansion) where writes and deletes
    /// are allowed BEYOND the current workspace — e.g. a projects folder.
    /// The workspace itself and the system temp dirs are always writable;
    /// everything else is deny-by-default for writes/deletes.
    #[serde(default)]
    pub allow_roots: Vec<String>,
}

impl GuardConfig {
    pub fn is_empty(&self) -> bool {
        self.enabled.is_none() && self.allow_roots.is_empty()
    }

    pub fn enabled(&self) -> bool {
        self.enabled == Some(true)
    }
}

/// Team/org governance (PLAN §9e, D18). Off by default; enforced by `doctor`.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct Policy {
    /// Capability names that must be present in the manifest.
    #[serde(default)]
    pub require: Vec<String>,
    /// Capability names that must NOT be present.
    #[serde(default)]
    pub forbid: Vec<String>,
    /// Glob patterns a capability's source must match (e.g.
    /// `git:github.com/acme/*`, `registry:*`, `path:*`). Empty = allow any.
    #[serde(default)]
    pub allowed_sources: Vec<String>,
    /// Per-server tool rules, enforced at the runtime gateway (the MCP
    /// firewall). `[policy.tools]` maps a server name to glob patterns over its
    /// tool names: plain patterns allow, `!`-prefixed patterns deny. With at
    /// least one allow pattern the list is an allowlist (a tool must match an
    /// allow and no deny); with only deny patterns everything else is allowed.
    /// A denied tool is invisible to discovery and refused if called.
    #[serde(default)]
    pub tools: IndexMap<String, Vec<String>>,
    /// Per-server outbound host rules, same grammar as `tools` (globs over
    /// hostnames, `!` denies, `"*"` key is rename-proof), with an optional
    /// `:port` suffix: `api.example.com:443` scopes to that port, a bare host
    /// means any port. The write/spawn-time check matches the host and defers
    /// the port; the Phase-2 proxy enforces the exact CONNECT port.
    #[serde(default)]
    pub egress: IndexMap<String, Vec<String>>,
    /// Per-server secret access, same grammar (globs over `${REF}` names).
    /// Enforced fail-closed at both substitution sites: a ref outside a
    /// server's effective set never resolves for it — not into a rendered
    /// config, not into a gateway upstream.
    #[serde(default)]
    pub secrets: IndexMap<String, Vec<String>>,
    /// Bundle-global filesystem scopes (path globs). The `write` scope is
    /// enforced in sandbox mode — the workspace mounts read-only unless the
    /// effective scope covers it (deny-by-default; see
    /// `CompiledRuleset::workspace_write_decision` in the policy crate).
    /// `read` scopes are informational, and host mode enforces neither.
    #[serde(default, skip_serializing_if = "FsPolicy::is_empty")]
    pub filesystem: FsPolicy,
}

/// `[policy.filesystem]` — read/write path-glob scopes. New table, so typos
/// are rejected outright (`deny_unknown_fields`) — unlike the long-shipped
/// `Policy` fields, there is no existing config to stay lenient for.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FsPolicy {
    #[serde(default)]
    pub read: Vec<String>,
    #[serde(default)]
    pub write: Vec<String>,
    /// Path globs no tool call may touch at all — read or write. Unlike
    /// `read`/`write` (allow scopes), `deny` is a pure blocklist and the
    /// effective set is the UNION of the machine and bundle layers: a repo
    /// can add denies but never remove the machine's. Patterns are matched
    /// against the workspace-relative path, the absolute path, AND the bare
    /// file name, so `".env*"` catches a `.env` anywhere in the tree.
    /// Enforced by the host-mode hook guard (`agentstack guard`); the
    /// sandbox mask-mount enforcement is tracked for a later session.
    #[serde(default)]
    pub deny: Vec<String>,
}

impl FsPolicy {
    pub fn is_empty(&self) -> bool {
        self.read.is_empty() && self.write.is_empty() && self.deny.is_empty()
    }
}

/// Which policy dimension a rule belongs to. Typed so a refusal carries its
/// dimension as data, not as a substring of its message — the `Display` form
/// is the exact `[policy.…]` spelling users write in the manifest, so
/// rendered errors read the same as before this type existed.
///
/// (For the TS-minded: this enum + `match` is a discriminated union with an
/// exhaustive switch — adding a dimension is a compile error at every
/// `match`, unlike a new magic string.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dimension {
    Tools,
    Egress,
    Secrets,
    FsRead,
    FsWrite,
    FsDeny,
}

impl Dimension {
    pub fn as_str(self) -> &'static str {
        match self {
            Dimension::Tools => "[policy.tools]",
            Dimension::Egress => "[policy.egress]",
            Dimension::Secrets => "[policy.secrets]",
            Dimension::FsRead => "[policy.filesystem] read",
            Dimension::FsWrite => "[policy.filesystem] write",
            Dimension::FsDeny => "[policy.filesystem] deny",
        }
    }
}

impl std::fmt::Display for Dimension {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One policy refusal, layer-agnostic: which dimension refused and the
/// rendered rule text. core checks a SINGLE policy value and cannot know
/// whether it was the machine's or a bundle's — the composing engine in the
/// `policy` crate wraps this with the layer (`PolicyDenial` there). The
/// `message` already reads as a complete sentence (it embeds the dimension
/// and the pattern), so `Display` is just the message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleDenial {
    pub dimension: Dimension,
    pub message: String,
}

impl std::fmt::Display for RuleDenial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for RuleDenial {}

impl Policy {
    pub fn is_empty(&self) -> bool {
        self.require.is_empty()
            && self.forbid.is_empty()
            && self.allowed_sources.is_empty()
            && self.tools.is_empty()
            && self.egress.is_empty()
            && self.secrets.is_empty()
            && self.filesystem.is_empty()
    }

    /// Whether `source` is allowed (any source allowed when the list is empty).
    pub fn source_allowed(&self, source: &str) -> bool {
        self.allowed_sources.is_empty()
            || self.allowed_sources.iter().any(|p| glob_match(p, source))
    }

    /// Whether `server`'s `tool` passes `[policy.tools]`. `Ok(())` when allowed;
    /// `Err(rule)` names the pattern (or the allowlist) that blocks it.
    ///
    /// Rules under the exact server name AND under the `"*"` wildcard key both
    /// apply. Named rules are keyed on the manifest-chosen server name — which
    /// the repo controls and can rename — so `"*"` is how a machine-level rule
    /// is written rename-proof: it constrains every server, whatever a
    /// manifest calls it.
    pub fn tool_allowed(&self, server: &str, tool: &str) -> Result<(), RuleDenial> {
        map_allowed(&self.tools, Dimension::Tools, server, |pat| {
            glob_to_match(pat, tool)
        })
    }

    /// Whether `server` may reach `host` per `[policy.egress]` — the same
    /// keyed grammar and `"*"` rename-proofing as `tool_allowed`. Host-only
    /// (no query port), so a `host:port` pattern is treated as matching here
    /// and the exact port is enforced at runtime (the proxy calls the compiled
    /// ruleset with the real port).
    pub fn egress_allowed(&self, server: &str, host: &str) -> Result<(), RuleDenial> {
        map_allowed(&self.egress, Dimension::Egress, server, |pat| {
            egress_match(pat, host, None)
        })
    }

    /// Whether `server` may resolve the secret named `reference` per
    /// `[policy.secrets]` — same keyed grammar and `"*"` rename-proofing.
    pub fn secret_allowed(&self, server: &str, reference: &str) -> Result<(), RuleDenial> {
        map_allowed(&self.secrets, Dimension::Secrets, server, |pat| {
            glob_to_match(pat, reference)
        })
    }
}

/// The one keyed matcher behind every per-server policy dimension. Rules
/// under the exact server name AND under the `"*"` wildcard key both apply.
/// Named rules are keyed on the manifest-chosen server name — which the repo
/// controls and can rename — so `"*"` is how a machine-level rule is written
/// rename-proof: it constrains every server, whatever a manifest calls it.
/// Grammar: plain globs allow, `!`-prefixed globs deny; a key with at least
/// one allow pattern is an allowlist; an absent key constrains nothing
/// (uniform allow-by-default — least privilege is an explicit `"*" = ["!*"]`).
/// Adapter for the glob dimensions, whose grammar has no malformed form —
/// any string is a valid glob, so the outcome is only ever Match/NoMatch.
pub fn glob_to_match(pattern: &str, subject: &str) -> PatternMatch {
    if glob_match(pattern, subject) {
        PatternMatch::Match
    } else {
        PatternMatch::NoMatch
    }
}

/// The fail-closed denial for a pattern the grammar can't interpret. A
/// malformed pattern denies the WHOLE decision — even when it sits in an
/// allowlist another entry of which matches — because a half-working rule
/// set is exactly when quiet misinterpretation is most dangerous; the
/// message names the pattern so the fix is one edit away.
fn malformed_denial(dimension: Dimension, key: &str, pattern: &str) -> RuleDenial {
    RuleDenial {
        dimension,
        message: format!(
            "malformed {dimension} pattern for {key}: \"{pattern}\" — failing closed; \
             fix the pattern in the manifest"
        ),
    }
}

fn map_allowed(
    map: &IndexMap<String, Vec<String>>,
    dimension: Dimension,
    server: &str,
    // Given a pattern (with its leading `!` already stripped for denies),
    // how it matches the subject the closure captured. Dimensions differ
    // only here: tools/secrets glob-match a bare name; egress scopes by port
    // (and is the one grammar that can report `Malformed`).
    matches: impl Fn(&str) -> PatternMatch,
) -> Result<(), RuleDenial> {
    let keys: &[&str] = if server == "*" {
        &["*"]
    } else {
        &[server, "*"]
    };
    for key in keys {
        let Some(rules) = map.get(*key) else {
            continue;
        };
        for r in rules {
            if let Some(deny) = r.strip_prefix('!') {
                match matches(deny) {
                    PatternMatch::Match => {
                        return Err(RuleDenial {
                            dimension,
                            message: format!("denied by {dimension} {key} = \"!{deny}\""),
                        });
                    }
                    // A deny the grammar can't read must never be inert.
                    PatternMatch::Malformed => {
                        return Err(malformed_denial(dimension, key, r));
                    }
                    PatternMatch::NoMatch => {}
                }
            }
        }
        let allows: Vec<&String> = rules.iter().filter(|r| !r.starts_with('!')).collect();
        if !allows.is_empty() {
            let mut hit = false;
            for a in &allows {
                match matches(a) {
                    PatternMatch::Match => hit = true,
                    PatternMatch::Malformed => {
                        return Err(malformed_denial(dimension, key, a));
                    }
                    PatternMatch::NoMatch => {}
                }
            }
            if !hit {
                return Err(RuleDenial {
                    dimension,
                    message: format!(
                        "not in the {dimension} allowlist for {key} ({})",
                        allows
                            .iter()
                            .map(|s| s.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                });
            }
        }
    }
    Ok(())
}

/// The outcome of matching one policy pattern against a subject. Three-state
/// because "the pattern itself is broken" must be distinguishable from "the
/// pattern didn't match": a malformed DENY that merely reads as no-match is
/// an inert deny — a fail-open. Checks treat `Malformed` as an error (deny
/// the whole decision, naming the pattern); manifest validation rejects the
/// pattern at authoring time so runs never see one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatternMatch {
    Match,
    NoMatch,
    /// The pattern cannot be interpreted (today: only egress `host:port`
    /// grammar can be malformed — a bad bracket form or an invalid port).
    Malformed,
}

/// Egress pattern match: the pattern is `host` (any port) or `host:port` (that
/// exact port), matched against `host` and an optional query `port`. Host uses
/// the same [`glob_match`] as every other dimension; the port, when the pattern
/// pins one, must equal the query's. A `None` query port (a host-only, e.g.
/// write-time, check) defers the port test to runtime and matches. Bracketed
/// IPv6 (`[::1]:443`) is understood; a bare `::1` (multiple colons, unbracketed)
/// is treated as a host with no port so its own colons aren't read as a port.
///
/// A pattern the grammar can't interpret returns [`PatternMatch::Malformed`]
/// so the caller fails CLOSED. (Before 2026-07-16 a malformed pattern was
/// treated as a host glob over its own junk text and matched nothing — which
/// made a malformed deny like `!evil.example:443junk` silently inert.)
pub fn egress_match(pattern: &str, host: &str, port: Option<u16>) -> PatternMatch {
    let Some((pat_host, pat_port)) = split_pattern_port(pattern) else {
        return PatternMatch::Malformed;
    };
    if !glob_match(pat_host, host) {
        return PatternMatch::NoMatch;
    }
    let hit = match (pat_port, port) {
        (None, _) => true,            // pattern pins no port → any port
        (Some(_), None) => true,      // no query port → port enforced at runtime
        (Some(p), Some(q)) => p == q, // pinned port must match exactly
    };
    if hit {
        PatternMatch::Match
    } else {
        PatternMatch::NoMatch
    }
}

/// Canonicalize a hostname for policy matching and gateway-only
/// classification: strip a single trailing `.` (the DNS root label —
/// `a.example.` and `a.example` are the same host) and ASCII-lowercase it (DNS
/// is case-insensitive). One shared implementation so the CLI that builds the
/// gateway-only host set, the egress proxy that parses a CONNECT target, and
/// the ruleset lookup all normalize identically — a mismatch between producer
/// and enforcer would be a fence bypass, not just a cosmetic difference.
pub fn normalize_host(host: &str) -> String {
    host.strip_suffix('.').unwrap_or(host).to_ascii_lowercase()
}

/// Extract the host from an MCP server URL. `None` when the host can't be
/// determined statically OR can't be trusted to stay fixed — a scheme that
/// isn't HTTP(S), no host segment, an unresolved `${REF}` anywhere in the
/// authority, or a non-canonical host spelling (non-ASCII / percent-encoded).
/// The returned host is RAW-but-ASCII (not normalized); callers normalize with
/// [`normalize_host`] where they need canonical form.
///
/// This is the ONE host extractor shared by every producer of a host from a
/// declared URL — the write-time egress check in the CLI (`declared_host`) and
/// the D4 gateway-only fence classifier both call it, so a URL can never be
/// read two different ways. Deliberately small: MCP URLs are
/// `scheme://host[:port]/path`; no URL crate, no surprises.
///
/// Two fail-closed rules matter for the security fence and are intentional:
/// - **No `${REF}` anywhere in the authority.** A placeholder in userinfo or the
///   port — not just the host label — can resolve to a value carrying `@` and
///   re-parse the authority to a DIFFERENT host after the fence is frozen. The
///   host is only statically known when the whole authority is literal.
/// - **Canonical ASCII host only.** `normalize_host` lowercases ASCII and strips
///   a trailing dot but does NOT percent-decode or IDNA-canonicalize, while the
///   resolver/HTTP client that dispatches the request does. A non-ASCII or
///   percent-encoded host would be fenced under one spelling and reached under
///   another. Reject it so a lockdown run fails closed rather than fences a host
///   the agent can still reach by its canonical name.
pub fn host_from_url(url: &str) -> Option<String> {
    // Require a valid HTTP(S) URL shape (scheme case-insensitive). A scheme-less
    // or other-scheme string is NOT a bare host — reject it rather than
    // classify garbage. A `${REF}` before the authority delimiter can't sneak a
    // scheme past this because a placeholder never spells `http(s)://`.
    let lower = url.to_ascii_lowercase();
    let rest = if lower.starts_with("https://") {
        &url["https://".len()..]
    } else if lower.starts_with("http://") {
        &url["http://".len()..]
    } else {
        return None;
    };
    // Authority is everything before the first path/query/fragment delimiter.
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return None;
    }
    // A `${REF}` ANYWHERE in the authority (userinfo, host, or port) is
    // unclassifiable: its resolved value could re-target the effective host
    // (e.g. a port ref resolving to `443@evil.example`). Fail closed on the
    // whole authority, not just the host label. A ref in a path/query — already
    // stripped above — can't change the host, so it's fine.
    if authority.contains("${") {
        return None;
    }
    // Drop any userinfo (`user:pass@host`) now that we know it holds no ref.
    let authority = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = if let Some(inner) = authority.strip_prefix('[') {
        // Bracketed IPv6 literal `[::1]:443` → `::1`.
        inner.split_once(']').map(|(h, _)| h.to_string())?
    } else {
        // `host` or `host:port` → `host`.
        authority.split(':').next().unwrap_or(authority).to_string()
    };
    // The host must be canonical ASCII with no percent-encoding — see the
    // doc comment. `normalize_host` can't reconcile these spellings, so a
    // non-canonical host is a fence seam; reject it (fail closed).
    if host.is_empty() || !host.is_ascii() || host.contains('%') {
        None
    } else {
        Some(host)
    }
}

/// Whether an egress pattern (leading `!` allowed) is malformed — the
/// authoring-time probe behind manifest validation, so a typo'd port is
/// rejected at `apply`/`doctor` instead of surfacing as a fail-closed denial
/// at run time. Same grammar as [`egress_match`], by construction.
pub fn egress_pattern_is_malformed(pattern: &str) -> bool {
    let body = pattern.strip_prefix('!').unwrap_or(pattern);
    split_pattern_port(body).is_none()
}

/// Parse an egress pattern into its host part and an optional pinned port.
/// `host:443` → (`host`, Some(443)); `host` or `host:*` → (`host`, None);
/// `[::1]:443` → (`::1`, Some(443)); a bare `::1` → (`::1`, None).
///
/// `None` means the pattern is MALFORMED — the grammar cannot interpret it:
/// an unclosed `[`, a non-`:port` suffix after `]`, a single-colon suffix
/// that isn't a valid nonzero port or `*` (hostnames cannot contain `:`, so
/// `host:junk` can only be a typo'd port pin, never a legitimate host glob),
/// or port `0`. Callers must fail closed on `None` — the pre-2026-07-16
/// behavior of degrading a malformed pattern to a host glob over its own
/// text made malformed denies silently inert.
fn split_pattern_port(pattern: &str) -> Option<(&str, Option<u16>)> {
    if let Some(rest) = pattern.strip_prefix('[') {
        // Bracketed IPv6 literal: `[addr]` or `[addr]:port`.
        let (addr, after) = rest.split_once(']')?; // unclosed `[` → malformed
        if after.is_empty() {
            return Some((addr, None)); // `[addr]` → any port
        }
        let p = after.strip_prefix(':')?; // `[addr]garbage` → malformed
        if p == "*" {
            return Some((addr, None)); // explicit any-port
        }
        return match p.parse::<u16>() {
            Ok(port) if port != 0 => Some((addr, Some(port))),
            _ => None, // `[addr]:junk`, `[addr]:`, `[addr]:0` → malformed
        };
    }
    // Only read a trailing `:port` when there's a SINGLE colon — otherwise a
    // bare IPv6 literal's own colons would be misread as a port.
    if let Some((h, p)) = pattern.rsplit_once(':') {
        if !h.contains(':') {
            if p == "*" {
                return Some((h, None)); // explicit any-port
            }
            return match p.parse::<u16>() {
                Ok(port) if port != 0 => Some((h, Some(port))),
                _ => None, // `host:junk`, `host:`, `host:0` → malformed
            };
        }
    }
    Some((pattern, None))
}

/// Minimal glob: `*` matches any run of characters (including empty). No `?`.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let mut pos = 0;
    // First segment must be a prefix.
    if !text[pos..].starts_with(parts[0]) {
        return false;
    }
    pos += parts[0].len();
    // Middle segments must appear in order.
    for seg in &parts[1..parts.len() - 1] {
        match text[pos..].find(seg) {
            Some(i) => pos += i + seg.len(),
            None => return false,
        }
    }
    // Last segment must be a suffix of the remainder.
    let last = parts[parts.len() - 1];
    text[pos..].len() >= last.len() && text.ends_with(last)
}

/// One instruction fragment: a markdown file applied to some/all harnesses.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Instruction {
    pub path: String,
    /// Adapter ids this fragment applies to; `["*"]` (the default) = all.
    #[serde(default = "all_targets")]
    pub targets: Vec<String>,
    /// True when this fragment was inherited from the machine-level manifest
    /// (`~/.agentstack/agentstack.toml`, see [`super::merge_user_layer`])
    /// rather than declared by this project or its local overlay. Inherited
    /// fragments compile at GLOBAL scope only — personal rules never land in
    /// a repo's committed project-scope CLAUDE.md / AGENTS.md. Load-time
    /// provenance; never (de)serialized.
    #[serde(skip)]
    pub from_user_layer: bool,
}

pub fn all_targets() -> Vec<String> {
    vec!["*".to_string()]
}

/// Serialization guard: the `["*"]` default stays implicit so existing
/// manifests and freshly added servers don't grow a `targets` line.
fn is_all_targets(targets: &[String]) -> bool {
    targets.len() == 1 && targets[0] == "*"
}

/// One lifecycle hook: run `command` on a harness `event` (optionally filtered
/// by `matcher`). Compiled into each hook-capable harness's native hooks config.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Hook {
    /// Lifecycle event, e.g. `PreToolUse`, `PostToolUse`, `SessionStart`.
    pub event: String,
    /// Tool/agent/notification filter for events that support it (optional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matcher: Option<String>,
    /// Command to run.
    pub command: String,
    /// Extra command arguments (optional).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Timeout in seconds (optional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
    /// Adapter ids this hook applies to; `["*"]` (the default) = all hook-capable.
    #[serde(default = "all_targets")]
    pub targets: Vec<String>,
}

/// One AgentStack-managed plugin recipe. This is the portable source of truth;
/// `agentstack plugins sync` renders it into each harness's native plugin
/// package/marketplace shape.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct PluginRecipe {
    pub version: String,
    pub description: String,
    /// Recipe role. `Some("pack")` marks an *install ledger* written by
    /// `agentstack add <pack>`: it records every member so `remove` can undo the
    /// install, but it is NOT a publishable plugin and is skipped by
    /// `plugins sync`/`doctor`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// Adapter ids this recipe should render for; `["*"]` = every supported
    /// plugin-capable adapter.
    #[serde(default = "all_targets")]
    pub targets: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub servers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hooks: Vec<String>,
    /// Instruction-fragment member names (used by pack ledgers).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub instructions: Vec<String>,
    /// Where this recipe was resolved from (`catalog:<id>` or
    /// `git:<url>@<tag>[#subdir]`); recorded by pack ledgers, parsed by
    /// `upgrade` to re-resolve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// The commit a git pack's tag resolved to at install time (provenance;
    /// content digests live in the lock via each extracted skill).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
}

impl Instruction {
    /// Whether this fragment applies to adapter `id`.
    pub fn applies_to(&self, id: &str) -> bool {
        self.targets.iter().any(|t| t == "*" || t == id)
    }

    /// Whether this fragment compiles into `target_id`'s instruction file at
    /// `scope`. Machine-layer fragments ([`Self::from_user_layer`]) are
    /// personal: they compile at global scope only, never into a repo's
    /// committed project-scope CLAUDE.md / AGENTS.md. The single predicate the
    /// compile ([`crate::render::instructions::plan_instructions`]) filters on,
    /// so no other site can drift from it.
    pub fn compiles_at(&self, target_id: &str, scope: crate::scope::Scope) -> bool {
        self.applies_to(target_id)
            && !(self.from_user_layer && scope == crate::scope::Scope::Project)
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct Meta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Meta {
    pub fn is_empty(&self) -> bool {
        self.name.is_none()
    }
}

/// Transport kind for an MCP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum ServerType {
    Http,
    Stdio,
}

/// A single MCP server definition (transport-neutral).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Server {
    #[serde(rename = "type")]
    pub server_type: ServerType,

    // Scalars and arrays first, then map/subtable fields last, so the struct
    // serializes to valid TOML (a key after a `[subtable]` header would be
    // captured by that subtable).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Working directory a stdio server is launched from. Some servers only
    /// start correctly when spawned from their own directory (e.g. relative
    /// dynamic `import()`s that resolve against `process.cwd()`); a harness
    /// otherwise inherits its own project cwd. Rendered to each adapter's
    /// native working-directory key where one exists; adapters whose config
    /// format has no such key drop it (the server may still need a shell
    /// wrapper there). Supports `${REF}`/path expansion like other fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// D3 integrity roots (locked-run contract §8): repository-relative files
    /// or directory subtrees whose **content** this stdio server's interpreted
    /// payload depends on (transitive imports, sourced files, required
    /// modules). `agentstack lock` pins each declared root by a symlink-free
    /// content digest, and a one-byte change anywhere inside a root re-gates
    /// trust. Roots are declared, not inferred — pinning only the entry script
    /// would leave `import`ed/`source`d files unbound. Paths must stay inside
    /// the project (traversal, absolute paths, and symlinks are hard errors at
    /// pin/verify time). Empty for http servers and for stdio servers whose
    /// command is an external `$PATH` binary with no local payload.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub integrity_roots: Vec<String>,
    /// Adapter ids `apply` renders this server to; `["*"]` (the default) = all
    /// targets. An explicit empty list opts the server out of the direct
    /// `[servers]` fan-out entirely — how adopted plugin servers are stored:
    /// the native plugin already provides the server on its own harness and
    /// the generated plugin package carries it anywhere else it's installed,
    /// so a direct render would configure the same server twice.
    #[serde(default = "all_targets", skip_serializing_if = "is_all_targets")]
    pub targets: Vec<String>,
    /// Adapter id whose live config is the source of truth for this server
    /// (`owner = "codex"`). Some harness apps rewrite their own server entries
    /// (e.g. the Codex desktop app refreshes env values on every self-update);
    /// without this, the manifest goes stale and a blind `apply --write` would
    /// downgrade the app's fresh values. With an owner set, every plan
    /// (apply/diff/doctor) refreshes the definition from the owner's on-disk
    /// config before rendering, so drift on the owner's config never proposes
    /// a revert — it proposes refreshing the manifest and re-fanning the fresh
    /// values out to the other targets. Keys whose manifest value carries a
    /// `${REF}` stay manifest-canonical (secret hygiene); everything else
    /// follows the owner's disk, including keys the owner adds or removes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub headers: IndexMap<String, String>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub env: IndexMap<String, String>,
    /// Target-specific keys with no transport-neutral equivalent, keyed by
    /// adapter id then native field name:
    ///
    /// ```toml
    /// [servers.miro.extra.codex]
    /// startup_timeout_sec = 20
    /// ```
    ///
    /// That adapter's renderer passes them through verbatim (string values
    /// still get `${REF}` substitution) and `init`/`adopt` lift unknown config
    /// keys back into here, so hand-tuned target keys round-trip instead of
    /// being dropped on the next `apply`.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub extra: IndexMap<String, IndexMap<String, serde_json::Value>>,
}

impl Server {
    /// Whether `apply` renders this server to adapter `id` (same contract as
    /// [`Instruction::applies_to`]; an empty list applies nowhere).
    pub fn applies_to(&self, id: &str) -> bool {
        self.targets.iter().any(|t| t == "*" || t == id)
    }
}

/// A skill: a portable directory containing a `SKILL.md`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Skill {
    /// Local path source (relative to the manifest, or absolute).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Git source URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<String>,
    /// Pinned git revision (branch, tag, or commit). Latest if absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    /// For git sources: the skill's directory within the repo (the common
    /// marketplace/monorepo layout, where `SKILL.md` lives in a subdir rather
    /// than at the repo root). Ignored for path sources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subpath: Option<String>,
}

/// Where a skill's content comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillSource {
    Path(String),
    Git {
        url: String,
        rev: Option<String>,
        /// Directory within the repo holding the skill (`None` = repo root).
        subpath: Option<String>,
    },
}

impl Skill {
    /// Resolve which source this skill declares (git wins if both present).
    pub fn source(&self) -> anyhow::Result<SkillSource> {
        if let Some(url) = &self.git {
            Ok(SkillSource::Git {
                url: url.clone(),
                rev: self.rev.clone(),
                subpath: self.subpath.clone(),
            })
        } else if let Some(path) = &self.path {
            Ok(SkillSource::Path(path.clone()))
        } else {
            anyhow::bail!("skill has neither `path` nor `git` source")
        }
    }
}

/// A native harness extension: executable add-on code (pi's `.ts` extensions,
/// OpenCode's `.js` plugins) rendered into one harness's native extension
/// directory. The highest-risk capability kind agentstack manages: the code
/// executes INSIDE the harness process with full user permissions, outside
/// the policy ceiling — agentstack pins and delivers the bytes, it never runs
/// or governs them at runtime (D6; docs/design/extensions-capability.md).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Extension {
    /// Local path source, relative to the manifest dir (same anchoring as
    /// skills and instructions — a `.agentstack/` layout keeps extension
    /// sources under `.agentstack/`). Pinned by the strict integrity-root
    /// digest, which rejects traversal and symlinks outright.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Git source URL, fetched through the shared store. A git extension is
    /// always digested at its `subpath` anchored at the checkout root (a
    /// clone's `.git` can never be part of a reproducible pin), so validation
    /// requires `subpath` alongside `git`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subpath: Option<String>,
    /// The ONE adapter id this extension's code is written against (`pi`,
    /// `opencode`, …). Deliberately singular, unlike `targets` lists
    /// elsewhere: extension code is harness-specific by nature, so there is
    /// no `"*"` and no fan-out.
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A profile selects a subset of servers and skills.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct Profile {
    #[serde(default)]
    pub servers: Vec<String>,
    /// May contain the wildcard `"*"` meaning "all skills".
    #[serde(default)]
    pub skills: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct Targets {
    /// Adapter ids `apply` writes to when `--target` is not given.
    #[serde(default)]
    pub default: Vec<String>,
}

impl Profile {
    /// Whether this profile loads every skill in the manifest.
    pub fn loads_all_skills(&self) -> bool {
        self.skills.iter().any(|s| s == "*")
    }
}

impl Server {
    /// Every `${REF}` secret name THIS server references — across url, command,
    /// args, cwd, headers, env, and nested adapter `extra` string fields —
    /// de-duplicated in first-seen order. Pure syntax: no value resolution.
    /// Shared by [`Manifest::referenced_secrets`], `doctor`, and grant secret
    /// validation so every consumer covers the identical field set (including
    /// `extra`, which an earlier per-server copy in `doctor` omitted).
    pub fn referenced_secrets(&self) -> Vec<String> {
        let mut refs: Vec<String> = Vec::new();
        let mut push = |s: &str| {
            for r in crate::refs::refs_in(s) {
                if !refs.contains(&r) {
                    refs.push(r);
                }
            }
        };
        if let Some(u) = &self.url {
            push(u);
        }
        if let Some(c) = &self.command {
            push(c);
        }
        for a in &self.args {
            push(a);
        }
        if let Some(cwd) = &self.cwd {
            push(cwd);
        }
        for v in self.headers.values() {
            push(v);
        }
        for v in self.env.values() {
            push(v);
        }
        for fields in self.extra.values() {
            for v in fields.values() {
                for s in json_strings(v) {
                    push(s);
                }
            }
        }
        refs
    }
}

impl Manifest {
    /// Every `${REF}` secret name referenced by any server (see
    /// [`Server::referenced_secrets`]) or hook, de-duplicated and sorted. Used
    /// by `secret list` and `doctor`.
    pub fn referenced_secrets(&self) -> Vec<String> {
        let mut refs: Vec<String> = Vec::new();
        for server in self.servers.values() {
            for r in server.referenced_secrets() {
                if !refs.contains(&r) {
                    refs.push(r);
                }
            }
        }
        let mut push = |s: &str| {
            for r in crate::refs::refs_in(s) {
                if !refs.contains(&r) {
                    refs.push(r);
                }
            }
        };
        for hook in self.hooks.values() {
            push(&hook.command);
            for a in &hook.args {
                push(a);
            }
        }
        refs.sort();
        refs
    }
}

/// Every string leaf in a JSON value, depth-first (extras may nest).
fn json_strings(v: &serde_json::Value) -> Vec<&str> {
    match v {
        serde_json::Value::String(s) => vec![s.as_str()],
        serde_json::Value::Array(a) => a.iter().flat_map(json_strings).collect(),
        serde_json::Value::Object(o) => o.values().flat_map(json_strings).collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_policy_allow_and_deny() {
        let p: Policy = toml::from_str(
            "tools = { github = [\"get_*\", \"list_*\", \"!list_secrets\"], jira = [\"!delete_*\"] }",
        )
        .unwrap();
        // Allowlist: must match an allow and no deny.
        assert!(p.tool_allowed("github", "get_issue").is_ok());
        assert!(p.tool_allowed("github", "list_repos").is_ok());
        assert!(p.tool_allowed("github", "create_issue").is_err());
        assert!(p.tool_allowed("github", "list_secrets").is_err());
        // Deny-only: everything else passes.
        assert!(p.tool_allowed("jira", "get_issue").is_ok());
        assert!(p.tool_allowed("jira", "delete_issue").is_err());
        // No rules for a server → unrestricted.
        assert!(p.tool_allowed("other", "anything").is_ok());
        // The refusal names the rule.
        let err = p.tool_allowed("jira", "delete_issue").unwrap_err();
        assert_eq!(err.dimension, Dimension::Tools);
        assert!(err.message.contains("!delete_*"), "{err}");
    }

    /// Bool view for the match-only assertions below; the three-state
    /// verdict is asserted directly where malformed-ness is the point.
    fn matches(pattern: &str, host: &str, port: Option<u16>) -> bool {
        egress_match(pattern, host, port) == PatternMatch::Match
    }

    #[test]
    fn egress_match_scopes_by_port() {
        // Bare host pattern matches any port.
        assert!(matches("api.example.com", "api.example.com", Some(443)));
        assert!(matches("api.example.com", "api.example.com", Some(22)));
        // host:port pins the exact port.
        assert!(matches("api.example.com:443", "api.example.com", Some(443)));
        assert!(!matches("api.example.com:443", "api.example.com", Some(22)));
        // Host-only query (no port) defers the port test → matches.
        assert!(matches("api.example.com:443", "api.example.com", None));
        // Glob host still applies, with a pinned port.
        assert!(matches("*.example.com:443", "api.example.com", Some(443)));
        assert!(!matches("*.example.com:443", "api.example.com", Some(80)));
        // Explicit any-port.
        assert!(matches("api.example.com:*", "api.example.com", Some(9999)));
        // Host mismatch is a mismatch regardless of port.
        assert!(!matches("api.example.com:443", "evil.example", Some(443)));
    }

    /// The fail-closed witness for malformed patterns (CLAUDE.md rule 2's
    /// spirit): a deny with a typo'd port (`!evil.example:443junk`) used to
    /// degrade to a host glob over its own junk text — matching nothing, an
    /// INERT deny that failed open. Now any decision that consults a
    /// malformed pattern is refused outright, naming the pattern.
    /// NEVER weaken this to "matches nothing".
    #[test]
    fn malformed_egress_pattern_fails_the_decision_closed() {
        let mut p = Policy::default();
        p.egress
            .insert("api".into(), vec!["!evil.example:443junk".into()]);
        // The old behavior ALLOWED this call (inert deny). Now it is refused,
        // and the refusal says the pattern is malformed.
        let err = p.egress_allowed("api", "evil.example").unwrap_err();
        assert!(err.message.contains("malformed"), "{err}");
        assert!(err.message.contains("!evil.example:443junk"), "{err}");
        // A malformed ALLOW also refuses the decision (never half-works),
        // even when another allow entry would match.
        let mut p = Policy::default();
        p.egress.insert(
            "api".into(),
            vec!["api.example.com".into(), "api.example.com:junk".into()],
        );
        let err = p.egress_allowed("api", "api.example.com").unwrap_err();
        assert!(err.message.contains("malformed"), "{err}");
        // Well-formed policies are untouched.
        let mut p = Policy::default();
        p.egress
            .insert("api".into(), vec!["!evil.example:443".into()]);
        assert!(p.egress_allowed("api", "api.example.com").is_ok());
        assert!(p.egress_allowed("api", "evil.example").is_err());
    }

    #[test]
    fn egress_match_handles_ipv6_literals() {
        // A bare IPv6 literal's own colons are NOT read as a port.
        assert!(matches("::1", "::1", Some(443)));
        // Bracketed form pins a port.
        assert!(matches("[::1]:443", "::1", Some(443)));
        assert!(!matches("[::1]:443", "::1", Some(80)));
        assert!(matches("[::1]", "::1", Some(80)));
        // A MALFORMED pattern is reported as such — not silently widened to
        // any-port, and (since 2026-07-16) not degraded to an inert host glob
        // either: the caller fails closed on the verdict.
        for bad in [
            "[::1]:443junk",
            "[::1]:",
            "[::1]garbage",
            "[::1]:0",
            "[::1",
            "api.example.com:junk",
            "api.example.com:",
            "api.example.com:0",
        ] {
            assert_eq!(
                egress_match(bad, "::1", Some(443)),
                PatternMatch::Malformed,
                "pattern {bad} must be reported malformed"
            );
        }
    }

    #[test]
    fn host_from_url_extracts_host_dropping_scheme_port_path_and_userinfo() {
        assert_eq!(
            host_from_url("https://mcp.example.com/mcp/"),
            Some("mcp.example.com".into())
        );
        assert_eq!(
            host_from_url("https://mcp.example.com:8443/x"),
            Some("mcp.example.com".into())
        );
        assert_eq!(
            host_from_url("https://user:pass@mcp.example.com/mcp"),
            Some("mcp.example.com".into())
        );
        assert_eq!(
            host_from_url("https://[2001:db8::1]:443/x"),
            Some("2001:db8::1".into())
        );
        // Scheme is case-insensitive.
        assert_eq!(
            host_from_url("HTTPS://mcp.example.com/mcp"),
            Some("mcp.example.com".into())
        );
        // A placeholder in the HOST is unclassifiable…
        assert_eq!(host_from_url("https://${HOST}/mcp"), None);
        assert_eq!(host_from_url("https://${HOST}:443/mcp"), None);
        // …and so is a placeholder ANYWHERE else in the authority: a port or
        // userinfo ref can resolve to a value carrying `@` and re-target the
        // effective host after the fence is frozen, so fail closed.
        assert_eq!(host_from_url("https://mcp.example.com:${PORT}/mcp"), None);
        assert_eq!(
            host_from_url("https://user:${PASS}@mcp.example.com/mcp"),
            None
        );
        // A placeholder in a path/query (already stripped) can't change the
        // host, so the host is still classifiable.
        assert_eq!(
            host_from_url("https://mcp.example.com/tenants/${TENANT}"),
            Some("mcp.example.com".into())
        );
        // A non-canonical host spelling — non-ASCII (IDN) or percent-encoded —
        // must fail closed: `normalize_host` can't reconcile it with the
        // canonical form the resolver dials, so it would be a fence seam. The
        // first host uses a Cyrillic homoglyph of `s`.
        assert_eq!(host_from_url("https://\u{0455}cp.example.com/mcp"), None);
        assert_eq!(host_from_url("https://mcp%2eexample.com/mcp"), None);
        // Punycode is already ASCII — the canonical wire form — so it is fine.
        assert_eq!(
            host_from_url("https://xn--e1afmkfd.example/mcp"),
            Some("xn--e1afmkfd.example".into())
        );
        // Require a real HTTP(S) URL shape — a scheme-less or other-scheme
        // string must NOT be accepted as a bare host.
        assert_eq!(host_from_url("mcp.example.com/mcp"), None);
        assert_eq!(host_from_url("ftp://mcp.example.com/mcp"), None);
        assert_eq!(host_from_url("https:///onlypath"), None);
    }

    #[test]
    fn glob_matches_wildcards() {
        assert!(glob_match("registry:*", "registry:io.github.x/y"));
        assert!(glob_match(
            "git:github.com/acme/*",
            "git:github.com/acme/repo"
        ));
        assert!(!glob_match(
            "git:github.com/acme/*",
            "git:github.com/other/repo"
        ));
        assert!(glob_match("path:*", "path:./skills/x"));
        assert!(glob_match("exact", "exact"));
        assert!(!glob_match("exact", "other"));
        assert!(glob_match("*github*", "git:github.com/x"));
    }

    #[test]
    fn pack_members_are_visible_through_normal_sections() {
        // A pack rides normal manifest sections, so its server secret + skill are
        // seen by the same machinery doctor uses — no special-casing.
        let m: Manifest = toml::from_str(
            r#"
            version = 1

            [servers.linear-pack]
            type = "http"
            url = "https://mcp.linear.app/mcp"

            [servers.linear-pack.headers]
            Authorization = "Bearer ${LINEAR_PACK_TOKEN}"

            [skills.linear_breakdown]
            path = "./skills/linear/breakdown"

            [plugins.linear-pack]
            kind = "pack"
            version = "0.1.0"
            description = "Linear pack"
            servers = ["linear-pack"]
            skills = ["linear_breakdown"]
            "#,
        )
        .unwrap();
        assert!(m
            .referenced_secrets()
            .contains(&"LINEAR_PACK_TOKEN".to_string()));
        assert!(m.skills.contains_key("linear_breakdown"));
    }

    #[test]
    fn server_targets_default_scope_and_empty_list_round_trip() {
        let m: Manifest = toml::from_str(
            r#"
            version = 1

            [servers.default]
            type = "http"
            url = "https://x"

            [servers.scoped]
            type = "http"
            url = "https://x"
            targets = ["codex"]

            [servers.recipe-owned]
            type = "http"
            url = "https://x"
            targets = []
            "#,
        )
        .unwrap();
        // No `targets` → applies everywhere (back-compat for every existing
        // manifest and library server definition).
        assert!(m.servers["default"].applies_to("codex"));
        assert!(m.servers["default"].applies_to("claude-code"));
        // Explicit list scopes the fan-out.
        assert!(m.servers["scoped"].applies_to("codex"));
        assert!(!m.servers["scoped"].applies_to("claude-code"));
        // Empty list = direct fan-out nowhere (adopted plugin servers).
        assert!(!m.servers["recipe-owned"].applies_to("codex"));

        let out = toml::to_string(&m).unwrap();
        let back: Manifest = toml::from_str(&out).unwrap();
        assert_eq!(back, m);
        // The all-targets default stays implicit; the deliberate empty list
        // must survive serialization (it is NOT the default).
        assert!(!out.contains("targets = [\"*\"]"), "{out}");
        assert!(out.contains("targets = []"), "{out}");
    }

    #[test]
    fn server_cwd_round_trips_and_is_a_referenced_secret_source() {
        let m: Manifest = toml::from_str(
            r#"
            version = 1

            [servers.tldraw]
            type = "stdio"
            command = "node"
            args = ["dist/index.js"]
            cwd = "${TLDRAW_HOME}/server"

            [servers.plain]
            type = "http"
            url = "https://x"
            "#,
        )
        .unwrap();
        assert_eq!(
            m.servers["tldraw"].cwd.as_deref(),
            Some("${TLDRAW_HOME}/server")
        );
        assert_eq!(m.servers["plain"].cwd, None);
        // `${REF}`s inside cwd are surfaced like any other field.
        assert!(m.referenced_secrets().contains(&"TLDRAW_HOME".to_string()));

        let out = toml::to_string(&m).unwrap();
        let back: Manifest = toml::from_str(&out).unwrap();
        assert_eq!(back, m);
        assert!(out.contains("cwd = \"${TLDRAW_HOME}/server\""), "{out}");
        // Absent cwd stays implicit.
        assert!(!out.contains("[servers.plain]\ncwd"), "{out}");
    }

    #[test]
    fn server_owner_round_trips_and_defaults_to_none() {
        let m: Manifest = toml::from_str(
            r#"
            version = 1

            [servers.node_repl]
            type = "stdio"
            command = "node"
            owner = "codex"

            [servers.plain]
            type = "http"
            url = "https://x"
            "#,
        )
        .unwrap();
        assert_eq!(m.servers["node_repl"].owner.as_deref(), Some("codex"));
        assert_eq!(m.servers["plain"].owner, None);

        let out = toml::to_string(&m).unwrap();
        let back: Manifest = toml::from_str(&out).unwrap();
        assert_eq!(back, m);
        assert!(out.contains("owner = \"codex\""), "{out}");
    }

    #[test]
    fn server_extras_round_trip_through_toml() {
        let src = r#"
            version = 1

            [servers.miro]
            type = "stdio"
            command = "npx"
            args = ["-y", "@mirohq/mcp-server"]

            [servers.miro.extra.codex]
            startup_timeout_sec = 20
            "#;
        let m: Manifest = toml::from_str(src).unwrap();
        let miro = &m.servers["miro"];
        assert_eq!(
            miro.extra["codex"]["startup_timeout_sec"],
            serde_json::json!(20)
        );
        // Serializes back to TOML with the extras intact.
        let out = toml::to_string(&m).unwrap();
        let back: Manifest = toml::from_str(&out).unwrap();
        assert_eq!(back, m);
        assert!(out.contains("[servers.miro.extra.codex]"), "{out}");
    }

    #[test]
    fn referenced_secrets_sees_refs_in_extras_but_not_shell_syntax() {
        let m: Manifest = toml::from_str(
            r#"
            version = 1

            [servers.s]
            type = "stdio"
            command = "zsh"
            args = ["-lc", "x=${MIRO_ACCESS_TOKEN:-$MIRO_OAUTH_TOKEN} run"]

            [servers.s.extra.codex]
            note = "${EXTRA_TOKEN}"
            "#,
        )
        .unwrap();
        assert_eq!(m.referenced_secrets(), vec!["EXTRA_TOKEN".to_string()]);
    }

    #[test]
    fn server_referenced_secrets_covers_extra_and_own_fields() {
        // The shared per-server extractor sees command/args AND nested adapter
        // `extra` string fields — the field an earlier doctor-local copy omitted.
        let server: Server = toml::from_str(
            r#"
            type = "stdio"
            command = "run ${CMD_TOKEN}"
            args = ["--flag", "${ARG_TOKEN}"]

            [extra.codex]
            note = "${EXTRA_TOKEN}"
            "#,
        )
        .unwrap();
        let refs = server.referenced_secrets();
        assert!(refs.contains(&"CMD_TOKEN".to_string()), "{refs:?}");
        assert!(refs.contains(&"ARG_TOKEN".to_string()), "{refs:?}");
        assert!(
            refs.contains(&"EXTRA_TOKEN".to_string()),
            "a ref nested in `extra` must be seen: {refs:?}"
        );
    }

    #[test]
    fn server_integrity_roots_parse_roundtrip_and_stay_out_of_undeclared_definitions() {
        // Declared roots parse and survive a serialize/parse roundtrip (the
        // definition digest is over this serialization, so a declaration is
        // digest-relevant — trust re-gates when roots change).
        let server: Server = toml::from_str(
            r#"
            type = "stdio"
            command = "python"
            args = ["./tools/agent.py"]
            integrity_roots = ["tools"]
            "#,
        )
        .unwrap();
        assert_eq!(server.integrity_roots, vec!["tools".to_string()]);
        let text = toml::to_string(&server).unwrap();
        let reparsed: Server = toml::from_str(&text).unwrap();
        assert_eq!(reparsed.integrity_roots, server.integrity_roots);

        // A server with no declaration serializes without the field at all, so
        // every pre-D3 server definition digest is byte-identical to before.
        let plain: Server = toml::from_str("type = \"stdio\"\ncommand = \"node\"\n").unwrap();
        assert!(plain.integrity_roots.is_empty());
        assert!(!toml::to_string(&plain).unwrap().contains("integrity_roots"));
    }

    #[test]
    fn policy_source_allowed() {
        let p = Policy {
            allowed_sources: vec!["git:github.com/acme/*".into(), "registry:*".into()],
            ..Default::default()
        };
        assert!(p.source_allowed("git:github.com/acme/skill"));
        assert!(p.source_allowed("registry:io.github.x/y"));
        assert!(!p.source_allowed("git:github.com/evil/x"));
        // Empty policy allows anything.
        assert!(Policy::default().source_allowed("anything"));
    }
}
