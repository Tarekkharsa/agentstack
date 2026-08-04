//! Command-line surface (clap derive).
//!
//! The visible set is decided by ONE rule, and the rule is a guarantee, not a
//! taste: **a command the product tells you to run must be findable from plain
//! `agentstack --help`.** Guidance that names a command a reader cannot find is
//! the defect this file exists to prevent — it is how `secret`, `gateway` and
//! `lock` came to be printed in doctor's fix column while absent from the help
//! screen. So the visible list is derived from the emitters (the first-run
//! ladder in `commands::overview`, doctor's `↳ fix` column, and the
//! machine-readable `next_action` / `fix` fields), plus one obvious verb for
//! each of the four ideas the help already promises — Setup · Toolset · Status
//! · Undo.
//!
//! That derivation yields fifteen, not ten. It is the honest number: `lock`,
//! `secret` and `adopt` are here because guidance names them, and hiding one to
//! reach a rounder count would trade a real guarantee for a tidier screen.
//!
//! Everything else moves behind `agentstack x <command>` — the same commands,
//! one hop away, listed and grouped by [`namespace_listing`]. Nothing is
//! removed: every hidden command still runs at its own name, and `--help --all`
//! still prints the complete map. `x` is display-only sugar — [`strip_namespace`]
//! removes the `x` before clap parses, so dispatch has exactly one path.
//!
//! A guidance string may still name a command that lives behind `x`
//! (`agentstack guard install`, `agentstack self link`, …). Those verbs are
//! listed by name on the `--help` screen itself, under "Also named by
//! guidance", so the guarantee at the top of this comment holds for them too.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::manifest::ServerType;
use crate::scope::Scope;

/// Was this binary compiled with the optional `sandbox` feature?
///
/// Release binaries are; a plain `cargo build --release` is not, and the two
/// otherwise carry the same version number and the same `--help`. That silence
/// produced confusing bug reports (a contributor testing a sandbox fix on a
/// build that cannot run one), so every surface that could mislead about which
/// build is in hand says so: `--version` below, `doctor`'s adapters section,
/// and the `run --sandbox` refusal.
pub const SANDBOX_SUPPORT: bool = cfg!(feature = "sandbox");

/// `--version`'s payload — the crate version plus the compiled-in feature set
/// (`0.15.0 (sandbox: yes)`). clap prints the binary name in front of it. Two
/// `cfg`-selected constants rather than one runtime `format!` because clap
/// wants a `&'static str`.
#[cfg(feature = "sandbox")]
pub const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (sandbox: yes)");
#[cfg(not(feature = "sandbox"))]
pub const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (sandbox: no)");

#[derive(Parser, Debug)]
#[command(
    name = "agentstack",
    version = VERSION,
    about = "Define your agent setup once. Use it across every coding CLI.",
    long_about = "Set up your MCP servers and skills once, and use them across \
                  Claude Code, Codex, and every other coding CLI you have — \
                  switchable by task, and reversible.",
    // Phase 3, vocabulary completion: the default help teaches the four ideas
    // and nothing else. It used to end with a "Words:" glossary defining CLI,
    // harness, adapter, and [targets] — five mechanism nouns before the reader
    // had seen a single result. Those words still exist and still matter; they
    // are now in `--help --all`, where someone who has a reason to want them
    // will look. Nothing was renamed to achieve this.
    after_help = "\
Start here:
  agentstack                     where this project stands, and the one next step
  agentstack init                Setup — find the CLIs you have and bring them together
  agentstack status              Status — is it ready, and if not, the one thing that fixes it
  agentstack yes                 review and activate what you dropped in
  agentstack undo                Undo — take back a recent change

Four ideas cover the whole product: Setup (what you have) · Toolset (what this
task needs) · Status (is it ready) · Undo (how to take it back).

Everything else lives one hop away, grouped by task:
  agentstack x                   the rest of the toolbox

Every command, grouped by task — and what the pieces are called underneath:
  agentstack --help --all"
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
    /// Setup: find the CLIs you have and bring their setups together.
    ///
    /// At a terminal this is the guided first run: it detects your CLIs,
    /// imports their configs, lifts inline tokens to `${REF}` placeholders,
    /// asks where those values live, then previews, confirms, applies and
    /// verifies — setup finished in one command. Scripted runs (`--yes`,
    /// `--plan`, `--secrets`) are the promptless primitive and stop after the
    /// manifest, naming `apply --write` as the next step.
    Init(InitArgs),

    /// Set this machine up from a setup that already exists: one command.
    ///
    /// For a fresh machine holding a checkout. Finds the CLIs you have,
    /// verifies the environment against `agentstack.lock`, renders each CLI's
    /// config, and names what is left — which on a new machine is this
    /// machine's secrets. `init` is for a setup that does not exist yet; this
    /// is for one that does.
    #[command(hide = true)]
    Up(UpArgs),

    /// Status: where this project stands, on one screen, and the one next step.
    ///
    /// The same orientation bare `agentstack` prints — reachable by name so
    /// muscle memory (`git status`, `docker status`, …) and scripts land
    /// somewhere useful. Deep verification stays in `agentstack doctor`.
    Status(StatusArgs),

    /// Add a server or skill to this project's setup.
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

    /// Write this setup into each CLI's own config.
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

    /// Check the setup in depth: what is wired up, what is missing, what changed.
    Doctor(DoctorArgs),

    // ── Capabilities & library ───────────────────────────────────────────
    /// Remove a server or skill from the manifest (and lockfile).
    #[command(hide = true)]
    Remove(RemoveArgs),

    /// Fetch skill sources into the store and write the lockfile.
    #[command(hide = true)]
    Install(InstallArgs),

    /// Share this setup as a signed bundle others can review.
    ///
    /// Signing is not a flag: a bundle is signed as part of sharing, because
    /// an opt-in signature is one nobody opts into. Receivers review before
    /// anything activates.
    #[command(hide = true)]
    Share(ShareArgs),

    /// Review a shared bundle, then decide.
    ///
    /// The bundle is staged inert and carded first. A signature from a
    /// publisher you recognize makes the card shorter — never optional.
    #[command(hide = true)]
    Receive(ReceiveArgs),

    /// Your publishing key, and the publishers you recognize.
    #[command(hide = true)]
    Publisher(PublisherArgs),

    /// Resolve each toolset's skill + server refs and pin `agentstack.lock`.
    ///
    /// Previews by default and shows the pins it would add, change, or
    /// remove; `--write` pins them. Library-aware resolution; no configs
    /// rendered, no skills materialized — the lock-only counterpart of
    /// `use <toolset> --write`, for clean-at-rest repos that keep no generated
    /// files. Computing the preview resolves sources, so git-backed sources are
    /// fetched. Every write needs `--write`: `--update` re-resolves git skills
    /// to their latest and `--upgrade` re-resolves an installed vendor pack —
    /// `--update` has no preview and refuses without `--write`.
    Lock(LockArgs),

    /// Try a skill without installing anything: stage, scan, and emit a
    /// wrapper prompt on stdout for piping into any agent CLI.
    ///
    /// `agentstack try owner/repo --skill pdf | claude` — no manifest, lock,
    /// or config is touched; support files land under ~/.agentstack/try/.
    #[command(hide = true)]
    Try(TryArgs),

    /// Manage your linked capability library sources.
    ///
    /// Any folder can be linked as a source, several at once; the first source
    /// holding a name wins. `~/.agentstack/lib/` is the one you start with.
    #[command(hide = true)]
    Lib(LibArgs),

    // ── Activate & run ───────────────────────────────────────────────────
    /// Work with toolsets: name one that bundles what you already have.
    ///
    /// A toolset is a named subset of this project's servers and skills — one
    /// for backend work, one for incident response — so you switch context
    /// without editing five config files. `agentstack use <name>` activates
    /// one; `agentstack use --list` shows them all.
    #[command(subcommand)]
    Toolset(ToolsetCmd),

    /// Toolset: switch to one — its servers and skills go live in your CLIs.
    Use(UseArgs),

    /// Review and activate the files you dropped into this project — one step.
    Yes(YesArgs),

    /// Use a toolset temporarily: load it for now, then put every file back.
    #[command(hide = true)]
    Session(SessionArgs),

    /// Launch an agent CLI as a tracked run.
    ///
    /// Optionally apply a toolset for its lifetime, then observe/kill it
    /// here or through an integrated supervisor such as t3code.
    Run(RunArgs),

    /// Kill a tracked run by id (and revert its toolset if it owned one).
    #[command(hide = true)]
    Kill(KillArgs),

    /// Compose one toolset and its pinned capabilities into a container image.
    ///
    /// The image carries the exact bytes you reviewed — skills laid down where
    /// the tool reads them, server definitions with their `${REF}`
    /// placeholders untouched — plus a start-up guard that refuses to launch
    /// until the secrets those placeholders name are present in the run's own
    /// environment. Nothing is resolved into the image, and nothing is pushed
    /// anywhere.
    ///
    /// Dry-run by default: a bare `agentstack image` writes no file and does
    /// not contact Docker. The artifact and every claim it does and does not
    /// make are written up in `docs/design/packaging.md`.
    #[command(hide = true)]
    Image(ImageArgs),

    /// Exec-through launcher shim for external supervisors (e.g. t3code).
    ///
    /// `shim make <cli>` writes a tiny wrapper under `~/.agentstack/shims/`;
    /// point the supervisor's binary-path setting at it and every session it
    /// starts gets a per-run identity (`AGENTSTACK_RUN_ID` + `events.jsonl`)
    /// instead of landing in the global audit only. Read-only toward the
    /// supervisor: agentstack never edits its settings.
    #[command(hide = true, subcommand)]
    Shim(ShimCmd),

    /// Run a reviewed multi-agent task using toolsets you already approved.
    ///
    /// Visible since the six interpreter-boundary review findings closed, each
    /// with its own witness (the watchdog's no-I/O exit, interpreter memory
    /// bounds, host-native re-entrancy, a run-total native call budget,
    /// cross-host resume determinism, and the crate boundary). Un-hiding
    /// changed DISCOVERABILITY only — not one enforcement boundary moved with
    /// it, and `docs/workflows.md`'s *Honest limits* still hold in full: a
    /// host-tier step is cooperative-guard only, step outputs are untrusted
    /// model data, and the interpreter's residual bounds are stated in
    /// `agentstack workflow report`'s posture block rather than papered over.
    ///
    /// Full detail lives in `agentstack workflow --help`: each `agent()` call
    /// is admitted against the trust gate, verified against a strict lock,
    /// capped by the machine ceiling, and spawned as its own locked child run.
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

    /// Runtime lease registry: which toolset leases are open on this machine.
    ///
    /// A lease is the temporary runtime activation of one toolset over an MCP
    /// connection. It is owned by the MCP process that opened it and disappears
    /// with that process — so the registry stores a RECORD, and liveness is
    /// derived on every read from the recorded PID plus that process's start
    /// time. A record whose process is gone, or whose PID has been reused,
    /// reads as stale and never as live.
    #[command(subcommand, hide = true)]
    Lease(LeaseCmd),

    /// How each capability reaches each of your tools — and the one override.
    ///
    /// Delivery is routed, not chosen: skills and MCP servers are served live
    /// to tools that can take them, while house rules, settings, hooks and
    /// extensions are written into native files, as is everything for a tool
    /// that reads files only. `agentstack delivery` shows the routing;
    /// `agentstack delivery render-locally` is the single override — write
    /// files even where the live channel would have worked.
    #[command(hide = true)]
    Delivery(DeliveryArgs),

    /// Review and approve this project's declared capabilities — required
    /// before anything activates them.
    ///
    /// Until trusted, none of the project's servers are spawned or contacted
    /// and no secrets are resolved: `session start` refuses, and an
    /// auto-discovered project reaching the zero-files gateway gets
    /// control-plane tools only. Trust pins the content digest of the
    /// manifest layers AND the lockfile — editing either (a `git pull`, an
    /// `agentstack lock`) requires re-trusting.
    Trust(TrustArgs),

    // ── Recover ──────────────────────────────────────────────────────────
    // Undo is one of the four beginner concepts (Setup, Toolset, Status,
    // Undo), so both recovery verbs stay in the default `--help`: a user who
    // broke something must find the way back without reading the README.
    /// Undo a recorded write: revert what apply/use/session changed.
    ///
    /// Reverts a history entry (servers, settings, hooks, instructions), or
    /// restores one adapter's config from its single-slot backup.
    #[command(
        hide = true,
        after_help = "\
Examples:
  agentstack restore --last --write
  agentstack restore claude-code --write"
    )]
    Restore(RestoreArgs),

    /// Take it back: pick a point from your recent changes and revert to it.
    ///
    /// The same recorded changes `restore` works with, asked the other way
    /// round — newest first, pick one. The revert is itself recorded, so
    /// going one step too far is recoverable.
    #[command(after_help = "\
