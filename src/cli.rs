//! Command-line surface (clap derive). Phase 0 ships the read-only commands:
//! `apply` (dry-run by default), `diff`, and `adapters`.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::manifest::ServerType;
use crate::scope::Scope;

#[derive(Parser, Debug)]
#[command(
    name = "agentstack",
    version,
    about = "One portable manifest, every agent CLI.",
    long_about = "Manage MCP servers + skills across Claude Code, Codex, and more, \
                  from a single portable agentstack.toml."
)]
pub struct Cli {
    /// Directory containing agentstack.toml (defaults to the current directory).
    #[arg(long, global = true, value_name = "DIR")]
    pub manifest_dir: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Discover installed CLIs and reverse-engineer a manifest from their
    /// existing MCP configs, lifting inline secrets into `${REF}`s.
    Init(InitArgs),

    /// Add a server or skill to the manifest.
    Add(AddArgs),

    /// Fetch skill sources into the store and write the lockfile.
    Install(InstallArgs),

    /// Re-resolve git skills to their latest and rewrite the lockfile.
    Update(UpdateArgs),

    /// Remove a server or skill from the manifest (and lockfile).
    Remove(RemoveArgs),

    /// Render the manifest into each target's native config.
    ///
    /// Read-only by default: shows the diff and writes nothing. Pass `--write`
    /// to apply the changes.
    Apply(ApplyArgs),

    /// Show drift between the manifest and the on-disk configs.
    Diff(DiffArgs),

    /// Explain a server or skill: where it came from, what secrets it needs,
    /// which tools get it and what files get written, and its safety signals.
    Explain(ExplainArgs),

    /// Activate a profile: render its servers + materialize its skills.
    Use(UseArgs),

    /// Manage ephemeral sessions: load a profile (+ optional plugin) for now,
    /// then revert it. A safety hatch for the dashboard's session feature.
    Session(SessionArgs),

    /// Compile instruction fragments into each harness's CLAUDE.md / AGENTS.md.
    Instructions(InstructionsArgs),

    /// Pull hand-added servers from a target config back into the manifest.
    Adopt(AdoptArgs),

    /// Gather scattered skills from every CLI's skills dir into one managed
    /// home (`~/.agentstack/skills/`), symlinking the originals back.
    Consolidate(ConsolidateArgs),

    /// Restore a CLI config from its pre-write backup (undo an apply).
    Restore(RestoreArgs),

    /// Verify everything is wired up: adapters, secrets, drift, quirks, skills.
    Doctor(DoctorArgs),

    /// Search the capability catalog (and mark what's already added).
    Search(SearchArgs),

    /// Show local usage analytics (activation counts + footprint).
    Stats,

    /// Inspect the available CLI adapters.
    Adapters(AdaptersArgs),

    /// Manage AgentStack plugin recipes and generated native marketplaces.
    Plugins(PluginsArgs),

    /// Manage secrets in the OS keychain.
    Secret(SecretArgs),

    /// Export the manifest (+ lock, + optionally secrets) as an encrypted bundle.
    Export(ExportArgs),

    /// Import an encrypted bundle on a new machine.
    Import(ImportArgs),

    /// Open the local web dashboard.
    Dashboard(DashboardArgs),

    /// Run agentstack as an MCP server over stdio (for an agent to call).
    Mcp,

    /// Print a shell hook for per-directory profile auto-activation.
    Hook(HookArgs),
}

