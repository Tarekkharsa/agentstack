//! Adapter descriptors: the data-driven definition of how to render the
//! manifest into one CLI's native config. Supporting a new CLI = adding one of
//! these YAML files, not editing core code.

use std::path::PathBuf;

use serde::Deserialize;

use crate::scope::Scope;
use crate::util::paths;

/// One CLI's full descriptor, deserialized from `adapters/<id>.yaml`.
#[derive(Debug, Clone, Deserialize)]
pub struct AdapterDescriptor {
    pub id: String,
    pub display: String,
    #[serde(default)]
    pub detect: Detect,
    /// Global config location (and the canonical format).
    pub config: ConfigSpec,
    /// Project-scope config location, if the CLI supports project files.
    #[serde(default)]
    pub project: Option<ProjectSpec>,
    pub mcp: McpSpec,
    #[serde(default)]
    pub skills: Option<SkillsSpec>,
    /// Instruction file locations (CLAUDE.md / AGENTS.md).
    #[serde(default)]
    pub instructions: Option<InstructionsSpec>,
}

impl AdapterDescriptor {
    /// The config path + format for a given scope. `None` for `Project` when the
    /// CLI has no project config concept.
    pub fn config_for(
        &self,
        scope: Scope,
        project_dir: &std::path::Path,
    ) -> Option<(PathBuf, Format)> {
        match scope {
            Scope::Global => Some((paths::expand_tilde(&self.config.path), self.config.format)),
            Scope::Project => {
                let p = self.project.as_ref()?;
                let fmt = p.format.unwrap_or(self.config.format);
                Some((project_dir.join(&p.config), fmt))
            }
        }
    }

    /// The skills directory for a given scope, if the CLI has one.
    pub fn skills_dir_for(&self, scope: Scope, project_dir: &std::path::Path) -> Option<PathBuf> {
        let s = self.skills.as_ref()?;
        match scope {
            Scope::Global => Some(paths::expand_tilde(&s.dir)),
            Scope::Project => s.project_dir.as_ref().map(|d| project_dir.join(d)),
        }
    }

    /// Whether this CLI supports the given scope at all.
    pub fn supports_scope(&self, scope: Scope) -> bool {
        match scope {
            Scope::Global => true,
            Scope::Project => self.project.is_some() || self.skills_has_project(),
        }
    }

    fn skills_has_project(&self) -> bool {
        self.skills
            .as_ref()
            .and_then(|s| s.project_dir.as_ref())
            .is_some()
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Detect {
    /// Binary that, if on PATH, indicates the CLI is installed.
    #[serde(default)]
    pub bin: Option<String>,
    /// Config path that, if present, indicates the CLI is configured.
    #[serde(default)]
    pub config: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Json,
    Toml,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ConfigSpec {
    /// Path to the CLI config file (may start with `~`).
    pub path: String,
    pub format: Format,
}

#[derive(Debug, Clone, Deserialize)]
pub struct McpSpec {
    /// Dotted/plain key under which servers live (e.g. `mcpServers`,
    /// `mcp_servers`).
    pub location: String,
    pub fields: Fields,
    /// How (or whether) this CLI tags transport with a key.
    #[serde(default)]
    pub transport: Option<Transport>,
    /// TOML only: render nested objects (headers/env) as standalone subtables
    /// rather than inline tables.
    #[serde(default)]
    pub headers_as_subtable: bool,
    #[serde(default)]
    pub secret_mode: SecretMode,
}

/// Target field names for each canonical field. `None` means the CLI does not
/// support that field and it is dropped.
#[derive(Debug, Clone, Deserialize)]
pub struct Fields {
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Option<String>,
    #[serde(default)]
    pub headers: Option<String>,
    #[serde(default)]
    pub env: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Transport {
    /// The key that holds the transport tag (e.g. `type`).
    pub key: String,
    pub http_value: String,
    #[serde(default)]
    pub stdio_value: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SecretMode {
    /// Write the resolved secret value into the target config (the target
    /// already holds plaintext today; the manifest stays clean).
    #[default]
    Literal,
    /// Pass the `${REF}` through unchanged (CLI expands it itself).
    Passthrough,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SkillsSpec {
    /// Global skills directory (e.g. `~/.claude/skills`).
    pub dir: String,
    /// Project-scoped skills directory relative to the repo (e.g.
    /// `.claude/skills`). Absent → the CLI has no project skills concept.
    #[serde(default)]
    pub project_dir: Option<String>,
    /// How active skills are made present in `dir` / `project_dir`.
    #[serde(default)]
    pub strategy: SkillStrategy,
}

/// How a skill is materialized into a target's skills directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillStrategy {
    /// Symlink the library skill dir into the target (default; no duplication).
    #[default]
    Symlink,
    /// Copy the skill dir (Windows / sandboxes where symlinks are awkward).
    Copy,
}

/// Instruction-file locations for a CLI (markdown, managed-region merge).
#[derive(Debug, Clone, Deserialize)]
pub struct InstructionsSpec {
    /// Global instruction file (e.g. `~/.claude/CLAUDE.md`).
    pub global: String,
    /// Project instruction file relative to the repo (e.g. `CLAUDE.md`).
    #[serde(default)]
    pub project: Option<String>,
}

impl InstructionsSpec {
    pub fn path_for(&self, scope: Scope, project_dir: &std::path::Path) -> Option<PathBuf> {
        match scope {
            Scope::Global => Some(paths::expand_tilde(&self.global)),
            Scope::Project => self.project.as_ref().map(|p| project_dir.join(p)),
        }
    }
}

/// Project-scope config location for a CLI that supports project files.
#[derive(Debug, Clone, Deserialize)]
pub struct ProjectSpec {
    /// Project config path relative to the repo (e.g. `.mcp.json`).
    pub config: String,
    /// Format if it differs from the global config (else inferred / inherited).
    #[serde(default)]
    pub format: Option<Format>,
}