Examples:
  agentstack undo                    what changed recently
  agentstack undo --to 2 --write     back to before change 2")]
    Undo(UndoArgs),

    /// Keep a hand-edit: pull a change you made in a CLI back into this setup.
    ///
    /// Imports hand-added servers and hand-edited fields from target configs
    /// so the manifest stays the source of truth.
    Adopt(AdoptArgs),

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

    /// Manage secrets in the OS keychain.
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

    /// Manage this binary's own install: `self update` upgrades it to the newest
    /// published release (checksum-verified); `self link` puts a stable
    /// `agentstack` on PATH (a symlink, no installer needed); `self which` shows
    /// which binary a bare `agentstack` runs and flags stale links.
    #[command(name = "self", hide = true)]
    SelfCmd(SelfArgs),

    /// Print a tab-completion script for bash, zsh, or fish.
    ///
    /// Writes to stdout — see docs/reference.md for where each shell wants the
    /// file. Completes command names, subcommands, and long flags; values are
    /// left to the shell.
    #[command(hide = true)]
    Completions(CompletionsArgs),

    // ── Panel actions (launch plan Lane B) ───────────────────────────────────
    // Digest-bound fixed argv the t3code toolset UI drives. Hidden: internal
    // control-plane plumbing, not part of the daily terminal surface. Each
    // mutation runs manifest → re-lock → re-render, bound to a consent digest
    // exactly like `apply-setup`. New UI capability = new fixed argv actions in
    // this closed set (pinned by tests/t3code_parity.rs), never MCP-in-browser.
    /// Add a skill to a toolset and activate it (panel action; digest-bound).
    #[command(name = "add-skill-to-profile", hide = true)]
    AddSkillToProfile(PanelAddSkillArgs),

    /// Add a server to a toolset and activate it (panel action; digest-bound).
    #[command(name = "add-server-to-profile", hide = true)]
    AddServerToProfile(PanelAddServerArgs),

    /// Remove everything AgentStack manages, previewing first.
    #[command(
        hide = true,
        after_help = "\
The guaranteed exit. Reverts every managed region AgentStack rendered — servers,
settings, hooks, and instruction blocks — in each CLI's own config, then removes
AgentStack's own state directory.

  agentstack uninstall                    show what would be removed (default)
  agentstack uninstall --verbose          ...with the full diff of each file
  agentstack uninstall --write            do it
  agentstack uninstall --write --keep-home  keep ~/.agentstack (and the undo ledger)

Your agentstack.toml is never touched — this removes rendered output, not your
setup, so you can re-`apply` at any time. Foreign entries you or another tool
added to those files are left alone. Every file edit is captured first, so
`agentstack restore` still works afterwards unless you also removed ~/.agentstack."
    )]
    Uninstall(UninstallArgs),

    /// Fixed-argv alias of `agentstack toolset create` (panel action).
    ///
    /// Same code path, same consent digest — kept under its original name
    /// because t3code emits it as fixed argv. People should reach for
    /// `agentstack toolset create`.
    #[command(
        name = "create-profile",
        hide = true,
        after_help = "\
Machine surface. `agentstack toolset create` is the same action under the name
a person reads; this name is frozen because the t3code panel emits it as fixed
argv. Both run one authority path and produce the same consent digest."
    )]
    CreateProfile(PanelCreateProfileArgs),

    /// Record whether this project manages its `.gitignore` block (panel
    /// action; digest-bound).
    ///
    /// The durable answer a per-run `--no-gitignore` cannot give: the next
    /// toolset switch would re-add the block. Disabling also removes a block
    /// already on disk — consented here and nowhere else.
    #[command(
        name = "set-gitignore",
        hide = true,
        after_help = "\
Machine surface. Records `[meta] gitignore` in the project manifest — the
durable answer a per-run `--no-gitignore` cannot give, since the next toolset
switch would re-add the block. Disabling also removes a block already on disk:
that removal is consented HERE and nowhere else, because routine commands must
never strip a block a team may have committed."
    )]
    SetGitignore(PanelSetGitignoreArgs),

    /// Retired: delivery is routed, not a mode you pick.
    ///
    /// It refuses and names the replacement. The Mode axis retired with
    /// STRATEGY.md v3; `set-mode-v1` is in `ui_contract::SUPERSEDED` so a
    /// panel can tell a retired picker from a binary too old to have one.
    #[command(
        name = "set-mode",
        hide = true,
        after_help = "\
Retired. AgentStack routes each capability to its lane by kind and harness —
there is no mode to choose, so this command changes nothing and refuses.

  agentstack status                          what delivery decided, per CLI
  agentstack uninstall                       stop rendering files here
  agentstack delivery render-locally         keep files where live would work"
    )]
    SetMode(PanelSetModeArgs),

    /// Change one toolset's membership as a batch (panel action; digest-bound).
    #[command(name = "edit-profile", hide = true)]
    EditProfile(PanelEditProfileArgs),

    /// Fixed-argv alias of `agentstack toolset rename` (panel action).
    #[command(name = "rename-profile", hide = true)]
    RenameProfile(PanelRenameProfileArgs),

    /// Fixed-argv alias of `agentstack toolset delete` (panel action).
    #[command(name = "delete-profile", hide = true)]
    DeleteProfile(PanelDeleteProfileArgs),

    /// Activate an existing toolset (panel action; digest-bound).
    #[command(name = "use-profile", hide = true)]
    UseProfile(PanelUseProfileArgs),

    /// The library catalog (skills + servers), merged across linked sources,
    /// for the panel browser.
    #[command(name = "library-index", hide = true)]
    LibraryIndex,

    /// Remove a skill or server from the library (panel action;
    /// digest-bound). Moves it to the library trash — recoverable with
    /// `agentstack lib trash --restore <id> --write`.
    #[command(name = "remove-from-library", hide = true)]
    RemoveFromLibrary(PanelRemoveFromLibraryArgs),

    /// Remove a skill or server from this project's manifest (panel action;
    /// digest-bound), then re-lock and re-render.
    #[command(name = "remove-capability", hide = true)]
    RemoveCapability(PanelRemoveCapabilityArgs),
}

