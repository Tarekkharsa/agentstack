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
    /// Headless (prompt-in/text-out) invocation argv, if the CLI supports a
    /// non-interactive mode (e.g. `claude -p`, `codex exec`). Absent → the CLI
    /// cannot be driven by `run --locked --prompt`.
    #[serde(default)]
    pub headless: Option<HeadlessSpec>,
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

    /// This CLI's own name for one selection dimension, or `None` when its
    /// settings catalog has no such notion. Pure lookup, for surfaces that
    /// report what a harness can and cannot carry without needing a value.
    pub fn selection_key(&self, dimension: Dimension) -> Option<&str> {
        self.settings.as_ref().and_then(|s| s.key_for(dimension))
    }

    /// What this harness can do with `value` for `dimension` on ONE headless
    /// launch — the single entry point to a substituted selection fragment.
    ///
    /// `Ok(Selection::Args(..))` is the deliverable case, and it is the ONLY
    /// path that substitutes: the value has passed [`check_selection_value`]
    /// against this adapter's own catalog by then, which is what licenses the
    /// inside-a-token substitution [`SelectionSpec`] documents. The other two
    /// `Ok` answers are honest capability facts for a caller to report, not
    /// swallow. `Err` is a value this adapter's catalog refuses — a manifest
    /// authoring error, named in full rather than silently dropped.
    pub fn select(&self, dimension: Dimension, value: &str) -> Result<Selection, String> {
        let Some(key) = self.selection_key(dimension) else {
            return Ok(Selection::NoNotion);
        };
        let Some(spec) = self.headless.as_ref().and_then(|h| h.selection(dimension)) else {
            return Ok(Selection::NotPerLaunch {
                key: key.to_string(),
            });
        };
        let field = self.settings.as_ref().and_then(|s| s.field(key));
        check_selection_value(&self.id, dimension, key, field, value)?;
        Ok(Selection::Args(spec.argv(value)))
    }
}

/// How to invoke a CLI headless: an argv template where each element is either
/// a literal or the exact string `{prompt}`, replaced whole by the prompt text.
///
/// Validation lives in deserialization (`try_from`), so EVERY parse path —
/// embedded descriptors, user drop-ins, direct `serde_yaml::from_str` — rejects
/// a malformed spec on two counts: (1) the placeholder must be a WHOLE element
/// (no splicing prompt text into another token), and (2) the placeholder must
/// be immediately preceded by a literal `--` end-of-options separator. Guard
/// (2) closes an OPTION-INJECTION hole the OS-level "one argv element" property
/// does NOT: a prompt like `--dangerously-skip-permissions` is a single argv
/// element, but the CHILD CLI's own flag parser would read a leading-dash
/// operand as a flag, not as prompt text. `--` makes the harness treat every
/// following token as a positional, so hostile prompt text can never reach the
/// child as a flag (rule 7: prompt is data, not syntax). All shipped agent CLIs
/// (claude, codex, clap/commander-based tools) honor `--`. (For a TypeScript
/// reader: `try_from` is serde's version of parsing into a raw shape and
/// running a validating constructor over it — like `zod.transform` with a
/// throwing refine.)
#[derive(Debug, Clone, Deserialize)]
#[serde(try_from = "RawHeadlessSpec")]
pub struct HeadlessSpec {
    args: Vec<String>,
    mcp_injection: Option<McpInjectionSpec>,
    model_selection: Option<SelectionSpec>,
    effort_selection: Option<SelectionSpec>,
}

/// The unvalidated wire shape `HeadlessSpec` is parsed through.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHeadlessSpec {
    args: Vec<String>,
    #[serde(default)]
    mcp_injection: Option<McpInjectionSpec>,
    #[serde(default)]
    model_selection: Option<RawSelectionSpec>,
    #[serde(default)]
    effort_selection: Option<RawSelectionSpec>,
}

/// The placeholder a headless argv element may consist of — the WHOLE element,
/// never a substring of one (rule 7: prompt text is data, not syntax).
pub const PROMPT_PLACEHOLDER: &str = "{prompt}";

/// The end-of-options separator that MUST immediately precede the placeholder,
/// so the child CLI parses the prompt as a positional and never as a flag.
pub const OPTIONS_TERMINATOR: &str = "--";

impl TryFrom<RawHeadlessSpec> for HeadlessSpec {
    type Error = String;

