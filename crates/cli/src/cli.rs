//! Command-line surface (clap derive). The visible set is the beginner loop —
//! init/status/add/search/apply/use/doctor/run/trust (`setup` survives only as
//! a hidden alias of init) — and EVERY command, visible or hidden, appears in
//! the task-grouped map in `after_help` below, so `--help` is one complete
//! screen: a short list to start from, a full map to grow into. Hidden
//! commands still run and still have their own `--help`.

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
  agentstack init                one command sets up everything: import, choose, apply, verify
  agentstack status              where this project stands, on one screen

The list above is the everyday loop. This is the full map, grouped by task —
every command (listed or not) has its own --help:

  Set up      init · status · adapters · settings · self
  Edit        add · set · search · remove · install · lib · adopt · export · import
  Render      apply · use · instructions · lock · session · diff · restore
  Protect     trust · explain · secret · guard · sign · verify
  Run         run · kill · workflow · gateway · mcp · try
  Inspect     doctor · report · dashboard · optimize · proxy

Words: a CLI (a.k.a. harness) is the agent tool you run; an adapter compiles
its native config; [targets] in the manifest lists which CLIs commands act on.

Full inventory with one-line summaries: agentstack --help --all"
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
    /// Hidden alias of interactive `init` — same guided wizard, older name.
    ///
    /// Kept so muscle memory and old links keep working; never advertised.
    #[command(hide = true)]
    Setup(SetupArgs),

    /// Set up everything in one command: detect, import, choose, apply, verify.
    ///
    /// Detects CLIs, imports their configs, lets you choose where secrets
    /// live, previews, confirms, applies, and verifies. Interactive runs are
    /// guided; scripts get the promptless primitive via flags.
    Init(InitArgs),

    /// Where this project stands, on one screen: detected CLIs, manifest,
    /// trust, secrets, and the one next step.
    ///
    /// The same orientation bare `agentstack` prints — reachable by name so
    /// muscle memory (`git status`, `docker status`, …) and scripts land
    /// somewhere useful. Deep verification stays in `agentstack doctor`.
    Status(StatusArgs),

    /// Add a server or skill to the manifest.
    Add(AddArgs),

    /// Create or update a manifest entry in place (idempotent `add`).
    ///
    /// `set server <name> …` writes the definition whether or not the name
    /// already exists — the safe, copy-pasteable repair path when validation
    /// flags a bad field. Same flags as `add server`.
    #[command(hide = true)]
    Set(SetArgs),

    /// Search the capability catalog (and mark what's already added).
    Search(SearchArgs),

    /// Render the manifest into each target's native config.
    ///
    /// Shows the diff first. In a terminal, asks before writing; pass `--write`
    /// to apply directly.
    Apply(ApplyArgs),

    /// Compile [instructions.*] into each CLI's CLAUDE.md / AGENTS.md.
    ///
    /// Fragments render into a managed region; hand-written prose is
    /// preserved. Dry-run by default; `--write` applies.
    #[command(hide = true)]
    Instructions(InstructionsArgs),

    /// Verify everything is wired up: adapters, secrets, drift, skills, per-CLI details.
    Doctor(DoctorArgs),

    /// Open the local web dashboard — a read-only view of your stack.
    ///
    /// Shows state, diffs, doctor, runs, and audited calls. Every change
    /// happens through the CLI.
    #[command(hide = true)]
    Dashboard(DashboardArgs),

    // ── Capabilities & library ───────────────────────────────────────────
    /// Remove a server or skill from the manifest (and lockfile).
    #[command(hide = true)]
    Remove(RemoveArgs),

    /// Fetch skill sources into the store and write the lockfile.
    #[command(hide = true)]
    Install(InstallArgs),

    /// Resolve each profile's skill + server refs and pin `agentstack.lock`.
    ///
    /// Library-aware resolution; no configs rendered, no skills
    /// materialized — the lock-only counterpart of `use <profile> --write`,
    /// for clean-at-rest repos that keep no generated files. `--update`
    /// re-resolves git skills to their latest first; `--upgrade` re-resolves
    /// an installed vendor pack and applies its changes.
    #[command(hide = true)]
    Lock(LockArgs),

    /// Try a skill without installing anything: stage, scan, and emit a
    /// wrapper prompt on stdout for piping into any agent CLI.
    ///
    /// `agentstack try owner/repo --skill pdf | claude` — no manifest, lock,
    /// or config is touched; support files land under ~/.agentstack/try/.
    #[command(hide = true)]
    Try(TryArgs),

    /// Manage the central capability library.
    ///
    /// `~/.agentstack/lib/` holds capabilities that projects reference by
    /// name instead of copying files.
    #[command(hide = true)]
    Lib(LibArgs),

    /// Keep a hand-edit: pull drifted native config back into the manifest.
    ///
    /// Imports hand-added servers and hand-edited fields from target configs
    /// so the manifest stays the source of truth.
    #[command(hide = true)]
    Adopt(AdoptArgs),

    // ── Activate & run ───────────────────────────────────────────────────
    /// Activate a profile: render its servers + materialize its skills.
    Use(UseArgs),

    /// Manage ephemeral sessions: load a profile for now, then revert it.
    #[command(hide = true)]
    Session(SessionArgs),

    /// Launch an agent CLI as a tracked run.
    ///
    /// Optionally apply a profile for its lifetime, then observe/kill it
    /// here or from the dashboard.
    Run(RunArgs),

    /// Kill a tracked run by id (and revert its profile if it owned one).
    #[command(hide = true)]
    Kill(KillArgs),

    /// Exec-through launcher shim for external supervisors (e.g. t3code).
    ///
    /// `shim make <cli>` writes a tiny wrapper under `~/.agentstack/shims/`;
    /// point the supervisor's binary-path setting at it and every session it
    /// starts gets a per-run identity (`AGENTSTACK_RUN_ID` + `events.jsonl`)
    /// instead of landing in the global audit only. Read-only toward the
    /// supervisor: agentstack never edits its settings.
    #[command(hide = true, subcommand)]
    Shim(ShimCmd),

    /// Governed workflows (preview): run a pinned workflow under full
    /// admission — trust gate, strict lock verification, machine-capped
    /// ceilings — with every `agent()` call becoming a locked child run.
    /// Hidden while the lane is preview (Stage E adds `workflow report`).
    #[command(subcommand, hide = true)]
    Workflow(WorkflowCmd),

    /// Every "what happened" view in one place.
    ///
    /// A sandboxed run's flight recorder, live tracked runs, usage
    /// analytics, and brokered-call activity.
    #[command(subcommand, hide = true)]
    Report(ReportCmd),

    /// Sign this project's agentstack.lock with a fresh ed25519 key (writes a
    /// detached agentstack.lock.sig, prints the public key to publish).
    #[command(hide = true)]
    Sign(SignArgs),

    /// Verify agentstack.lock against a published ed25519 public key and its
    /// detached signature.
    #[command(hide = true)]
    Verify(VerifyArgs),

    /// Machine-level destructive-command guard.
    ///
    /// Wires `agentstack guard check` into every detected agent CLI as a
    /// pre-tool-use hook. Blocks destructive commands (rm -rf, git reset
    /// --hard, …), reads/writes of `[policy.filesystem] deny` paths (.env
    /// and friends), and writes outside the workspace + `[guard]
    /// allow_roots`. Cooperative accident protection — the kernel-enforced
    /// story is `run --sandbox`.
    #[command(hide = true)]
    Guard(GuardArgs),

    // ── Zero-files gateway ────────────────────────────────────────────────
    /// The zero-files gateway: register it once per CLI (`connect`) and
    /// every trusted repo brings its own servers through `agentstack mcp
    /// --auto-project` with no per-project files.
    #[command(subcommand, hide = true)]
    Gateway(GatewayCmd),

    /// Trust a project's manifest for the zero-files gateway (direnv-style).
    ///
    /// Until trusted, an auto-discovered project gets control-plane tools
    /// only: none of its servers are spawned or contacted, no secrets are
    /// resolved. Trust pins the content digest of the manifest layers AND
    /// the lockfile — editing either (a `git pull`, an `agentstack lock`)
    /// requires re-trusting.
    Trust(TrustArgs),

    /// Run agentstack as an MCP server over stdio (for an agent to call).
    #[command(hide = true)]
    Mcp(McpArgs),

    // ── Inspect & tune ───────────────────────────────────────────────────
    /// Show drift between the manifest and the on-disk configs.
    #[command(hide = true)]
    Diff(DiffArgs),

    /// Explain a server, skill, or instruction before you rely on it.
    ///
    /// Shows where it came from, what secrets it needs, which tools get it and
    /// what files get written, and its safety signals.
    #[command(
        hide = true,
        after_help = "\
Examples:
  agentstack explain github
  agentstack explain sql-review"
    )]
    Explain(ExplainArgs),

    /// Turn agentstack's collected signals into concrete recommendations.
    ///
    /// Usage, call audit log, context costs, and trust ledger feed
    /// inert-server, firewall-narrowing, denied/erroring-tool, and
    /// stale-trust findings. Every recommendation carries evidence, the
    /// exact command/TOML, and why it is safe or needs review. Read-only by
    /// default; `--write` applies only the safe class.
    #[command(hide = true)]
    Optimize(OptimizeArgs),

    /// Start the wire relay: a localhost proxy in front of the Anthropic API.
    ///
    /// Forwards every request verbatim (observe only) while accounting the
    /// tools block's per-turn token cost. Point a CLI at it with
    /// `ANTHROPIC_BASE_URL=http://127.0.0.1:<port>`, then rank what it
    /// observed with `agentstack report wire`.
    #[command(hide = true)]
    Proxy(ProxyStartArgs),

    /// Undo a recorded write: revert an apply/use/session history entry
    /// (servers, settings, hooks, instructions), or restore one adapter's
    /// config from its single-slot backup.
    #[command(
        hide = true,
        after_help = "\
Examples:
  agentstack restore --last --write
  agentstack restore claude-code --write"
    )]
    Restore(RestoreArgs),

    /// Manage secrets in the OS keychain.
    #[command(hide = true)]
    Secret(SecretArgs),

    /// Edit a target's native `[settings.<target>]` entries.
    ///
    /// e.g. Claude Code `model`, instead of hand-editing the manifest.
    /// Dry-run by default; `--write` applies.
    #[command(hide = true)]
    Settings(SettingsArgs),

    // ── Share & extend ───────────────────────────────────────────────────
    /// Export the manifest (+ lock, + optionally secrets) as an encrypted bundle.
    #[command(hide = true)]
    Export(ExportArgs),

    /// Import an encrypted bundle on a new machine.
    #[command(hide = true)]
    Import(ImportArgs),

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
    /// Regenerate the "All commands" inventory in docs/reference.md from the
    /// live clap tree. No flag prints the block; `--write` splices it into the
    /// managed region. A maintainer/CI command, not part of the daily surface.
    #[command(hide = true)]
    Docs(SelfDocsArgs),
}

