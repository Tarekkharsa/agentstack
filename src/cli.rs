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
                  from a single portable .agentstack/agentstack.toml.",
    after_help = "\
Start here:
  agentstack                     orientation + the one next step for this directory
  agentstack setup               guided one-command setup: import, preview, apply
  init → bootstrap → apply                       the same steps, run individually

The list above is the everyday surface. Everything else is grouped below —
run `agentstack <command> --help` for any of them:

  Capabilities & library   remove · install · update · lock · upgrade · lib · consolidate · adopt
  Activate & run           use · session · run · runs · kill · hook
  Zero-files bridge        connect · trust · disconnect · mcp · codemode
  Inspect & tune           diff · explain · audit · optimize · stats · restore · secret
  Share & extend           export · import · pack · plugins · adapters · self"
)]
pub struct Cli {
    /// Project or manifest directory (prefers .agentstack/agentstack.toml).
    #[arg(long, global = true, value_name = "DIR")]
    pub manifest_dir: Option<PathBuf>,

    /// Omitted → a short status overview (detected CLIs, manifest state, next
    /// step) instead of the full help.
    #[command(subcommand)]
    pub command: Option<Command>,
}

// The subcommand surface is large; to keep `--help` navigable we show only the
// everyday core loop in clap's native "Commands" list and hide the rest with
// `hide = true`. Hidden commands still run, still have their own `--help`, and
// are cataloged (grouped by how often you reach for them) in the `after_help`
// map on `Cli` above. This is display-only progressive disclosure — dispatch
// (src/main.rs) matches by variant, so grouping/ordering here is free to change
// without touching behavior. Promote a command to the everyday list by dropping
// its `hide` attribute (and moving it out of the after_help group).
#[derive(Subcommand, Debug)]
pub enum Command {
    // ── Everyday: the core loop most projects ever need (shown in --help) ─
    /// Guided one-command setup: import if needed, configure, preview, confirm,
    /// apply, then verify — the everyday loop behind a single command.
    Setup(SetupArgs),

    /// Discover installed CLIs and reverse-engineer a manifest from their
    /// existing MCP configs, lifting inline secrets into `${REF}`s.
    Init(InitArgs),

    /// Add a server or skill to the manifest.
    Add(AddArgs),

    /// Search the capability catalog (and mark what's already added).
    Search(SearchArgs),

    /// Guided setup: install skills, check secrets, preview/apply, then doctor.
    Bootstrap(BootstrapArgs),

    /// Render the manifest into each target's native config.
    ///
    /// Shows the diff first. In a terminal, asks before writing; pass `--write`
    /// to apply directly.
    Apply(ApplyArgs),

    /// Compile [instructions.*] fragments into each harness's CLAUDE.md /
    /// AGENTS.md (a managed region; hand-written prose is preserved). Dry-run
    /// by default; `--write` applies.
    Instructions(InstructionsArgs),

    /// Verify everything is wired up: adapters, secrets, drift, quirks, skills.
    Doctor(DoctorArgs),

    /// Open the local web dashboard.
    Dashboard(DashboardArgs),

    // ── Capabilities & library (hidden from --help; see the after_help map) ─
    /// Remove a server or skill from the manifest (and lockfile).
    #[command(hide = true)]
    Remove(RemoveArgs),

    /// Fetch skill sources into the store and write the lockfile.
    #[command(hide = true)]
    Install(InstallArgs),

    /// Re-resolve git skills to their latest and rewrite the lockfile.
    #[command(hide = true)]
    Update(UpdateArgs),

    /// Resolve each profile's skill + server refs (library-aware) and pin them
    /// in `agentstack.lock` — no configs rendered, no skills materialized. The
    /// lock-only counterpart of `use <profile> --write`, for clean-at-rest
    /// repos that keep no generated files.
    #[command(hide = true)]
    Lock(LockArgs),

    /// Re-resolve an installed vendor pack from its recorded source and apply
    /// any changes (server, skills, house rules), re-pinning the lockfile.
    #[command(hide = true)]
    Upgrade(UpgradeArgs),

    /// Manage the central capability library (`~/.agentstack/lib/`) that projects
    /// reference by name instead of copying files.
    #[command(hide = true)]
    Lib(LibArgs),

    /// Gather scattered skills from every CLI's skills dir into one managed
    /// home (`~/.agentstack/skills/`), symlinking the originals back.
    #[command(hide = true)]
    Consolidate(ConsolidateArgs),

    /// Pull hand-added servers from a target config back into the manifest.
    #[command(hide = true)]
    Adopt(AdoptArgs),

