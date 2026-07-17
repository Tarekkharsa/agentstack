//! Adapter descriptors: the data-driven definition of how to render the
//! manifest into one CLI's native config. Supporting a new CLI = adding one of
//! these YAML files, not editing core code.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use agentstack_core::scope::Scope;
use agentstack_core::util::paths;

/// Project-scope paths anchor at the PROJECT ROOT. Callers usually hold the
/// manifest dir, which under the `.agentstack/` layout is one level below the
/// root — normalize before joining so `.mcp.json`, `.claude/skills/`, etc.
/// land where the CLIs actually look.
fn project_root(project_dir: &Path) -> PathBuf {
    agentstack_core::manifest::project_root_of(project_dir)
}

/// Where an adapter descriptor was loaded from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum AdapterSource {
    /// Shipped inside the binary.
    #[default]
    BuiltIn,
    /// A user-supplied file under `~/.agentstack/adapters/` (may override a
    /// built-in id).
    User(PathBuf),
}

/// One CLI's full descriptor, deserialized from `adapters/<id>.yaml`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterDescriptor {
    pub id: String,
    pub display: String,
    #[serde(default)]
    pub detect: Detect,
    /// Global MCP config location (and the canonical format). Absent for CLIs
    /// that have no MCP support (e.g. Pi manages only skills/settings).
    #[serde(default)]
    pub config: Option<ConfigSpec>,
    /// Project-scope config location, if the CLI supports project files.
    #[serde(default)]
    pub project: Option<ProjectSpec>,
    /// How to render MCP servers. Absent for CLIs with no MCP support.
    #[serde(default)]
    pub mcp: Option<McpSpec>,
    #[serde(default)]
    pub skills: Option<SkillsSpec>,
    /// Instruction file locations (CLAUDE.md / AGENTS.md).
    #[serde(default)]
    pub instructions: Option<InstructionsSpec>,
    /// Native settings file (e.g. Claude Code `~/.claude/settings.json`).
    #[serde(default)]
    pub settings: Option<SettingsSpec>,
    /// Lifecycle-hook destination, if the CLI supports hooks.
    #[serde(default)]
    pub hooks: Option<HooksSpec>,
    /// Native extension/add-on directory, if the CLI supports it (e.g. Pi's
    /// `~/.pi/agent/extensions`). Discovered read-only.
    #[serde(default)]
    pub extensions: Option<ExtensionsSpec>,
    /// Where this descriptor was loaded from — set by the registry, not parsed
    /// from the file.
    #[serde(skip)]
    pub source: AdapterSource,
    /// SHA-256 (hex) of the EXACT descriptor YAML bytes, retained by the registry
    /// at load. Crate-private and read-only via
    /// [`AdapterDescriptor::definition_digest`], so a caller can neither forge it
    /// nor mutate the descriptor and its digest independently. Empty for a
    /// descriptor not loaded through the registry.
    #[serde(skip)]
    pub(crate) definition_digest: String,
}

impl AdapterDescriptor {
    /// The exact-bytes definition digest the registry retained, or `None` for a
    /// descriptor not loaded through the registry (which therefore cannot form a
    /// grant's bound adapter identity).
    pub fn definition_digest(&self) -> Option<&str> {
        (!self.definition_digest.is_empty()).then_some(self.definition_digest.as_str())
    }

    /// The config path + format for a given scope. `None` for `Project` when the
    /// CLI has no project config concept.
    pub fn config_for(
        &self,
        scope: Scope,
        project_dir: &std::path::Path,
    ) -> Option<(PathBuf, Format)> {
        let config = self.config.as_ref()?;
        match scope {
            Scope::Global => Some((paths::expand_tilde(&config.path), config.format)),
            Scope::Project => {
                let p = self.project.as_ref()?;
                let fmt = p.format.unwrap_or(config.format);
                Some((project_root(project_dir).join(&p.config), fmt))
            }
        }
    }