#[derive(Args, Debug)]
pub struct SelfDocsArgs {
    /// Splice the generated block into docs/reference.md (else print to stdout).
    #[arg(long)]
    pub write: bool,
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
    /// `agentstack gateway connect` registers.
    #[arg(long)]
    pub auto_project: bool,

    /// Advertise the proxied upstream tools in `tools/list` (policy-filtered,
    /// namespaced `<server>__<tool>`) so any standard MCP client can call them
    /// without learning agentstack's control-plane tools first. Default is
    /// compact mode: upstream tools are reached via `tools_search`/code mode,
    /// keeping the agent's tool context small.
    #[arg(long)]
    pub transparent: bool,

    /// Consume a frozen run-grant artifact written by `agentstack run
    /// --locked` instead of re-deriving authority from disk (D2). Fail-closed:
    /// a missing, stale, wrong-project, or version-skewed artifact serves
    /// NOTHING — never a fallback to disk re-derivation. Not meant to be set
    /// by hand; the launch-scoped config written by `run --locked` carries it.
    #[arg(
        long,
        value_name = "PATH",
        hide = true,
        conflicts_with = "auto_project"
    )]
    pub grant: Option<std::path::PathBuf>,
}

#[derive(Args, Debug)]
pub struct ConnectArgs {
    /// CLI ids to register the gateway in (e.g. `claude-code`
    /// `codex`). With none given, use --all.
    #[arg(value_name = "CLI")]
    pub harnesses: Vec<String>,

    /// Register in every installed CLI that supports MCP.
    #[arg(long)]
    pub all: bool,