    fn try_from(raw: RawHeadlessSpec) -> Result<Self, Self::Error> {
        let mut prompt_at: Option<usize> = None;
        for (i, a) in raw.args.iter().enumerate() {
            if a == PROMPT_PLACEHOLDER {
                if prompt_at.is_some() {
                    // More than one placeholder has no defined meaning.
                    return Err(format!(
                        "headless args must contain exactly one {PROMPT_PLACEHOLDER} element"
                    ));
                }
                prompt_at = Some(i);
            } else if a.contains(PROMPT_PLACEHOLDER) {
                // An embedded placeholder ("--flag={prompt}") would splice
                // hostile prompt text into the middle of another token —
                // refuse the descriptor at load, not the run at launch.
                return Err(format!(
                    "headless arg {a:?} embeds {PROMPT_PLACEHOLDER} inside another token — \
                     the placeholder must be a whole argv element"
                ));
            }
        }
        // Zero placeholders would silently drop the prompt from the committed
        // argv.
        let Some(i) = prompt_at else {
            return Err(format!(
                "headless args must contain exactly one {PROMPT_PLACEHOLDER} element (found none)"
            ));
        };
        // The placeholder must sit directly after a literal `--`, so a hostile
        // leading-dash prompt cannot be parsed as an option by the child CLI.
        if i == 0 || raw.args[i - 1] != OPTIONS_TERMINATOR {
            return Err(format!(
                "headless {PROMPT_PLACEHOLDER} must be immediately preceded by a literal \
                 {OPTIONS_TERMINATOR:?} end-of-options separator (so a leading-dash prompt \
                 cannot be parsed as a flag by the harness) — e.g. [\"exec\", \"--\", \"{{prompt}}\"]"
            ));
        }
        // And that guard must be the ONLY `--`: an earlier one would end
        // option parsing first, demoting everything after it — including a
        // spliced mcp_injection — into the child's positional region, where
        // strict-scope flags are silently ignored.
        if raw
            .args
            .iter()
            .enumerate()
            .any(|(j, a)| a == OPTIONS_TERMINATOR && j != i - 1)
        {
            return Err(format!(
                "headless args may contain {OPTIONS_TERMINATOR:?} exactly once — the guard \
                 immediately before {PROMPT_PLACEHOLDER}; an additional {OPTIONS_TERMINATOR:?} \
                 would end option parsing early and demote later options to positionals"
            ));
        }
        // The two per-launch selection fragments validate the same way, each
        // against its OWN placeholder — done here (rather than in a plain
        // `Deserialize` impl on `SelectionSpec`) because only the enclosing
        // field name says which dimension a fragment is for.
        let model_selection = raw
            .model_selection
            .map(|r| SelectionSpec::validated(r, Dimension::Model))
            .transpose()?;
        let effort_selection = raw
            .effort_selection
            .map(|r| SelectionSpec::validated(r, Dimension::Effort))
            .transpose()?;
        Ok(HeadlessSpec {
            args: raw.args,
            mcp_injection: raw.mcp_injection,
            model_selection,
            effort_selection,
        })
    }
}

impl HeadlessSpec {
    /// Build the concrete argv for one prompt: whole-argument substitution
    /// only. The prompt string — however hostile — becomes exactly one argv
    /// element; no shell, no quoting, no splitting is ever involved.
    pub fn argv(&self, prompt: &str) -> Vec<String> {
        self.argv_with_injection(prompt, &[])
    }

    /// Like [`argv`](Self::argv), with already-substituted MCP-injection
    /// arguments spliced into the OPTIONS region — immediately before the `--`
    /// terminator that guards the prompt — so they are parsed as flags while
    /// the prompt stays a positional. The injection elements are
    /// launcher-authored trusted data (a path or config text the launcher
    /// itself rendered — see [`McpInjectionSpec::argv`]), never prompt or repo
    /// text, and like everything else in this argv they reach the child
    /// without a shell in between.
    pub fn argv_with_injection(&self, prompt: &str, injection: &[String]) -> Vec<String> {
        let mut out: Vec<String> = Vec::with_capacity(self.args.len() + injection.len());
        for a in &self.args {
            if a == PROMPT_PLACEHOLDER {
                // Validation guarantees the element before the placeholder is
                // the `--` terminator; the injection goes before THAT.
                let terminator = out.pop().expect("validated: `--` precedes {prompt}");
                out.extend(injection.iter().cloned());
                out.push(terminator);
                out.push(prompt.to_string());
            } else {
                out.push(a.clone());
            }
        }
        out
    }

    /// The per-child MCP config injection block, if this harness declared one.
    /// `None` → the launcher must fall back to launch-scoping the shared
    /// project config (park/swap), which serializes concurrent locked runs.
    pub fn mcp_injection(&self) -> Option<&McpInjectionSpec> {
        self.mcp_injection.as_ref()
    }

    /// The per-launch selection fragment for one dimension, if this harness
    /// declared a CONFIRMED way to select it at launch. `None` means exactly
    /// that and nothing more: the harness may still have the setting (see
    /// [`AdapterDescriptor::selection_key`]) — it just has no flag that
    /// applies it to a single non-interactive run, and writing a governed
    /// child's choice into the CLI's persistent settings file is not an
    /// option. Callers report that; they never guess a flag.
    pub fn selection(&self, dimension: Dimension) -> Option<&SelectionSpec> {
        match dimension {
            Dimension::Model => self.model_selection.as_ref(),
            Dimension::Effort => self.effort_selection.as_ref(),
        }
    }
}

// ─────────────────── per-launch model / effort selection ────────────────────

/// The placeholder a `headless.model_selection` fragment substitutes the
/// chosen model into.
pub const MODEL_PLACEHOLDER: &str = "{model}";

/// The placeholder a `headless.effort_selection` fragment substitutes the
/// chosen reasoning-effort level into.
pub const EFFORT_PLACEHOLDER: &str = "{effort}";

/// The two per-launch choices a workflow role makes about the child it spawns:
/// WHICH model runs, and HOW MUCH reasoning effort it spends.
///
/// A dimension is deliberately not a per-CLI branch. Each adapter declares in
/// its OWN descriptor what it calls the setting (`settings.model_key` /
/// `settings.effort_key`, which must name a key the settings catalog already
/// documents) and how to select it for one launch
/// (`headless.model_selection` / `headless.effort_selection`). Nothing on the
/// delivery path names a CLI, so supporting a new harness stays a YAML edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dimension {
    Model,
    Effort,
}

impl Dimension {
    /// The dimension's name in user-facing sentences.
    pub fn label(self) -> &'static str {
        match self {
            Dimension::Model => "model",
            Dimension::Effort => "effort",
        }
    }

    /// The placeholder its `*_selection` fragment substitutes.
    pub fn placeholder(self) -> &'static str {
        match self {
            Dimension::Model => MODEL_PLACEHOLDER,
            Dimension::Effort => EFFORT_PLACEHOLDER,
        }
    }

    /// The `headless.*` descriptor field, for errors that point at the YAML.
    fn selection_field(self) -> &'static str {
        match self {
            Dimension::Model => "model_selection",
            Dimension::Effort => "effort_selection",
        }
    }

    /// The `settings.*` descriptor field naming the catalog key.
    pub fn settings_key_field(self) -> &'static str {
        match self {
            Dimension::Model => "settings.model_key",
            Dimension::Effort => "settings.effort_key",
        }
    }

    /// Both dimensions, in the order surfaces report them.
    pub const ALL: [Dimension; 2] = [Dimension::Model, Dimension::Effort];
}