    /// The native settings file path + format for a given scope, if the CLI has
    /// one. `None` for `Project` when the CLI has no project settings file.
    pub fn settings_for(
        &self,
        scope: Scope,
        project_dir: &std::path::Path,
    ) -> Option<(PathBuf, Format)> {
        let s = self.settings.as_ref()?;
        match scope {
            Scope::Global => Some((paths::expand_tilde(&s.global), s.format)),
            Scope::Project => s
                .project
                .as_ref()
                .map(|p| (project_root(project_dir).join(p), s.format)),
        }
    }

    /// The hooks destination file + format for a scope, if the CLI has one.
    pub fn hooks_for(
        &self,
        scope: Scope,
        project_dir: &std::path::Path,
    ) -> Option<(PathBuf, Format)> {
        let h = self.hooks.as_ref()?;
        match scope {
            Scope::Global => Some((paths::expand_tilde(&h.global), h.format)),
            Scope::Project => h
                .project
                .as_ref()
                .map(|p| (project_root(project_dir).join(p), h.format)),
        }
    }

    /// The native extensions directory for a scope, if the CLI has one.
    pub fn extensions_dir_for(
        &self,
        scope: Scope,
        project_dir: &std::path::Path,
    ) -> Option<PathBuf> {
        let e = self.extensions.as_ref()?;
        match scope {
            Scope::Global => Some(paths::expand_tilde(&e.dir)),
            Scope::Project => e
                .project_dir
                .as_ref()
                .map(|d| project_root(project_dir).join(d)),
        }
    }

    /// The skills directory for a given scope, if the CLI has one.
    pub fn skills_dir_for(&self, scope: Scope, project_dir: &std::path::Path) -> Option<PathBuf> {
        let s = self.skills.as_ref()?;
        match scope {
            Scope::Global => Some(paths::expand_tilde(&s.dir)),
            Scope::Project => s
                .project_dir
                .as_ref()
                .map(|d| project_root(project_dir).join(d)),
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
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct ConfigSpec {
    /// Path to the CLI config file (may start with `~`).
    pub path: String,
    pub format: Format,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
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
    /// Render `command` + `args` as a single combined array under the `command`
    /// field (e.g. OpenCode's `command: ["npx", "-y", "pkg"]`) instead of a
    /// command string plus a separate `args` array. When set, the `args` field
    /// mapping is ignored.
    #[serde(default)]
    pub command_array: bool,
    #[serde(default)]
    pub secret_mode: SecretMode,
    /// Server-NAME constraint this CLI enforces at its own startup, when we
    /// know one. A name outside the charset must be skipped from the render
    /// (with a loud reason) — writing it produces a config the CLI rejects
    /// with a startup error on every launch. Absent = no known constraint.
    #[serde(default)]
    pub name_charset: Option<NameCharset>,
}

/// Known server-name charsets, by id. An enum (not a regex) on purpose: the
/// reviewed crates avoid a regex dependency, and each variant documents the
/// CLI that demands it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum NameCharset {
    /// `^[A-Za-z0-9_-]+$` — Codex refuses any other name at startup
    /// ("Invalid MCP server name … must match pattern ^[a-zA-Z0-9_-]+$").
    #[serde(rename = "ascii-word-dash")]
    AsciiWordDash,
}

impl NameCharset {
    pub fn permits(self, name: &str) -> bool {
        match self {
            NameCharset::AsciiWordDash => {
                !name.is_empty()
                    && name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            }
        }
    }

    /// Human phrase for the rule, used in the skip reason.
    pub fn describe(self) -> &'static str {
        match self {
            NameCharset::AsciiWordDash => "letters, digits, '_' and '-' only",
        }
    }
}

/// Target field names for each canonical field. `None` means the CLI does not
/// support that field and it is dropped.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fields {
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Option<String>,
    /// Native working-directory key for stdio servers (e.g. `cwd`). `None` when
    /// the CLI's config has no such field — the manifest `cwd` is then dropped
    /// for this target.
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub headers: Option<String>,
    #[serde(default)]
    pub env: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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
            Scope::Project => self
                .project
                .as_ref()
                .map(|p| project_root(project_dir).join(p)),
        }
    }
}