    /// Register the gateway in transparent mode (`agentstack mcp --auto-project
    /// --transparent`): upstream tools are advertised in `tools/list` instead
    /// of being reached via `tools_search`.
    #[arg(long)]
    pub transparent: bool,

    /// Write the change (else preview).
    #[arg(long)]
    pub write: bool,

    /// Path to the agentstack binary to register (default: this executable).
    #[arg(long, value_name = "PATH")]
    pub command: Option<String>,
}

#[derive(Args, Debug)]
pub struct DisconnectArgs {
    /// CLI ids to remove the gateway from.
    #[arg(value_name = "CLI")]
    pub harnesses: Vec<String>,

    /// Remove from every CLI that currently has the gateway registered.
    #[arg(long)]
    pub all: bool,

    /// Write the change (else preview).
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

    /// Grant without a terminal: acknowledge the review non-interactively
    /// (required when stdin is not a TTY).
    #[arg(long)]
    pub yes: bool,

    /// Emit the review surface as JSON and grant NOTHING (read-only). The
    /// machine-readable consent screen for external UIs — the actual grant
    /// stays the gated `agentstack trust` flow.
    #[arg(long)]
    pub preview: bool,
}

#[derive(Args, Debug)]
pub struct RunsArgs {
    /// Emit machine-readable JSON instead of the text table.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct ReportArgs {
    /// The run id (e.g. `r-1a2b3c4d`), as shown when `run --sandbox` starts.
    pub run: String,

    /// Emit the report as JSON instead of the human-readable form.
    #[arg(long)]
    pub json: bool,
}

/// One front door for every "what happened" view. The subcommands keep their
/// original implementations; only the entry point moved here.
#[derive(Subcommand, Debug)]
pub enum ReportCmd {
    /// Show a sandboxed run's flight-recorder report (lifecycle, egress
    /// decisions, and tool calls) by run id.
    Run(ReportArgs),

    /// List live tracked runs (CLI, pid, profile, uptime).
    Runs(RunsArgs),

    /// Show local usage analytics (activation counts + footprint + context
    /// cost).
    Usage(StatsArgs),

    /// Report brokered call activity and library-wide dead weight.
    ///
    /// From the audit log: capabilities installed but never used.
    /// Read-only, local.
    Calls(AnalyzeArgs),

    /// Rank what's been observed on the wire by the `proxy` relay.
    ///
    /// Per-capability tokens/turn, how many turns each tool was actually
    /// called, and a loaded-vs-called hint. On-wire ground truth
    /// complementing `report usage`.
    Wire(WireArgs),
}

/// Governed workflows (preview, design doc §12.4 Stage C): the drive-loop
/// composition over the `agentstack-workflow` engine.
#[derive(Subcommand, Debug)]
pub enum WorkflowCmd {
    /// Run a pinned `[workflows.<name>]` entry: admission first (trust,
    /// strict lock verify, roles resolved to profiles, ceilings intersected),
    /// then the governed drive loop — each `agent()` call spawns a locked
    /// child run under its role profile's fence, with per-child MCP config
    /// injection where the harness supports it.
    Run(WorkflowRunArgs),

    /// Render a workflow run's evidence tree (Stage E): identity and
    /// effective ceilings, each step joined to its child run's recorded
    /// grant digest / posture / outcome, taint marks, and the honest
    /// posture label — evidence as recorded, never reconstructed.
    Report(WorkflowReportArgs),

    /// List every declared `[workflows.*]` manifest entry with its admission
    /// status (trust + lock), read-only.
    ///
    /// Unlike `run`, this lists EVERY declared entry — including untrusted
    /// or drifted ones — so it never gates on admission; it reports the
    /// admission state instead.
    List(WorkflowListArgs),
}

#[derive(Args, Debug)]
pub struct WorkflowRunArgs {
    /// The `[workflows.<name>]` entry to run (must be pinned and trusted).
    #[arg(value_name = "NAME")]
    pub name: String,

    /// JSON exposed to the script as its read-only `args` global. Untrusted
    /// invoker input: size- and depth-bounded before it reaches the engine.
    #[arg(long = "args-json", value_name = "JSON")]
    pub args_json: Option<String>,

    /// Resume an interrupted workflow run (`w-…`) by replaying its recorded
    /// step results — no journaled step re-executes. Byte-identical is the
    /// precondition: the same pinned script, the same effective ceilings and
    /// roles, and the same `--args-json` bytes as the original invocation;
    /// any divergence refuses. Only a run with no recorded terminal outcome,
    /// or one ended by `wall_deadline` / `watchdog_kill`, is resumable — the
    /// resumed session gets a fresh wall clock. Assumes the original session
    /// is dead (no cross-process liveness guard).
    #[arg(long = "resume", value_name = "RUN_ID")]
    pub resume: Option<String>,
}

#[derive(Args, Debug)]
pub struct WorkflowReportArgs {
    /// The workflow run id (`w-…`, printed on the run's admission banner).
    #[arg(value_name = "RUN_ID")]
    pub run_id: String,

    /// Emit the evidence tree as JSON instead of the human-readable text
    /// render — the same recorded join, structured for scripting.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct WorkflowListArgs {
    /// Emit the declared workflow list as JSON instead of a human table.
    #[arg(long)]
    pub json: bool,
}

/// The zero-files gateway lifecycle: `connect` registers it in a harness's
/// global MCP config, `disconnect` removes it. The gateway process itself is
/// the (machine-invoked) `agentstack mcp` — that name is written into harness
/// configs, so it stays a top-level command.
#[derive(Subcommand, Debug)]
pub enum GatewayCmd {
    /// Register the agentstack gateway once, globally, in a CLI's MCP
    /// config.
    ///
    /// After that, every trusted repo brings its own servers through
    /// `agentstack mcp --auto-project` with no per-project files. Dry-run by
    /// default.
    Connect(ConnectArgs),

    /// Remove the agentstack gateway entry from a CLI's global MCP config.
    Disconnect(DisconnectArgs),
}

#[derive(Args, Debug)]
pub struct SignArgs {
    /// Print only the public-key line (for scripting).
    #[arg(long)]
    pub print_key_only: bool,
}

#[derive(Args, Debug)]
pub struct VerifyArgs {
    /// The publisher's ed25519 public key (64 hex chars).
    #[arg(long)]
    pub pubkey: String,

