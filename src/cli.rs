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

    /// Activate a profile: render its servers + materialize its skills.
    Use(UseArgs),

    /// Compile instruction fragments into each harness's CLAUDE.md / AGENTS.md.
    Instructions(InstructionsArgs),

    /// Pull hand-added servers from a target config back into the manifest.
    Adopt(AdoptArgs),

    /// Verify everything is wired up: adapters, secrets, drift, quirks, skills.
    Doctor(DoctorArgs),

    /// Search the capability catalog (and mark what's already added).
    Search(SearchArgs),

    /// Show local usage analytics (activation counts + footprint).
    Stats,

    /// Inspect the available CLI adapters.
    Adapters(AdaptersArgs),

    /// Manage secrets in the OS keychain.
    Secret(SecretArgs),

    /// Export the manifest (+ lock, + optionally secrets) as an encrypted bundle.
    Export(ExportArgs),

    /// Import an encrypted bundle on a new machine.
    Import(ImportArgs),

    /// Open the local web dashboard.
    Dashboard(DashboardArgs),
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
    /// Add an MCP server.
    Server(AddServerArgs),
    /// Add a skill (a SKILL.md directory).
    Skill(AddSkillArgs),
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

    /// Which scope to write: global (~) or project (repo). Defaults to project
    /// when a manifest is in the working dir, else global.
    #[arg(long, value_enum)]
    pub scope: Option<Scope>,
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
}

#[derive(Args, Debug)]
pub struct DashboardArgs {
    /// Port to bind (default: an OS-assigned free port).
    #[arg(long)]
    pub port: Option<u16>,

    /// Don't open the browser automatically.
    #[arg(long)]
    pub no_open: bool,

    /// Read-only mode (reserved; the dashboard is read-only in this phase).
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