/// Native extension/add-on directory for a CLI (code modules placed in a dir,
/// e.g. Pi extensions). Each entry is a file or a directory.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionsSpec {
    /// Global extensions directory (e.g. `~/.pi/agent/extensions`).
    pub dir: String,
    /// Project extensions directory relative to the repo (e.g. `.pi/extensions`).
    #[serde(default)]
    pub project_dir: Option<String>,
}

/// Native settings-file locations for a CLI (permissions, feature flags, etc.).
/// Distinct from the MCP config file; merged non-destructively at the top level.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsSpec {
    /// File format (json for Claude `settings.json`, toml for Codex `config.toml`).
    pub format: Format,
    /// Global settings file (e.g. `~/.claude/settings.json`).
    pub global: String,
    /// Project settings file relative to the repo (e.g. `.claude/settings.json`).
    #[serde(default)]
    pub project: Option<String>,
    /// Curated catalog of this CLI's known settings, so the dashboard can render
    /// typed controls (toggles / dropdowns) instead of a raw JSON box. Keys not
    /// listed here are still honored — they're just edited by hand.
    #[serde(default)]
    pub fields: Vec<SettingField>,
}

/// One known setting in a CLI's settings file. `key` is a dotted path
/// (`permissions.defaultMode`) into the settings object.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct SettingField {
    pub key: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(rename = "type")]
    pub kind: SettingKind,
    /// Allowed values for `enum` settings.
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default)]
    pub help: Option<String>,
    /// Section heading in the dashboard (e.g. "Permissions", "Git").
    #[serde(default)]
    pub group: Option<String>,
    /// The CLI's own default, shown as a hint (not written unless chosen).
    #[serde(default)]
    pub default: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SettingKind {
    Bool,
    String,
    Number,
    Enum,
}

/// Lifecycle-hook destination for a CLI. Claude Code keeps hooks under the
/// `hooks` key of its settings.json; other harnesses may differ.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HooksSpec {
    pub format: Format,
    /// Global hooks file (e.g. `~/.claude/settings.json`).
    pub global: String,
    /// Project hooks file relative to the repo.
    #[serde(default)]
    pub project: Option<String>,
    /// Top-level key the hooks object lives under (e.g. `hooks`).
    pub key: String,
    /// How to shape the hooks object. Only `claude` is supported today.
    #[serde(default)]
    pub shape: HookShape,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HookShape {
    /// Claude form: event → [{matcher?, hooks: [{type, command, …}]}].
    #[default]
    Claude,
}

/// Project-scope config location for a CLI that supports project files.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectSpec {
    /// Project config path relative to the repo (e.g. `.mcp.json`).
    pub config: String,
    /// Format if it differs from the global config (else inferred / inherited).
    #[serde(default)]
    pub format: Option<Format>,
}

#[cfg(test)]
mod name_charset_tests {
    use super::NameCharset;

    /// Security-adjacent witness: the codex charset must track Codex's own
    /// startup validation (^[a-zA-Z0-9_-]+$) — a name it wrongly permits
    /// renders a config Codex errors on at every launch; a name it wrongly
    /// rejects silently drops a working server.
    #[test]
    fn ascii_word_dash_matches_codexs_startup_rule() {
        let cs = NameCharset::AsciiWordDash;
        for good in ["kibana", "gha-search", "node_repl", "Context7", "a1"] {
            assert!(cs.permits(good), "{good} must be permitted");
        }
        for bad in ["upstash/context7", "a.b", "a b", "café", "", "a:b"] {
            assert!(!cs.permits(bad), "{bad:?} must be rejected");
        }
    }
}