#[derive(Args, Debug)]
pub struct HookArgs {
    /// Which shell to emit the hook for.
    #[arg(value_enum)]
    pub shell: Shell,
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum Shell {
    Zsh,
    Bash,
    Fish,
}

#[derive(Args, Debug)]
pub struct InstallArgs {
    /// Fail if resolving would change the lockfile (CI / reproducible installs).
    #[arg(long)]
    pub locked: bool,
}

#[derive(Args, Debug)]
pub struct UpdateArgs {
    /// Only update this skill (default: all git skills).
    pub name: Option<String>,
}

#[derive(Args, Debug)]
pub struct RemoveArgs {
    /// Name of the server or skill to remove.
    pub name: String,
    /// Write the change (else dry-run).
    #[arg(long)]
    pub write: bool,
}

#[derive(Args, Debug)]
pub struct AddArgs {
    #[command(subcommand)]
    pub kind: AddKind,
}

#[derive(Subcommand, Debug)]
pub enum AddKind {
    /// Add a capability from a provider (catalog or official MCP Registry).
    From(AddFromArgs),
    /// Add an MCP server.
    Server(AddServerArgs),
    /// Add a skill (a SKILL.md directory).
    Skill(AddSkillArgs),
}

#[derive(Args, Debug)]
pub struct AddFromArgs {
    /// Catalog name or registry id (e.g. `github`, `io.github.x/server`).
    pub id: String,
    /// Also add to this profile's server list.
    #[arg(long)]
    pub profile: Option<String>,
    /// Write the change (else dry-run).
    #[arg(long)]
    pub write: bool,
}

#[derive(Args, Debug)]
pub struct AddServerArgs {
    pub name: String,
    #[arg(long = "type", value_enum, default_value = "http")]
    pub transport: ServerType,
    /// HTTP server URL.
    #[arg(long)]
    pub url: Option<String>,
    /// Header `Key=Value` (repeatable); values may contain `${REF}`.
    #[arg(long = "header", value_name = "K=V")]
    pub headers: Vec<String>,
    /// stdio command.
    #[arg(long)]
    pub command: Option<String>,
    /// stdio arg (repeatable). Accepts leading-dash values (e.g. `--arg -y`).
    #[arg(long = "arg", value_name = "ARG", allow_hyphen_values = true)]
    pub args: Vec<String>,
    /// Env `Key=Value` (repeatable).
    #[arg(long = "env", value_name = "K=V")]
    pub env: Vec<String>,
    /// Also add to this profile's server list.
    #[arg(long)]
    pub profile: Option<String>,
    /// Write the change (else dry-run).
    #[arg(long)]
    pub write: bool,
}

#[derive(Args, Debug)]
pub struct AddSkillArgs {
    pub name: String,
    /// Path to the skill directory.
    #[arg(long)]
    pub path: String,
    /// Also add to this profile's skill list.
    #[arg(long)]
    pub profile: Option<String>,
    #[arg(long)]
    pub write: bool,
}

#[derive(Args, Debug)]
pub struct InitArgs {
    /// Overwrite an existing agentstack.toml.
    #[arg(long)]
    pub force: bool,

    /// Show what would be imported without writing the manifest or storing
    /// secrets.
    #[arg(long)]
    pub dry_run: bool,

    /// Don't store lifted secrets in the keychain (just reference them).
    #[arg(long)]
    pub no_keychain: bool,
}

#[derive(Args, Debug)]
pub struct ApplyArgs {
    /// Only act on these target ids (repeatable). Defaults to [targets].default.
    #[arg(long = "target", value_name = "ID")]
    pub targets: Vec<String>,

    /// Render only the servers in this profile.
    #[arg(long, value_name = "NAME")]
    pub profile: Option<String>,

    /// Show what would change without writing (the default).
    #[arg(long)]
    pub dry_run: bool,

    /// Actually write the changes to disk.
    #[arg(long)]
    pub write: bool,

    /// Which scope to write: global (~) or project (repo). Defaults to global;
    /// pass `--scope project` to write repo-local config.
    #[arg(long, value_enum)]
    pub scope: Option<Scope>,

    /// Allow writing even when a `${REF}` did not resolve on this machine. By
    /// default unresolved secrets block the write for that target.
    #[arg(long)]
    pub allow_unresolved: bool,
}

#[derive(Args, Debug)]
pub struct DiffArgs {
    #[arg(long = "target", value_name = "ID")]
    pub targets: Vec<String>,

    #[arg(long, value_name = "NAME")]
    pub profile: Option<String>,

    #[arg(long, value_enum)]
    pub scope: Option<Scope>,
}

#[derive(Args, Debug)]
pub struct UseArgs {
    /// Profile name to activate.
    pub profile: String,

    /// Only act on these target ids (repeatable).
    #[arg(long = "target", value_name = "ID")]
    pub targets: Vec<String>,

    #[arg(long, value_enum)]
    pub scope: Option<Scope>,

    /// Actually write configs and materialize skills (else dry-run).
    #[arg(long)]
    pub write: bool,