/// The `toolset` group: the human-named entry point to the toolset verbs.
///
/// `create` shares its args type and its implementation with the hidden
/// `create-profile` fixed argv the t3code panel drives — one authority path
/// (`commands::panel_edit::create_profile`), one consent digest, two spellings.
#[derive(Subcommand, Debug)]
pub enum ToolsetCmd {
    /// Name a toolset: bundle some of what you already have. Does not activate it.
    #[command(after_help = "\
A toolset is a named subset of this project's servers and skills — one for
backend work, one for incident response — so you switch context without
editing five config files.

Naming one does not switch to it: this writes the manifest entry and re-locks,
and renders nothing.

  agentstack toolset create backend --server github
      at a terminal: shows what it will create, asks, then writes and re-locks

  agentstack toolset list                see every toolset here
  agentstack use backend --write         switch to it

Scripts and graphical clients get the two-step contract instead: --preview
emits the plan plus a consent digest, and applying needs
`--yes --consented <digest>`. A bare non-interactive call refuses and says so.")]
    Create(ToolsetCreateArgs),

    /// Rename a toolset, keeping everything in it.
    #[command(after_help = "\
Renames the toolset's own entry in the manifest and nothing else — its servers
and skills come with it, and nothing is rendered.

It refuses rather than guessing when the name is load-bearing elsewhere:

  · a live session is using it        end the session first
  · a workflow declares it as a role  a role name is that workflow's reviewed
                                      authority request — pinned in the lockfile
                                      and hashed into its grant digest — so
                                      renaming it here would rewrite a consented
                                      surface and strand any parked run

  agentstack toolset rename backend --to api")]
    Rename(ToolsetRenameArgs),

    /// Delete a toolset. The servers and skills in it stay declared.
    #[command(after_help = "\
Removes the toolset's entry from the manifest. A toolset is a selection over
your servers and skills, not their owner, so everything it named stays declared
here and stays in your library.

It refuses when a live session is using it, when a workflow declares it as a
role, and when it is the last toolset you have — with none declared, the render
and the proxied server surface fall back to every server in the manifest, so
that last delete widens rather than tidies.

  agentstack toolset delete backend")]
    Delete(ToolsetDeleteArgs),

    /// List the toolsets declared here, and whether each one is ready to use.
    #[command(after_help = "\
The same read as `agentstack use --list`, under the noun. Each toolset's
resolved skills, servers and harness, plus a readiness flag: is everything it
references pinned in agentstack.lock and matching?

  agentstack toolset list                see every toolset here
  agentstack toolset list --json         the same read, machine-readable

Listing never activates anything and writes nothing. To switch:
`agentstack use <toolset> --write`; to switch only for now:
`agentstack session start <toolset>`.")]
    List(ToolsetListArgs),
}

/// `toolset list` is a fixed-argv alias of `use --list` — one read path, one
/// implementation. It carries only the flags that read makes sense with, so
/// the noun is discoverable without growing a second way to activate.
#[derive(Args, Debug)]
pub struct ToolsetListArgs {
    /// Emit machine-readable JSON.
    #[arg(long)]
    pub json: bool,
}

/// The human spelling of `create-profile`'s args: the name is positional
/// (`toolset create backend`, like `git branch <name>`) with `--name` kept as
/// an equivalent flag. Converts into [`PanelCreateProfileArgs`] before
/// dispatch, so both spellings run the one authority path and produce the
/// same consent digest — the digest binds resolved params, never raw argv.
#[derive(Args, Debug)]
pub struct ToolsetCreateArgs {
    /// New toolset name (must not already exist).
    #[arg(value_name = "NAME", required_unless_present = "name_flag")]
    pub name: Option<String>,

    /// Flag spelling of NAME (same as the positional; kept for scripts).
    #[arg(long = "name", value_name = "NAME", conflicts_with = "name")]
    pub name_flag: Option<String>,

    /// Skill to include (repeatable). `*` means every inline skill.
    #[arg(long = "skill", value_name = "NAME")]
    pub skills: Vec<String>,

    /// Server to include (repeatable).
    #[arg(long = "server", value_name = "NAME")]
    pub servers: Vec<String>,

    #[command(flatten)]
    pub consent: PanelConsent,
}

impl ToolsetCreateArgs {
    /// Borrowed (dispatch matches the parsed `Command` by reference).
    pub fn to_panel_args(&self) -> PanelCreateProfileArgs {
        PanelCreateProfileArgs {
            // clap guarantees exactly one spelling is present
            // (required_unless_present + conflicts_with).
            name: self
                .name
                .clone()
                .or_else(|| self.name_flag.clone())
                .unwrap_or_default(),
            skills: self.skills.clone(),
            servers: self.servers.clone(),
            consent: self.consent.clone(),
        }
    }
}

/// The human spelling of `rename-profile`'s args (positional name, `--name`
/// kept as an equivalent flag) — see [`ToolsetCreateArgs`].
#[derive(Args, Debug)]
pub struct ToolsetRenameArgs {
    /// The toolset to rename.
    #[arg(value_name = "NAME", required_unless_present = "name_flag")]
    pub name: Option<String>,

    /// Flag spelling of NAME (same as the positional; kept for scripts).
    #[arg(long = "name", value_name = "NAME", conflicts_with = "name")]
    pub name_flag: Option<String>,

    /// Its new name.
    #[arg(long = "to", value_name = "NAME")]
    pub to: String,

    #[command(flatten)]
    pub consent: PanelConsent,
}

impl ToolsetRenameArgs {
    /// Borrowed (dispatch matches the parsed `Command` by reference).
    pub fn to_panel_args(&self) -> PanelRenameProfileArgs {
        PanelRenameProfileArgs {
            name: self
                .name
                .clone()
                .or_else(|| self.name_flag.clone())
                .unwrap_or_default(),
            to: self.to.clone(),
            consent: self.consent.clone(),
        }
    }
}

/// The human spelling of `delete-profile`'s args (positional name, `--name`
/// kept as an equivalent flag) — see [`ToolsetCreateArgs`].
#[derive(Args, Debug)]
pub struct ToolsetDeleteArgs {
    /// The toolset to delete.
    #[arg(value_name = "NAME", required_unless_present = "name_flag")]
    pub name: Option<String>,

    /// Flag spelling of NAME (same as the positional; kept for scripts).
    #[arg(long = "name", value_name = "NAME", conflicts_with = "name")]
    pub name_flag: Option<String>,

    #[command(flatten)]
    pub consent: PanelConsent,
}

impl ToolsetDeleteArgs {
    /// Borrowed (dispatch matches the parsed `Command` by reference).
    pub fn to_panel_args(&self) -> PanelDeleteProfileArgs {
        PanelDeleteProfileArgs {
            name: self
                .name
                .clone()
                .or_else(|| self.name_flag.clone())
                .unwrap_or_default(),
            consent: self.consent.clone(),
        }
    }
}

impl ToolsetListArgs {
    /// Widen into the `use --list` read this delegates to. Every activation
    /// field stays at its default, so the alias cannot render or write.
    /// Borrowed because dispatch matches the parsed `Command` by reference.
    pub fn to_use_args(&self) -> UseArgs {
        UseArgs {
            profile: None,
            targets: Vec::new(),
            scope: None,
            write: false,
            allow_unresolved: false,
            prune_foreign: false,
            no_gitignore: false,
            list: true,
            json: self.json,
            quiet: false,
        }
    }
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
    /// Upgrade this binary to the newest published release: previews what it
    /// would install, verifies the download against the release's published
    /// sha256 before anything is unpacked, then swaps it in with `--write`.
    #[command(after_help = "\
Replaces the binary you are running with the latest GitHub release. Same shape as
every other mutating command: it previews by default and only acts on --write.