/// How a harness selects a model or an effort level FOR ONE LAUNCH: extra argv
/// elements spliced into the options region — exactly where
/// [`McpInjectionSpec`]'s are, and before the `--` guard for the same reason —
/// with one occurrence of the dimension's placeholder standing in for the
/// chosen value.
///
/// **One deliberate difference from every other placeholder in this file:** the
/// value MAY be substituted inside a larger argv token (`-c model={model}`),
/// because that is the only shape Codex's `-c key=value` override accepts.
/// Splicing a value into the middle of a token is precisely what rule 7 forbids
/// for repository content, and it is safe here for one reason only: the value
/// is checked against THIS adapter's own settings catalog before substitution
/// (see [`AdapterDescriptor::select`]), so it is either one of the `enum`
/// options the descriptor itself lists, or a bounded string over a conservative
/// charset that contains no whitespace, no `=`, and no shell metacharacter.
/// Remove that check and this becomes an injection point into the child's own
/// config-override parser. (There is no shell anywhere on the path either way —
/// each element reaches `execve` whole.)
///
/// Validated in `try_from` on the enclosing [`HeadlessSpec`], so every parse
/// path — embedded descriptors, user drop-ins, direct `serde_yaml::from_str` —
/// refuses a malformed fragment at LOAD rather than at launch.
#[derive(Debug, Clone)]
pub struct SelectionSpec {
    args: Vec<String>,
    dimension: Dimension,
}

/// The unvalidated wire shape a [`SelectionSpec`] is parsed through. It has no
/// `Deserialize` route of its own into a validated spec: only
/// [`HeadlessSpec`]'s `try_from` builds one, because only the field name it was
/// read from says which dimension the fragment belongs to.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSelectionSpec {
    args: Vec<String>,
}

impl SelectionSpec {
    fn validated(raw: RawSelectionSpec, dimension: Dimension) -> Result<Self, String> {
        let field = dimension.selection_field();
        let placeholder = dimension.placeholder();
        // A fragment with no arguments would select nothing while claiming the
        // harness can select — the exact silent-no-op this task forbids.
        if raw.args.is_empty() {
            return Err(format!(
                "{field} args must not be empty — a fragment that carries no argv elements \
                 would silently select nothing"
            ));
        }
        let mut occurrences = 0usize;
        for a in &raw.args {
            if a == OPTIONS_TERMINATOR {
                // Same reason as `mcp_injection`: the fragment is spliced into
                // the OPTIONS region, and a `--` there would demote every flag
                // after it — including the prompt guard — to positional text.
                return Err(format!(
                    "{field} args may not contain a literal {OPTIONS_TERMINATOR:?} — an \
                     end-of-options separator inside the options region would demote the \
                     flags after it to positionals"
                ));
            }
            occurrences += a.matches(placeholder).count();
            // Every OTHER known placeholder is a descriptor bug here: this
            // fragment is substituted with one value and nothing else, so a
            // stray `{prompt}` or `{mcp_config_path}` would reach the child
            // verbatim as a bogus literal.
            for other in [
                PROMPT_PLACEHOLDER,
                MCP_CONFIG_PATH_PLACEHOLDER,
                MCP_SERVERS_TOML_PLACEHOLDER,
                MODEL_PLACEHOLDER,
                EFFORT_PLACEHOLDER,
            ] {
                if other != placeholder && a.contains(other) {
                    return Err(format!(
                        "{field} arg {a:?} contains {other} — only {placeholder} is substituted \
                         here, so any other placeholder would reach the harness verbatim"
                    ));
                }
            }
            if a != placeholder && a.starts_with('{') && a.ends_with('}') {
                return Err(format!(
                    "{field} arg {a:?} is not a known placeholder — only {placeholder} is \
                     recognized in this fragment"
                ));
            }
        }
        // Zero occurrences would drop the value; more than one has no defined
        // meaning (which copy is the value, which is a literal?).
        if occurrences != 1 {
            return Err(format!(
                "{field} args must contain exactly one {placeholder} occurrence (found \
                 {occurrences})"
            ));
        }
        Ok(SelectionSpec {
            args: raw.args,
            dimension,
        })
    }

    /// Which dimension this fragment carries.
    pub fn dimension(&self) -> Dimension {
        self.dimension
    }

    /// Substitute an ALREADY-VALIDATED value. Private on purpose: the only
    /// route to a substituted fragment is [`AdapterDescriptor::select`], which
    /// runs the catalog check first — see this type's docstring for why that
    /// ordering is the whole safety argument.
    fn argv(&self, value: &str) -> Vec<String> {
        self.args
            .iter()
            .map(|a| a.replace(self.dimension.placeholder(), value))
            .collect()
    }
}

/// The upper bound on a free-form (non-`enum`) selection value, in bytes.
/// A model name is a short identifier; anything longer is not one, and an
/// unbounded value spliced into an argv token is unbounded input reaching a
/// child's parser (rule 7: bound it).
pub const MAX_SELECTION_VALUE_BYTES: usize = 64;

/// The conservative charset a free-form selection value may draw from: ASCII
/// alphanumerics plus the punctuation real model ids actually use
/// (`claude-opus-4-5`, `openai/gpt-5.5`, `anthropic:claude@latest`). No
/// whitespace, no `=`, no quote, no shell metacharacter — which is what makes
/// substituting the value INSIDE a `-c key=value` token safe.
fn selection_char_permitted(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':' | '/' | '@')
}