    /// Allow writing even when a `${REF}` did not resolve (off by default).
    #[arg(long)]
    pub allow_unresolved: bool,
}

#[derive(Args, Debug)]
pub struct ExplainArgs {
    /// Name of a server or skill in the manifest.
    pub name: String,
}

#[derive(Args, Debug)]
pub struct SessionArgs {
    #[command(subcommand)]
    pub cmd: SessionCmd,
}

#[derive(Subcommand, Debug)]
pub enum SessionCmd {
    /// Start a session: load a profile (+ optional plugin) for now.
    Start {
        /// Profile to load.
        profile: String,
        #[arg(long, value_enum, default_value_t = Scope::Project)]
        scope: Scope,
        /// Also install this plugin recipe for the session.
        #[arg(long)]
        plugin: Option<String>,
    },
    /// End the active session here (or everywhere with --all), reverting it.
    End {
        /// End every active session on this machine, not just this directory's.
        #[arg(long)]
        all: bool,
    },
    /// List active sessions.
    List,
    /// Freeze the active session's resolved set (profile servers + the skills
    /// actually loaded) into a new profile, so CI can replay it deterministically.
    Freeze {
        /// Name for the frozen profile (default: <profile>-frozen).
        #[arg(long)]
        name: Option<String>,
    },
}

#[derive(Args, Debug)]
pub struct RestoreArgs {
    /// Adapter id to restore (omit to list available backups).
    pub adapter: Option<String>,

    #[arg(long, value_enum)]
    pub scope: Option<Scope>,

    /// Actually restore (else show what would change).
    #[arg(long)]
    pub write: bool,
}

#[derive(Args, Debug)]
pub struct AdoptArgs {
    #[arg(long = "target", value_name = "ID")]
    pub targets: Vec<String>,

    #[arg(long, value_enum)]
    pub scope: Option<Scope>,

    /// Actually update agentstack.toml (else dry-run).
    #[arg(long)]
    pub write: bool,

    /// Don't store lifted secrets in the keychain (just reference them).
    #[arg(long)]
    pub no_keychain: bool,
}

#[derive(Args, Debug)]
pub struct ConsolidateArgs {
    /// Specific skill names to consolidate (default: all discovered).
    #[arg(value_name = "SKILL")]
    pub names: Vec<String>,

    /// Just list the skills found on disk; don't move anything.
    #[arg(long)]
    pub list: bool,
}

#[derive(Args, Debug)]
pub struct InstructionsArgs {
    #[arg(long = "target", value_name = "ID")]
    pub targets: Vec<String>,

    #[arg(long, value_enum)]
    pub scope: Option<Scope>,

    /// Actually write the instruction files (else dry-run).
    #[arg(long)]
    pub write: bool,
}

#[derive(Args, Debug)]
pub struct DoctorArgs {
    /// Exit nonzero if any check fails (for CI gates).
    #[arg(long)]
    pub ci: bool,

    /// Also perform live MCP handshakes against HTTP servers.
    #[arg(long)]
    pub live: bool,

    /// Repair safe issues (re-apply drifted target configs).
    #[arg(long)]
    pub fix: bool,
}

#[derive(Args, Debug)]
pub struct DashboardArgs {
    /// Port to bind (default: an OS-assigned free port).
    #[arg(long)]
    pub port: Option<u16>,

    /// Don't open the browser automatically.
    #[arg(long)]
    pub no_open: bool,

    /// Disable all writes: the dashboard can browse state and preview diffs but
    /// every mutation endpoint (apply, toggle, secrets, settings, install…) is
    /// refused. Without this flag the dashboard can write to disk.
    #[arg(long)]
    pub read_only: bool,
}

#[derive(Args, Debug)]
pub struct ExportArgs {
    /// Output file.
    #[arg(long, short, default_value = "agentstack-bundle.age")]
    pub output: PathBuf,

    /// Also include referenced secrets (resolved on this machine).
    #[arg(long)]
    pub secrets: bool,

    /// Passphrase (otherwise prompted).
    #[arg(long)]
    pub passphrase: Option<String>,
}

#[derive(Args, Debug)]
pub struct ImportArgs {
    /// Bundle file to import.
    pub file: PathBuf,

    /// Overwrite an existing manifest.
    #[arg(long)]
    pub force: bool,

    /// Don't restore secrets to the keychain.
    #[arg(long)]
    pub no_keychain: bool,