  agentstack self update            show what a newer release would install
  agentstack self update --write    download, verify the sha256, install it

The archive is checked against the checksums.txt published with the release
BEFORE it is unpacked or moved into place. A mismatch aborts and leaves the
binary you already have byte-for-byte untouched. That proves the transfer, not
the provenance of the release — for provenance, verify the build attestation:
`gh attestation verify <asset> --repo Tarekkharsa/agentstack`.

A Homebrew install is handled by `brew upgrade agentstack`, a source build by
`cargo build --release`, and a binary in a directory you cannot write needs
`sudo` — each is detected and explained before anything is downloaded.

Set AGENTSTACK_NO_UPDATE_CHECK=1 to stop AgentStack contacting the release
channel at all (this command and `doctor`'s once-a-day note).")]
    Update(SelfUpdateArgs),

    /// Regenerate the "All commands" inventory in docs/reference.md from the
    /// live clap tree. No flag prints the block; `--write` splices it into the
    /// managed region. A maintainer/CI command, not part of the daily surface.
    #[command(hide = true)]
    Docs(SelfDocsArgs),
}

#[derive(Args, Debug)]
pub struct SelfUpdateArgs {
    /// Download, verify, and install the newer release (else preview only).
    #[arg(long)]
    pub write: bool,
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
pub struct CompletionsArgs {
    /// Which shell to emit for.
    #[arg(value_enum)]
    pub shell: Shell,
}

/// The shells `agentstack completions` can emit for. Deliberately a closed set
/// rather than a free string: each one has a hand-written emitter in
/// `commands/completions.rs`, so an unknown name has nothing to produce.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
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
    /// --locked` instead of re-deriving authority from disk. Fail-closed:
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

#[derive(Args, Debug, Default)]
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
    /// (required when stdin is not a TTY). Requires `--consented-digest` —
    /// the `surface_digest` from `trust --preview` — so the grant is bound
    /// to the exact bytes that were reviewed.
    #[arg(long)]
    pub yes: bool,

    /// The `surface_digest` a `trust --preview` emitted alongside the surface
    /// that was reviewed. The grant refuses unless it still matches the bytes
    /// on disk — CLI-enforced "a human reviewed this exact surface".
    #[arg(long, value_name = "DIGEST")]
    pub consented_digest: Option<String>,

    /// Emit the review surface as JSON and grant NOTHING (read-only). The
    /// machine-readable consent screen for external UIs — the actual grant
    /// stays the gated `agentstack trust` flow. Includes `surface_digest`,
    /// the value a later `--yes --consented-digest` grant must present.
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

    /// List live tracked runs (CLI, pid, toolset, uptime).
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

/// Governed workflows (design doc §12.4): the drive-loop
/// composition over the `agentstack-workflow` engine.
#[derive(Subcommand, Debug)]
pub enum WorkflowCmd {
    /// Run a pinned `[workflows.<name>]` entry: admission first (trust,
    /// strict lock verify, roles resolved to toolsets, ceilings intersected),
    /// then the governed drive loop — each `agent()` call spawns a locked
    /// child run under its role toolset's fence, with per-child MCP config
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

    /// List recorded workflow runs (the `w-…` evidence directories), newest
    /// first — the durable history behind `workflow report`, read-only.
    ///
    /// Each row joins the run's recorded identity and terminal outcome with
    /// the live-runs registry for an honest three-state: `running` (no
    /// terminal recorded, envelope process alive), `completed`/`failed`
    /// (terminal recorded), `interrupted` (no terminal, process gone —
    /// resumable via `workflow run <name> --resume <id>`).
    Runs(WorkflowRunsArgs),

    /// Explain what a workflow would cost BEFORE running it: its declared
    /// roles and ceilings, which roles launch serially, and the `agent()`
    /// call sites the pinned script contains.
    ///
    /// Static and read-only — the script is parsed, never executed, and no
    /// child is spawned. The point is to catch "this fans out wider than the
    /// ceiling allows" at authoring time instead of after paying for the
    /// first N children.
    Explain(WorkflowExplainArgs),

    /// Declare a workflow in ONE transaction: stage the script (and the
    /// blueprint it was approved from), add its `[workflows.<name>]` manifest
    /// entry, validate, and re-lock — or, on any failure, put everything back
    /// exactly as it was.
    ///
    /// Authoring a workflow by hand is six separate writes (script, manifest
    /// entry, role toolsets, lock, trust, run); this is one command, one
    /// rollback, and one `agentstack restore` entry.
    ///
    /// It stops before `trust` on purpose: consent is the human's step, and a
    /// command that granted it would be the second authority path this
    /// codebase refuses to grow.
    // History: a failure at step four used to leave a half-written manifest
    // behind a button labelled "Approve" (review finding F14) — the atomic
    // transaction is the fix.
    Declare(WorkflowDeclareArgs),
}

#[derive(Args, Debug)]
pub struct WorkflowDeclareArgs {
    /// The workflow name — becomes `[workflows.<name>]` and the staged
    /// filename. Must be a plain path component and must not already exist.
    #[arg(long)]
    pub name: String,

    /// Path to the workflow script to stage into `.agentstack/workflows/`.
    #[arg(long)]
    pub script: PathBuf,

    /// Path to the `agentstack-blueprint` JSON this script was authored from.
    /// Staged beside the script and pinned with it, so the graph a user
    /// approved and the bytes that run are one consent.
    #[arg(long)]
    pub blueprint: Option<PathBuf>,

    /// A role the script's `agent()` calls may name (repeatable). Every role
    /// must already be a declared toolset — this command declares a workflow,
    /// it never mints authority.
    //
    // The manifest key behind a toolset is still spelled `[profiles.<name>]`,
    // and this help used to say so. It cannot now that `workflow` is visible:
    // `visible_help_says_toolset` reserves the word "profile" for the manifest
    // key, the wire contract, and the frozen panel argv, and clap help is
    // prose a beginner reads. The key name is not lost — `docs/workflows.md`
    // and `agentstack toolset --help` both carry it, in places that can put it
    // in a code span.
    #[arg(long = "role")]
    pub roles: Vec<String>,

    /// Requested ceiling on total agent spawns. Narrowed by the machine cap.
    #[arg(long)]
    pub max_agents: Option<u32>,

    /// Requested wall-clock ceiling in seconds. Narrowed by the machine cap.
    #[arg(long)]
    pub max_wall_seconds: Option<u64>,

    /// Show what would be written and change nothing. The default for every
    /// non-interactive caller.
    #[arg(long)]
    pub preview: bool,

    /// Perform the transaction.
    #[arg(long)]
    pub write: bool,
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
pub struct WorkflowExplainArgs {
    /// The `[workflows.<name>]` entry to explain.
    #[arg(value_name = "NAME")]
    pub name: String,

    /// Emit the static analysis as JSON instead of a human render.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct WorkflowListArgs {
    /// Emit the declared workflow list as JSON instead of a human table.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct WorkflowRunsArgs {
    /// Emit the run history as JSON instead of a human table.
    #[arg(long)]
    pub json: bool,

    /// Newest-first cap on how many recorded runs are read and listed —
    /// the evidence directory only grows, so the listing is always bounded.
    #[arg(long, value_name = "N", default_value_t = 20)]
    pub limit: usize,
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

/// `agentstack image` — one toolset, materialized as something you run.
///
/// One command, not a subcommand tree: there is exactly one artifact and
/// exactly one act (build it). `--write` is the whole gate — without it
/// nothing touches disk and the Docker daemon is never contacted.
#[derive(Args, Debug)]
pub struct ImageArgs {
    /// Which toolset to package. Optional only when the project declares
    /// exactly one — an image is a composition, and picking one of several by
    /// guess would make the artifact's identity a guess too.
    #[arg(long, value_name = "NAME", alias = "profile")]
    pub toolset: Option<String>,

    /// Which tool the image launches (an adapter id, e.g. `claude-code`).
    /// Defaults to the toolset's own `harness`, then to the project's single
    /// default target.
    #[arg(long, value_name = "ID")]
    pub harness: Option<String>,

    /// Tag for the built image (default `agentstack/<toolset>:latest`). Local
    /// only — nothing is ever pushed.
    #[arg(long, value_name = "TAG")]
    pub tag: Option<String>,

    /// Base image to build `FROM`. Defaults to the same image `run --sandbox`
    /// would have launched, so a packaged image is that runner plus one
    /// toolset. Pass a digest reference to close the base-image axis; a
    /// floating tag leaves it open, and AgentStack will not claim otherwise.
    #[arg(long, value_name = "IMAGE")]
    pub from: Option<String>,

    /// Emit the plan as JSON (contract `image-plan-v1`).
    #[arg(long)]
    pub json: bool,

    /// Actually stage the build context and build the image (default: plan
    /// only).
    #[arg(long)]
    pub write: bool,
}

/// `agentstack delivery` — show the routing, or set the one override.
///
/// There is deliberately no `--mode` here and no second knob: the automatic
/// answer is the routed one, and **Render locally** is the whole escape hatch.
#[derive(Args, Debug)]
pub struct DeliveryArgs {
    #[command(subcommand)]
    pub command: Option<DeliveryCmd>,