    /// The detached signature (128 hex chars). Defaults to reading
    /// `agentstack.lock.sig` next to the lockfile.
    #[arg(long)]
    pub signature: Option<String>,
}

#[derive(Args, Debug)]
pub struct RunArgs {
    /// CLI id to launch, e.g. `claude-code` or `codex` (`agentstack adapters list` shows all ids).
    #[arg(value_name = "CLI")]
    pub harness: String,

    /// Promote this host run to the Protected tier (fail-closed): refuse to
    /// launch unless the project is explicitly trusted, every input in the
    /// declared integrity surface is pinned and matching, and the declared
    /// capability requests fit under the machine policy ceiling — recording
    /// what was decided, including refusals. No Docker required. Not kernel
    /// isolation: see the printed limits.
    #[arg(long)]
    pub locked: bool,

    /// Run the harness headless with TEXT as its prompt (requires --locked).
    /// The prompt is delivered as one whole argv element via the adapter's
    /// declared headless invocation (e.g. `claude -p`, `codex exec`) — never
    /// through a shell — and is committed verbatim into the frozen grant's
    /// argv, so the evidence binds what the agent was asked to do. Stdout is
    /// captured (bounded), relayed to this process's stdout, and recorded by
    /// digest + byte count only; all launcher banners go to stderr so stdout
    /// carries the harness output and nothing else. Cannot be combined with
    /// trailing harness arguments (they would land after the prompt's `--`
    /// terminator and silently misparse as positionals).
    #[arg(long, value_name = "TEXT")]
    pub prompt: Option<String>,

    /// Apply this profile's servers + skills for the life of the run.
    #[arg(long, value_name = "NAME")]
    pub profile: Option<String>,

    /// Scope to apply the profile in (only meaningful with --profile).
    /// Defaults to the manifest home: global for the machine manifest,
    /// project for a repository manifest.
    #[arg(long, value_enum)]
    pub scope: Option<Scope>,

    /// Leave the applied profile in place after the run exits (default: revert).
    #[arg(long)]
    pub keep: bool,

    /// Launch the CLI inside a sandbox container instead of on the host
    /// (Phase 2). The container mounts the project as its workspace and points
    /// HTTPS traffic at the policy proxy, but its ordinary bridge still permits
    /// direct connections that ignore the proxy. Use `--lockdown` to remove that
    /// route. Requires a build with `--features sandbox` and a running Docker
    /// daemon.
    #[arg(long)]
    pub sandbox: bool,

    /// Stronger egress confinement (implies --sandbox): put the container on
    /// an internal Docker network with NO host route and NO internet, whose
    /// only reachable peer is the AgentStack egress-proxy sidecar. Ignoring
    /// the proxy env then reaches nothing. The sidecar image is pulled from
    /// GHCR (published per release, pinned to this version); override with
    /// `AGENTSTACK_EGRESS_IMAGE` (e.g. a local docker/egress-proxy.Dockerfile
    /// build).
    #[arg(long)]
    pub lockdown: bool,

    /// Print the fully-assembled execution plan — trust state, effective policy
    /// mount, egress mode, and the exact command — then exit WITHOUT running
    /// anything. The one auditable description of what a sandbox run would do.
    /// Works without Docker or the `sandbox` feature.
    #[arg(long)]
    pub plan: bool,

    /// Extra arguments passed through to the CLI (after `--`).
    #[arg(
        trailing_var_arg = true,
        allow_hyphen_values = true,
        value_name = "ARG"
    )]
    pub args: Vec<String>,
}

#[derive(Args, Debug)]
pub struct KillArgs {
    /// Run id (from `agentstack report runs`).
    pub id: String,

    /// Send SIGKILL immediately instead of SIGTERM-then-escalate.
    #[arg(long)]
    pub force: bool,
}

#[derive(Args, Debug)]
pub struct GuardArgs {
    #[command(subcommand)]
    pub cmd: GuardCmd,
}

#[derive(clap::Subcommand, Debug)]
pub enum GuardCmd {
    /// The hook entrypoint (agent CLIs call this; you rarely will): reads
    /// one tool-call payload from stdin, answers in the CLI's own dialect.
    #[command(hide = true)]
    Check {
        /// Payload/response dialect: claude, codex, gemini, cursor,
        /// copilot, antigravity, windsurf. Omitted → detected from the
        /// payload shape.
        #[arg(long)]
        protocol: Option<String>,
    },
    /// Judge a shell command against the current guard policy and exit
    /// nonzero on deny — try `agentstack guard test rm -rf /`.
    Test {
        /// The command (quoted or as trailing words).
        #[arg(trailing_var_arg = true, required = true)]
        command: Vec<String>,
    },
    /// Wire the guard into every detected hook-capable CLI (global scope)
    /// and seed [guard] + [policy.filesystem] deny in the machine manifest.
    Install {},
    /// Remove every hook `install` wrote and set [guard] enabled = false.
    Uninstall {},
    /// Show guard config and per-CLI installation state.
    Status {},
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
pub struct TryArgs {
    /// owner/repo, a git URL, or a spelled local path (./dir, /abs, ~/dir).
    pub source: String,
    /// The skill to run when the source holds several.
    #[arg(long)]
    pub skill: Vec<String>,
    /// Branch/tag/commit to resolve (git sources).
    #[arg(long)]
    pub rev: Option<String>,
    /// Directory within the repo to scope discovery to (git sources).
    #[arg(long)]
    pub subpath: Option<String>,
    /// Admit content the scan flagged high-severity.
    #[arg(long)]
    pub allow_flagged: bool,
}

#[derive(Args, Debug, Default)]
pub struct LockArgs {
    /// Only pin this profile's refs (default: every profile in the manifest).
    #[arg(long, value_name = "NAME")]
    pub profile: Option<String>,

    /// Re-resolve git skills to their latest and rewrite the lockfile — all
    /// git skills, or just NAME.
    #[arg(long, value_name = "NAME", num_args = 0..=1)]
    pub update: Option<Option<String>>,

    /// Re-resolve an installed vendor pack from its recorded source and apply
    /// any changes (server, skills, house rules), re-pinning the lockfile.
    /// Names one pack; combine with --all for every installed pack.
    #[arg(long, value_name = "PACK", num_args = 0..=1)]
    pub upgrade: Option<Option<String>>,