/// Check one declared value against the adapter's OWN settings catalog before
/// it is ever substituted into an argv token.
///
/// Two rules, in order: an `enum` field is authoritative (the value must be one
/// of the options the descriptor lists — the CLI would reject anything else
/// anyway, and we would rather refuse loudly here than launch a child that
/// dies on its own config parse); anything else falls back to the conservative
/// bounded charset above. Errors name the adapter, the setting key, the
/// offending value, and the legal set, because a refusal a user cannot act on
/// is a worse outcome than the misconfiguration.
fn check_selection_value(
    adapter: &str,
    dimension: Dimension,
    key: &str,
    field: Option<&SettingField>,
    value: &str,
) -> Result<(), String> {
    let label = dimension.label();
    if let Some(f) = field {
        if f.kind == SettingKind::Enum {
            if f.options.iter().any(|o| o == value) {
                return Ok(());
            }
            return Err(format!(
                "{adapter} cannot take {label} {value:?}: its `{key}` setting is an enum whose \
                 legal values are {}",
                if f.options.is_empty() {
                    "(none declared)".to_string()
                } else {
                    f.options.join(", ")
                }
            ));
        }
    }
    if value.is_empty() || value.len() > MAX_SELECTION_VALUE_BYTES {
        return Err(format!(
            "{adapter} cannot take {label} {value:?} for its `{key}` setting: a free-form \
             selection value must be 1..={MAX_SELECTION_VALUE_BYTES} bytes long"
        ));
    }
    if !value.chars().all(selection_char_permitted) {
        return Err(format!(
            "{adapter} cannot take {label} {value:?} for its `{key}` setting: a free-form \
             selection value may only use ASCII letters, digits, and '. _ - : / @' — the value \
             is spliced into an argv token, so the charset is the bound that keeps it inert"
        ));
    }
    Ok(())
}

/// What one adapter can do with a per-role model/effort value. Three answers,
/// and only the first one launches with the value applied — the other two are
/// facts a surface must SAY, never swallow (rule 8: claims match enforcement).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    /// Deliverable: these argv elements carry the validated value and splice
    /// into the options region.
    Args(Vec<String>),
    /// The harness's settings catalog names no key for this dimension at all —
    /// it has no notion of it, so there is nothing to select.
    NoNotion,
    /// The harness HAS the setting (its catalog names `key`) but declares no
    /// confirmed way to select it for a single launch. The value would only
    /// apply if written into that CLI's persistent settings file, which a
    /// governed child run must never do.
    NotPerLaunch { key: String },
}

/// The placeholder for a per-run MCP config FILE the launcher renders into the
/// run dir (e.g. `claude --mcp-config <path>`). Like [`PROMPT_PLACEHOLDER`],
/// it may only ever be a WHOLE argv element.
pub const MCP_CONFIG_PATH_PLACEHOLDER: &str = "{mcp_config_path}";

/// The placeholder for the launcher-rendered MCP server set as ONE inline
/// `key=value` TOML override element (e.g. `codex -c 'mcp_servers={…}'`).
/// Whole argv element only.
pub const MCP_SERVERS_TOML_PLACEHOLDER: &str = "{mcp_servers_toml}";

/// How a harness accepts a per-child MCP config at launch (`headless.mcp_injection`
/// in the descriptor): extra argv elements spliced into the options region,
/// where exactly the known placeholders above stand in for launcher-rendered
/// values. This is what lets N concurrent locked children share one project
/// without touching (or serializing on) the shared project MCP config.
///
/// Same validation discipline as [`HeadlessSpec`], enforced in `try_from` so
/// every parse path refuses a malformed block at LOAD: only the two known
/// placeholders are recognized, each must be a WHOLE argv element (never
/// embedded in another token), each may appear at most once, at least one must
/// appear (a block that references no per-run value could not inject
/// anything), and `{prompt}` may not appear here at all — prompt delivery
/// belongs to `headless.args` behind its `--` guard, never to the options
/// region.
#[derive(Debug, Clone, Deserialize)]
#[serde(try_from = "RawMcpInjectionSpec")]
pub struct McpInjectionSpec {
    args: Vec<String>,
}

/// The unvalidated wire shape `McpInjectionSpec` is parsed through.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMcpInjectionSpec {
    args: Vec<String>,
}

impl TryFrom<RawMcpInjectionSpec> for McpInjectionSpec {
    type Error = String;

    fn try_from(raw: RawMcpInjectionSpec) -> Result<Self, Self::Error> {
        let mut seen_path = false;
        let mut seen_toml = false;
        for a in &raw.args {
            match a.as_str() {
                MCP_CONFIG_PATH_PLACEHOLDER => {
                    if seen_path {
                        return Err(format!(
                            "mcp_injection args may contain {MCP_CONFIG_PATH_PLACEHOLDER} at most once"
                        ));
                    }
                    seen_path = true;
                }
                MCP_SERVERS_TOML_PLACEHOLDER => {
                    if seen_toml {
                        return Err(format!(
                            "mcp_injection args may contain {MCP_SERVERS_TOML_PLACEHOLDER} at most once"
                        ));
                    }
                    seen_toml = true;
                }
                OPTIONS_TERMINATOR => {
                    // Injection is spliced into the OPTIONS region; a literal
                    // `--` there would end option parsing early and demote the
                    // rest of the injection to positional text the harness
                    // silently ignores.
                    return Err(format!(
                        "mcp_injection args may not contain a literal {OPTIONS_TERMINATOR:?} — \
                         an end-of-options separator inside the options region would demote the \
                         flags after it to positionals"
                    ));
                }
                other => {
                    // An embedded placeholder ("--mcp-config={mcp_config_path}")
                    // would splice a substituted value into the middle of
                    // another token; an unknown "{...}" placeholder would reach
                    // the child verbatim as a bogus literal. Both are descriptor
                    // bugs — refuse at load, not at launch.
                    for p in [
                        MCP_CONFIG_PATH_PLACEHOLDER,
                        MCP_SERVERS_TOML_PLACEHOLDER,
                        PROMPT_PLACEHOLDER,
                    ] {
                        if other.contains(p) {
                            return Err(format!(
                                "mcp_injection arg {other:?} embeds {p} inside another token — \
                                 a placeholder must be a whole argv element (and {PROMPT_PLACEHOLDER} \
                                 is not valid in mcp_injection at all)"
                            ));
                        }
                    }
                    if other.starts_with('{') && other.ends_with('}') {
                        return Err(format!(
                            "mcp_injection arg {other:?} is not a known placeholder — only \
                             {MCP_CONFIG_PATH_PLACEHOLDER} and {MCP_SERVERS_TOML_PLACEHOLDER} \
                             are recognized"
                        ));
                    }
                }
            }
        }
        if !seen_path && !seen_toml {
            return Err(format!(
                "mcp_injection args must contain {MCP_CONFIG_PATH_PLACEHOLDER} or \
                 {MCP_SERVERS_TOML_PLACEHOLDER} — a block that references no per-run \
                 value cannot inject a per-child config"
            ));
        }
        Ok(McpInjectionSpec { args: raw.args })
    }
}