    /// Emit the routing as JSON (contract `delivery-routing-v1`).
    #[arg(long)]
    pub json: bool,
}

#[derive(Subcommand, Debug)]
pub enum DeliveryCmd {
    /// Write files even where the live channel would have worked.
    ///
    /// The reasons this exists, so nobody has to re-argue them: offline
    /// operation; deterministic native files; inspection with ordinary
    /// filesystem tools; a corporate policy that forbids a persistent
    /// background process; debugging without another runtime dependency; and
    /// compatibility testing against a CLI's own behaviour.
    ///
    /// Records `[delivery] render_locally` in the manifest so the answer is the
    /// same on every clone and every run. Dry-run by default.
    RenderLocally {
        /// Set it for one tool only (an adapter id, e.g. `claude-code`).
        /// Omitted, it applies to the whole project.
        #[arg(long, value_name = "ID")]
        harness: Option<String>,

        /// Remove the override instead of setting it — back to automatic.
        #[arg(long)]
        off: bool,

        /// Actually write the manifest (default: preview only).
        #[arg(long)]
        write: bool,
    },
}

/// The runtime lease registry's read surface. Read-only by design: leases are
/// opened and closed by the MCP connection that owns them, never from here.
#[derive(Subcommand, Debug)]
pub enum LeaseCmd {
    /// Show the lease records on this machine, each with liveness derived now
    /// from its PID and that process's start time (contract `lease-status-v1`).
    Status {
        /// Emit the same reading as JSON.
        #[arg(long)]
        json: bool,
    },
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

    /// Ask for the Protected tier explicitly (already the default; kept for
    /// compatibility). `--locked --sandbox` and `--locked --lockdown` refuse.
    // History: Protected became the default tier, so this flag stopped
    // selecting anything. It survives for the scripts, docs, and panels that
    // already type it, and it keeps its own combination rules.
    #[arg(long)]
    pub locked: bool,

    /// Opt OUT of the Protected default and launch on the host with no
    /// pre-launch gate: no trust check, no strict lock verification, no policy
    /// admission, no frozen grant. Labelled `HOST / ADVISORY`, because that is
    /// what it is. The escape hatch for a project you have not locked or
    /// trusted yet, and for anything the gates refuse for a reason you have
    /// decided to accept — never the way to run day to day.
    #[arg(long)]
    pub unprotected: bool,

    /// Run the harness headless with TEXT as its prompt. Cannot be combined
    /// with --unprotected, --sandbox, --lockdown, or trailing harness
    /// arguments. Stdout carries the harness output; launcher banners go to
    /// stderr.
    // Why it works this way: the prompt is delivered as one whole argv element
    // through the adapter's declared headless invocation (e.g. `claude -p`,
    // `codex exec`) — never through a shell — and is committed verbatim into
    // the frozen grant's argv, so the evidence binds what the agent was asked
    // to do. Stdout is captured (bounded), relayed to this process's stdout,
    // and recorded by digest + byte count only; banners are routed to stderr
    // so stdout carries the harness output and nothing else. Trailing harness
    // arguments are refused because they would land after the prompt's `--`
    // terminator and silently misparse as positionals.
    #[arg(long, value_name = "TEXT")]
    pub prompt: Option<String>,

    /// Apply this toolset's servers + skills for the life of the run.
    // `--profile` stays as an alias: it is the spelling every existing script
    // and the t3code panel already type, and dropping it would break them for
    // a vocabulary fix. The field keeps its name — no user reads it.
    #[arg(long = "toolset", alias = "profile", value_name = "NAME")]
    pub profile: Option<String>,

    /// Scope to apply the toolset in (only meaningful with --toolset).
    /// Defaults to the manifest home: global for the machine manifest,
    /// project for a repository manifest.
    #[arg(long, value_enum)]
    pub scope: Option<Scope>,

    /// Leave the applied toolset in place after the run exits (default: revert).
    #[arg(long)]
    pub keep: bool,

    /// Launch the CLI inside a sandbox container instead of on the host.
    /// The container mounts the project as its workspace and points
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

    /// Which model this run's child should use, and how much reasoning effort
    /// it should spend. INTERNAL PLUMBING, not user-facing flags — hence
    /// `#[arg(skip)]`: the only producer is the workflow drive loop, which
    /// copies them from the role's toolset (`[profiles.<role>] model/effort`),
    /// and the only consumer is the headless launch path, which asks the
    /// adapter's descriptor how (or whether) to carry them.
    ///
    /// Deliberately NOT exposed as `agentstack run --model`. Two reasons, both
    /// already in this file's grain: `launch_argv`'s doc comment explains why a
    /// user-typed harness flag on `run` is a misparse hazard once `--prompt`'s
    /// `--` terminator is in play, and a user-facing selection flag is product
    /// surface this item did not ask for. The manifest is where a model is
    /// declared; the flag would be a second, undeclared authority for it.
    #[arg(skip)]
    pub model: Option<String>,
    #[arg(skip)]
    pub effort: Option<String>,

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
pub struct ShareArgs {
    /// Name for the bundle (used for the filename by default).
    pub name: String,

    /// Write the bundle here instead of `<name>.astack`.
    #[arg(long)]
    pub out: Option<std::path::PathBuf>,
}

#[derive(Args, Debug)]
pub struct ReceiveArgs {
    /// The `.astack` bundle to review.
    pub path: std::path::PathBuf,

    /// Accept without the interactive prompt. The review still prints — this
    /// answers it, it does not skip it.
    #[arg(long)]
    pub yes: bool,
}

#[derive(Args, Debug)]
pub struct PublisherArgs {
    #[command(subcommand)]
    pub cmd: Option<PublisherCmd>,
}

#[derive(Subcommand, Debug)]
pub enum PublisherCmd {
    /// Show your publishing key and the publishers you recognize.
    Show {},
    /// Recognize a publisher's key, so their bundles say so on the card.
    Trust {
        /// Their full public key (64 hex characters).
        key: String,
        /// What to call them locally.
        #[arg(long)]
        label: String,
    },
}

#[derive(Args, Debug)]
pub struct UpArgs {
    /// Only render these CLIs (default: every detected one).
    #[arg(long)]
    pub targets: Vec<String>,

    // Spelled `--toolset` on the visible surface with `--profile` kept as an
    // alias — the same split `apply` and `use` make. The rationale lives in a
    // plain comment, not a doc comment: clap renders `///` as the flag's long
    // help, so explaining the word "profile" here would itself put "profile"
    // on the visible surface, which is what the rule forbids. (That is how
    // this failed the first time.)
    /// Materialize this toolset rather than the active one.
    #[arg(long = "toolset", alias = "profile", value_name = "NAME")]
    pub profile: Option<String>,

    /// Do not maintain the managed `.gitignore` block.
    #[arg(long)]
    pub no_gitignore: bool,
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
    /// Composed-call only: suppress this command's own summary and next-step
    /// lines. Set by the funnel, which prints ONE card and must not have three
    /// sub-commands each proposing a competing next action. Never a flag.
    #[arg(skip)]
    pub quiet: bool,

    /// Only pin this toolset's servers and packages (default: every toolset in
    /// the manifest). Declared skills always pin manifest-wide: the trust gate
    /// reviews the whole `[skills]` table, so a narrowed skill pin set would
    /// leave the project un-trustable.
    // Spelled `--toolset` since `lock` joined the visible list: everything a
    // user can see says toolset, and the older noun survives only as the
    // manifest key, the wire contract, and the frozen panel argv. The old
    // spelling stays as a hidden alias so every existing invocation and script
    // keeps working — a rename that breaks a working command line is not a
    // rename, it is a removal.
    #[arg(long = "toolset", alias = "profile", value_name = "NAME")]
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

    /// Write the pins (else preview). With --upgrade, writes the upgrade.
    #[arg(long)]
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

/// `status` takes no flags of its own beyond `--json` — `--manifest-dir` is
/// global, and the deep flags all belong to `doctor`.
#[derive(Args, Debug)]
pub struct StatusArgs {
    /// Emit the same reading as JSON (contract `json-reads-v1`).
    #[arg(long)]
    pub json: bool,
}

/// `setup` is the interactive newcomer wizard; it deliberately has no `--write`
/// (it confirms in a terminal and stays dry-run everywhere else). Scripts use
/// `init` + `apply --write` + `use <toolset> --write`.
#[derive(Args, Debug)]
pub struct SetupArgs {
    /// Only configure these CLIs (repeatable). Defaults to [targets].default.
    #[arg(long = "target", value_name = "CLI")]
    pub targets: Vec<String>,

    /// Configure only the servers in this toolset.
    #[arg(long, value_name = "NAME")]
    pub profile: Option<String>,

    /// Which scope to write: global (~) or project (repo). Defaults to the
    /// manifest's home — project for a repo manifest, global for the machine
    /// manifest (~/.agentstack).
    #[arg(long, value_enum)]
    pub scope: Option<Scope>,

    /// Write the imported MCP servers into this project's manifest as inline
    /// `[servers.*]` entries instead of into your first linked library source.
    /// Carried through to the wizard's import step, so `agentstack init
    /// --project-servers` means the same thing on both routes.
    #[arg(long)]
    pub project_servers: bool,
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
    /// Also add to this toolset's server list.
    #[arg(long = "toolset", alias = "profile", value_name = "NAME")]
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
    /// Also add to this toolset's server list.
    #[arg(long = "toolset", alias = "profile", value_name = "NAME")]
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
    /// Also add to this toolset's skill list.
    #[arg(long = "toolset", alias = "profile", value_name = "NAME")]
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