    // ── Activate & run ───────────────────────────────────────────────────
    /// Activate a profile: render its servers + materialize its skills.
    #[command(hide = true)]
    Use(UseArgs),

    /// Manage ephemeral sessions: load a profile (+ optional plugin) for now,
    /// then revert it. A safety hatch for the dashboard's session feature.
    #[command(hide = true)]
    Session(SessionArgs),

    /// Launch an agent CLI as a tracked run: optionally apply a profile for its
    /// lifetime, then observe/kill it here or from the dashboard.
    #[command(hide = true)]
    Run(RunArgs),

    /// List live tracked runs (harness, pid, profile, uptime).
    #[command(hide = true)]
    Runs(RunsArgs),

    /// Kill a tracked run by id (and revert its profile if it owned one).
    #[command(hide = true)]
    Kill(KillArgs),

    /// Print a shell hook for per-directory profile auto-activation.
    #[command(hide = true)]
    Hook(HookArgs),

    // ── Zero-files bridge ────────────────────────────────────────────────
    /// Register the agentstack gateway once, globally, in a harness's MCP
    /// config — after that, every trusted repo brings its own servers through
    /// `agentstack mcp --auto-project` with no per-project files (zero-files
    /// mode made automatic). Dry-run by default.
    #[command(hide = true)]
    Connect(ConnectArgs),

    /// Trust a project's manifest for the zero-files bridge (direnv-style).
    /// Until trusted, an auto-discovered project gets control-plane tools only:
    /// none of its servers are spawned or contacted, no secrets are resolved.
    /// Trust pins the manifest's content digest — editing the manifest requires
    /// re-trusting it.
    #[command(hide = true)]
    Trust(TrustArgs),

    /// Remove the agentstack gateway entry from a harness's global MCP config.
    #[command(hide = true)]
    Disconnect(DisconnectArgs),

    /// Run agentstack as an MCP server over stdio (for an agent to call).
    #[command(hide = true)]
    Mcp(McpArgs),

    /// Generate a typed code-mode client for this project's proxied MCP servers,
    /// so an agent can call several upstream tools from one program it runs in
    /// its own sandbox. Read-only by default; `--write` materializes the files.
    #[command(hide = true)]
    Codemode(CodemodeArgs),

    // ── Inspect & tune ───────────────────────────────────────────────────
    /// Show drift between the manifest and the on-disk configs.
    #[command(hide = true)]
    Diff(DiffArgs),

    /// Explain a server or skill: where it came from, what secrets it needs,
    /// which tools get it and what files get written, and its safety signals.
    #[command(hide = true)]
    Explain(ExplainArgs),

    /// Scan skill sources and instruction files for hidden Unicode and
    /// prompt-injection heuristics. Exits nonzero on high-severity findings.
    #[command(hide = true)]
    Audit(AuditArgs),

    /// Turn the signals agentstack already collects (usage, call audit log,
    /// context costs, trust ledger) into concrete recommendations: inert
    /// servers, firewall narrowing, denied/erroring tools, stale trust. Every
    /// recommendation carries evidence, the exact command/TOML, and why it is
    /// safe or needs review. Read-only by default; `--write` applies only the
    /// safe class.
    #[command(hide = true)]
    Optimize(OptimizeArgs),

    /// Show local usage analytics (activation counts + footprint + context cost).
    #[command(hide = true)]
    Stats(StatsArgs),

    /// Restore a CLI config from its pre-write backup (undo an apply).
    #[command(hide = true)]
    Restore(RestoreArgs),

    /// Manage secrets in the OS keychain.
    #[command(hide = true)]
    Secret(SecretArgs),

    // ── Share & extend ───────────────────────────────────────────────────
    /// Export the manifest (+ lock, + optionally secrets) as an encrypted bundle.
    #[command(hide = true)]
    Export(ExportArgs),

    /// Import an encrypted bundle on a new machine.
    #[command(hide = true)]
    Import(ImportArgs),

    /// Author a publishable pack (a git repo with a pack.toml).
    #[command(subcommand, hide = true)]
    Pack(PackCmd),

    /// Manage AgentStack plugin recipes and generated native marketplaces.
    #[command(hide = true)]
    Plugins(PluginsArgs),

    /// Inspect the available CLI adapters.
    #[command(hide = true)]
    Adapters(AdaptersArgs),

    /// Manage this binary's own install: `self link` puts a stable `agentstack`
    /// on PATH (a symlink, no installer needed); `self which` shows which
    /// binary a bare `agentstack` runs and flags stale links.
    #[command(name = "self", hide = true)]
    SelfCmd(SelfArgs),
}