    /// Passphrase (otherwise prompted).
    #[arg(long)]
    pub passphrase: Option<String>,
}

#[derive(Args, Debug)]
pub struct SearchArgs {
    /// Free-text query over name, description, and tags (lists all if omitted).
    pub query: Option<String>,
}

#[derive(Args, Debug)]
pub struct AdaptersArgs {
    #[command(subcommand)]
    pub command: AdaptersCommand,
}

#[derive(Subcommand, Debug)]
pub enum AdaptersCommand {
    /// List known adapters and whether each CLI looks installed.
    List,
    /// Print one adapter descriptor.
    Show {
        /// Adapter id, e.g. `claude-code`.
        id: String,
    },
}

#[derive(Args, Debug)]
pub struct PluginsArgs {
    #[command(subcommand)]
    pub command: PluginsCommand,
}

#[derive(Subcommand, Debug)]
pub enum PluginsCommand {
    /// List AgentStack-managed plugin recipes in the manifest.
    List,
    /// Show generated/native install status and next actions for recipes.
    Status(PluginsStatusArgs),
    /// Create a plugin recipe from existing manifest servers, skills, and hooks.
    Create(Box<PluginsCreateArgs>),
    /// Adopt an installed native Claude Code or Codex plugin into the manifest.
    Adopt(PluginsAdoptArgs),
    /// Generate repo-local native plugin packages and marketplaces.
    Sync(PluginsSyncArgs),
    /// Add this repo marketplace to native harnesses and install a recipe.
    Install(PluginsNativeArgs),
    /// Remove a recipe from native harness plugin installs.
    Remove(PluginsNativeArgs),
}

#[derive(Args, Debug)]
pub struct PluginsCreateArgs {
    /// Recipe/native plugin id, e.g. `play`.
    pub name: String,
    /// Plugin version.
    #[arg(long, default_value = "0.1.0")]
    pub version: String,
    /// Human description shown in native plugin UIs.
    #[arg(long)]
    pub description: String,
    #[arg(long)]
    pub display: Option<String>,
    #[arg(long)]
    pub category: Option<String>,
    #[arg(long = "target", value_name = "ID")]
    pub targets: Vec<String>,
    #[arg(long = "server", value_name = "NAME")]
    pub servers: Vec<String>,
    #[arg(long = "skill", value_name = "NAME")]
    pub skills: Vec<String>,
    #[arg(long = "hook", value_name = "NAME")]
    pub hooks: Vec<String>,
    #[arg(long)]
    pub homepage: Option<String>,
    #[arg(long)]
    pub repository: Option<String>,
    #[arg(long)]
    pub license: Option<String>,
    #[arg(long)]
    pub author: Option<String>,
    /// Set plugin defaultEnabled=true in generated native manifests.
    #[arg(long)]
    pub default_enabled: bool,
    /// Actually update agentstack.toml (else dry-run).
    #[arg(long)]
    pub write: bool,
}

#[derive(Args, Debug)]
pub struct PluginsStatusArgs {
    /// Optional recipe name to inspect.
    pub name: Option<String>,
}

#[derive(Args, Debug)]
pub struct PluginsAdoptArgs {
    /// Native plugin name to adopt.
    pub name: String,
    /// Restrict adoption to one native harness.
    #[arg(long)]
    pub harness: Option<String>,
    /// Restrict adoption to one marketplace.
    #[arg(long)]
    pub marketplace: Option<String>,
    /// Override the AgentStack recipe name.
    #[arg(long)]
    pub as_name: Option<String>,
    /// Actually update agentstack.toml (else dry-run).
    #[arg(long)]
    pub write: bool,
}

#[derive(Args, Debug)]
pub struct PluginsSyncArgs {
    /// Only sync these target ids (repeatable). Defaults to Codex + Claude Code
    /// when their adapters exist.
    #[arg(long = "target", value_name = "ID")]
    pub targets: Vec<String>,
    /// Actually write generated files (else dry-run).
    #[arg(long)]
    pub write: bool,
}

#[derive(Args, Debug)]
pub struct PluginsNativeArgs {
    /// Plugin recipe name.
    pub name: String,
    /// Only act on these target ids (repeatable). Defaults to the recipe's targets.
    #[arg(long = "target", value_name = "ID")]
    pub targets: Vec<String>,
    /// Actually run native harness commands (else dry-run).
    #[arg(long)]
    pub write: bool,
}

#[derive(Args, Debug)]
pub struct SecretArgs {
    #[command(subcommand)]
    pub command: SecretCommand,
}

#[derive(Subcommand, Debug)]
pub enum SecretCommand {
    /// Store a secret in the keychain (prompts hidden if --value omitted).
    Set {
        name: String,
        /// Provide the value inline (otherwise you'll be prompted).
        #[arg(long)]
        value: Option<String>,
    },
    /// Print a secret's value.
    Get { name: String },
    /// Remove a secret from the keychain.
    Rm { name: String },
    /// Show every secret the manifest references and whether it resolves.
    List,
}