    /// Detection only, as JSON: which CLIs were found, which MCP servers
    /// would be imported, which inline secrets would be lifted (their `${REF}`
    /// names and origins — never values) and the proposed destination. Writes
    /// nothing, prompts nothing. The read primitive behind external setup
    /// wizards (UI control-plane §4).
    #[arg(long)]
    pub plan: bool,

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

    /// Write the imported MCP servers into this project's manifest as inline
    /// `[servers.*]` entries instead of into your first linked library source.
    /// The default is library-first: the project references them by name.
    #[arg(long)]
    pub project_servers: bool,

    /// Run the promptless import without a terminal: acknowledge that the
    /// manifest (and any lifted token values) will be written. Required when
    /// stdin is not a TTY and no other init-shaping flag is given.
    #[arg(long)]
    pub yes: bool,

    /// The `plan_digest` an `init --plan` emitted alongside the plan that was
    /// reviewed. The scripted import (with `--yes`) then refuses if detection
    /// no longer produces that exact plan — a CLI config edited between plan
    /// and apply forces a fresh review instead of importing unseen content.
    #[arg(long, value_name = "DIGEST")]
    pub consented_plan: Option<String>,
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

    /// Render only the servers in this toolset.
    #[arg(long = "toolset", alias = "profile", value_name = "NAME")]
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

    /// Show the full contents of every rendered file, not just a per-target
    /// summary line.
    #[arg(long, short)]
    pub verbose: bool,
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

#[derive(Args, Debug, Default)]
pub struct UseArgs {
    /// Composed-call only — see [`LockArgs::quiet`]. Never a flag.
    #[arg(skip)]
    pub quiet: bool,

    /// Toolset to activate. Optional: with one toolset declared it is chosen
    /// automatically, and with none declared the implicit default — every
    /// inline skill and server — activates. Several toolsets need a name.
    #[arg(value_name = "TOOLSET")]
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

    /// List declared toolsets instead of activating: each toolset's resolved
    /// skills/servers/harness plus a readiness flag — is everything it
    /// references pinned in agentstack.lock and matching? The read primitive
    /// behind external toolset pickers (UI control-plane §5).
    #[arg(long)]
    pub list: bool,

    /// With --list: emit machine-readable JSON.
    #[arg(long)]
    pub json: bool,
}

/// Consent + apply flags shared by every panel edit action. Preview is the
/// default (nothing writes); applying requires `--yes` AND a `--consented`
/// digest from a prior preview — the same non-interactive gate `apply-setup`
/// and `trust-consent` enforce.
/// `yes` — the funnel's single action: declare, pin, review, activate
/// the locally-authored files waiting in this project, behind one preview and
/// one confirmation.
#[derive(Args, Debug, Default)]
pub struct YesArgs {
    /// Acknowledge the review without an interactive prompt. Same meaning it
    /// has on `trust`: the reviewer asserts they read what was printed.
    #[arg(long)]
    pub yes: bool,
}

#[derive(clap::Args, Debug, Clone)]
pub struct PanelConsent {
    /// Emit the enveloped plan + consent digest and write nothing. The default
    /// for every non-interactive caller; pass it explicitly to force the JSON
    /// shape at a terminal too.
    #[arg(long)]
    pub preview: bool,

    /// Apply non-interactively. Requires `--consented`; refuses without it.
    #[arg(long)]
    pub yes: bool,

    /// The consent digest from a prior preview. Apply refuses on any mismatch.
    #[arg(long, value_name = "DIGEST")]
    pub consented: Option<String>,

    /// Let activation proceed even if a `${REF}` did not resolve (off by
    /// default — an unresolved secret blocking the render is a feature).
    #[arg(long)]
    pub allow_unresolved: bool,
}

/// `add-skill-to-profile` — define a new skill (`--git`/`--path`) or enroll an
/// existing library/inline skill by name (neither flag), into `--profile`.
#[derive(Args, Debug)]
pub struct PanelAddSkillArgs {
    /// Existing toolset to add the skill to and activate.
    #[arg(long)]
    pub profile: String,

    /// Manifest / library name the skill is referenced by.
    #[arg(long)]
    pub name: String,

    /// New git-sourced skill: the source URL. Omit --git and --path to enroll an
    /// existing library/inline skill by name.
    #[arg(long)]
    pub git: Option<String>,

    /// Branch/tag/commit for a git source (the exact commit is pinned in lock).
    #[arg(long)]
    pub rev: Option<String>,

    /// Directory within the repo for a git source.
    #[arg(long)]
    pub subpath: Option<String>,

    /// New path-sourced skill: the local SKILL.md directory.
    #[arg(long)]
    pub path: Option<String>,

    #[command(flatten)]
    pub consent: PanelConsent,
}

/// `add-server-to-profile` — define a new server (any of the wire flags) or
/// enroll an existing library/inline server by name, into `--profile`.
#[derive(Args, Debug)]
pub struct PanelAddServerArgs {
    /// Existing toolset to add the server to and activate.
    #[arg(long)]
    pub profile: String,

    /// Manifest / library name the server is referenced by.
    #[arg(long)]
    pub name: String,

    /// Transport for a new server definition.
    #[arg(long = "type", value_enum, default_value = "http")]
    pub transport: ServerType,

    /// HTTP server URL (new definition).
    #[arg(long)]
    pub url: Option<String>,

    /// Header `Key=Value` (repeatable); values may contain `${REF}`.
    #[arg(long = "header", value_name = "K=V")]
    pub headers: Vec<String>,

    /// stdio command (new definition).
    #[arg(long)]
    pub command: Option<String>,

    /// stdio arg (repeatable). Accepts leading-dash values.
    #[arg(long = "arg", value_name = "ARG", allow_hyphen_values = true)]
    pub args: Vec<String>,

    /// Working directory the stdio server launches from; may contain `${REF}`.
    #[arg(long)]
    pub cwd: Option<String>,

    /// Env `Key=Value` (repeatable); values may contain `${REF}`.
    #[arg(long = "env", value_name = "K=V")]
    pub env: Vec<String>,

    #[command(flatten)]
    pub consent: PanelConsent,
}

/// `uninstall` — revert every managed region and remove AgentStack's own state.
#[derive(Args, Debug)]
pub struct UninstallArgs {
    /// Which rendered output to revert: `project`, `global`, or `all`.
    #[arg(long, default_value = "all", value_parser = ["project", "global", "all"])]
    pub scope: String,

    /// Actually remove. Without it this only shows what would be removed.
    #[arg(long)]
    pub write: bool,

    /// Show the full diff of every file, not just its name.
    #[arg(long, short)]
    pub verbose: bool,

    /// Leave `~/.agentstack` in place — its undo ledger, trust store, and
    /// central library survive, so `agentstack restore` keeps working.
    #[arg(long)]
    pub keep_home: bool,
}

/// `set-gitignore` — this project's durable answer to whether agentstack
/// maintains its managed `.gitignore` block.
///
/// A per-run `--no-gitignore` cannot express the panel's "keep .gitignore as
/// is": the next toolset switch would re-add the block. So this records the
/// decision in the manifest, where every later command reads it.
#[derive(Args, Debug)]
pub struct PanelSetGitignoreArgs {
    /// `true` restores the default (the block is maintained and the key is
    /// removed); `false` opts this project out.
    #[arg(long, action = clap::ArgAction::Set)]
    pub enabled: bool,

    #[command(flatten)]
    pub consent: PanelConsent,
}

/// `set-mode` — switch this project's delivery mode, previewing the real plan.
#[derive(Args, Debug)]
pub struct PanelSetModeArgs {
    /// The target mode: static | clean-at-rest | zero-files — the same labels
    /// `doctor --json` reports, so a panel round-trips the string it read.
    #[arg(value_name = "MODE")]
    pub mode: String,

    #[command(flatten)]
    pub consent: PanelConsent,
}

/// `create-profile` — a new toolset bundling existing/library skills + servers.
#[derive(Args, Debug)]
pub struct PanelCreateProfileArgs {
    /// New toolset name (must not already exist).
    #[arg(long)]
    pub name: String,

    /// Skill to include (repeatable). `*` means every inline skill.
    #[arg(long = "skill", value_name = "NAME")]
    pub skills: Vec<String>,

    /// Server to include (repeatable).
    #[arg(long = "server", value_name = "NAME")]
    pub servers: Vec<String>,

    #[command(flatten)]
    pub consent: PanelConsent,
}

/// `edit-profile` — one toolset's membership, changed as a single batch.
///
/// The existing `add-*-to-profile` verbs each mutate, re-lock and re-render on
/// their own, so composing a toolset by hand costs one write, one lock and one
/// render per capability — and there was no way to take something out again at
/// all. This applies every add and every removal in one write, under ONE
/// consent digest, followed by one re-lock and one re-render.
///
/// Removal here ends a MEMBERSHIP: the capability stays declared in the
/// manifest and stays in the central library. Removing the capability itself is
/// `remove-from-library`, which is machine-wide and says so.
#[derive(Args, Debug)]
pub struct PanelEditProfileArgs {
    /// The toolset whose membership is changing.
    #[arg(long)]
    pub profile: String,