#[derive(Args, Debug)]
pub struct SelfArgs {
    #[command(subcommand)]
    pub command: SelfCommand,
}

#[derive(Subcommand, Debug)]
pub enum SelfCommand {
    /// Symlink the running binary into a PATH dir, so `agentstack` works from
    /// every shell (interactive or not) without an installer or shell wrapper.
    Link(SelfLinkArgs),
    /// Show what `agentstack` on PATH resolves to vs the binary running now,
    /// flagging stale or broken links (e.g. after a rebuild).
    Which,
}

#[derive(Args, Debug)]
pub struct SelfLinkArgs {
    /// Directory to link into. Default: $AGENTSTACK_PREFIX, else
    /// /usr/local/bin when writable, else ~/.local/bin (same as install.sh).
    #[arg(long, value_name = "DIR")]
    pub prefix: Option<PathBuf>,

    /// Replace an existing regular file at the destination (an existing
    /// symlink is always re-pointed; a real file is refused without this).
    #[arg(long)]
    pub force: bool,
}

#[derive(Args, Debug)]
pub struct McpArgs {
    /// Discover the active project per session instead of pinning to the launch
    /// cwd: MCP client roots → cwd walk-up → $AGENTSTACK_MANIFEST_DIR → none.
    /// Auto-discovered projects are trust-gated (`agentstack trust`): an
    /// untrusted manifest exposes control-plane tools only. This is the flag
    /// `agentstack connect` registers.
    #[arg(long)]
    pub auto_project: bool,
}

#[derive(Args, Debug)]
pub struct ConnectArgs {
    /// Harness/adapter ids to register the gateway in (e.g. `claude-code`
    /// `codex`). With none given, use --all.
    #[arg(value_name = "HARNESS")]
    pub harnesses: Vec<String>,

    /// Register in every installed harness that supports MCP.
    #[arg(long)]
    pub all: bool,

    /// Write the change (else dry-run: show the diff per harness).
    #[arg(long)]
    pub write: bool,

    /// Path to the agentstack binary to register (default: this executable).
    #[arg(long, value_name = "PATH")]
    pub command: Option<String>,
}

#[derive(Args, Debug)]
pub struct DisconnectArgs {
    /// Harness/adapter ids to remove the gateway from.
    #[arg(value_name = "HARNESS")]
    pub harnesses: Vec<String>,

    /// Remove from every harness that currently has the gateway registered.
    #[arg(long)]
    pub all: bool,

    /// Write the change (else dry-run: show the diff per harness).
    #[arg(long)]
    pub write: bool,
}

#[derive(Args, Debug)]
pub struct TrustArgs {
    /// Project directory (walks up to find the manifest). Defaults to `.`.
    #[arg(value_name = "PATH")]
    pub path: Option<PathBuf>,

    /// List every trusted project and whether its manifest still matches.
    #[arg(long)]
    pub list: bool,

    /// Withdraw trust for the project instead of granting it.
    #[arg(long)]
    pub revoke: bool,
}

#[derive(Args, Debug)]
pub struct RunsArgs {
    /// Emit machine-readable JSON instead of the text table.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct RunArgs {
    /// Harness/adapter id to launch, e.g. `claude-code` or `codex`.
    pub harness: String,

    /// Apply this profile's servers + skills for the life of the run.
    #[arg(long, value_name = "NAME")]
    pub profile: Option<String>,

    /// Scope to apply the profile in (only meaningful with --profile).
    #[arg(long, value_enum, default_value_t = Scope::Project)]
    pub scope: Scope,

    /// Leave the applied profile in place after the run exits (default: revert).
    #[arg(long)]
    pub keep: bool,

    /// Extra arguments passed through to the harness (after `--`).
    #[arg(
        trailing_var_arg = true,
        allow_hyphen_values = true,
        value_name = "ARG"
    )]
    pub args: Vec<String>,
}

#[derive(Args, Debug)]
pub struct KillArgs {
    /// Run id (from `agentstack runs`).
    pub id: String,

    /// Send SIGKILL immediately instead of SIGTERM-then-escalate.
    #[arg(long)]
    pub force: bool,
}

#[derive(Args, Debug)]
pub struct CodemodeArgs {
    /// Write the generated client to `.agentstack/codemode/` (else dry-run: just
    /// show what would be written).
    #[arg(long)]
    pub write: bool,
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

    /// Install a skill even when content scanning finds high-severity issues
    /// (hidden Unicode). Findings still print as warnings.
    #[arg(long)]
    pub allow_flagged: bool,
}