impl McpInjectionSpec {
    /// Whether this spec needs the launcher to render a per-run config FILE.
    pub fn needs_config_path(&self) -> bool {
        self.args.iter().any(|a| a == MCP_CONFIG_PATH_PLACEHOLDER)
    }

    /// Whether this spec needs the launcher to render the server set as one
    /// inline TOML override value.
    pub fn needs_servers_toml(&self) -> bool {
        self.args.iter().any(|a| a == MCP_SERVERS_TOML_PLACEHOLDER)
    }

    /// Build the concrete injection argv: whole-element substitution of the
    /// launcher-rendered values, mirroring [`HeadlessSpec::argv`]. Both values
    /// are launcher-authored trusted data (a run-dir path / rendered config
    /// text — never prompt or repo text); a needed value the caller failed to
    /// supply is an error, never a placeholder leaked into a child's argv.
    pub fn argv(
        &self,
        config_path: Option<&str>,
        servers_toml: Option<&str>,
    ) -> Result<Vec<String>, String> {
        self.args
            .iter()
            .map(|a| match a.as_str() {
                MCP_CONFIG_PATH_PLACEHOLDER => config_path.map(str::to_string).ok_or_else(|| {
                    format!("{MCP_CONFIG_PATH_PLACEHOLDER} needed but no config path was rendered")
                }),
                MCP_SERVERS_TOML_PLACEHOLDER => servers_toml.map(str::to_string).ok_or_else(|| {
                    format!(
                        "{MCP_SERVERS_TOML_PLACEHOLDER} needed but no server table was rendered"
                    )
                }),
                _ => Ok(a.clone()),
            })
            .collect()
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
    /// A **live** (non-file) channel this harness can take instruction content
    /// through, and how well that is actually known
    /// (`docs/design/instruction-variants.md` §"Channels: confirmation-gated").
    ///
    /// Absent means no live channel is claimed at all. This lives on the
    /// descriptor so that adding a harness — or upgrading a channel from
    /// `unconfirmed` to `confirmed` — is a YAML edit plus a line of evidence,
    /// with no branch anywhere in the delivery path that names a CLI.
    #[serde(default)]
    pub live: Option<LiveInstructionChannel>,
}

/// How well a harness is known to consume a live instruction channel.
///
/// Two states, and the difference is the whole honesty rule: `Confirmed` means
/// somebody observed it working; `Unconfirmed` means documented or
/// protocol-level and never verified here. An unconfirmed channel is never used
/// as though it worked, and no surface may present it as confirmed.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Confirmation {
    Confirmed,
    Unconfirmed,
}

impl Confirmation {
    pub fn slug(self) -> &'static str {
        match self {
            Confirmation::Confirmed => "confirmed",
            Confirmation::Unconfirmed => "unconfirmed",
        }
    }
}

/// One live instruction channel a harness declares.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LiveInstructionChannel {
    /// Machine id of the channel (`mcp-instructions`).
    pub channel: String,
    /// Human name of the channel, for the sentences surfaces print.
    pub display: String,
    pub confirmation: Confirmation,
    /// Why the confirmation state is what it is — the evidence, in one line.
    #[serde(default)]
    pub note: Option<String>,
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
#[serde(try_from = "RawSettingsSpec")]
pub struct SettingsSpec {
    /// File format (json for Claude `settings.json`, toml for Codex `config.toml`).
    pub format: Format,
    /// Global settings file (e.g. `~/.claude/settings.json`).
    pub global: String,
    /// Project settings file relative to the repo (e.g. `.claude/settings.json`).
    pub project: Option<String>,
    /// Curated catalog of this CLI's known settings, so external UIs can render
    /// typed controls (toggles / dropdowns) instead of a raw JSON box. Keys not
    /// listed here are still honored — they're just edited by hand.
    pub fields: Vec<SettingField>,
    /// Which catalog key means "which model", if this CLI has such a notion.
    ///
    /// A POINTER into `fields`, never a second copy of it: the catalog is
    /// already the authority on what this CLI calls its settings and which
    /// values are legal, so naming the key here buys per-role value checking
    /// (an `enum` field's `options`, a `string` field's charset bound) with no
    /// duplicated knowledge to drift. Absent = this harness has no notion of a
    /// model at all, which surfaces report rather than hide.
    pub model_key: Option<String>,
    /// Which catalog key means "how much reasoning effort" — same contract as
    /// [`Self::model_key`].
    pub effort_key: Option<String>,
}

/// The unvalidated wire shape `SettingsSpec` is parsed through, so the
/// pointer-into-`fields` invariant is checked on EVERY parse path.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSettingsSpec {
    format: Format,
    global: String,
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    fields: Vec<SettingField>,
    #[serde(default)]
    model_key: Option<String>,
    #[serde(default)]
    effort_key: Option<String>,
}