    /// Skill to add (repeatable).
    #[arg(long = "add-skill", value_name = "NAME")]
    pub add_skills: Vec<String>,

    /// Skill to take out of this toolset (repeatable). It stays declared.
    #[arg(long = "remove-skill", value_name = "NAME")]
    pub remove_skills: Vec<String>,

    /// Server to add (repeatable).
    #[arg(long = "add-server", value_name = "NAME")]
    pub add_servers: Vec<String>,

    /// Server to take out of this toolset (repeatable). It stays declared.
    #[arg(long = "remove-server", value_name = "NAME")]
    pub remove_servers: Vec<String>,

    #[command(flatten)]
    pub consent: PanelConsent,
}

/// `rename-profile` — re-key one `[profiles.<name>]` table.
///
/// Deliberately narrow: it refuses rather than cascading. A toolset name is
/// also a workflow's role name (`[workflows.*].roles`), which is pinned in
/// `agentstack.lock` and length-framed into that workflow's grant digest — so
/// renaming one out from under a workflow would rewrite a reviewed authority
/// surface without consent and permanently strand any parked run holding the
/// old digest. Those cases refuse, naming the command that clears them.
#[derive(Args, Debug)]
pub struct PanelRenameProfileArgs {
    /// The toolset to rename.
    #[arg(long)]
    pub name: String,

    /// Its new name.
    #[arg(long = "to", value_name = "NAME")]
    pub to: String,

    #[command(flatten)]
    pub consent: PanelConsent,
}

/// `delete-profile` — drop one `[profiles.<name>]` table.
///
/// Removes the grouping only: the servers and skills it named stay declared and
/// stay in the library, because a toolset is a selection over them rather than
/// their owner. Refuses the same cases `rename-profile` does, plus deleting the
/// last toolset — with `[profiles]` empty the render and the proxied server
/// surface both fall back to *everything* in the manifest, so that delete
/// widens rather than tidies.
#[derive(Args, Debug)]
pub struct PanelDeleteProfileArgs {
    /// The toolset to delete.
    #[arg(long)]
    pub name: String,

    #[command(flatten)]
    pub consent: PanelConsent,
}

/// `use-profile` — activate an existing toolset (re-lock + re-render only).
#[derive(Args, Debug)]
pub struct PanelUseProfileArgs {
    /// Toolset to activate.
    #[arg(long)]
    pub profile: String,

    #[command(flatten)]
    pub consent: PanelConsent,
}

/// The library collections the panel may remove from. Skills and servers are
/// the two kinds `library-index` publishes, so they are the two the browser can
/// act on; extensions and hooks stay CLI-only (`lib remove-extension`,
/// `lib remove-hook`) until the browser lists them.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelLibraryKind {
    Skill,
    Server,
}

impl PanelLibraryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            PanelLibraryKind::Skill => "skill",
            PanelLibraryKind::Server => "server",
        }
    }
}

/// `remove-from-library` — drop one central-library capability, machine-wide.
///
/// Unlike the `add-*-to-profile` verbs this touches no manifest, no lockfile,
/// and renders nothing: the central library is machine state, shared by every
/// project. It is still digest-bound, because it is a destructive-looking
/// change the user must see first — and it is recoverable, because the body and
/// index row move to `lib/.trash` rather than being deleted.
///
/// It carries the shared [`PanelConsent`] block so every panel verb has one
/// consent shape; `--allow-unresolved` is inert here (nothing renders, so no
/// `${REF}` is ever resolved) and t3code never emits it for this verb.
#[derive(Args, Debug)]
pub struct PanelRemoveFromLibraryArgs {
    /// Which library collection the name lives in.
    #[arg(long, value_enum)]
    pub kind: PanelLibraryKind,

    /// The central-library name to remove.
    #[arg(long)]
    pub name: String,

    #[command(flatten)]
    pub consent: PanelConsent,
}

/// `remove-capability` — delete one project-owned server or skill definition.
///
/// Unlike [`PanelRemoveFromLibraryArgs`], this is project-scoped: the central
/// library is untouched. The definition and every toolset membership disappear
/// together, then AgentStack re-locks and re-renders the resulting manifest.
#[derive(Args, Debug)]
pub struct PanelRemoveCapabilityArgs {
    /// Which manifest collection the name lives in.
    #[arg(long, value_enum)]
    pub kind: PanelLibraryKind,

    /// The project-owned capability name to remove.
    #[arg(long)]
    pub name: String,

    #[command(flatten)]
    pub consent: PanelConsent,
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
    /// Start a session: load a toolset for now.
    Start {
        /// Toolset to load.
        #[arg(value_name = "TOOLSET")]
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
    List {
        /// Emit the same listing as JSON (contract `json-reads-v1`).
        #[arg(long)]
        json: bool,
    },
    /// Freeze the active session's resolved set (toolset servers + the skills
    /// actually loaded) into a new toolset, so CI can replay it deterministically.
    Freeze {
        /// Name for the frozen toolset (default: <toolset>-frozen).
        #[arg(long)]
        name: Option<String>,
    },
}

#[derive(Args, Debug)]
pub struct UndoArgs {
    /// Revert to before change <n> from the timeline — everything newer comes
    /// off with it, because that is what "back to that point" means. Omit to
    /// be asked (or, with no terminal, just to see the list).
    #[arg(long, value_name = "N")]
    pub to: Option<usize>,

    /// Do it (else preview which files would move).
    #[arg(long)]
    pub write: bool,

    /// Machine-readable timeline. Read-only — there is no JSON path that
    /// performs a revert, so a UI has to go through the same explicit
    /// `--write` a person does.
    #[arg(long)]
    pub json: bool,
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

    /// List everything undoable — the same listing a bare `agentstack
    /// restore` prints. Spelled out for the common case of typing the verb
    /// and reaching for a flag out of habit.
    #[arg(long, conflicts_with_all = ["adapter", "last"])]
    pub list: bool,

    /// Where writes land: global (each CLI user-level config) or project
    /// (repo-local). Defaults to the manifest home.
    #[arg(long, value_enum)]
    pub scope: Option<Scope>,

    /// Write the change (else preview).
    #[arg(long)]
    pub write: bool,

    /// Machine-readable output: the undoable-change list (no argument) or the
    /// selected undo's preview/result (`--last`, or an id). External UIs use
    /// this to show a real Undo affordance instead of prose.
    #[arg(long)]
    pub json: bool,
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

    /// Also save adopted skills to the central library, so other projects can
    /// use them. Skills only — instructions stay project-local.
    #[arg(long)]
    pub to_library: bool,
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
    /// Remove a skill from the library source that holds it.
    Remove(LibRemoveArgs),
    /// Remove a server from the library source that holds it.
    RemoveServer(LibRemoveServerArgs),
    /// Remove an extension from the library source that holds it.
    RemoveExtension(LibRemoveExtensionArgs),
    /// Remove a hook from the library source that holds it.
    RemoveHook(LibRemoveHookArgs),
    /// List what removal put in the library trash, and restore or empty it.
    /// Every `lib remove*` moves the entry here instead of deleting it.
    #[command(after_help = "\
Examples:
  agentstack lib trash                                  # what's recoverable
  agentstack lib trash --restore skill-pdf-1753574400 --write
  agentstack lib trash --empty --write                  # delete it for good")]
    Trash(LibTrashArgs),
    /// Sync the central library across machines as a git repo (commit local
    /// changes, pull, push). Secrets never travel — server defs are `${REF}`.
    Sync(LibSyncArgs),
    /// Scaffold a publishable pack (pack.toml + example skill) in the current
    /// directory. Publish by pushing the repo and tagging a version (e.g.
    /// v0.1.0); install with `agentstack add from git:<host>/<repo>@<tag>`.
    PackInit(PackInitArgs),
    /// Link a folder as a library source. Any folder works — a git clone, a
    /// synced drive, or a plain directory.
    #[command(after_help = "\
Examples:
  agentstack lib link ~/work/team-capabilities --write
  agentstack lib link ~/scratch/skills --name scratch --first --write")]
    Link(LibLinkArgs),
    /// Unlink a library source. The folder itself is left alone.
    Unlink(LibUnlinkArgs),
    /// Show the linked library sources in precedence order, and every name one
    /// source shadows in another.
    Sources,
    /// Set the precedence order of the linked library sources. Name every
    /// linked source exactly once, first to last.
    Reorder(LibReorderArgs),
}

#[derive(Args, Debug)]
pub struct LibLinkArgs {
    /// The folder to link.
    pub path: String,
    /// The name this source is addressed by in a `<source>:<name>` reference.
    /// Defaults to the folder's own name.
    #[arg(long)]
    pub name: Option<String>,
    /// Link at the front of the list, so this source wins name collisions.
    #[arg(long)]
    pub first: bool,
    /// A one-line note about what this source is.
    #[arg(long)]
    pub note: Option<String>,
    /// Write the change (else preview).
    #[arg(long)]
    pub write: bool,
}

#[derive(Args, Debug)]
pub struct LibUnlinkArgs {
    /// The source name to unlink.
    pub name: String,
    /// Write the change (else preview).
    #[arg(long)]
    pub write: bool,
}

#[derive(Args, Debug)]
pub struct LibReorderArgs {
    /// Every linked source name, in the order you want them resolved.
    #[arg(required = true)]
    pub names: Vec<String>,
    /// Write the change (else preview).
    #[arg(long)]
    pub write: bool,
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
    /// Which linked library source to sync (default: the first one).
    #[arg(long)]
    pub source: Option<String>,
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
pub struct LibTrashArgs {
    /// Put a trashed entry back in the library (its id, from the listing).
    #[arg(long, value_name = "ID", conflicts_with = "empty")]
    pub restore: Option<String>,
    /// Permanently delete the trash — everything, or just `--id <ID>`. This is
    /// the one library operation that destroys content.
    #[arg(long)]
    pub empty: bool,
    /// Limit `--empty` to one entry.
    #[arg(long, value_name = "ID", requires = "empty")]
    pub id: Option<String>,
    /// Restore over a same-named entry (or an existing body) that came back
    /// after the removal.
    #[arg(long, requires = "restore")]
    pub replace: bool,
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

#[derive(Args, Debug, Default)]
pub struct InstructionsArgs {
    /// Only act on these CLIs (repeatable). Defaults to [targets].default.
    #[arg(long = "target", value_name = "CLI")]
    pub targets: Vec<String>,

