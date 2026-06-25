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

    /// Where `apply` writes by default and which adapters are in play.
    #[serde(default)]
    pub targets: Targets,
}

/// One instruction fragment: a markdown file applied to some/all harnesses.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Instruction {
    pub path: String,
    /// Adapter ids this fragment applies to; `["*"]` (the default) = all.
    #[serde(default = "all_targets")]
    pub targets: Vec<String>,
}

fn all_targets() -> Vec<String> {
    vec!["*".to_string()]
}

impl Instruction {
    /// Whether this fragment applies to adapter `id`.
    pub fn applies_to(&self, id: &str) -> bool {
        self.targets.iter().any(|t| t == "*" || t == id)
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
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub headers: IndexMap<String, String>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub env: IndexMap<String, String>,
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
}

/// Where a skill's content comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillSource {
    Path(String),
    Git { url: String, rev: Option<String> },
}

impl Skill {
    /// Resolve which source this skill declares (git wins if both present).
    pub fn source(&self) -> anyhow::Result<SkillSource> {
        if let Some(url) = &self.git {
            Ok(SkillSource::Git {
                url: url.clone(),
                rev: self.rev.clone(),
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
            for v in server.headers.values() {
                push(v);
            }
            for v in server.env.values() {
                push(v);
            }
        }
        refs.sort();
        refs
    }
}