impl TryFrom<RawSettingsSpec> for SettingsSpec {
    type Error = String;

    fn try_from(raw: RawSettingsSpec) -> Result<Self, Self::Error> {
        // A declared key that names no catalog field would give a per-role
        // value nothing to be checked against — silently degrading the
        // enum/charset check into "anything goes". Refuse the descriptor at
        // load instead.
        for (dimension, declared) in [
            (Dimension::Model, raw.model_key.as_deref()),
            (Dimension::Effort, raw.effort_key.as_deref()),
        ] {
            let Some(key) = declared else { continue };
            if !raw.fields.iter().any(|f| f.key == key) {
                return Err(format!(
                    "{} names {key:?}, which is not a key in this adapter's settings `fields` \
                     catalog — the catalog is what a per-role {} value is checked against, so \
                     the key must exist there",
                    dimension.settings_key_field(),
                    dimension.label()
                ));
            }
        }
        Ok(SettingsSpec {
            format: raw.format,
            global: raw.global,
            project: raw.project,
            fields: raw.fields,
            model_key: raw.model_key,
            effort_key: raw.effort_key,
        })
    }
}

impl SettingsSpec {
    /// The catalog key this CLI uses for one dimension, if it has the notion.
    pub fn key_for(&self, dimension: Dimension) -> Option<&str> {
        match dimension {
            Dimension::Model => self.model_key.as_deref(),
            Dimension::Effort => self.effort_key.as_deref(),
        }
    }

    /// The catalog entry for a key, if the catalog documents it.
    pub fn field(&self, key: &str) -> Option<&SettingField> {
        self.fields.iter().find(|f| f.key == key)
    }
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
    /// Section heading in external UIs (e.g. "Permissions", "Git").
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
mod headless_spec_tests {
    use super::AdapterDescriptor;

    /// Security witness (W2): a hostile prompt — shell metacharacters,
    /// embedded newlines, quotes, AND a leading dash — lands as exactly ONE
    /// trailing argv element, byte for byte, after the `--` guard. This is the
    /// argv the grant commits and the process spawns with; there is no shell in
    /// between to reinterpret it, and the `--` stops the child's flag parser
    /// from reading the leading-dash text as an option.
    #[test]
    fn hostile_prompt_is_exactly_one_argv_element() {
        let desc: AdapterDescriptor = serde_yaml::from_str(
            "id: x\ndisplay: X\nheadless:\n  args: [\"-p\", \"--\", \"{prompt}\"]\n",
        )
        .unwrap();
        let hostile = "--dangerously-skip-permissions\n; rm -rf ~ #\n\"$(whoami)\" 'q' `tick`";
        let argv = desc.headless.unwrap().argv(hostile);
        assert_eq!(
            argv,
            vec!["-p".to_string(), "--".to_string(), hostile.to_string()]
        );
    }

    /// Security witness (W2): every malformed spec is refused at LOAD, on every
    /// parse path — an embedded placeholder ("--flag={prompt}"), a missing
    /// placeholder (prompt silently dropped), and — the option-injection guard
    /// — a placeholder NOT immediately preceded by a literal `--` (a
    /// leading-dash prompt would otherwise be parsed as a flag by the harness).
    #[test]
    fn malformed_headless_specs_are_refused_at_parse() {
        let embedded = serde_yaml::from_str::<AdapterDescriptor>(
            "id: x\ndisplay: X\nheadless:\n  args: [\"--\", \"--flag={prompt}\"]\n",
        );
        assert!(
            embedded.is_err(),
            "embedded placeholder must be refused at load"
        );

        let missing = serde_yaml::from_str::<AdapterDescriptor>(
            "id: x\ndisplay: X\nheadless:\n  args: [\"exec\"]\n",
        );
        assert!(
            missing.is_err(),
            "a spec with no {{prompt}} element must be refused at load"
        );

        // No `--` before the placeholder: option-injectable, refused.
        let unguarded = serde_yaml::from_str::<AdapterDescriptor>(
            "id: x\ndisplay: X\nheadless:\n  args: [\"exec\", \"{prompt}\"]\n",
        );
        assert!(
            unguarded.is_err(),
            "a placeholder not preceded by `--` must be refused at load"
        );

        // A `--` present but not immediately before the placeholder does not
        // count — the terminator only guards what directly follows it.
        let wrong_place = serde_yaml::from_str::<AdapterDescriptor>(
            "id: x\ndisplay: X\nheadless:\n  args: [\"--\", \"exec\", \"{prompt}\"]\n",
        );
        assert!(
            wrong_place.is_err(),
            "`--` must be immediately before the placeholder"
        );
    }

    /// Security witness (W2.5 hardening): a SECOND `--` ahead of the guard is
    /// refused at load — it would end option parsing early, so everything
    /// spliced after it (the whole mcp_injection, strict-scope flags included)
    /// would land in the child's positional region and be silently ignored.
    #[test]
    fn duplicate_options_terminator_is_refused_at_parse() {
        let doubled = serde_yaml::from_str::<AdapterDescriptor>(
            "id: x\ndisplay: X\nheadless:\n  args: [\"exec\", \"--\", \"--\", \"{prompt}\"]\n",
        );
        assert!(
            doubled.is_err(),
            "a duplicate `--` in headless args must be refused at load"
        );
    }
}

#[cfg(test)]
mod mcp_injection_spec_tests {
    use super::AdapterDescriptor;

    fn parse(mcp_injection_args: &str) -> Result<AdapterDescriptor, serde_yaml::Error> {
        serde_yaml::from_str(&format!(
            "id: x\ndisplay: X\nheadless:\n  args: [\"-p\", \"--\", \"{{prompt}}\"]\n  \
             mcp_injection:\n    args: [{mcp_injection_args}]\n"
        ))
    }