    /// With --upgrade: re-resolve every installed pack instead of one.
    #[arg(long, requires = "upgrade")]
    pub all: bool,

    /// With --upgrade: accept the vendor's house-rule instructions on upgrade.
    #[arg(long, requires = "upgrade")]
    pub with_instructions: bool,

    /// With --upgrade: accept all changes without the confirmation gate (CI).
    #[arg(long, requires = "upgrade")]
    pub yes: bool,

    /// With --upgrade: write the change (else preview).
    #[arg(long, requires = "upgrade")]
    pub write: bool,
}

#[derive(Args, Debug)]
pub struct RemoveArgs {
    /// Name of the server or skill to remove.
    pub name: String,
    /// Write the change (else preview).
    #[arg(long)]
    pub write: bool,
}

#[derive(Args, Debug)]
pub struct UpgradeArgs {
    /// Vendor pack name (the `[packs.<vendor>]` ledger key). Optional with
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

/// `status` takes no flags of its own — `--manifest-dir` is global, and the
/// deep flags all belong to `doctor`.
#[derive(Args, Debug)]
pub struct StatusArgs {}

/// `setup` is the interactive newcomer wizard; it deliberately has no `--write`
/// (it confirms in a terminal and stays dry-run everywhere else). Scripts use
/// `init` + `apply --write` + `use <profile> --write`.
#[derive(Args, Debug)]
pub struct SetupArgs {
    /// Only configure these CLIs (repeatable). Defaults to [targets].default.
    #[arg(long = "target", value_name = "CLI")]
    pub targets: Vec<String>,

    /// Configure only the servers in this profile.
    #[arg(long, value_name = "NAME")]
    pub profile: Option<String>,

    /// Which scope to write: global (~) or project (repo). Defaults to the
    /// manifest's home — project for a repo manifest, global for the machine
    /// manifest (~/.agentstack).
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
    #[command(after_help = "\
Examples:
  agentstack add server github --type http --url https://api.githubcopilot.com/mcp/ --header \"Authorization=Bearer ${GH_PAT}\" --write
  agentstack add server gitlab --type stdio --command npx --arg -y --arg @modelcontextprotocol/server-gitlab --env \"GITLAB_TOKEN=${GITLAB_TOKEN}\" --write")]
    Server(AddServerArgs),
    /// Add a skill (a SKILL.md directory).
    Skill(AddSkillArgs),
}

#[derive(Args, Debug)]
pub struct SetArgs {
    #[command(subcommand)]
    pub kind: SetKind,
}

#[derive(Subcommand, Debug)]
pub enum SetKind {
    /// Create or update an MCP server (same flags as `add server`).
    #[command(after_help = "\
Examples:
  agentstack set server github --type http --url https://api.githubcopilot.com/mcp/ --write
  agentstack set server gitlab --type stdio --command npx --arg -y --arg @modelcontextprotocol/server-gitlab --write")]
    Server(AddServerArgs),
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
    /// Write the change (else preview).
    #[arg(long)]
    pub write: bool,
}

#[derive(Args, Debug)]
pub struct AddServerArgs {
    /// Server name used in the manifest and policy rules, e.g. github.
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
    /// Working directory the stdio server is launched from; may contain `${REF}`.
    #[arg(long)]
    pub cwd: Option<String>,
    /// Env `Key=Value` (repeatable).
    #[arg(long = "env", value_name = "K=V")]
    pub env: Vec<String>,
    /// Also add to this profile's server list.
    #[arg(long)]
    pub profile: Option<String>,
    /// Render only into this CLI (repeatable, e.g. --target claude-code).
    /// Default: every CLI in [targets]. Unknown adapter ids are an error.
    #[arg(long = "target", value_name = "CLI")]
    pub targets: Vec<String>,
    /// Write the change (else preview).
    #[arg(long)]
    pub write: bool,
}

#[derive(Args, Debug)]
pub struct AddSkillArgs {
    /// owner/repo, a git URL (incl. /tree/<ref>/<subpath>), or a spelled
    /// local path (./dir, ../dir, /abs, ~/dir).
    pub source: String,
    /// Select skills by name (repeatable). Required in scripts when the
    /// source holds several.
    #[arg(long)]
    pub skill: Vec<String>,
    /// List the source's skills and exit — adds nothing.
    #[arg(long)]
    pub list: bool,
    /// Branch/tag/commit recorded in the manifest; the exact commit is
    /// pinned in the lock.
    #[arg(long)]
    pub rev: Option<String>,
    /// Directory within the repo to scope discovery to.
    #[arg(long)]
    pub subpath: Option<String>,
    /// Manifest name override (single selection only) — for a source whose
    /// directory name doesn't fit the name contract.
    #[arg(long)]
    pub name: Option<String>,
    /// Also add to this profile's skill list.
    #[arg(long)]
    pub profile: Option<String>,
    /// Admit content the scan flagged high-severity.
    #[arg(long)]
    pub allow_flagged: bool,
    /// Write the change (else preview).
    #[arg(long)]
    pub write: bool,
}

#[derive(Args, Debug)]
pub struct SettingsArgs {
    #[command(subcommand)]
    pub kind: SettingsKind,
}

#[derive(Subcommand, Debug)]
pub enum SettingsKind {
    /// Set a `[settings.<target>]` key (dotted paths like
    /// `permissions.defaultMode` are supported).
    #[command(after_help = "\
Examples:
  agentstack settings set claude-code permissions.defaultMode auto --write
  agentstack settings set codex model gpt-5.5")]
    Set(SettingsSetArgs),
    /// Remove a `[settings.<target>]` key.
    Unset(SettingsUnsetArgs),
}

#[derive(Args, Debug)]
pub struct SettingsSetArgs {
    /// Adapter id whose settings to edit (e.g. `claude-code`, `codex`).
    pub target: String,
    /// Setting key; a dotted path descends into nested tables
    /// (e.g. `permissions.defaultMode`).
    pub key: String,
    /// Value; coerced to bool/number/enum for keys in the adapter's catalog,
    /// stored as a string otherwise.
    pub value: String,
    /// Write the change (else preview).
    #[arg(long)]
    pub write: bool,
}