#[derive(Args, Debug)]
pub struct UpdateArgs {
    /// Only update this skill (default: all git skills).
    pub name: Option<String>,
}

#[derive(Args, Debug)]
pub struct LockArgs {
    /// Only pin this profile's refs (default: every profile in the manifest).
    #[arg(long, value_name = "NAME")]
    pub profile: Option<String>,
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
pub struct UpgradeArgs {
    /// Vendor pack name (the `[plugins.<vendor>]` ledger key). Optional with
    /// `--all`.
    pub name: Option<String>,
    /// Re-resolve every installed pack instead of one.
    #[arg(long)]
    pub all: bool,
    /// Accept the vendor's house-rule instructions on upgrade (they steer your
    /// daily-driver agent). Required to apply an instruction-body change to a
    /// pack that has instructions installed.
    #[arg(long)]
    pub with_instructions: bool,
    /// Accept all changes — including instruction-body changes — without the
    /// confirmation gate. For CI / scripting.
    #[arg(long)]
    pub yes: bool,
    /// Write the change (else dry-run / diff preview).
    #[arg(long)]
    pub write: bool,
}

#[derive(Args, Debug)]
pub struct BootstrapArgs {
    /// Only act on these target ids (repeatable). Defaults to [targets].default.
    #[arg(long = "target", value_name = "ID")]
    pub targets: Vec<String>,

    /// Bootstrap only the servers in this profile.
    #[arg(long, value_name = "NAME")]
    pub profile: Option<String>,

    /// Which scope to write: global (~) or project (repo). Defaults to global.
    #[arg(long, value_enum)]
    pub scope: Option<Scope>,

    /// Check the lockfile without updating it during the install step.
    #[arg(long)]
    pub locked: bool,

    /// Actually install/apply. Without this flag, bootstrap is a read-only
    /// preflight plus diff preview.
    #[arg(long)]
    pub write: bool,
}

/// `setup` is the interactive newcomer wizard; it deliberately has no `--write`
/// (it confirms in a terminal and stays dry-run everywhere else). Scripts use
/// `init` + `bootstrap --write`.
#[derive(Args, Debug)]
pub struct SetupArgs {
    /// Only configure these target ids (repeatable). Defaults to [targets].default.
    #[arg(long = "target", value_name = "ID")]
    pub targets: Vec<String>,

    /// Configure only the servers in this profile.
    #[arg(long, value_name = "NAME")]
    pub profile: Option<String>,

    /// Which scope to write: global (~) or project (repo). Defaults to global.
    #[arg(long, value_enum)]
    pub scope: Option<Scope>,
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
    /// For packs: also install the vendor's house-rule instructions (opt-in —
    /// they steer your daily-driver agent). Off by default.
    #[arg(long)]
    pub with_instructions: bool,
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
    /// Seed the machine-level manifest (`~/.agentstack/agentstack.toml`)
    /// instead of importing a project: an empty [instructions] block plus an
    /// `instructions/` dir for personal, cross-project fragments compiled into
    /// each CLI's global CLAUDE.md / AGENTS.md. Nothing is imported.
    #[arg(long)]
    pub global: bool,

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

    /// Show what would change without writing, and skip the interactive prompt.
    #[arg(long)]
    pub dry_run: bool,

    /// Write the changes to disk without prompting.
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

    /// Also prune global entries that a *different* manifest applied. By
    /// default those are kept (and reported) — pruning them would silently
    /// delete another setup's servers; `agentstack adopt` pulls them into
    /// this manifest instead.
    #[arg(long)]
    pub prune_foreign: bool,

    /// Skip the managed .gitignore block for generated project artifacts —
    /// pass this when your team commits the rendered files.
    #[arg(long)]
    pub no_gitignore: bool,
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

    /// Also prune global entries that a *different* manifest applied (kept
    /// and reported by default — see `agentstack apply --help`).
    #[arg(long)]
    pub prune_foreign: bool,

    /// Skip the managed .gitignore block for generated project artifacts —
    /// pass this when your team commits the rendered files.
    #[arg(long)]
    pub no_gitignore: bool,
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

    /// Overwrite a library entry that already exists with different content.
    #[arg(long)]
    pub replace: bool,