    /// Security witness (W2.5): injection args splice into the OPTIONS region
    /// — before the `--` guard — so the harness parses them as flags while the
    /// prompt stays a guarded positional, and the substituted value is exactly
    /// one argv element.
    #[test]
    fn injection_splices_before_the_terminator_as_whole_elements() {
        let desc = parse("\"--mcp-config\", \"{mcp_config_path}\", \"--strict-mcp-config\"")
            .expect("valid spec");
        let headless = desc.headless.unwrap();
        let inj = headless
            .mcp_injection()
            .unwrap()
            .argv(Some("/runs/r1/mcp-config.json"), None)
            .unwrap();
        let argv = headless.argv_with_injection("do the thing", &inj);
        assert_eq!(
            argv,
            vec![
                "-p".to_string(),
                "--mcp-config".to_string(),
                "/runs/r1/mcp-config.json".to_string(),
                "--strict-mcp-config".to_string(),
                "--".to_string(),
                "do the thing".to_string(),
            ]
        );
    }

    /// Security witness (W2.5): every malformed injection block is refused at
    /// LOAD on every parse path — unknown placeholder, embedded placeholder,
    /// duplicate placeholder, `{prompt}` in the options region, and a block
    /// with no placeholder at all.
    #[test]
    fn malformed_injection_specs_are_refused_at_parse() {
        for (bad, why) in [
            ("\"--flag\", \"{mcp_config}\"", "unknown placeholder"),
            ("\"--mcp-config={mcp_config_path}\"", "embedded placeholder"),
            (
                "\"{mcp_config_path}\", \"{mcp_config_path}\"",
                "duplicate placeholder",
            ),
            ("\"-c\", \"{prompt}\"", "{prompt} in the options region"),
            ("\"--strict-mcp-config\"", "no placeholder at all"),
            (
                "\"--\", \"--mcp-config\", \"{mcp_config_path}\"",
                "a literal `--` in the options region",
            ),
        ] {
            assert!(parse(bad).is_err(), "{why} must be refused at load");
        }
    }

    /// A needed value the caller failed to supply errors instead of leaking a
    /// literal placeholder into a child's argv.
    #[test]
    fn missing_substitution_value_is_an_error() {
        let desc = parse("\"-c\", \"{mcp_servers_toml}\"").expect("valid spec");
        let headless = desc.headless.unwrap();
        let spec = headless.mcp_injection().unwrap();
        assert!(spec.needs_servers_toml() && !spec.needs_config_path());
        assert!(spec.argv(Some("/ignored"), None).is_err());
    }
}

#[cfg(test)]
mod selection_spec_tests {
    use super::{AdapterDescriptor, Dimension, Selection};

    /// A minimal descriptor with a settings catalog and both selection
    /// fragments: `model` is a free-form string, `effort` an enum — the two
    /// value-checking regimes, on one adapter.
    fn desc() -> AdapterDescriptor {
        serde_yaml::from_str(
            r#"
id: fake
display: Fake
headless:
  args: ["exec", "--", "{prompt}"]
  model_selection:
    args: ["-c", "model={model}"]
  effort_selection:
    args: ["--effort", "{effort}"]
settings:
  format: json
  global: ~/.fake/settings.json
  model_key: model
  effort_key: reasoning
  fields:
    - { key: model, type: string }
    - { key: reasoning, type: enum, options: [low, medium, high] }
"#,
        )
        .expect("valid descriptor")
    }

    /// The happy path, including the one shape that makes the value check
    /// load-bearing: `{model}` substituted INSIDE a `-c key=value` token.
    #[test]
    fn a_validated_value_substitutes_into_the_declared_fragment() {
        let d = desc();
        assert_eq!(
            d.select(Dimension::Model, "gpt-5.5").unwrap(),
            Selection::Args(vec!["-c".to_string(), "model=gpt-5.5".to_string()])
        );
        assert_eq!(
            d.select(Dimension::Effort, "high").unwrap(),
            Selection::Args(vec!["--effort".to_string(), "high".to_string()])
        );
    }

    /// An `enum` field is authoritative: a value outside its options is
    /// refused, and the refusal names the adapter, the key, the value, and the
    /// legal set — everything a user needs to fix the manifest.
    #[test]
    fn an_enum_field_refuses_a_value_outside_its_options() {
        let err = desc()
            .select(Dimension::Effort, "extreme")
            .expect_err("an out-of-catalog enum value must be refused");
        for expected in ["fake", "reasoning", "extreme", "low, medium, high"] {
            assert!(err.contains(expected), "{err}");
        }
    }

    /// The hostile-input bound (rule 7): because a free-form value is spliced
    /// INSIDE an argv token, anything carrying whitespace, `=`, quotes, shell
    /// metacharacters, a leading-dash injection attempt, or sheer length is
    /// refused before substitution — the check the inside-a-token splice
    /// depends on.
    #[test]
    fn a_free_form_value_is_bounded_by_a_conservative_charset() {
        let d = desc();
        for hostile in [
            "gpt 5.5",
            "x=y",
            "$(whoami)",
            "a;rm -rf ~",
            "a\nb",
            "\"q\"",
            "café",
            "",
        ] {
            let err = d
                .select(Dimension::Model, hostile)
                .expect_err("hostile free-form value must be refused: {hostile:?}");
            assert!(err.contains("fake") && err.contains("model"), "{err}");
        }
        let too_long = "a".repeat(super::MAX_SELECTION_VALUE_BYTES + 1);
        assert!(d.select(Dimension::Model, &too_long).is_err());
        // …and the boundary itself still passes.
        let at_bound = "a".repeat(super::MAX_SELECTION_VALUE_BYTES);
        assert!(d.select(Dimension::Model, &at_bound).is_ok());
    }