#[derive(Args, Debug)]
pub struct SettingsUnsetArgs {
    /// Adapter id whose settings to edit (e.g. `claude-code`, `codex`).
    pub target: String,
    /// Setting key to remove (dotted paths supported).
    pub key: String,
    /// Write the change (else preview).
    #[arg(long)]
    pub write: bool,
}

#[derive(Args, Clone, Debug)]
pub struct InitArgs {
    /// Seed the machine-level manifest (`~/.agentstack/agentstack.toml`)
    /// instead of importing a project: an empty [instructions] block plus an
    /// `instructions/` dir for personal, cross-project fragments compiled into
    /// each CLI's global CLAUDE.md / AGENTS.md, and the machine `[guard]` +
    /// `[policy.filesystem]` deny defaults (the same list `guard install`
    /// seeds, then offered for install into detected CLIs). No project is imported.
    #[arg(long)]
    pub global: bool,

    /// Overwrite an existing agentstack.toml.
    #[arg(long)]
    pub force: bool,

    /// Show what would be imported without writing the manifest or storing
    /// secrets.
    #[arg(long)]
    pub dry_run: bool,

    /// Where lifted token values are stored on the non-interactive path:
    /// `env` (project `.env`, gitignored), `keychain` (OS keychain), or
    /// `skip` (write only `${REF}` placeholders — you provide values later).
    /// Interactive runs prompt for this instead; when absent and
    /// non-interactive, the default is `keychain` (CI/scripts never start
    /// writing plaintext files by surprise).
    #[arg(long, value_enum, value_name = "STORE")]
    pub secrets: Option<SecretStore>,

    /// Deprecated alias for `--secrets skip`. Lifted values are NOT stored;
    /// the run prints each unstored `${REF}` and how to store it.
    #[arg(long)]
    pub no_keychain: bool,

    /// Run the promptless import without a terminal: acknowledge that the
    /// manifest (and any lifted token values) will be written. Required when
    /// stdin is not a TTY and no other init-shaping flag is given.
    #[arg(long)]
    pub yes: bool,
}

/// Where `init` (and `secret set`) put lifted token values when the manifest's
/// `${REF}` placeholders need real values on this machine.
#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum SecretStore {
    /// Project `.env` file next to the manifest (plaintext, gitignored).
    Env,
    /// The OS keychain (service `agentstack`).
    Keychain,
    /// Store nothing — only `${REF}` placeholders are written.
    Skip,
}

#[derive(Args, Debug)]
pub struct ApplyArgs {
    /// Only act on these CLIs (repeatable). Defaults to [targets].default.
    #[arg(long = "target", value_name = "CLI")]
    pub targets: Vec<String>,

    /// Render only the servers in this profile.
    #[arg(long, value_name = "NAME")]
    pub profile: Option<String>,

    /// Show what would change without writing, and skip the interactive prompt.
    #[arg(long)]
    pub dry_run: bool,

    /// Write the changes without prompting (else interactive preview).
    #[arg(long)]
    pub write: bool,

    /// Which scope to write: global (~) or project (repo). Defaults to the
    /// manifest's home — project (repo-local config) for a repo manifest,
    /// global for the machine manifest (~/.agentstack).
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
    /// Only act on these CLIs (repeatable). Defaults to [targets].default.
    #[arg(long = "target", value_name = "CLI")]
    pub targets: Vec<String>,

    #[arg(long, value_name = "NAME")]
    pub profile: Option<String>,

    /// Where writes land: global (each CLI user-level config) or project
    /// (repo-local). Defaults to the manifest home.
    #[arg(long, value_enum)]
    pub scope: Option<Scope>,

    /// Emit the drift report as machine-readable JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct UseArgs {
    /// Profile to activate. Optional: with one profile declared it is chosen
    /// automatically, and with none declared the implicit default — every
    /// inline skill and server — activates. Several profiles need a name.
    pub profile: Option<String>,

    /// Only act on these CLIs (repeatable). Defaults to [targets].default.
    #[arg(long = "target", value_name = "CLI")]
    pub targets: Vec<String>,

    /// Where writes land: global (each CLI user-level config) or project
    /// (repo-local). Defaults to the manifest home.
    #[arg(long, value_enum)]
    pub scope: Option<Scope>,

    /// Write the change (else preview).
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

    /// Emit provenance and safety signals as machine-readable JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct SessionArgs {
    #[command(subcommand)]
    pub cmd: SessionCmd,
}

#[derive(Subcommand, Debug)]
pub enum SessionCmd {
    /// Start a session: load a profile for now.
    Start {
        /// Profile to load.
        profile: String,
        /// Where writes land: global (each CLI user-level config) or project
        /// (repo-local). Defaults to the manifest home.
        #[arg(long, value_enum)]
        scope: Option<Scope>,
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
    /// What to undo: a recorded change id (unique prefix; `restore` with no
    /// argument lists them) or an adapter id for its single-slot config
    /// backup. Omit to list everything undoable.
    pub adapter: Option<String>,

    /// Undo the most recent recorded change that isn't already undone.
    #[arg(long, conflicts_with = "adapter")]
    pub last: bool,

    /// Where writes land: global (each CLI user-level config) or project
    /// (repo-local). Defaults to the manifest home.
    #[arg(long, value_enum)]
    pub scope: Option<Scope>,

    /// Write the change (else preview).
    #[arg(long)]
    pub write: bool,
}

#[derive(Args, Debug)]
pub struct AdoptArgs {
    /// Only act on these CLIs (repeatable). Defaults to [targets].default.
    #[arg(long = "target", value_name = "CLI")]
    pub targets: Vec<String>,

    /// Where writes land: global (each CLI user-level config) or project
    /// (repo-local). Defaults to the manifest home.
    #[arg(long, value_enum)]
    pub scope: Option<Scope>,

    /// Write the change (else preview).
    #[arg(long)]
    pub write: bool,