    /// Compile the house rules this toolset selects — its `model` picks the
    /// per-(CLI, model) variant. Without it the model is unknown and the least
    /// specific matching body is used, which the output states.
    #[arg(long = "toolset", alias = "profile", value_name = "NAME")]
    pub toolset: Option<String>,

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

    /// Actually start each stdio server and check it comes up (the stdio
    /// counterpart of --live).
    ///
    /// The one doctor flag with side effects: it spawns the commands your
    /// manifest declares, speaks the MCP `initialize` handshake, and stops
    /// them again. Every child is bounded by a hard timeout and killed with
    /// its process group. Refuses to spawn anything for a project that is not
    /// trusted at its current bytes, and never spawns a server whose `${REF}`
    /// does not resolve.
    #[arg(long)]
    pub probe: bool,

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
    /// Emit the report as JSON (for t3code or further processing).
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

    /// Also interleave on-demand skill loads into the `--json` events feed,
    /// each row tagged with a `kind` ("call" / "skill_load"). Off by default:
    /// without it the feed is byte-identical to before, so a consumer that
    /// predates load rows never meets a shape it can't decode. Loads are
    /// activity, never calls — they are absent from every count and table.
    #[arg(long)]
    pub include_loads: bool,

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

    /// Show every match instead of the most relevant few. The default page is
    /// short on purpose: a first screen of substring matches teaches you to
    /// search somewhere else. `--json` is unaffected and always carries the
    /// complete ranked list.
    #[arg(long)]
    pub all: bool,

    /// Emit the same results as JSON (contract `json-reads-v1`).
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct AdaptersArgs {
    #[command(subcommand)]
    pub command: AdaptersCommand,
}

#[derive(Subcommand, Debug)]
pub enum AdaptersCommand {
    /// List known adapters and whether each CLI looks installed.
    List {
        /// Emit the same listing as JSON (contract `json-reads-v1`).
        #[arg(long)]
        json: bool,
    },
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

/// The name of the namespace that holds everything outside the visible list.
///
/// One letter, because it is punctuation rather than vocabulary: `x` names no
/// concept the product teaches, so it cannot compete with Setup, Toolset,
/// Status and Undo for a reader's attention.
pub const NAMESPACE: &str = "x";

/// What `agentstack x` prints: the rest of the toolbox, grouped by task.
///
/// Grouped by the same headings as `--help --all`, minus the fifteen commands
/// the default help already lists — this screen is the complement of that one,
/// not a second copy of it.
pub fn namespace_listing() -> String {
    String::from(
        "agentstack x — the rest of the toolbox. Every one of these also runs at its\n\
         own name: `agentstack x guard install` and `agentstack guard install` are the\n\
         same command. Run `agentstack x <command> --help` for flags and details.\n\
         \n  \
         Set up      up · adapters · settings · self · completions\n  \
         Edit        set · remove · install · lib · export · import\n  \
         Share       share · receive · publisher\n  \
         Render      instructions · session · diff · uninstall · delivery\n  \
         Undo        restore\n  \
         Protect     explain · guard · sign · verify\n  \
         Run         kill · shim · workflow · image · gateway · mcp · try\n  \
         Inspect     report · lease · optimize · proxy\n\
         \n\
         The everyday fifteen are on `agentstack --help`. For all of it at once,\n\
         including the fixed actions a graphical panel invokes:\n  \
         agentstack --help --all\n",
    )
}

/// Strip a leading `x` so `agentstack x <cmd> …` parses as `agentstack <cmd> …`.
///
/// `argv` is the full command line including the binary name. Returns `None`
/// when the namespace was not used, so the caller can pass the original
/// through untouched.
///
/// Deliberately display-only: the `x` is removed BEFORE clap sees it, so there
/// is exactly one parse tree and exactly one dispatch path in `main.rs`. A
/// nested clap subcommand would need a second copy of the whole `Command` enum
/// and a second dispatch arm for every verb — two places to forget.
///
/// Only the first argument is considered. `agentstack apply x` is an argument
/// to `apply`, not a namespace, and stays that way.
pub fn strip_namespace(argv: &[String]) -> Option<Vec<String>> {
    let (bin, rest) = argv.split_first()?;
    let (first, tail) = rest.split_first()?;
    if first != NAMESPACE {
        return None;
    }
    let mut out = Vec::with_capacity(argv.len() - 1);
    out.push(bin.clone());
    out.extend_from_slice(tail);
    Some(out)
}

/// The `--help --all` view: every command — visible or hidden — with its
/// one-line summary, subcommands indented under their parent. This is the
/// "long" half of the progressive-disclosure pair; the default `--help` shows
/// only the beginner loop plus the grouped name map.
pub fn full_command_inventory() -> String {
    use clap::CommandFactory;

    /// Fixed argv the t3code panel drives; not commands a person runs. They
    /// stay listed (the inventory is complete by definition) but under their
    /// own heading, so the human map is not padded with machine surface.
    /// `create-profile` belongs here now that `toolset create` is its visible
    /// spelling: the by-hand path a person reads about is the `toolset` group,
    /// and this name survives only because t3code emits it as fixed argv.
    const PANEL_ONLY: &[&str] = &[
        "add-skill-to-profile",
        "add-server-to-profile",
        "create-profile",
        "edit-profile",
        "rename-profile",
        "delete-profile",
        "use-profile",
        "library-index",
        "remove-from-library",
        "remove-capability",
        "set-gitignore",
        "set-mode",
    ];

    fn push(out: &mut String, cmd: &clap::Command, indent: usize, panel: bool) {
        for sub in cmd.get_subcommands() {
            if sub.get_name() == "help" {
                continue;
            }
            // Top level only: nested subcommands of a panel verb travel with
            // their parent, and no nested name collides with these.
            if indent == 2 && PANEL_ONLY.contains(&sub.get_name()) != panel {
                continue;
            }
            let about = sub.get_about().map(|s| s.to_string()).unwrap_or_default();
            let pad = " ".repeat(indent);
            out.push_str(&format!("{pad}{:<16} {about}\n", sub.get_name()));
            push(out, sub, indent + 2, panel);
        }
    }

    let cmd = Cli::command();
    // The task-grouped name map lives here rather than in the default `--help`.
    // Printing all ~40 names two lines under the short curated list undid the
    // curation on the same screen; a reader who asked for `--all` has opted in.
    let mut out = String::from(
        "agentstack — every command, including the ones the default --help groups away.\n\
         Run `agentstack <command> --help` for flags and details.\n\
         \n\
         Words, for when you want them: a CLI (a.k.a. harness) is the agent tool you\n\
         run; an adapter compiles its native config; the manifest is the one file that\n\
         declares your setup, and [targets] in it lists which CLIs commands act on;\n\
         the lock pins the exact content you reviewed. None of these are needed to\n\
         use the four ideas — they are here because this is the full map.\n\
         \n\
         The map, grouped by task:\n\
         \n  \
         Set up      init · up · status · adapters · settings · self · completions\n  \
         Edit        add · set · search · remove · install · lib · toolset · adopt · export · import\n  \
         Share       share · receive · publisher\n  \
         Render      apply · use · yes · instructions · lock · session · diff · uninstall · delivery\n  \
         Undo        undo · restore\n  \
         Protect     trust · explain · secret · guard · sign · verify\n  \
         Run         run · kill · shim · workflow · image · gateway · mcp · try\n  \
         Inspect     doctor · report · lease · optimize · proxy\n\
         \n\
         And in full:\n\n",
    );
    push(&mut out, &cmd, 2, false);
    out.push_str(
        "\nIntegration contract (t3code) — fixed actions a graphical panel invokes.\n\
         Not part of the everyday surface; each is digest-bound and previews before it writes.\n\n",
    );
    push(&mut out, &cmd, 2, true);
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