    /// The two honest capability answers, distinguished: no catalog key at all
    /// (`NoNotion`) versus a catalog key with no confirmed per-launch flag
    /// (`NotPerLaunch`, which names the key so a surface can say what it is).
    #[test]
    fn an_undeliverable_dimension_is_reported_not_swallowed() {
        let no_notion: AdapterDescriptor = serde_yaml::from_str(
            "id: bare\ndisplay: Bare\nheadless:\n  args: [\"--\", \"{prompt}\"]\n",
        )
        .unwrap();
        assert_eq!(
            no_notion.select(Dimension::Model, "anything").unwrap(),
            Selection::NoNotion
        );

        let key_only: AdapterDescriptor = serde_yaml::from_str(
            r#"
id: keyed
display: Keyed
headless:
  args: ["--", "{prompt}"]
settings:
  format: json
  global: ~/.keyed/settings.json
  effort_key: effortLevel
  fields:
    - { key: effortLevel, type: enum, options: [low, high] }
"#,
        )
        .unwrap();
        assert_eq!(
            key_only.select(Dimension::Effort, "low").unwrap(),
            Selection::NotPerLaunch {
                key: "effortLevel".to_string()
            }
        );
        // A value the harness cannot deliver is never silently "valid":
        // reporting outranks checking, so no error is raised either.
        assert_eq!(
            key_only.select(Dimension::Model, "x").unwrap(),
            Selection::NoNotion
        );
    }

    /// Every malformed selection fragment is refused at LOAD, on every parse
    /// path — the same discipline `mcp_injection` gets, because these
    /// fragments splice into the same options region.
    #[test]
    fn malformed_selection_fragments_are_refused_at_parse() {
        let parse = |fragment: &str| {
            serde_yaml::from_str::<AdapterDescriptor>(&format!(
                "id: x\ndisplay: X\nheadless:\n  args: [\"--\", \"{{prompt}}\"]\n  {fragment}\n"
            ))
        };
        for (bad, why) in [
            ("model_selection:\n    args: []", "an empty fragment"),
            (
                "model_selection:\n    args: [\"--model\"]",
                "no placeholder at all",
            ),
            (
                "model_selection:\n    args: [\"--model\", \"{model}\", \"{model}\"]",
                "a duplicate placeholder",
            ),
            (
                "model_selection:\n    args: [\"-c\", \"model={model}-{model}\"]",
                "two occurrences inside one token",
            ),
            (
                "model_selection:\n    args: [\"--\", \"--model\", \"{model}\"]",
                "a literal `--` in the options region",
            ),
            (
                "model_selection:\n    args: [\"--model\", \"{effort}\"]",
                "the other dimension's placeholder",
            ),
            (
                "model_selection:\n    args: [\"--model\", \"{model}\", \"{prompt}\"]",
                "{prompt} in a selection fragment",
            ),
            (
                "model_selection:\n    args: [\"--model\", \"{model}\", \"{nope}\"]",
                "an unknown placeholder",
            ),
            (
                "effort_selection:\n    args: [\"--effort\", \"{model}\"]",
                "the wrong placeholder for the field",
            ),
        ] {
            assert!(parse(bad).is_err(), "{why} must be refused at load");
        }
    }

    /// A `model_key`/`effort_key` that names no catalog field is refused at
    /// LOAD: it would leave a per-role value with nothing to be checked
    /// against, quietly turning the enum/charset guard into "anything goes".
    #[test]
    fn a_selection_key_naming_no_catalog_field_is_refused_at_parse() {
        let dangling = serde_yaml::from_str::<AdapterDescriptor>(
            "id: x\ndisplay: X\nsettings:\n  format: json\n  global: ~/x.json\n  \
             model_key: model\n  fields: []\n",
        );
        assert!(
            dangling.is_err(),
            "a model_key with no matching catalog field must be refused at load"
        );
    }

    /// The shipped descriptors say what this task claims they say. A witness,
    /// not a restatement: the delivery path has no CLI branch, so these three
    /// YAML declarations ARE the feature for claude-code, codex and pi.
    #[test]
    fn the_shipped_descriptors_declare_what_they_can_carry() {
        let reg = crate::registry::Registry::load().unwrap();

        let claude = reg.get("claude-code").unwrap();
        assert!(matches!(
            claude.select(Dimension::Model, "claude-opus-4-5"),
            Ok(Selection::Args(_))
        ));
        // Claude Code has effortLevel in its catalog but no confirmed
        // per-launch flag — the honest middle case.
        assert_eq!(
            claude.select(Dimension::Effort, "high").unwrap(),
            Selection::NotPerLaunch {
                key: "effortLevel".to_string()
            }
        );

        let codex = reg.get("codex").unwrap();
        assert_eq!(
            codex.select(Dimension::Model, "gpt-5.5").unwrap(),
            Selection::Args(vec!["-c".to_string(), "model=gpt-5.5".to_string()])
        );
        assert_eq!(
            codex.select(Dimension::Effort, "high").unwrap(),
            Selection::Args(vec![
                "-c".to_string(),
                "model_reasoning_effort=high".to_string()
            ])
        );
        // codex's effort IS an enum in its catalog, so a bogus level refuses.
        assert!(codex.select(Dimension::Effort, "turbo").is_err());

        // gemini has no settings block at all: no notion of either dimension.
        let gemini = reg.get("gemini").unwrap();
        for dimension in Dimension::ALL {
            assert_eq!(gemini.select(dimension, "x").unwrap(), Selection::NoNotion);
        }

        // pi has both keys but no headless block: neither is per-launch.
        let pi = reg.get("pi").unwrap();
        assert!(matches!(
            pi.select(Dimension::Model, "sonnet").unwrap(),
            Selection::NotPerLaunch { .. }
        ));
        assert!(matches!(
            pi.select(Dimension::Effort, "high").unwrap(),
            Selection::NotPerLaunch { .. }
        ));
    }
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