    /// Don't store lifted secrets in the keychain (just reference them).
    #[arg(long)]
    pub no_keychain: bool,
}

#[derive(Args, Debug)]
pub struct LibArgs {
    #[command(subcommand)]
    pub kind: LibKind,
}

#[derive(Subcommand, Debug)]
pub enum LibKind {
    /// Scaffold a new skill: ./<name>/SKILL.md with the house template.
    New(LibNewArgs),
    /// Add a skill to the central library from a local path or git source.
    Add(LibAddArgs),
    /// Add an MCP server definition to the central library from a `.toml` file.
    AddServer(LibAddServerArgs),
    /// Add a native harness extension to the central library from a local path
    /// or git source.
    #[command(after_help = "\
Examples:
  agentstack lib add-extension checkpoint --target pi --path ./extensions/checkpoint --write
  agentstack lib add-extension checkpoint --target pi --git https://github.com/acme/checkpoint --subpath ext --write")]
    AddExtension(LibAddExtensionArgs),
    /// Add a declarative lifecycle hook definition to the central library from a
    /// `.toml` file or by lifting it out of the current manifest.
    AddHook(LibAddHookArgs),
    /// List the skills, servers, extensions, and hooks in the central library.
    List,
    /// Remove a skill from the central library.
    Remove(LibRemoveArgs),
    /// Remove a server from the central library.
    RemoveServer(LibRemoveServerArgs),
    /// Remove an extension from the central library.
    RemoveExtension(LibRemoveExtensionArgs),
    /// Remove a hook from the central library.
    RemoveHook(LibRemoveHookArgs),
    /// Sync the central library across machines as a git repo (commit local
    /// changes, pull, push). Secrets never travel — server defs are `${REF}`.
    Sync(LibSyncArgs),
    /// Scaffold a publishable pack (pack.toml + example skill) in the current
    /// directory. Publish by pushing the repo and tagging a version (e.g.
    /// v0.1.0); install with `agentstack add from git:<host>/<repo>@<tag>`.
    PackInit(PackInitArgs),
}

#[derive(Args, Debug)]
pub struct LibSyncArgs {
    /// Set up the library as a git repo (first-time). With --remote pointing at
    /// an existing library repo and an empty/absent library, this clones it.
    #[arg(long)]
    pub init: bool,
    /// The git remote URL — recorded on --init, or added/updated on a later run.
    #[arg(long)]
    pub remote: Option<String>,
    /// Show working-tree changes and ahead/behind vs. the remote; change nothing.
    #[arg(long)]
    pub status: bool,
    /// Commit message for local changes (default: a snapshot line).
    #[arg(long)]
    pub message: Option<String>,
    /// Push even if a server definition contains a literal secret (normally the
    /// sync is blocked — secrets should be `${REF}` placeholders).
    #[arg(long)]
    pub allow_secrets: bool,
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
    /// Write the change (else preview).
    #[arg(long)]
    pub write: bool,
}

#[derive(Args, Debug)]
pub struct LibRemoveServerArgs {
    /// The library server name to remove.
    pub name: String,
    /// Write the change (else preview).
    #[arg(long)]
    pub write: bool,
}

#[derive(Args, Debug)]
pub struct LibAddHookArgs {
    /// The name projects will reference this hook by.
    pub name: String,
    /// Path to a hook definition `.toml` (a `manifest::Hook` table with
    /// `event`/`command`/…, `${REF}` secrets only — never plaintext).
    #[arg(long, conflicts_with = "from_manifest")]
    pub file: Option<String>,
    /// Lift the `[hooks.<name>]` definition from the current manifest into the
    /// library instead of reading a file.
    #[arg(long)]
    pub from_manifest: bool,
    /// Overwrite an existing library hook of the same name.
    #[arg(long)]
    pub replace: bool,
    /// Write the change (else preview).
    #[arg(long)]
    pub write: bool,
}

#[derive(Args, Debug)]
pub struct LibRemoveHookArgs {
    /// The library hook name to remove.
    pub name: String,
    /// Write the change (else preview).
    #[arg(long)]
    pub write: bool,
}

#[derive(Args, Debug)]
pub struct LibAddExtensionArgs {
    /// The name projects will reference this extension by.
    pub name: String,
    /// The one adapter id this extension's code is written against (e.g. `pi`,
    /// `opencode`). Extension code is harness-specific — never `"*"`.
    #[arg(long)]
    pub target: String,
    /// Add from a local extension directory or single source file.
    #[arg(long, conflicts_with = "git")]
    pub path: Option<String>,
    /// Add from a git source URL. Requires --subpath (a checkout's `.git`
    /// cannot be part of a reproducible pin).
    #[arg(long, conflicts_with = "path")]
    pub git: Option<String>,
    /// Pin a git revision (branch, tag, or commit). Git sources only.
    #[arg(long, requires = "git")]
    pub rev: Option<String>,
    /// Directory within the git repo holding the extension. Git sources only.
    #[arg(long, requires = "git")]
    pub subpath: Option<String>,
    /// One-line description shown by `lib list`.
    #[arg(long)]
    pub description: Option<String>,
    /// Overwrite an existing library extension of the same name.
    #[arg(long)]
    pub replace: bool,
    /// Add even if the content scan finds high-severity items (hidden Unicode).
    #[arg(long)]
    pub allow_flagged: bool,
    /// Write the change (else preview).
    #[arg(long)]
    pub write: bool,
}

#[derive(Args, Debug)]
pub struct LibRemoveExtensionArgs {
    /// The library extension name to remove.
    pub name: String,
    /// Write the change (else preview).
    #[arg(long)]
    pub write: bool,
}

#[derive(Args, Debug)]
pub struct LibRemoveArgs {
    /// The library skill name to remove.
    pub name: String,
    /// Write the change (else preview).
    #[arg(long)]
    pub write: bool,
}

#[derive(Args, Debug)]
pub struct LibNewArgs {
    /// Skill name (the directory and manifest key — lowercase [a-z0-9._-]).
    pub name: String,
}

#[derive(Args, Debug)]
pub struct LibAddArgs {
    /// owner/repo, a git URL (incl. /tree/<ref>/<subpath>), or a spelled
    /// local path (./dir, ../dir, /abs, ~/dir).
    pub source: String,
    /// Select skills by name (repeatable) when the source holds several.
    #[arg(long)]
    pub skill: Vec<String>,
    /// List the source's skills and exit — adds nothing.
    #[arg(long)]
    pub list: bool,
    /// Library name override (single selection only).
    #[arg(long)]
    pub name: Option<String>,
    /// Branch/tag/commit to resolve (git sources).
    #[arg(long)]
    pub rev: Option<String>,
    /// Directory within the repo to scope discovery to (git sources).
    #[arg(long)]
    pub subpath: Option<String>,
    /// Overwrite a same-named library entry.
    #[arg(long)]
    pub replace: bool,
    /// Admit content the scan flagged high-severity.
    #[arg(long)]
    pub allow_flagged: bool,
    /// Write the change (else preview).
    #[arg(long)]
    pub write: bool,
}

#[derive(Args, Debug)]
pub struct InstructionsArgs {
    /// Only act on these CLIs (repeatable). Defaults to [targets].default.
    #[arg(long = "target", value_name = "CLI")]
    pub targets: Vec<String>,