    /// Write the changes (else dry-run/preview).
    #[arg(long)]
    pub write: bool,
}

#[derive(Args, Debug)]
pub struct LibArgs {
    #[command(subcommand)]
    pub kind: LibKind,
}

#[derive(Subcommand, Debug)]
pub enum LibKind {
    /// Add a skill to the central library from a local path or git source.
    Add(LibAddArgs),
    /// Add an MCP server definition to the central library from a `.toml` file.
    AddServer(LibAddServerArgs),
    /// List the skills and servers installed in the central library.
    List,
    /// Remove a skill from the central library.
    Remove(LibRemoveArgs),
    /// Remove a server from the central library.
    RemoveServer(LibRemoveServerArgs),
    /// Migrate skills from the legacy `~/.agentstack/skills/` home into the
    /// central library. Copy-first and reversible: originals are left in place.
    Migrate(LibMigrateArgs),
}

#[derive(Args, Debug)]
pub struct LibAddServerArgs {
    /// The name projects will reference this server by.
    pub name: String,
    /// Path to a server definition `.toml` (a `manifest::Server` table, with
    /// `${REF}` secrets only — never plaintext).
    #[arg(long, conflicts_with = "from_manifest")]
    pub file: Option<String>,
    /// Lift the `[servers.<name>]` definition from the current manifest into
    /// the library instead of reading a file.
    #[arg(long)]
    pub from_manifest: bool,
    /// Overwrite an existing library server of the same name.
    #[arg(long)]
    pub replace: bool,
    /// Write the change (else dry-run/preview).
    #[arg(long)]
    pub write: bool,
}

#[derive(Args, Debug)]
pub struct LibRemoveServerArgs {
    /// The library server name to remove.
    pub name: String,
    /// Write the change (else dry-run/preview).
    #[arg(long)]
    pub write: bool,
}

#[derive(Args, Debug)]
pub struct LibMigrateArgs {
    /// Overwrite library entries that already exist with the same name.
    #[arg(long)]
    pub replace: bool,
    /// Write the change (else dry-run/preview).
    #[arg(long)]
    pub write: bool,
}

#[derive(Args, Debug)]
pub struct LibRemoveArgs {
    /// The library skill name to remove.
    pub name: String,
    /// Write the change (else dry-run/preview).
    #[arg(long)]
    pub write: bool,
}

#[derive(Args, Debug)]
pub struct LibAddArgs {
    /// The name projects will reference this skill by.
    pub name: String,
    /// Add from a local skill directory (must contain SKILL.md).
    #[arg(long, conflicts_with = "git")]
    pub path: Option<String>,
    /// Add from a git source URL.
    #[arg(long, conflicts_with = "path")]
    pub git: Option<String>,
    /// Pin a git revision (branch, tag, or commit). Git sources only.
    #[arg(long, requires = "git")]
    pub rev: Option<String>,
    /// Overwrite an existing library entry of the same name.
    #[arg(long)]
    pub replace: bool,
    /// Write the change (else dry-run/preview).
    #[arg(long)]
    pub write: bool,
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

    /// Run the supply-chain content scan (reads every skill body — slow on
    /// large libraries). Always on with --ci.
    #[arg(long)]
    pub deep: bool,
}

#[derive(Args, Debug)]
pub struct AuditArgs {
    /// Emit machine-readable JSON instead of the text report.
    #[arg(long)]
    pub json: bool,

    /// Summarize the runtime call audit log (~/.agentstack/audit/calls.jsonl):
    /// every tool call brokered by the gateway, grouped by server/tool, with
    /// denials. Argument values are never logged — only digests.
    #[arg(long)]
    pub calls: bool,

    /// With --calls: only entries from the last N days.
    #[arg(long, value_name = "DAYS")]
    pub since: Option<u64>,
}

#[derive(clap::Subcommand, Debug)]
pub enum PackCmd {
    /// Scaffold a pack.toml + example skill in the current directory. Publish
    /// by pushing the repo and tagging a version (e.g. v0.1.0); install with
    /// `agentstack add from git:<host>/<repo>@<tag>`.
    Init(PackInitArgs),
}

#[derive(Args, Debug)]
pub struct PackInitArgs {
    /// Pack name (defaults to the current directory's name).
    pub name: Option<String>,
}

#[derive(Args, Debug)]
pub struct StatsArgs {
    /// Measure each server's live context cost (tools/list token footprint)
    /// through the gateway, then cache it for offline display. Spawns/contacts
    /// the manifest's servers once.
    #[arg(long)]
    pub live: bool,
}

#[derive(Args, Debug)]
pub struct OptimizeArgs {
    /// Emit the recommendations as machine-readable JSON.
    #[arg(long)]
    pub json: bool,

    /// Apply the recommendations marked safe (inert manifest entries, dead
    /// trust grants). Everything else stays a printed suggestion.
    #[arg(long, conflicts_with = "json")]
    pub write: bool,

    /// Only consider audit-log records from the last N days.
    #[arg(long, value_name = "DAYS")]
    pub since: Option<u64>,
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
