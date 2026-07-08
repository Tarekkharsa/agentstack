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
}

impl Policy {
    pub fn is_empty(&self) -> bool {
        self.require.is_empty()
            && self.forbid.is_empty()
            && self.allowed_sources.is_empty()
            && self.tools.is_empty()
    }

    /// Whether `source` is allowed (any source allowed when the list is empty).
    pub fn source_allowed(&self, source: &str) -> bool {
        self.allowed_sources.is_empty()
            || self.allowed_sources.iter().any(|p| glob_match(p, source))
    }

    /// Whether `server`'s `tool` passes `[policy.tools]`. `Ok(())` when allowed;
    /// `Err(rule)` names the pattern (or the allowlist) that blocks it.
    pub fn tool_allowed(&self, server: &str, tool: &str) -> Result<(), String> {
        let Some(rules) = self.tools.get(server) else {
            return Ok(());
        };
        for r in rules {
            if let Some(deny) = r.strip_prefix('!') {
                if glob_match(deny, tool) {
                    return Err(format!("denied by [policy.tools] {server} = \"!{deny}\""));
                }
            }
        }
        let allows: Vec<&String> = rules.iter().filter(|r| !r.starts_with('!')).collect();
        if !allows.is_empty() && !allows.iter().any(|a| glob_match(a, tool)) {
            return Err(format!(
                "not in the [policy.tools] allowlist for {server} ({})",
                allows
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        Ok(())
    }
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

pub(crate) fn all_targets() -> Vec<String> {
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

impl Manifest {
    /// Every `${REF}` secret name referenced by any server, de-duplicated and
    /// sorted. Used by `secret list` and `doctor`.
    pub fn referenced_secrets(&self) -> Vec<String> {
        let mut refs: Vec<String> = Vec::new();
        let mut push = |s: &str| {
            for r in crate::secret::refs_in(s) {
                if !refs.contains(&r) {
                    refs.push(r);
                }
            }
        };
        for server in self.servers.values() {
            if let Some(u) = &server.url {
                push(u);
            }
            if let Some(c) = &server.command {
                push(c);
            }
            for a in &server.args {
                push(a);
            }
            if let Some(cwd) = &server.cwd {
                push(cwd);
            }
            for v in server.headers.values() {
                push(v);
            }
            for v in server.env.values() {
                push(v);
            }
            for fields in server.extra.values() {
                for v in fields.values() {
                    for s in json_strings(v) {
                        push(s);
                    }
                }
            }
        }
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
        assert!(err.contains("!delete_*"), "{err}");
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
        assert!(m
            .referenced_secrets()
            .contains(&"TLDRAW_HOME".to_string()));

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