    /// Where writes land: global (each CLI user-level config) or project
    /// (repo-local). Defaults to the manifest home.
    #[arg(long, value_enum)]
    pub scope: Option<Scope>,

    /// Write the change (else preview).
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

    /// Show every section, including ones for features this project doesn't
    /// use (hidden by default; --ci always shows everything).
    #[arg(long)]
    pub all: bool,

    /// Emit the full report as machine-readable JSON instead of the text
    /// report (the structured surface the retired `audit --json` occupied).
    #[arg(long)]
    pub json: bool,

    /// Internal (not a CLI flag): suppress the server render-drift section.
    /// The clean-at-rest wizard fork deliberately renders nothing, so the
    /// usual "N change(s) pending ↳ apply --write" comparison would be a false
    /// alarm that contradicts the chosen mode. `#[arg(skip)]` keeps it off the
    /// parsed surface and defaults it to `false` everywhere else.
    #[arg(skip)]
    pub skip_drift: bool,
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
pub struct AnalyzeArgs {
    /// Emit the report as JSON (for the dashboard or further processing).
    #[arg(long)]
    pub json: bool,

    /// Only count call-log entries from the last N days.
    #[arg(long, value_name = "DAYS")]
    pub since: Option<u64>,

    /// Also list the last N individual calls (after --since / --project
    /// filtering). With --json this adds an `events` array — the stable
    /// machine-readable activity feed for external UIs; argument digests
    /// only, never values.
    #[arg(long, value_name = "N")]
    pub tail: Option<usize>,

    /// Only count calls recorded for this project root.
    #[arg(long, value_name = "PATH")]
    pub project: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
pub enum ShimCmd {
    /// Write the wrapper script for a CLI and print where to point the
    /// supervisor.
    Make(ShimMakeArgs),

    /// Internal: what the wrapper script runs. Mints a run id, opens the
    /// run's event log, then replaces itself with the real binary.
    #[command(hide = true)]
    Exec(ShimExecArgs),
}

#[derive(Args, Debug)]
pub struct ShimMakeArgs {
    /// The CLI to wrap (the shim file takes this name), e.g. `claude`.
    pub cli: String,

    /// Path to the real binary. Default: first `<cli>` on PATH that is not
    /// itself inside the shims directory.
    #[arg(long, value_name = "PATH")]
    pub binary: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct ShimExecArgs {
    /// The real binary to become.
    pub binary: PathBuf,

    /// Arguments passed through verbatim.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<std::ffi::OsString>,
}

#[derive(Args, Debug)]
pub struct ProxyStartArgs {
    /// Loopback port to listen on.
    #[arg(long, default_value_t = crate::proxy::DEFAULT_PORT)]
    pub port: u16,

    /// Upstream API base URL to relay to.
    #[arg(long, default_value = crate::proxy::DEFAULT_UPSTREAM)]
    pub upstream: String,
}

#[derive(Args, Debug)]
pub struct WireArgs {
    /// Emit the aggregate as JSON instead of the ranked table.
    #[arg(long)]
    pub json: bool,
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
    /// Validate a user adapter descriptor file (parse + basic checks) before
    /// dropping it into `~/.agentstack/adapters/`.
    Validate {
        /// Path to a `.yaml` adapter descriptor.
        file: String,
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
        /// Secret reference name, as used in ${REF} placeholders — e.g. GH_PAT.
        name: String,
        /// Provide the value inline (otherwise you'll be prompted).
        #[arg(long)]
        value: Option<String>,
        /// Write the value to the project `.env` (gitignored) instead of the
        /// OS keychain.
        #[arg(long)]
        env_file: bool,
    },
    /// Print a secret's value.
    Get {
        /// Secret reference name, as used in ${REF} placeholders — e.g. GH_PAT.
        name: String,
    },
    /// Remove a secret from the keychain.
    Rm {
        /// Secret reference name, as used in ${REF} placeholders — e.g. GH_PAT.
        name: String,
    },
    /// Show every secret the manifest references and whether it resolves.
    List,
}

/// The `--help --all` view: every command — visible or hidden — with its
/// one-line summary, subcommands indented under their parent. This is the
/// "long" half of the progressive-disclosure pair; the default `--help` shows
/// only the beginner loop plus the grouped name map.
pub fn full_command_inventory() -> String {
    use clap::CommandFactory;

    fn push(out: &mut String, cmd: &clap::Command, indent: usize) {
        for sub in cmd.get_subcommands() {
            if sub.get_name() == "help" {
                continue;
            }
            let about = sub.get_about().map(|s| s.to_string()).unwrap_or_default();
            let pad = " ".repeat(indent);
            out.push_str(&format!("{pad}{:<16} {about}\n", sub.get_name()));
            push(out, sub, indent + 2);
        }
    }

    let cmd = Cli::command();
    let mut out = String::from(
        "agentstack — every command, including the ones the default --help groups away.\n\
         Run `agentstack <command> --help` for flags and details.\n\n",
    );
    push(&mut out, &cmd, 2);
    out
}

/// Clap tree used by the real parser. Hidden commands are discoverable from
/// the top-level task map; once a user reaches one directly, its own help also
/// points back to the complete inventory. Existing command-specific examples
/// are preserved and the footer is appended.
pub fn runtime_command() -> clap::Command {
    use clap::CommandFactory;

    fn decorate(cmd: clap::Command) -> clap::Command {
        cmd.mut_subcommands(|sub| {
            let hidden = sub.is_hide_set();
            let existing = sub.get_after_help().map(ToString::to_string);
            let sub = decorate(sub);
            if hidden {
                let footer = "Full command list: agentstack --help --all";
                let help = existing
                    .filter(|s| !s.trim().is_empty())
                    .map(|s| format!("{s}\n\n{footer}"))
                    .unwrap_or_else(|| footer.to_string());
                sub.after_help(help)
            } else {
                sub
            }
        })
    }

    decorate(Cli::command())
}
