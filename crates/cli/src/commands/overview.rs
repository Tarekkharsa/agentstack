//! Bare `agentstack` — orientation instead of a wall of subcommands: what's
//! detected on this machine, what state this directory's manifest is in, and
//! the one next command to run.

use std::path::Path;

use anyhow::Result;
use owo_colors::OwoColorize;

use crate::adapter::Registry;
use crate::manifest::load::MANIFEST_FILE;
use crate::scope::Scope;

/// The three delivery modes a project can be in (see docs/design P4). They are
/// not stored anywhere — a project's mode is *derived* from what's observable on
/// disk, so "which mode am I in?" is never archaeology. Rust enums with methods
/// are like a TypeScript union type paired with a lookup table.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Mode {
    /// Rendered configs live on disk (static, the default).
    Static,
    /// Nothing between sessions; `session start`/`end` materialize + revert.
    CleanAtRest,
    /// Nothing ever written; the gateway serves capabilities live over MCP.
    ZeroFiles,
}

impl Mode {
    /// The short name shown on the orientation line and in the setup choice.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Mode::Static => "static",
            Mode::CleanAtRest => "clean-at-rest",
            Mode::ZeroFiles => "zero-files",
        }
    }

    /// A terse descriptor for the one-line orientation display.
    pub(crate) fn short(self) -> &'static str {
        match self {
            Mode::Static => "config files on disk, kept out of git",
            Mode::CleanAtRest => "active only while you work, then restored",
            // Honesty rule (design §"Honesty rules"): never a bare "nothing on
            // disk". The project still holds its manifest and lock, and any
            // house-rules region stays in its file — what this mode removes is
            // the GENERATED artifacts.
            Mode::ZeroFiles => {
                "no generated files — capabilities served live to your CLIs after review"
            }
        }
    }

    /// The full one-line help (docs/design P4 wording), shown when setup
    /// presents the three modes as a choice. Outcome language first (Stage 1.4):
    /// the gateway/trust mechanics stay in the docs and the commands themselves,
    /// not in the first-run copy.
    pub(crate) fn help(self) -> &'static str {
        match self {
            Mode::Static => "Config files stay on disk, kept out of git. Works with every CLI, zero moving parts. This is what you have now.",
            Mode::CleanAtRest => "Use a toolset temporarily: `agentstack x session start` activates it and `session end` puts every file back exactly as it was. Nothing stays in your repo between sessions.",
            Mode::ZeroFiles => "No generated files are written; your CLIs fetch servers and skills live from agentstack, and each repo stays inert until you review it once. The repo still keeps its agentstack manifest and lock, and any house-rules region stays in its file. Best when you work across many repos.",
        }
    }
}

/// Decide the mode from the observable signals alone — a pure function so the
/// decision is testable without touching disk. Priority follows P4's
/// definitions: anything rendered on disk *is* static; otherwise a
/// trust-gated gateway registration means zero-files; a lockfile with nothing
/// rendered means clean-at-rest; a bare, never-activated project reads as the
/// default (static). Ambiguity resolves to the closest, without hand-wringing.
pub(crate) fn mode_from_signals(
    rendered: bool,
    gateway_connected: bool,
    trusted: bool,
    locked: bool,
) -> Mode {
    if rendered {
        Mode::Static
    } else if gateway_connected && trusted {
        Mode::ZeroFiles
    } else if locked {
        Mode::CleanAtRest
    } else {
        Mode::Static
    }
}

/// Has this project rendered any managed artifact? Reuses the apply/use write
/// ledger (`State`): a non-empty managed set for one of the project's target
/// keys means agentstack wrote configs or materialized skills here. Global-scope
/// keys are shared across manifests, so an entry only counts as *this* project's
/// when its recorded source manifest matches (the same guard `foreign_prunes`
/// uses); project-scope keys are already per-root.
pub(crate) fn has_rendered_artifacts(ctx: &super::Context, target_ids: &[String]) -> bool {
    let Ok(state) = crate::state::State::load() else {
        return false;
    };
    let scope = Scope::default_for(&ctx.dir);
    let identity = crate::state::manifest_identity(&ctx.dir);
    target_ids.iter().any(|id| {
        let key = crate::state::target_key(id, scope, &ctx.dir);
        let Some(t) = state.targets.get(&key) else {
            return false;
        };
        let ours =
            scope != Scope::Global || t.source_manifest.as_deref().is_none_or(|s| s == identity);
        ours && (!t.managed_servers.is_empty()
            || !t.managed_skills.is_empty()
            || !t.managed_settings.is_empty()
            || !t.managed_hooks.is_empty())
    }) || has_rendered_instructions(ctx, target_ids, scope, &state, &identity)
}

/// Did a house-rules render actually land on disk for any targeted harness?
///
/// The state ledger tracks servers, skills, settings, and hooks — not
/// instructions, which are written as a managed region inside the harness's
/// own file. So an instructions-only project read as "nothing rendered" even
/// with the region sitting in `CLAUDE.md`: `status` derived clean-at-rest from
/// it and offered `session start <toolset>` with no toolset declared, while
/// `doctor` stood on the Apply rung and offered `agentstack apply --write`,
/// which then reported "already in sync". Two surfaces, two commands, neither
/// of which does anything. Reading the marker directly is the cheap fix: one
/// small file per targeted harness, and only when the ledger found nothing.
/// Attribution at global scope: `~/.claude/CLAUDE.md` is shared by every
/// manifest on the machine, and the managed region carries no source stamp, so
/// a region some *project* wrote via `apply --scope global` would otherwise
/// make the machine-home manifest read "rendered" over a delivery it never
/// made (invariant 8). The state ledger is the only attribution that exists
/// here, so a global key the ledger credits to a different manifest does not
/// count. Residual, documented gap: a foreign write that touched *only*
/// instructions leaves no ledger entry, and nothing on disk distinguishes it —
/// closing that needs an attribution stamp in the rendered region itself,
/// which would change written bytes and belongs to the render layer.
fn has_rendered_instructions(
    ctx: &super::Context,
    target_ids: &[String],
    scope: Scope,
    state: &crate::state::State,
    identity: &str,
) -> bool {
    target_ids.iter().any(|id| {
        let Some(spec) = ctx.registry.get(id).and_then(|d| d.instructions.as_ref()) else {
            return false;
        };
        if scope == Scope::Global {
            let key = crate::state::target_key(id, scope, &ctx.dir);
            if state.manifest_source(&key).is_some_and(|s| s != identity) {
                return false;
            }
        }
        let Some(path) = spec.path_for(scope, &ctx.dir) else {
            return false;
        };
        std::fs::read_to_string(path)
            .is_ok_and(|text| text.contains(crate::render::merge_md::START))
    })
}

/// Is the agentstack gateway registered for **this one harness**?
///
/// The single definition of "the bridge is registered for harness X", used by
/// `status`, `doctor`, `delivery`, and `init`. It takes a `&Registry` rather
/// than a `&Context` so `init` — which has no `Context` — can call the same
/// function instead of repeating the probe.
///
/// A harness that is not detected, or that has no config/MCP descriptor, has
/// no bridge: there is nowhere for one to be registered.
pub(crate) fn bridge_registered(registry: &crate::adapter::Registry, id: &str) -> bool {
    let Some(desc) = registry.get(id) else {
        return false;
    };
    let (Some(cfg), Some(mcp)) = (desc.config.as_ref(), desc.mcp.as_ref()) else {
        return false;
    };
    if !desc.detected() {
        return false;
    }
    let path = crate::util::paths::expand_tilde(&cfg.path);
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    crate::commands::connect::has_bridge_entry(&existing, &mcp.location, cfg.format)
}

/// Could this harness host the bridge at all here — detected, with a config
/// file and an MCP location? Separates "no bridge" from "no bridge possible",
/// so an unconnected-bridge finding names only harnesses a user can connect.
pub(crate) fn bridge_capable_here(registry: &crate::adapter::Registry, id: &str) -> bool {
    registry
        .get(id)
        .is_some_and(|d| d.detected() && d.config.is_some() && d.mcp.is_some())
}

/// Is the agentstack gateway registered in **any** detected harness for this
/// project's targets?
///
/// Deliberately any-of, and deliberately narrow in use: it answers the
/// project-wide question "does a bridge exist here at all?", which is what the
/// mode reading and the trust-relevance test want. It must never stand in for
/// a per-harness delivery claim — that is [`bridge_registered`], and using
/// this value there is exactly the invariant-8 breach that made four
/// unconnected harnesses report "served live" because a fifth was connected.
pub(crate) fn gateway_connected(ctx: &super::Context, target_ids: &[String]) -> bool {
    target_ids
        .iter()
        .any(|id| bridge_registered(&ctx.registry, id))
}

/// Does this manifest declare any capability at all? The one definition both
/// `status` and `doctor` use for the "empty project" rung, so the two surfaces
/// cannot disagree about whether there is anything to work with.
pub(crate) fn declares_capabilities(m: &agentstack_core::manifest::Manifest) -> bool {
    !m.skills.is_empty()
        || !m.declared_server_names().is_empty()
        || !m.instructions.is_empty()
        || !m.settings.is_empty()
        || !m.hooks.is_empty()
        || !m.extensions.is_empty()
}

/// Observe this project's current delivery mode from disk state.
pub(crate) fn detect_mode(ctx: &super::Context, target_ids: &[String]) -> Mode {
    let base = crate::manifest::project_root_of(&ctx.dir);
    let trusted = crate::trust::check(&base) == crate::trust::TrustState::Trusted;
    let locked = crate::lock::Lock::path(&ctx.dir).exists();
    mode_from_signals(
        has_rendered_artifacts(ctx, target_ids),
        gateway_connected(ctx, target_ids),
        trusted,
        locked,
    )
}

/// Would trusting this project change its DELIVERY POSTURE here?
///
/// The prompting hint behind `status --json`'s `trust_relevant` and the
/// gateway-shaped "inert servers" note. `true` when a bridge is registered, or
/// when the derived mode is one the trust gate itself produces.
///
/// A pure function rather than an inline expression at the one call site,
/// because it is a machine-readable contract and
/// [`tests::the_trust_relevance_truth_table`] has to pin its answers rather
/// than restate its formula.
pub(crate) fn trust_relevant(gateway: bool, mode: Mode) -> bool {
    gateway || matches!(mode, Mode::ZeroFiles | Mode::CleanAtRest)
}

/// Is the trust gate currently standing between this project's declared
/// content and every harness?
///
/// The sibling reading of [`trust_relevant`], and the one a consumer asking
/// "do I need to mention trust to this user?" actually wants. Deliberately
/// independent of [`Mode`]: the gate refuses servers, instructions, hooks,
/// extensions, settings and skill materialization in all three modes, so mode
/// tells you nothing about it.
///
/// Both halves are load-bearing. Without `has_capabilities` an empty untrusted
/// project would claim a blockage it has nothing to suffer; without the trust
/// test every project with content would.
pub(crate) fn trust_blocks_delivery(
    trust: crate::trust::TrustState,
    has_capabilities: bool,
) -> bool {
    has_capabilities && trust != crate::trust::TrustState::Trusted
}

/// The single next command bare orientation recommends, from cheap signals.
/// Trust routing is the headline *only when trusting buys something here*
/// (`trust_relevant`, P16 refined): the gateway/bridge is registered for a
/// harness, or the derived mode depends on the trust gate (zero-files /
/// clean-at-rest). In those cases an untrusted or trust-stale manifest points
/// at `trust .` first, because until the digest is pinned the bridge serves
/// control-plane tools only and no server runs — trusting is the gate. A
/// static, no-gateway project has no bridge to unlock and no gate-dependent
/// mode to leave, so its untrusted state does not take the headline for a
/// reason about the BRIDGE it has not got.
///
/// What this must NOT be read as, and once was: that such a project therefore
/// falls through to the setup ladder untouched, or renders "whatever the trust
/// state". It does not. The trust gate reaches all of servers, instructions,
/// hooks, extensions, settings and skill materialization, so an untrusted
/// static project has every `apply --write` refused — measured, and pinned by
/// [`the_trust_relevance_truth_table`]. `trust_relevant` is a prompting hint
/// about delivery posture; [`trust_blocks_delivery`] is the reading that says
/// whether the gate is standing in the way, and it is a rung of this ladder in
/// its own right: below `adopt`, which still makes progress under the gate,
/// and above every setup rung, which the gate refuses.
///
/// That ladder: capabilities declared but nothing on
/// disk yet → `apply --write`; otherwise the wiring is in place → `doctor`.
/// Pure over its inputs so the routing is unit-tested without touching disk.
///
/// **This function never recommends `init`.** It is only reached once a
/// manifest has loaded, and `init` refuses when one exists — so recommending
/// it here sent a user who had just finished a successful first run into an
/// error. The state that used to trigger it (`!locked`, i.e. never activated)
/// is not something `init` fixes: a project is unlocked until `use`/`lock`
/// runs, which is a normal resting state for a static setup, not an unfinished
/// import. The honest question is "is this rendered?", which `rendered`
/// answers.
/// The one next step for a project with nothing to work with yet — no server
/// declared, so nothing to render, group, or serve.
///
/// It lives here, as a `const` tuple of two `&'static str`s, because `status`
/// and `doctor` both have to answer this state and answering it differently is
/// the dead end this pair removes: `status` used to say "agentstack doctor",
/// doctor said "agentstack toolset create … --server <server>" over zero
/// servers, and neither command moved the user forward. `search` runs in every
/// state, including an empty project.
/// The SENTENCE is stated as prose, not as `agentstack search <query>`: the
/// shape was written as a command, `machine_command` filtered the angle
/// brackets, and the screen then read as if a runnable command were on offer
/// when none was. The query is the one thing only the person with the problem
/// knows, and the sentence now says so.
///
/// The machine field beside it is `null`, and this rung was the last one
/// claiming otherwise. It used to map onto a bare `agentstack search`, on the
/// reasoning that nulling would break `status-v1`. That reasoning was wrong on
/// both counts, and both were measured rather than argued:
///
/// - `null` is already in-contract here. The Group and Verified rungs — the
///   largest healthy states on either surface — answer `null` from this same
///   field, so a consumer that cannot handle it is already broken on projects
///   far more common than an empty one.
/// - `agentstack search` was never a next ACTION. It is a read-only browse: it
///   exits 0 and leaves every observable field of the report identical, so a
///   driver polling this field runs it and is handed it again, forever.
///   `tests/guidance_is_executable.rs` reproduces that loop.
///
/// The browse is genuinely useful, so it stays where it helps — the `why`
/// below, read by a person who can act on a listing. See [`machine_command`].
///
/// # The `why` teaches shapes, and names no bare `add`
///
/// It used to end "then `agentstack add`", and that was this same bug wearing
/// the fix's clothes: the sentence written to stop a rung naming a command that
/// refuses, itself named one. Measured in the empty `version = 1` project this
/// rung answers, every `add` form exits 2 — bare `add` wants a subcommand,
/// `add server` wants `<NAME>`, `add skill` wants `<SOURCE>`, `add from` wants
/// `<ID>`. There is no bare `add` that makes progress, because the argument is
/// the one thing only the person with the problem can supply, which is what the
/// human half of this pair already says.
///
/// So the add step is taught as a SHAPE. A placeholder is right in prose and
/// wrong in the machine field, and that split is already enforced from both
/// ends: [`machine_command`] drops any `<…>` (untouched here), and
/// `tests/guidance_is_executable.rs` executes bare commands while skipping
/// shapes — which is exactly why `agentstack add` was caught and
/// `agentstack search <query>` was not.
///
/// Bare `agentstack search` stays, and is now worth naming on its own: it lists
/// what the local library and built-in catalog hold, with no network call and
/// no query, and exits 0 in this state (measured).
pub(crate) const EMPTY_PROJECT_NEXT: (&str, &str) = (
    "find a server or skill to add — only you know what this project needs",
    "`agentstack search` or `agentstack search <query>` lists what you can add — then name it: `agentstack add server <name>`, `agentstack add skill <source>`, or `agentstack add from <id>`",
);

/// Why the abandoned-render rung is the one next action when it fires.
///
/// One string, so `status` and `doctor` cannot describe the same rung
/// differently.
pub(crate) const ABANDONED_RENDER_WHY: &str =
    "a config file AgentStack no longer maintains is still being read by that tool";

/// The machine `next_action` for a human next step — the one command a PROGRAM
/// is contracted to execute verbatim, or `None`.
///
/// `next_step`'s answers are written for a human reading a terminal, and two
/// honest human answers are not runnable:
///
/// - A *shape*, not a command: `agentstack toolset create <name> --server
///   <server>` and `agentstack search <query>` tell a person what to type. A
///   driver that runs them verbatim gets `no server '<server>' in the manifest
///   or central library` — forever, since nothing it can do makes the angle
///   brackets resolve.
/// - A *prose remedy* (`open Codex once`, "add `description:` so search and
///   agents can find it") or a pointer at the report itself (`review the
///   errors above`). Correct advice, not a process.
///
/// Read-only summaries are excluded too. `agentstack status` and `agentstack
/// doctor` name each other and terminate nothing, and `doctor`'s own terminal
/// never answers with a command here — so leaving `status`'s `doctor` in would
/// re-open exactly the disagreement the shared [`ladder_rung`] closed.
///
/// `None` is a complete answer: "there is no command to run" is what a ready
/// project's machine field should say. Both surfaces keep printing the full
/// human sentence on screen; only the machine field narrows.
///
/// The empty project answers `None` too, and used to be special-cased into
/// `agentstack search`. That mapping was wrong for a reason no per-string rule
/// could see: `search` is a read-only browse. It exits 0, lists what the local
/// sources hold, and leaves every observable field of the report identical — so
/// a driver that polls `next_action`, runs it, and polls again is handed the
/// same command forever. `tests/guidance_is_executable.rs` reproduces exactly
/// that loop, and it is right to.
///
/// Nulling it is not a `status-v1` break. The key stays present and its type is
/// unchanged; `null` is already what this field answers from the Group and
/// Verified rungs, which are the largest healthy states on either surface, so
/// every consumer already handles it. `converge_once` in that same test names
/// `null` "the terminal answer, and a driver that reads it stops". A browse
/// worth running is still worth PRINTING — it stays in the human `why`, where a
/// person can act on a listing and a poller cannot mistake it for progress.
pub(crate) fn machine_command(cmd: &str) -> Option<&str> {
    match cmd {
        "agentstack status" | "agentstack doctor" => None,
        c if c.starts_with("agentstack ") && !c.contains('<') => Some(c),
        _ => None,
    }
}

/// The rung that has to come BEFORE the review on a project that has never
/// been pinned, and the reason it does.
pub(crate) const LOCK_RUNG_FIX: &str = "agentstack lock --write";
pub(crate) const LOCK_RUNG_WHY: &str =
    "pin this content first — the grant binds to the lockfile, so one given now is void the moment `use --write` writes it";

/// Would `agentstack lock --write` actually leave a lockfile behind here?
///
/// Deliberately NOT [`declares_capabilities`], which is the "is there anything
/// to deliver?" reading and includes hooks. `lock` pins skills, servers,
/// instructions, settings, extensions, workflows and toolset packages — it says
/// so in its own empty-project sentence — and hooks are in none of those. A
/// hooks-only manifest gets `lock --write`, exit 0, "pinned nothing new", and no
/// `agentstack.lock` on disk (measured). Keying [`correct_trust_rung`] off the
/// broader predicate would therefore route that project to `lock --write`
/// forever, which is the same poll-and-run dead end the rung exists to remove —
/// one rung earlier.
pub(crate) fn lock_pins_something(m: &agentstack_core::manifest::Manifest) -> bool {
    !m.skills.is_empty()
        || !m.declared_server_names().is_empty()
        || !m.instructions.is_empty()
        || !m.settings.is_empty()
        || !m.extensions.is_empty()
        || !m.workflows.is_empty()
        || m.profiles.values().any(|p| !p.packages.is_empty())
}

/// Rewrite a rung that names the review over a project nothing has pinned yet.
/// Applied by BOTH surfaces to EVERY rung that names `trust`, so the two cannot
/// disagree about the ceremony's order.
///
/// `trust` pins the content digest of the manifest layers **and the lockfile**
/// — its own `--help` says so. So a grant made while no lockfile exists is
/// bound to a surface the very next command changes: `use --write` mints the
/// lock, the digest moves, and the project lands back in
/// [`crate::trust::TrustState::Changed`] with `status` asking for the same
/// review it asked for a minute ago. Measured: `add server --write` → `trust .`
/// → `use --write` leaves `locked · trust stale (content changed)`, while
/// `add server --write` → `lock --write` → `trust .` → `use --write` leaves
/// `locked · trusted`. The grant survives only when the pins came first.
///
/// This is the order the product already states in its own words —
/// `agentstack yes` prints "`agentstack adopt --write` → `agentstack lock
/// --write` → `agentstack trust .`", and `lock --write`'s footer ends with
/// "Next: `agentstack trust .`". Only the ladder disagreed.
///
/// It is a rewrite rather than a new arm inside [`next_step`] for the same
/// reason [`correct_apply_rung`] is: `trust .` is named from four different
/// places on `status` alone (untrusted, drifted, gate-blocks-delivery, and the
/// refused-calls override) and from two more in `doctor`. Correcting the
/// *answer* catches all six; correcting one arm catches one.
pub(crate) fn correct_trust_rung(
    step: (String, &'static str),
    locked: bool,
    lock_pins: bool,
) -> (String, &'static str) {
    if locked || !lock_pins || !step.0.starts_with("agentstack trust") {
        return step;
    }
    (LOCK_RUNG_FIX.to_string(), LOCK_RUNG_WHY)
}

/// The `why` for a never-pinned surface that NO command repairs.
pub(crate) const UNPINNED_NO_FIX_WHY: &str =
    "no command can pin a body that is not on disk — restore it, or drop the declaration";

/// The never-pinned rung, shared by `status` and `doctor`, computed from the
/// findings THEMSELVES rather than from "is the list non-empty".
///
/// The distinction is the whole point. [`crate::commands::trust::ContentDrift`]
/// carries `fix: Option<&str>` so a blocker with no repairing command can say
/// so, and `trust --preview` honours it by emitting `fix: null`. Collapsing the
/// list to a bool here threw that away: both surfaces answered `agentstack lock
/// --write` over a declared body that is absent from disk, where that command
/// either exits non-zero and changes nothing, or — once every OTHER declared
/// item is pinned — prints a green tick and exits 0 with the blocking condition
/// untouched. Either way a driver re-reads the same field and runs the same
/// command forever, and the exit-0 shape is the worse of the two because
/// nothing in the output says it failed.
///
/// So: name the command only when a reported blocker actually carries one.
/// Otherwise the rung is terminal — the human sentence carries the finding's
/// own prose (which names the missing path and says the body must be restored)
/// and `machine_command` filters it to `null`, exactly as `trust --preview`
/// already answers for the same state.
pub(crate) fn unpinned_next_action(
    unpinned: &[crate::commands::trust::ContentDrift],
) -> Option<(String, &'static str)> {
    if unpinned.is_empty() {
        return None;
    }
    if unpinned.iter().any(|d| d.fix.is_some()) {
        return Some((
            crate::commands::trust::UNPINNED_FIX.to_string(),
            crate::commands::trust::UNPINNED_WHY,
        ));
    }
    // Terminal rung. One finding is named in full; the rest are counted, so a
    // one-line terminal sentence stays one line without hiding that there are
    // more. `reason` already reads as a sentence and already carries the path.
    let mut sentence = crate::text::sanitize_line(&unpinned[0].reason);
    if unpinned.len() > 1 {
        sentence.push_str(&format!(" (and {} more like it)", unpinned.len() - 1));
    }
    Some((sentence, UNPINNED_NO_FIX_WHY))
}

/// Which rung of the *setup* ladder a project stands on, once consent and
/// import are settled.
///
/// One function, two callers: `status`'s [`next_step`] tail and `doctor`'s
/// terminal. Before this, the two surfaces each decided the same question from
/// different inputs — `status` from "are any capabilities declared?", `doctor`
/// from "is a server declared?" — so a healthy skills-only project heard
/// "add a server or skill" from one and something else from the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Rung {
    /// Nothing declared: there is nothing to render, group, or serve.
    Empty,
    /// Declared, but nothing on disk for it yet.
    Apply,
    /// On disk and ungrouped — and there is a server to group.
    Group,
    /// The setup is done as far as these signals can tell.
    Verified,
}

/// The Apply rung's two possible commands, and the "why" that goes with the
/// second. Consts rather than bare literals so the correction below cannot
/// drift away from the arm it corrects.
pub(crate) const APPLY_RUNG_RENDER: &str = "agentstack apply --write";
pub(crate) const APPLY_RUNG_ACTIVATE: &str = "agentstack use --write";
pub(crate) const APPLY_RUNG_ACTIVATE_WHY: &str =
    "activate the skills this setup declares — `apply` does not render them";

/// Does `apply` render anything this manifest declares?
///
/// `apply` writes servers, instructions, settings, hooks and extensions —
/// never skills, which activate through `use`. A skills-only manifest that
/// stands on the Apply rung therefore hears `agentstack apply --write`, which
/// reports "already in sync" and leaves the rung exactly where it was, so the
/// ladder asks for it again: a poll-and-run loop with no exit for any driver
/// reading `next_action`. The one predicate `status` and `doctor` both use.
pub(crate) fn apply_renders_something(m: &agentstack_core::manifest::Manifest) -> bool {
    !m.declared_server_names().is_empty()
        || !m.instructions.is_empty()
        || !m.settings.is_empty()
        || !m.hooks.is_empty()
        || !m.extensions.is_empty()
}

/// Rewrite an Apply-rung step that names a render which cannot happen here.
/// Applied by BOTH surfaces to the SAME rung, so they cannot disagree about
/// it. Bare `use --write` is valid with or without declared toolsets — it
/// activates the single declared toolset, or everything inline when none is
/// declared — so this never names a command the state refuses.
pub(crate) fn correct_apply_rung(
    step: (&'static str, &'static str),
    apply_renders: bool,
) -> (&'static str, &'static str) {
    if step.0 == APPLY_RUNG_RENDER && !apply_renders {
        (APPLY_RUNG_ACTIVATE, APPLY_RUNG_ACTIVATE_WHY)
    } else {
        step
    }
}

pub(crate) fn ladder_rung(
    has_capabilities: bool,
    rendered: bool,
    no_toolsets: bool,
    declares_a_server: bool,
) -> Rung {
    if !has_capabilities {
        Rung::Empty
    } else if !rendered {
        Rung::Apply
    } else if no_toolsets && declares_a_server {
        // `toolset create … --server <server>` refuses without a server, and a
        // next step must never name a command that cannot run here.
        Rung::Group
    } else {
        Rung::Verified
    }
}

/// Does this project stand on the ladder's *delivered* rung — i.e. must the
/// Apply rung be considered satisfied?
///
/// The one definition `status` and `doctor` share. Zero-files delivery renders
/// nothing ON PURPOSE — the gateway serves the project live — so reading it as
/// "not rendered" puts the Apply rung on top and recommends
/// `agentstack apply --write`, the exact render this mode opts out of. Applying
/// then reports "already in sync" and `status` repeats itself forever, while
/// `doctor` (which already had this exemption) says the project is ready. Two
/// surfaces, one of them looping.
///
/// Takes plain `bool`s rather than a `Mode`, because `doctor` carries its mode
/// as a rendered `&str` label in its JSON report and has no `Mode` in hand.
pub(crate) fn stands_on_delivered_rung(zero_files: bool, rendered: bool) -> bool {
    zero_files || rendered
}

// Eight plain `bool`/enum signals, deliberately: the function is pure over
// exactly the observations the ladder branches on, which is what lets the
// whole routing be unit-tested without touching disk. Bundling them into a
// struct would only move the same eight fields one level out.
#[allow(clippy::too_many_arguments)]
pub(crate) fn next_step(
    trust: crate::trust::TrustState,
    rendered: bool,
    has_capabilities: bool,
    trust_relevant: bool,
    no_toolsets: bool,
    unimported_native: bool,
    undeclared_drops: bool,
    declares_a_server: bool,
) -> (&'static str, &'static str) {
    use crate::trust::TrustState;
    // A dropped-but-undeclared file outranks everything except a pending
    // re-review: the drop is the newest thing the user did, and `agentstack
    // yes` is the one verb built for it. Until this branch existed, `yes` was
    // orphaned — every detection surface routed drops to `adopt` or `trust .`
    // and the funnel was unreachable without reading the docs. Routing here
    // skips no review: `yes` holds clone-supplied content and collisions on
    // its preview and names the explicit path for them. It does NOT outrank
    // `TrustState::Changed` — content the user already approved has drifted,
    // and that re-review keeps the headline; the drop is offered next.
    if undeclared_drops && trust != TrustState::Changed {
        return (
            "agentstack yes",
            "dropped files are waiting — review them and take them live",
        );
    }
    match trust {
        TrustState::Untrusted if trust_relevant => {
            ("agentstack trust .", "review it to unlock its servers")
        }
        // Trust-stale routes here whether or not trust is "relevant". The
        // relevance test asks whether trusting *unlocks* something, which is the
        // right question for a project that has never been trusted. It is the
        // wrong question once content the user already approved has CHANGED:
        // that state is a re-review the Status line is already reporting, and
        // routing it to `doctor` made the cue cost two commands — status naming
        // doctor, doctor naming the review (pilot Run A).
        TrustState::Changed => (
            "agentstack trust .",
            "the content changed since you reviewed it — review and re-trust",
        ),
        // Untrusted but trust does not change this project's SHAPE (static, no
        // gateway), or already trusted: fall through to the normal ladder.
        _ => {
            // Servers configured natively here that this manifest doesn't know
            // about. Ahead of `apply`, because rendering a manifest that omits
            // half the setup is not the step that helps.
            //
            // Deliberately still ahead of the gate rung below, even though the
            // gate refuses this project's writes: `adopt` edits the manifest,
            // which is exactly the surface a grant binds itself to. Reviewing
            // first would approve a manifest the very next step rewrites, and
            // the user would land in `TrustState::Changed` and review again.
            if unimported_native {
                (
                    "agentstack adopt",
                    "servers are configured here that this setup doesn't cover yet",
                )
            } else if trust_blocks_delivery(trust, has_capabilities) {
                // The gate rung, and it sits here because it gates everything
                // below: `apply --write` is refused outright, `use --write` is
                // refused, and grouping or verifying content no harness will
                // ever receive is not the step that helps. Until this rung
                // existed, a static, no-gateway, already-rendered project fell
                // through to the Group rung and answered `agentstack toolset
                // create <name> --server <server>` — a shape, which
                // `machine_command` then filtered to `null`. So `status --json`
                // named NO next action in the one state whose answer is a
                // single concrete command, while `doctor`, whose ladder has had
                // this rung all along, answered `agentstack trust .` for the
                // same project. Two surfaces, one state, and the machine field
                // empty on the surface a panel reads.
                //
                // The reading is `trust_blocks_delivery`, not a fourth way of
                // asking: it is true exactly when content is declared AND the
                // gate is up, which is the condition under which every rung
                // below is refused. `trust_relevant` is NOT that question — it
                // asks whether trusting changes the project's delivery posture,
                // and it is false here by design (the arm above owns it).
                //
                // The command is concrete on purpose. `machine_command` drops
                // placeholders because a driver cannot resolve `<name>`; a rung
                // added to fix a `null` must not reintroduce one.
                (
                    "agentstack trust .",
                    "nothing this project declares reaches a harness until you review it",
                )
            } else {
                // The shared setup ladder — the SAME rungs doctor's terminal
                // uses, so the two surfaces cannot answer one state
                // differently.
                match ladder_rung(has_capabilities, rendered, no_toolsets, declares_a_server) {
                    // Nothing declared and nothing to import: every rung below
                    // (`apply`, `toolset create`, `doctor`) either has nothing
                    // to act on or sends the user to a surface that sends them
                    // back — the status↔doctor loop.
                    Rung::Empty => EMPTY_PROJECT_NEXT,
                    Rung::Apply => (APPLY_RUNG_RENDER, "render this setup into your CLIs"),
                    // The wiring is done. `doctor` here was a dead end: a user
                    // who ran it clean was offered it again, with nothing on
                    // screen saying the journey continues (pilot Run A). The
                    // next rung of the ladder is Switch, and it is stated as
                    // one.
                    // Prose, and a `null` machine field BY DESIGN — the same
                    // answer the Verified rung below already gives, for the
                    // same reason. `toolset create` takes a name, and a
                    // toolset's name is the one argument nothing on disk can
                    // supply: it is what the user is going to call this group
                    // of servers. (`--server` could be filled in when exactly
                    // one is declared; `<name>` never can, so the command stays
                    // unrunnable either way.) Written as a command it was a
                    // shape, `machine_command` dropped it, and the largest
                    // healthy state on either surface answered a panel with
                    // nothing. Written as a sentence it says why there is no
                    // command, and the shape survives in the `why` for the
                    // human who can substitute.
                    Rung::Group => (
                        "name a toolset to group these servers — the name is yours to choose",
                        "`agentstack toolset create <name> --server <server>`, then switch between toolsets",
                    ),
                    Rung::Verified => (
                        "agentstack doctor",
                        "verify the wiring — every warning names its fix",
                    ),
                }
            }
        }
    }
}

/// Delivery-mode override for the normal trust/init/doctor ladder. A trusted,
/// locked clean-at-rest project is ready to use; teach the session rhythm at
/// the moment it matters instead of sending it back through another doctor
/// pass. Active sessions point at their matching close operation.
///
/// Takes the declared toolset NAMES, not a pre-picked one, because the answer
/// genuinely differs three ways and the caller cannot collapse them without
/// lying. `session start` takes `<TOOLSET>` as a **required** positional (it
/// exits 2 without one, measured), so:
///
/// - **exactly one declared** — there is nothing to choose. Name it, and the
///   machine field carries a command a driver can run verbatim. This is the
///   only one of the three where a concrete command exists, and it already
///   worked; it is kept working here.
/// - **two or more** — which one is the user's call, and picking for them would
///   start the wrong toolset. The names are listed so the choice is one glance,
///   and the machine field is `null` BY DESIGN.
/// - **none declared** — `session start` cannot run at all: there is no toolset
///   for it to load. Naming it was worse than a placeholder; it was a command
///   this state refuses. The real step is to declare one, which needs a name
///   only the human can choose, so again `null` by design.
///
/// The old signature took a single `profile: &str` that the caller had already
/// flattened to the literal `"<toolset>"` for both the 0 and the 2+ case, which
/// is exactly how one honest answer and one impossible one came to be printed
/// with the same words.
pub(crate) fn clean_at_rest_next_step(
    mode: Mode,
    trust: crate::trust::TrustState,
    locked: bool,
    session_active: bool,
    toolsets: &[String],
) -> Option<(String, &'static str)> {
    if mode != Mode::CleanAtRest || trust != crate::trust::TrustState::Trusted || !locked {
        return None;
    }
    if session_active {
        return Some((
            "agentstack x session end".to_string(),
            "finish this session and restore the clean-at-rest state",
        ));
    }
    match toolsets {
        [one] => Some((
            format!("agentstack x session start {}", crate::text::sanitize_line(one)),
            "materialize the toolset for this session",
        )),
        [] => Some((
            "declare a toolset before a session can load one — the name is yours to choose"
                .to_string(),
            "`agentstack toolset create <name> --server <server>`, then `agentstack x session start <name>`",
        )),
        many => Some((
            format!(
                "pick the toolset for this session: {}",
                many.iter()
                    .map(|n| crate::text::sanitize_line(n))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            "`agentstack x session start <toolset>` materializes it; `agentstack x session end` puts every file back",
        )),
    }
}

/// The one-line explanation of an untrusted (or trust-stale) manifest shown
/// under the Status line (P16). `None` for a trusted manifest — there is
/// nothing to teach. A `&'static str` because the sentence never varies. The
/// caller shows it only when trust is *relevant* here (a bridge exists),
/// because the sentence is about the BRIDGE: it names the gateway serving
/// control-plane tools only, and a static, no-gateway project has no gateway
/// for that clause to describe. The withholding is about this sentence's
/// subject, not about the gate — an untrusted static project's servers are
/// every bit as blocked, they are simply blocked at `apply`/`use` rather than
/// at a bridge. So that project keeps the honest `· untrusted` Status label
/// without this line.
pub(crate) fn orientation_trust_note(trust: crate::trust::TrustState) -> Option<&'static str> {
    use crate::trust::TrustState;
    match trust {
        TrustState::Untrusted | TrustState::Changed => {
            Some("its servers are inert — the gateway serves control-plane tools only until you review it")
        }
        TrustState::Trusted => None,
    }
}

/// The named profile roster for orientation (P18): every name for a small set,
/// a truncated `N profiles: a, b, c, …` beyond four, with the active profile
/// (when a live session pins one) marked inline. Declaration order is kept —
/// the truncation shows the first three, so an active profile past that window
/// is not marked, which is honest: orientation stays a glance, not a report.
/// Pure over its inputs so the formatting is unit-tested without a manifest.
pub(crate) fn profiles_line(names: &[String], active: Option<&str>) -> String {
    let render = |n: &String| -> String {
        if Some(n.as_str()) == active {
            format!("{n} (active)")
        } else {
            n.clone()
        }
    };
    if names.len() <= 4 {
        names.iter().map(render).collect::<Vec<_>>().join(", ")
    } else {
        let shown = names
            .iter()
            .take(3)
            .map(render)
            .collect::<Vec<_>>()
            .join(", ");
        format!("{} toolsets: {shown}, …", names.len())
    }
}

/// Humanize how long a session has been running, for the Session status line.
/// Pure so the buckets are unit-testable.
pub(crate) fn session_age(secs: u64) -> String {
    if secs < 60 {
        "started just now".to_string()
    } else if secs < 3600 {
        format!("started {}m ago", secs / 60)
    } else {
        format!("started {}h {}m ago", secs / 3600, (secs % 3600) / 60)
    }
}

/// The Session status line's two parts — (headline, recovery hint) — for the
/// default surface. A live session states it is active temporarily; one that
/// reads as abandoned (Stage 2.2) is flagged as such and leads with the same
/// safe `session end` recovery. Pure so both wordings are unit-tested; the
/// abandoned judgment itself lives in `crate::session::is_abandoned`.
pub(crate) fn session_status_line(
    profile: &str,
    age_secs: u64,
    abandoned: bool,
) -> (String, String) {
    let end = "`agentstack x session end` restores your files".to_string();
    if abandoned {
        (
            format!("'{profile}' looks abandoned ({})", session_age(age_secs)),
            format!("probably a closed terminal — {end}"),
        )
    } else {
        (
            format!("'{profile}' active temporarily ({})", session_age(age_secs)),
            end,
        )
    }
}

/// Everything the orientation screen observes, gathered in one pass so the
/// human screen and `status --json` are provably the same reading. Before
/// this, the screen computed each fact inline as it printed it; a JSON body
/// built alongside would have been a second implementation of the same
/// questions, free to answer them differently.
pub(crate) struct Orientation {
    /// Display names of the CLIs detected FOR THIS DIRECTORY — installed,
    /// configured on this machine, or carrying a project-scope config here.
    detected_clis: Vec<String>,
    /// How many adapters the catalog knows about at all. Kept apart from
    /// `detected_clis` because they answer different questions: the screen used
    /// to print "none detected on this machine" above "13 detected CLI(s)",
    /// which is one screen contradicting itself. The second number was never
    /// detection — it is the fan-out fallback, i.e. the catalog.
    catalog_size: usize,
    /// Native configs found here that the manifest does not (yet) cover.
    native: Vec<crate::discover::NativeConfig>,
    /// Files dropped into this project's own `skills/`/`instructions/` that the
    /// manifest does not declare. Inert until adopted — see `crate::intake`.
    intake: Vec<crate::intake::Item>,
    /// Where the manifest is (or would be).
    manifest_path: std::path::PathBuf,
    manifest: ManifestState,
    /// The single next step: (command, why).
    next: (String, &'static str),
}

/// Which of the three shapes the manifest reading took. A caller branches on
/// this rather than probing for fields: only `Loaded` has project facts, and
/// only `Broken` has a reason.
pub(crate) enum ManifestState {
    /// No manifest in this directory.
    Missing,
    /// A manifest file exists but does not load. Carries the reason, already
    /// formatted for display.
    Broken(String),
    /// It loaded, so every project fact below is real.
    Loaded(Box<ProjectFacts>),
}

pub(crate) struct ProjectFacts {
    servers: usize,
    skills: usize,
    /// The other declared kinds. Phase 3 item 5: `status` counted servers and
    /// skills only, so a project whose setup was mostly instruction fragments,
    /// hooks, or CLI settings reported a footprint of "0 servers" — a true
    /// number and a false impression, on the surface whose whole job is
    /// saying what you have. Counted, not enumerated: `explain` is where a
    /// kind is looked at one at a time.
    instructions: usize,
    settings: usize,
    hooks: usize,
    extensions: usize,
    /// `[targets].default`, empty when nothing is pinned.
    pinned_targets: Vec<String>,
    /// How many detected CLIs the commands would fan out to when nothing is
    /// pinned — the honest number behind "no [targets] pinned".
    fanout_targets: usize,
    /// Whether `fanout_targets` counts CLIs actually detected here, or is the
    /// whole-catalog fallback `resolve_targets` uses when nothing is detected.
    /// Without it the line cannot be phrased truthfully.
    fanout_detected: bool,
    toolsets: Vec<String>,
    session: Option<SessionFacts>,
    locked: bool,
    trust: crate::trust::TrustState,
    /// `trust-content-drift-v1`: pinned bodies whose bytes on disk no longer
    /// match the lock. Held separately from `trust` above because it is a
    /// different reading: `trust` compares the manifest/lock BYTES to the
    /// consent digest, and those can be untouched while the content they pin
    /// has moved — which is exactly how `status` came to report `trusted` over
    /// content `doctor` errors on and `trust` refuses. Deep reads only; the
    /// bare screen leaves it empty rather than paying for the resolve pass.
    content_drift: Vec<crate::commands::trust::ContentDrift>,
    /// Declared items with no lockfile pin AT ALL — the sibling reading of
    /// `content_drift`, from the same shared detector the grant refuses on
    /// ([`crate::commands::trust::unpinned_surface`]). Held separately because
    /// it is a different sentence: nothing here was ever approved, so it is
    /// not "drift". Deep reads only, like `content_drift`.
    surface_unpinned: Vec<crate::commands::trust::ContentDrift>,
    /// Whether trusting this project would change its DELIVERY POSTURE here —
    /// a bridge is registered, or the derived mode depends on the gate. A
    /// prompting hint: it decides how loudly to ask, and drives both the
    /// gateway-shaped "inert servers" note and whether trust is the headline
    /// next step.
    ///
    /// It is deliberately NOT the answer to "can this project write?", and
    /// `false` here has never meant "trust buys nothing". The trust gate
    /// refuses servers, instructions, hooks, extensions, settings and skill
    /// materialization in EVERY mode, so a static, no-gateway project reads
    /// `trust_relevant: false` while `apply --write` refuses every one of its
    /// declared kinds. [`ProjectFacts::trust_blocks_delivery`] is the field
    /// that answers that question; this one keeps its shipped meaning byte for
    /// byte, because changing it would be a meaning change under a name
    /// external panels already gate on (`ui_contract::SCHEMA_VERSION`).
    trust_relevant: bool,
    /// Whether the trust gate currently stands between this project's declared
    /// content and every harness: the project declares at least one capability
    /// AND its trust state is not `Trusted`.
    ///
    /// The reading `trust_relevant` was repeatedly mistaken for, held as its
    /// own field because a consumer cannot derive it. `trust` alone
    /// over-predicts — an untrusted project that declares nothing has nothing
    /// to block — and the emitted counts cannot close the gap either: the JSON
    /// carries `servers` and `skills` but not `instructions`, `settings`,
    /// `hooks` or `extensions`, so an instructions-only project reads
    /// `servers: 0, skills: 0, trust_relevant: false` while `apply --write`
    /// refuses its one fragment.
    ///
    /// What it promises: the GATE, not a prediction about a particular
    /// command's exit code. `true` says every path that puts this content in
    /// front of an agent — the rendered lane's `apply`/`use`, the live lane's
    /// gateway serve, `session start` — refuses until the content is reviewed.
    /// An `apply --write` that exits 0 while this is `true` means there was
    /// nothing left to write, not that the gate opened.
    trust_blocks_delivery: bool,
    mode: Mode,
    gateway_connected: bool,
    /// Harnesses that registered the gateway but cannot launch it (W4
    /// precondition 6). Registered-and-broken is a different fact from
    /// not-registered, and the difference is the whole outage: the harness gets
    /// no tools, and AgentStack writes nothing in the gateway's place.
    gateway_outages: Vec<crate::commands::connect::GatewayOutage>,
    /// One line per library name that more than one linked source holds:
    /// which source wins, which are shadowed, and the qualified reference that
    /// pins the other copy. Empty is the common case, and the only case that
    /// prints nothing (docs/design/linked-library-sources.md).
    shadowed_names: Vec<String>,
    /// The delivery planner's routing, one plain-language line per CLI (W4).
    /// The planner runs silently; this is where `status` names what it did.
    delivery: Vec<String>,
    /// The per-harness house-rules honesty matrix (item 4): which channel
    /// actually carries instructions to each CLI, whether that CLI's live
    /// channel is confirmed or merely declared, and which variant it receives.
    /// Present for every targeted harness — including the ones with no
    /// instruction channel at all, because an adapter that quietly disappears
    /// from a coverage list reads as covered
    /// (`docs/design/instruction-variants.md`).
    instruction_channels: Vec<crate::instructions::HarnessChannel>,
    /// The `rendered lane:` line, present only when something is actually
    /// written — an empty lane line is its own small lie.
    delivery_rendered_lane: Option<String>,
    /// Whether anything DECLARED is routed to the live lane, which is the only
    /// condition under which the zero-artifacts sentence is true here.
    delivery_has_live: bool,
    /// Display names of live-lane harnesses with no bridge registered. The
    /// per-harness reading: the "register the bridge" hint appears while any
    /// one of them is unconnected, not only when all are.
    delivery_bridge_gaps: Vec<String>,
    /// Server configs an earlier rendered-lane `apply` wrote for a harness
    /// that now routes live. They are still on disk and the harness still
    /// reads them, so `status` names them rather than reporting a project
    /// that delivers nothing to files (invariant 8). Read from the state
    /// ledger plus disk — never from the routing plan, which only says what
    /// `apply` WOULD write.
    delivery_abandoned: Vec<crate::commands::apply::AbandonedRender>,
    rendered: bool,
    /// `None` when the caller did not ask for the secrets reading — bare
    /// `agentstack` never does, and asking is not free (it consults every
    /// resolver).
    secrets: Option<SecretFacts>,
    /// Refusals recorded against this project since its last yes (W1). `None`
    /// for a trusted project and for one nothing has tried to use.
    needs_your_yes: Option<NeedsYourYes>,
    /// Installed packs with a newer version resolvable **offline** (see
    /// [`crate::commands::updates::available_updates`]). Empty means "nothing
    /// to offer here", which is NOT the same as "up to date": the check never
    /// touches the network, so a pack whose local clone is stale, absent, or
    /// catalog-sourced is silently missing from this list. Every rendering of
    /// it offers; none of them may claim currency.
    updates: Vec<crate::commands::updates::PackUpdate>,
    /// The **effective member set** of every package this project pinned (W5,
    /// `docs/design/package-layer.md`). Read from the LOCK through
    /// [`crate::package::effective_members`] — never from the library, whose
    /// whole purpose is to be free to move ahead. Empty for a project that
    /// selects no package, and for one that has never been locked.
    packages: Vec<agentstack_core::lock::LockedPackage>,
    /// `context-cost-v1`. Deep reads only — it opens SKILL.md frontmatter and
    /// stats house-rule bodies, which bare `agentstack` does not pay for.
    /// Default (all zeroes) means "not asked", and renders as nothing, which is
    /// the same thing an unmeasurable project renders as.
    context: ContextCost,
}

/// `context-cost-v1` — what this project costs a harness in context-window
/// tokens, per session.
///
/// Every number here is an **estimate**, and every renderer says so. Two
/// deliberate constraints:
///
/// * There is exactly ONE token estimator in this binary
///   ([`crate::footprint::estimate_tokens`], the ~4-chars-per-token
///   heuristic), and this reading calls it rather than growing a second one.
///   Server costs are not re-derived at all: they are read from the measured
///   cache `footprint.json` that `agentstack x report usage --live` writes, so
///   `status` and `report usage` cannot disagree about a server's cost.
/// * **No data is not zero.** A server that has never been measured is counted
///   in `servers_unmeasured`, never as `0` tokens, and a project with nothing
///   measurable prints no line at all.
#[derive(Default)]
pub(crate) struct ContextCost {
    /// Measured servers this project declares, name → estimated tokens.
    servers: Vec<(String, u64)>,
    /// Declared servers with no measurement in the cache. Their cost is
    /// unknown, so it is reported as unknown — not folded into the total.
    servers_unmeasured: usize,
    /// (how many skill descriptions were readable, their estimated tokens).
    /// A harness injects every available skill's frontmatter `description`
    /// into context; the bodies load on demand and are not counted.
    skills: (usize, u64),
    /// (how many house-rule bodies were readable, their estimated tokens).
    house_rules: (usize, u64),
}

impl ContextCost {
    /// Only what was actually measured or read. Never a floor, never a claim
    /// about the unmeasured servers.
    pub(crate) fn total(&self) -> u64 {
        self.servers.iter().map(|(_, t)| t).sum::<u64>() + self.skills.1 + self.house_rules.1
    }

    /// Nothing to say at all: nothing measurable, and nothing missing either.
    pub(crate) fn is_silent(&self) -> bool {
        self.total() == 0 && self.servers_unmeasured == 0
    }

    /// The breakdown rows, largest first: measured servers one by one, then
    /// the two aggregate rows. Rows with no tokens never appear.
    fn rows(&self) -> Vec<(u64, String)> {
        let mut rows: Vec<(u64, String)> = self
            .servers
            .iter()
            .map(|(n, t)| (*t, n.clone()))
            .filter(|(t, _)| *t > 0)
            .collect();
        if self.skills.1 > 0 {
            rows.push((
                self.skills.1,
                super::count(self.skills.0, "skill description"),
            ));
        }
        if self.house_rules.1 > 0 {
            rows.push((
                self.house_rules.1,
                super::count(self.house_rules.0, "house rule"),
            ));
        }
        rows.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        rows
    }
}

pub(crate) struct SessionFacts {
    profile: String,
    started_unix: u64,
    age_secs: u64,
    abandoned: bool,
}

/// W1 — "needs your yes": the evidence-bearing form of an untrusted or drifted
/// project. Present only when calls were actually refused here since the last
/// yes, which is what separates "you have not reviewed this yet" (a state) from
/// "something tried to work and could not" (a consequence).
///
/// It carries no card. The one authoritative card is rendered by the command
/// named in [`NeedsYourYes::fix`] — `agentstack trust` — and there is
/// deliberately no second construction of it here or anywhere a UI could reach
/// without going through that command.
pub(crate) struct NeedsYourYes {
    /// How many refusals were recorded for this project since it was last
    /// trusted (since the beginning of the log when it never was).
    pub(crate) refused: usize,
    /// The most recent refusal's timestamp — so a surface can say "just now"
    /// rather than only "at some point".
    pub(crate) last_refused_ts: u64,
    /// The one command that answers it, naming the project explicitly so a
    /// caller acting from another directory does not have to guess.
    pub(crate) fix: String,
}

/// Count this project's recorded trust refusals since its last yes.
///
/// `None` for a trusted project — and cheaply so: the whole read is skipped,
/// which matters because `status` must feel instant and this walks the machine
/// audit log. A project that is untrusted or drifted has already lost the fast
/// path in every other sense, and it is the only one that can have anything to
/// report.
///
/// `manifest_dir` must be the same string form `seatbelt::record` files under
/// (the resolved manifest dir), or the filter silently matches nothing.
pub(crate) fn needs_your_yes(
    manifest_dir: &Path,
    root: &Path,
    trust: crate::trust::TrustState,
) -> Option<NeedsYourYes> {
    if trust == crate::trust::TrustState::Trusted {
        return None;
    }
    // Since the last yes, not since forever: refusals from before a grant
    // describe a project state the user already answered. No entry (never
    // trusted, or revoked) means the whole log is in scope.
    let since = crate::trust::TrustStore::load()
        .trusted
        .get(&crate::trust::key_for(root))
        .map(|entry| entry.trusted_at)
        .unwrap_or(0);
    let want = manifest_dir.display().to_string();
    let refusals: Vec<u64> = crate::calllog::read_all()
        .into_iter()
        .filter(|rec| {
            rec.tool == "trust"
                && rec.outcome == crate::calllog::CallOutcome::Denied
                && rec.ts >= since
                && rec.project.as_deref() == Some(want.as_str())
        })
        .map(|rec| rec.ts)
        .collect();
    if refusals.is_empty() {
        return None;
    }
    Some(NeedsYourYes {
        refused: refusals.len(),
        last_refused_ts: refusals.iter().copied().max().unwrap_or_default(),
        fix: format!(
            "agentstack trust {}",
            crate::text::sanitize_line(&root.display().to_string())
        ),
    })
}

pub(crate) struct SecretFacts {
    referenced: usize,
    /// The `${REF}` names that resolve from no layer here. NAMES only — a
    /// value never reaches this struct, let alone a serializer (invariant 5).
    unresolved: Vec<String>,
}

/// `agentstack status` — the orientation screen by name, plus the cheap health
/// signals a glance wants (secrets resolving?) and the pointer to the deep
/// check. Everything expensive (drift rendering, content scans) stays in
/// `doctor`; status must feel instant.
/// One sentence per shadowed library name, or nothing at all.
///
/// Cheap by construction — reading the linked indexes is the same work name
/// resolution already does — and best-effort: an unreadable source list must
/// never take the orientation screen down with it.
fn shadowed_name_lines() -> Vec<String> {
    let sources = crate::sources::Sources::load_or_warn();
    let Ok(library) = crate::library::Library::load_linked(&sources.linked()) else {
        return Vec::new();
    };
    library
        .linked
        .collisions
        .iter()
        .map(|c| {
            format!(
                "{} '{}' is in {} sources — '{}' wins; `{}` pins the other",
                c.kind.noun(),
                crate::text::sanitize_line(&c.name),
                c.shadowed.len() + 1,
                c.winner,
                c.qualified_shadowed(),
            )
        })
        .collect()
}

pub fn run_status(manifest_dir: Option<&Path>, json: bool) -> Result<()> {
    // `--json` changes only the rendering: the same collect, with the same
    // deep readings the named `status` screen already asks for.
    let orientation = collect(manifest_dir, true)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&crate::ui_contract::envelope(status_json(&orientation)))?
        );
        return Ok(());
    }
    print_orientation(&orientation, true);
    Ok(())
}

/// The `status --json` body for a project, without the envelope.
///
/// The one public seam onto the same reading `agentstack status --json` prints,
/// so a witness can assert the shipped sentences rather than a paraphrase of
/// them. Read-only, like the command.
pub fn status_body(manifest_dir: Option<&Path>) -> Result<serde_json::Value> {
    Ok(status_json(&collect(manifest_dir, true)?))
}

pub fn run(manifest_dir: Option<&Path>) -> Result<()> {
    let orientation = collect(manifest_dir, false)?;
    print_orientation(&orientation, false);
    Ok(())
}

/// The `status --json` body without the envelope (its caller wraps it) — the
/// testable seam, same shape as `workflow_list_json`.
///
/// `project` is `null` unless the manifest loaded: a consumer branches on that
/// one field instead of finding a dozen nulls, and `manifest.error` says why
/// when it is null for the second reason. `next_action` reuses the key
/// `doctor --json` already established for the same idea, because it IS the
/// same idea — one command, and why.
pub(crate) fn status_json(o: &Orientation) -> serde_json::Value {
    let (present, error, project) = match &o.manifest {
        ManifestState::Missing => (false, None, serde_json::Value::Null),
        ManifestState::Broken(err) => (
            true,
            Some(crate::text::sanitize_line(err)),
            serde_json::Value::Null,
        ),
        ManifestState::Loaded(f) => (true, None, project_json(f)),
    };
    serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        // Display names, not ids — the same list the screen prints. Adapter
        // ids live in `adapters list --json`, which is the read for that.
        "clis_detected": o.detected_clis,
        // The catalog size, kept a separate key for the same reason the screen
        // keeps it a separate sentence — it is not a detection count.
        "clis_supported": o.catalog_size,
        // Native configs found here carrying servers this manifest doesn't
        // declare. Names only; a discovered config's contents never land here.
        "native_unimported": o.native.iter()
            .filter(|n| !n.unimported.is_empty())
            .map(|n| serde_json::json!({
                "id": n.id,
                "scope": n.scope.as_str(),
                "path": n.path.display().to_string(),
                "servers": n.unimported,
            }))
            .collect::<Vec<_>>(),
        // Dropped-but-undeclared content in this project's own intake dirs.
        // Names, kinds, and the provenance classification — never file bodies.
        "intake": o.intake.iter().map(|i| serde_json::json!({
            "kind": i.kind.noun(),
            "name": i.name,
            "path": i.rel_path,
            "summary": i.summary,
            "locally_authored": i.provenance.is_local(),
            "provenance": i.provenance.reason(),
        })).collect::<Vec<_>>(),
        "manifest": {
            "path": o.manifest_path.display().to_string(),
            "present": present,
            "loaded": matches!(o.manifest, ManifestState::Loaded(_)),
            "error": error,
        },
        "project": project,
        // `command` is the ONLY machine field here: what a driver may run
        // verbatim, `null` wherever the honest human answer is a shape
        // (`<name>`) or a read-only summary — see `machine_command`.
        //
        // `sentence` always carries the line the screen prints, so a UI that
        // wants to show guidance still can. It was called `step` for one
        // round, and that name was a hazard: `step` reads as a command
        // carrier, so a driver — and the guidance guard, which detects machine
        // fields by SHAPE — took the placeholder sentence beside the filtered
        // `command` as something to execute. The field is display prose and is
        // now named as such.
        "next_action": {
            "command": machine_command(&o.next.0),
            "sentence": &o.next.0,
            "why": o.next.1,
        },
    })
}

fn project_json(f: &ProjectFacts) -> serde_json::Value {
    let mut body = serde_json::json!({
        "servers": f.servers,
        "skills": f.skills,
        // Pinned targets and the fan-out count are different questions, so
        // they are different fields: an empty `pinned` with `fanout` = 6 is
        // "no [targets] pinned, six detected CLIs", which one number cannot say.
        "targets": {
            "pinned": f.pinned_targets.iter().map(|t| crate::text::sanitize_line(t)).collect::<Vec<_>>(),
            "fanout": f.fanout_targets,
        },
        "toolsets": f.toolsets.iter().map(|t| crate::text::sanitize_line(t)).collect::<Vec<_>>(),
        "session": f.session.as_ref().map(|s| serde_json::json!({
            "profile": crate::text::sanitize_line(&s.profile),
            "started_unix": s.started_unix,
            "age_seconds": s.age_secs,
            "abandoned": s.abandoned,
        })),
        "locked": f.locked,
        // Same vocabulary `use --list --json` uses, so a UI holding both reads
        // does not need two trust lookup tables.
        "trust": match f.trust {
            // `trust-content-drift-v1`: content drift reads as `drifted` here.
            // The word means "approved bytes moved", and that is true whether
            // the manifest/lock bytes moved or the bodies they pin did — the
            // gate refuses both. `untrusted` still wins: never reviewed is a
            // stronger statement than reviewed-then-changed.
            _ if !f.content_drift.is_empty() && f.trust != crate::trust::TrustState::Untrusted => {
                "drifted"
            }
            crate::trust::TrustState::Trusted => "trusted",
            crate::trust::TrustState::Changed => "drifted",
            crate::trust::TrustState::Untrusted => "untrusted",
        },
        // The itemised reading behind that word, each with the command that
        // makes progress on it. `[]` when nothing drifted.
        "content_drift": f.content_drift.iter().map(|d| serde_json::json!({
            "kind": d.kind,
            "name": crate::text::sanitize_line(&d.name),
            "reason": crate::text::sanitize_line(&d.reason),
            "fix": crate::commands::trust::DRIFT_FIX,
        })).collect::<Vec<_>>(),
        // Declared but never pinned, each with the command that makes progress
        // on it. `[]` when everything declared is pinned. Deliberately NOT
        // folded into the `trust` word above: never-pinned is not drift, and
        // saying "drifted" over content nobody ever approved would be a lie.
        "surface_unpinned": f.surface_unpinned.iter().map(|d| serde_json::json!({
            "kind": d.kind,
            "name": crate::text::sanitize_line(&d.name),
            "reason": crate::text::sanitize_line(&d.reason),
            // Per item, and nullable: a declared body absent from disk has no
            // repairing command, and `trust --preview` already says so. This
            // key used to hard-code the pinning command for every entry, which
            // handed a driver the loop the `Option` exists to end.
            "fix": d.fix,
        })).collect::<Vec<_>>(),
        "trust_relevant": f.trust_relevant,
        // The reading `trust_relevant` does not give. Additive: a panel that
        // never asks for it reads exactly what it read before, and the field
        // above keeps its shipped value, so no schema-version bump is owed.
        "trust_blocks_delivery": f.trust_blocks_delivery,
        "mode": f.mode.label(),
        "gateway_connected": f.gateway_connected,
        // W4 precondition 6. One sentence per broken harness, the SAME text the
        // screen prints and `doctor` reports — a UI must never have to compose
        // its own account of an outage.
        "gateway_outages": f.gateway_outages.iter()
            .map(|o| serde_json::json!({
                "harness": crate::text::sanitize_line(&o.display),
                "command": crate::text::sanitize_line(&o.command),
                "explanation": o.sentence(),
            }))
            .collect::<Vec<_>>(),
        "shadowed_names": f.shadowed_names,
        // `instruction-channels-v1`. Always present (`[]` when the project
        // targets nothing), so a panel can tell "checked, nothing to say" from
        // an older binary that has no such key.
        "instruction_channels": f.instruction_channels.iter()
            .map(|c| c.to_json())
            .collect::<Vec<_>>(),
        "rendered": f.rendered,
        // Names of unresolved refs, never values (invariant 5). `null` when
        // the caller did not ask for the reading at all — distinct from an
        // empty list, which means "asked, everything resolves".
        "secrets": f.secrets.as_ref().map(|s| serde_json::json!({
            "referenced": s.referenced,
            "unresolved": s.unresolved,
        })),
    });
    // `needs-your-yes-v1`. Inserted rather than emitted as `null`, because the
    // question it answers is "has anything been refused here", and a project
    // where nothing has must read exactly as it did before this field existed.
    // No card payload rides along: `fix` names the command that renders the one
    // authoritative card, and that command is the only thing that renders it.
    if let Some(n) = &f.needs_your_yes {
        body["needs_your_yes"] = serde_json::json!({
            "refused": n.refused,
            "last_refused_ts": n.last_refused_ts,
            "fix": n.fix,
        });
    }

    // `context-cost-v1`. The per-session context tax, ESTIMATED. Inserted only
    // when there is something to say, for the reason the update offer is:
    // absence must never be readable as "this project is free". Every number is
    // flagged `"estimate": true` at the top of the object so no consumer can
    // render it as a measurement, and `servers_unmeasured` is what a server
    // with no measurement contributes — never a zero in `servers`.
    if !f.context.is_silent() {
        body["context_cost"] = serde_json::json!({
            "estimate": true,
            "total_est_tokens": f.context.total(),
            "servers": f.context.servers.iter().map(|(n, t)| serde_json::json!({
                "name": crate::text::sanitize_line(n),
                "est_tokens": t,
            })).collect::<Vec<_>>(),
            "servers_unmeasured": f.context.servers_unmeasured,
            "skills": {
                "described": f.context.skills.0,
                "est_tokens": f.context.skills.1,
            },
            "house_rules": {
                "counted": f.context.house_rules.0,
                "est_tokens": f.context.house_rules.1,
            },
            "detail": "agentstack x report usage",
        });
    }

    // `package-members-v1`. The effective member set, straight from the lock.
    // Inserted rather than emitted as an empty list, on the same reasoning as
    // `updates` below: a project that selects no package must read exactly as
    // it did before this field existed.
    //
    // Every member carries `origin`, and every package carries `removed`, so a
    // reader can always answer "which of these came from the package, and which
    // did this project change?" without holding the package to diff against —
    // which is the whole W5 acceptance criterion for overrides. `lane` is
    // derived from the kind, so an instruction member can never be rendered as
    // something served through the gateway.
    if !f.packages.is_empty() {
        if let Some(map) = body.as_object_mut() {
            map.insert(
                "packages".into(),
                serde_json::Value::Array(
                    f.packages
                        .iter()
                        .map(|p| {
                            serde_json::json!({
                                "name": crate::text::sanitize_line(&p.name),
                                "version": crate::text::sanitize_line(&p.version),
                                "source": crate::text::sanitize_line(&p.source),
                                "rev": p.rev.as_deref().map(crate::text::sanitize_line),
                                "toolsets": p.toolsets.iter()
                                    .map(|t| crate::text::sanitize_line(t))
                                    .collect::<Vec<_>>(),
                                "removed": p.removed.iter()
                                    .map(|r| crate::text::sanitize_line(r))
                                    .collect::<Vec<_>>(),
                                "overrides": p.members.iter()
                                    .filter(|m| m.origin
                                        == agentstack_core::lock::PackageMemberOrigin::ProjectOverride)
                                    .count(),
                                "members": p.members.iter().map(|m| serde_json::json!({
                                    "name": crate::text::sanitize_line(&m.name),
                                    "kind": m.kind.as_str(),
                                    "lane": m.kind.lane(),
                                    "origin": m.origin.as_str(),
                                    "checksum": m.checksum.hex(),
                                    "provenance": crate::text::sanitize_line(&m.provenance),
                                })).collect::<Vec<_>>(),
                            })
                        })
                        .collect(),
                ),
            );
        }
    }

    // `update-offer-v1`. Inserted, never emitted as `null` or as an empty
    // list: presence means "there is an offer", and its ABSENCE means only
    // "no offer was produced" — never "you are current" (the check is offline
    // and stays silent about everything it cannot answer). Keeping the key out
    // entirely is what stops a consumer from rendering an "up to date" badge
    // off an empty array.
    if !f.updates.is_empty() {
        if let Some(map) = body.as_object_mut() {
            map.insert(
                "updates".into(),
                serde_json::json!({
                    "packs": f.updates.iter().map(|u| serde_json::json!({
                        "name": u.name,
                        "current": u.current,
                        "available": u.available,
                    })).collect::<Vec<_>>(),
                    "fix": super::updates::fix_command(&f.updates),
                }),
            );
        }
    }
    body
}

/// Secrets at a glance for `status`: the single most common thing broken after
/// setup. Only the NAMES that fail to resolve are kept — `source_of` answers a
/// presence question, and nothing it learns beyond that leaves this function.
fn secret_facts(ctx: &super::Context) -> Option<SecretFacts> {
    let refs = ctx.loaded.manifest.referenced_secrets();
    if refs.is_empty() {
        return None;
    }
    let sources = crate::secret::SecretSources::detect(&ctx.dir);
    let unresolved: Vec<String> = refs
        .iter()
        .filter(|n| sources.source_of(n).is_none())
        .cloned()
        .collect();
    Some(SecretFacts {
        referenced: refs.len(),
        unresolved,
    })
}

/// The per-session context cost of this project, from the readings that
/// already exist (`context-cost-v1`).
///
/// Servers come from the measured cache only. Skills and house rules are
/// estimated from the bytes a harness actually injects, through the one shared
/// [`crate::footprint::estimate_tokens`] heuristic — this function derives no
/// token count of its own.
///
/// Best-effort throughout: anything unreadable is left out of the count rather
/// than guessed at, which is why a `0` from here always means "nothing to
/// measure" and never "measured, and it is free".
fn context_cost(ctx: &super::Context) -> ContextCost {
    use crate::footprint::estimate_tokens;

    let m = &ctx.loaded.manifest;
    let mut cost = ContextCost::default();

    // Servers: the measured cache, never a re-derivation. A declared server
    // with no entry is UNKNOWN, and is reported as such.
    let footprints = crate::footprint::Footprints::load().unwrap_or_default();
    for name in m.declared_server_names() {
        match footprints.get(&name) {
            Some(f) => cost.servers.push((name, f.est_tokens)),
            None => cost.servers_unmeasured += 1,
        }
    }
    cost.servers
        .sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

    // Skills: the frontmatter `description` of every skill this project can
    // activate — inline blocks and toolset references alike. That one line per
    // skill is what a harness puts in front of the model; the body is loaded on
    // demand and deliberately not counted here.
    let library = crate::library::Library::load_default_or_warn();
    let lib_home = crate::util::paths::lib_home();
    let mut names: Vec<String> = m.skills.keys().cloned().collect();
    for profile in m.profiles.values() {
        for s in &profile.skills {
            // `"*"` means "the inline skills already declared", which are
            // in the list already.
            if s != "*" && !names.contains(s) {
                names.push(s.clone());
            }
        }
    }
    for name in &names {
        // Inline declaration first, then the library — the same order
        // `explain` and `why` use, so the three cannot disagree about which
        // copy of a name they are describing.
        let inline = m
            .skills
            .get(name)
            .and_then(|s| s.path.as_deref())
            .and_then(|p| {
                let p = Path::new(p);
                let dir = if p.is_absolute() {
                    p.to_path_buf()
                } else {
                    ctx.dir.join(p)
                };
                crate::util::read_to_string_bounded(&dir.join("SKILL.md"), MAX_SKILL_MD).ok()
            })
            .and_then(|text| crate::library::parse_frontmatter_description(&text));
        let description = match inline {
            Some(d) => Some(d),
            None => library.get(name).and_then(|e| e.description(&lib_home)),
        };
        if let Some(d) = description {
            cost.skills.0 += 1;
            cost.skills.1 += estimate_tokens(d.chars().count());
        }
    }

    // House rules: the base body of every declared fragment. Size on disk, not
    // a read — the file is copied into the harness's instruction file whole, so
    // its length IS the cost, and a manifest can point at a large one. Bytes
    // stand in for characters here; for the ASCII-ish prose these files hold
    // the difference is far below the heuristic's own error.
    for (name, instr) in &m.instructions {
        let Ok(bodies) = crate::instructions::bodies(name, instr, &ctx.dir, &library) else {
            continue;
        };
        let path = bodies.source_of(&bodies.base);
        if let Ok(meta) = std::fs::metadata(&path) {
            cost.house_rules.0 += 1;
            cost.house_rules.1 += estimate_tokens(meta.len() as usize);
        }
    }

    cost
}

/// Bound on a `SKILL.md` we open only to read its frontmatter description
/// (invariant 7: repository content is hostile input, and is read bounded).
const MAX_SKILL_MD: u64 = 256 * 1024;

/// The update **offer** (design §Update model rule 2): name that newer
/// versions exist and the one command that takes them. Nothing is printed when
/// there is no offer — deliberately not a green "up to date", because the
/// check behind this is offline and cannot prove currency (see
/// [`crate::commands::updates::available_updates`]).
///
/// Rule 4 shapes the second line: keep-pinned is the resting state, so this
/// offers and then says so. It must not read as a warning, a nag, or a claim
/// that staying put is a fault.
fn print_updates_line(updates: &[crate::commands::updates::PackUpdate]) {
    if updates.is_empty() {
        return;
    }
    let list = updates
        .iter()
        .map(|u| format!("{} {} → {}", u.name, u.current, u.available))
        .collect::<Vec<_>>()
        .join(" · ");
    println!(
        "  {}  {} with a newer version: {list}",
        "Updates ".bold(),
        super::count(updates.len(), "pack")
    );
    println!(
        "            {}",
        format!(
            "take it with `{}` — staying on the pinned version is a complete answer",
            crate::commands::updates::fix_command(updates)
        )
        .dimmed()
    );
}

/// `context-cost-v1` — the per-session context tax, on the existing
/// label-and-value grid.
///
/// Four rules decide whether this line is honest or harmful, and all four are
/// enforced here:
///
/// 1. **It is an estimate**, and says so in the line itself. No rendering of
///    this reading may read as a measurement of a specific model's tokenizer.
/// 2. **No data is not zero.** Nothing measurable and nothing missing prints
///    nothing at all. Declared servers that were never measured print as
///    unmeasured, and are never counted as free.
/// 3. **Information, not a rung.** It prints below the state rows and above the
///    `Next:` line, it names no fix, and it never touches `next_action`.
/// 4. **Quiet when boring.** One contributor gets the total only; the breakdown
///    appears when there is genuinely something to compare. The full per-server
///    detail lives in `agentstack x report usage`.
fn print_context_line(c: &ContextCost) {
    if c.is_silent() {
        return;
    }
    let total = c.total();
    if total == 0 {
        // Only unmeasured servers: say that, rather than printing a total of 0
        // for a project whose servers are usually the whole bill.
        println!(
            "  {}  {} not measured — context cost unknown, not zero",
            "Context ".bold(),
            super::count(c.servers_unmeasured, "declared server")
        );
        // Deliberately not "measure it with …": the measuring pass is a live
        // connection behind a flag, and naming the read-only command as though
        // it measured would be a claim the command does not meet. The read-only
        // command is named, and it is the one that explains the measurement.
        println!("            {}", "see `agentstack x report usage`".dimmed());
        return;
    }

    println!(
        "  {}  ~{} per session (estimate)",
        "Context ".bold(),
        fmt_tokens_per_session(total)
    );

    let rows = c.rows();
    if rows.len() > 1 {
        for (tokens, label) in &rows {
            // A row that rounds to 0% is still not free — say `<1%` rather
            // than printing a zero share beside a non-zero cost.
            let share = (*tokens as f64 / total as f64 * 100.0).round() as u64;
            let share = if share == 0 {
                "<1%".to_string()
            } else {
                format!("{share}%")
            };
            println!(
                "            {:>8}  {label} ({share})",
                crate::footprint::fmt_tokens(*tokens)
            );
        }
    }
    if c.servers_unmeasured > 0 {
        println!(
            "            {}",
            format!(
                "plus {} never measured — the total above excludes them",
                super::count(c.servers_unmeasured, "declared server")
            )
            .dimmed()
        );
    }
    println!(
        "            {}",
        "detail: `agentstack x report usage`".dimmed()
    );
}

/// The headline total, in the same units the rest of the binary prints token
/// counts in — [`crate::footprint::fmt_tokens`], spelled out as `tokens`
/// because this line is read by a person deciding what to keep, not scanned in
/// a table.
fn fmt_tokens_per_session(t: u64) -> String {
    crate::footprint::fmt_tokens(t).replace(" tok", " tokens")
}

/// One aligned line when everything resolves; one line per missing secret,
/// each carrying its exact fix command.
fn print_secrets_line(facts: &SecretFacts) {
    if facts.unresolved.is_empty() {
        println!(
            "  {}  {} referenced, all resolve",
            "Secrets ".bold(),
            facts.referenced
        );
    } else {
        for name in &facts.unresolved {
            println!(
                "  {}  {} {name} not set   {}",
                "Secrets ".bold(),
                "✗".red(),
                format!("fix: agentstack secret set {name}").dimmed()
            );
        }
    }
}

/// The "you already have a setup here" line. Silent when every native config
/// found is already covered by the manifest — there is nothing to act on and a
/// permanent line would be noise. `no_manifest` changes only the pointer:
/// before a manifest exists the path in is `init`, after it, `adopt`.
fn print_native_line(native: &[crate::discover::NativeConfig], no_manifest: bool) {
    let pending: Vec<&crate::discover::NativeConfig> =
        native.iter().filter(|n| !n.unimported.is_empty()).collect();
    if pending.is_empty() {
        return;
    }
    let servers: usize = pending.iter().map(|n| n.unimported.len()).sum();
    let files = pending
        .iter()
        .map(|n| crate::text::sanitize_line(&tidy(&n.path)))
        .collect::<Vec<_>>()
        .join(" · ");
    println!(
        "  {}  {} configured here, not in this setup — {}",
        "Found   ".bold(),
        super::count(servers, "server"),
        files.dimmed()
    );
    println!(
        "            {}",
        if no_manifest {
            "`agentstack init` imports them"
        } else {
            "`agentstack adopt` brings them into the manifest"
        }
        .dimmed()
    );
}

/// Name dropped-but-undeclared skills/instructions, and what to do about them.
fn print_intake_line(items: &[crate::intake::Item]) {
    if items.is_empty() {
        return;
    }
    let names = items
        .iter()
        .map(|i| i.name.as_str())
        .collect::<Vec<_>>()
        .join(" · ");
    println!(
        "  {}  {} here, not in this setup — {}",
        "Dropped ".bold(),
        super::count(items.len(), "file"),
        names.dimmed()
    );
    // `yes` is the funnel built for exactly this moment. It is also the safe
    // router for content that may NOT take the short path: clone-supplied
    // drops and collisions are held on its preview with the reason and the
    // explicit-path commands, so naming `yes` here never skips a review.
    println!(
        "            {}",
        "`agentstack yes` reviews them and takes them live".dimmed()
    );
}

/// Project-relative display for a discovered config path, falling back to the
/// full path when it is not under the current directory.
fn tidy(path: &std::path::Path) -> String {
    std::env::current_dir()
        .ok()
        .and_then(|cwd| path.strip_prefix(cwd).ok())
        .unwrap_or(path)
        .display()
        .to_string()
}

/// Read every fact the orientation screen states. `deep_reads` is the one
/// knob, and it covers the two readings only the named `status` screen asks
/// for: the secrets resolution (bare `agentstack` has never consulted the
/// resolvers) and the offline update check (one local git ref read per
/// installed git pack). Gathering either unconditionally would make bare
/// `agentstack` slower for lines it does not print.
fn collect(manifest_dir: Option<&Path>, deep_reads: bool) -> Result<Orientation> {
    let registry = Registry::load()?;

    // Walk up to the project root so `agentstack` from `src/deep` describes
    // the ROOT manifest instead of steering toward a nested `init`.
    let base = super::project_base(manifest_dir)?;
    let dir = crate::manifest::resolve_manifest_dir(&base);
    let manifest_path = dir.join(MANIFEST_FILE);

    // Detection is asked OF THIS DIRECTORY (`detected_in`), not of the machine.
    // A repo whose only setup is a project-scope `.mcp.json` used to be reported
    // as "none detected on this machine" while four servers sat in the working
    // directory — the pilot's blocker #1.
    let detected_clis: Vec<String> = registry
        .iter()
        .filter(|d| d.detected_in(&dir))
        .map(|d| d.display.clone())
        .collect();

    if !manifest_path.exists() {
        // Nothing of ours here yet — so anything native we can see is the whole
        // story, and `init` must be told to expect it.
        let native = crate::discover::native_configs(&registry, &dir, &Default::default(), false);
        return Ok(Orientation {
            detected_clis,
            catalog_size: registry.ids().count(),
            native,
            intake: Vec::new(),
            manifest_path,
            manifest: ManifestState::Missing,
            next: (
                "agentstack init".to_string(),
                "guided one-command setup — import, preview, apply",
            ),
        });
    }

    let ctx = match super::load(manifest_dir) {
        Ok(ctx) => ctx,
        Err(err) => {
            return Ok(Orientation {
                detected_clis,
                catalog_size: registry.ids().count(),
                native: Vec::new(),
                intake: Vec::new(),
                manifest_path,
                manifest: ManifestState::Broken(format!("{err:#}")),
                next: ("agentstack doctor".to_string(), "diagnose the manifest"),
            })
        }
    };

    let m = &ctx.loaded.manifest;
    // No [targets] pinned → commands fan out to the detected CLIs (see
    // render::resolve_targets); "0 target(s)" would be false.
    let fanout_targets = crate::render::resolve_targets(m, &ctx.registry, &[], &ctx.dir)
        .map(|t| t.len())
        .unwrap_or_default();

    // The active profile is marked only when a live session pins it — the one
    // signal that *reliably* says which profile is loaded right now.
    let active_session = crate::session::active(&ctx.dir);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let session = active_session.as_ref().map(|s| SessionFacts {
        profile: s.profile.clone(),
        started_unix: s.started_unix,
        age_secs: now.saturating_sub(s.started_unix),
        abandoned: s.is_abandoned(now),
    });

    // Where this project actually stands, from cheap signals: lockfile (was it
    // ever activated/pinned?) and trust state.
    let project_root = crate::manifest::project_root_of(&ctx.dir);
    let trust = crate::trust::check(&project_root);
    let locked = crate::lock::Lock::path(&ctx.dir).exists();
    // The second half of the trust reading: bytes the consent pinned that have
    // since moved. Same shared detector `trust --preview` and `doctor` use — a
    // second drift check here is exactly how the surfaces came to disagree.
    let content_drift = if deep_reads {
        crate::commands::trust::content_drift(
            &ctx.dir,
            m,
            &crate::lock::Lock::load(&ctx.dir).unwrap_or_default(),
            &crate::library::Library::load_default_or_warn(),
        )
    } else {
        Vec::new()
    };
    // The step BEFORE that one: declared content that was never pinned. Same
    // shared detector the grant refuses on, read under the same deep-read
    // budget as the drift pass above.
    let surface_unpinned = if deep_reads {
        crate::commands::trust::unpinned_surface(
            &ctx.dir,
            m,
            &crate::lock::Lock::load(&ctx.dir).unwrap_or_default(),
            &crate::library::Library::load_default_or_warn(),
        )
    } else {
        Vec::new()
    };

    // Two different readings, deliberately kept apart.
    //
    // `trust_relevant` is about DELIVERY POSTURE: trusting changes what this
    // project is *shaped like* only through the bridge (zero-files) or the
    // trust-gated run/session paths (clean-at-rest). It is what decides whether
    // trust is the headline next step and whether the gateway-shaped "inert
    // servers" note is shown — a prompting hint, and nothing more. It does NOT
    // say a static, no-gateway project is unaffected by trust: the gate refuses
    // that project's servers, instructions, hooks, extensions, settings and
    // skills too. That claim used to be written here, and it was false.
    let target_ids: Vec<String> = ctx.registry.ids().map(str::to_string).collect();
    let gateway = gateway_connected(&ctx, &target_ids);
    let mode = detect_mode(&ctx, &target_ids);
    let trust_relevant = trust_relevant(gateway, mode);

    let has_capabilities = declares_capabilities(m);
    // ...and this is the CAPABILITY reading: is the gate actually standing
    // between declared content and every harness right now? Independent of
    // mode on purpose. `trust_relevant` flips from false to true when a
    // lockfile appears — even one a fully refused `apply --write` left behind —
    // while nothing about what trust governs has moved; this one does not.
    let trust_blocks_delivery = trust_blocks_delivery(trust, has_capabilities);
    // "Is anything on disk for these targets?" — the signal that actually
    // distinguishes "imported but not applied" from "set up and resting".
    // `locked` does not: a static project stays unlocked until `use`/`lock` runs.
    let rendered = has_rendered_artifacts(&ctx, &target_ids);
    // The rung `status` stands on. Distinct from `rendered` above, which stays
    // the literal on-disk fact reported in the JSON: zero-files delivery has
    // nothing on disk and is nonetheless fully delivered. Same disjunct
    // `doctor` applies, through the shared helper, so the two agree.
    let delivered_rung = stands_on_delivered_rung(matches!(mode, Mode::ZeroFiles), rendered);

    // Native configs here whose servers this manifest does not declare. Cheap
    // (a handful of small files at project scope) and the answer to the pilot's
    // silent case: a manifest that covers none of what is actually configured.
    let native = crate::discover::native_configs_with(
        &ctx.registry,
        &ctx.dir,
        // Name references count as declared — see `declared_server_names`.
        &crate::discover::declared_server_names(m),
        false,
    );
    let unimported = native.iter().any(|n| !n.unimported.is_empty());
    let any_detected = !detected_clis.is_empty();

    // Scanned before the next step is chosen: a dropped file changes what the
    // one next action is (`agentstack yes`), so the routing has to know.
    let intake = crate::intake::scan(&ctx.dir, &project_root, m).items;
    let undeclared_drops = !intake.is_empty();

    // W1: what actually got refused here since the last yes. Computed before
    // the next step is chosen, because evidence outranks every other routing
    // signal — see the `pending` override below.
    let pending = needs_your_yes(&ctx.dir, &project_root, trust);

    let fallback = next_step(
        trust,
        delivered_rung,
        has_capabilities,
        trust_relevant,
        m.profiles.is_empty(),
        unimported,
        undeclared_drops,
        !m.declared_server_names().is_empty(),
    );
    // The Apply rung names a render; `apply` never renders skills. See
    // `correct_apply_rung` — `doctor` applies the same correction to the same
    // rung, so the two surfaces still answer this state with one command.
    let fallback = correct_apply_rung(fallback, apply_renders_something(m));
    // The declared toolset names, whole. `clean_at_rest_next_step` needs to
    // tell "one, so name it" from "several, so let the user pick" from "none,
    // so `session start` cannot run"; flattening them here is what made the
    // last two print the same impossible placeholder.
    let toolset_names: Vec<String> = m.profiles.keys().cloned().collect();
    // A waiting drop also outranks the clean-at-rest session rhythm: starting
    // a session materializes only what is declared, so it would deliver
    // everything EXCEPT the file the user just dropped.
    let next = clean_at_rest_next_step(
        mode,
        trust,
        locked,
        active_session.is_some(),
        &toolset_names,
    )
    .filter(|_| !undeclared_drops)
    .unwrap_or_else(|| (fallback.0.to_string(), fallback.1));
    // ...and a refusal outranks the drop. `next_step`'s ladder puts a waiting
    // drop above an untrusted (not drifted) project, which is right when
    // nothing has tried to use the project yet. Once something HAS — and been
    // refused — the screen would otherwise print the needs-your-yes line and
    // then recommend a different command, which is the two-surfaces-disagree
    // failure the single-next-step rule exists to prevent. Same verb the
    // untrusted and drifted branches already name, so nothing new is invented
    // here; only the ordering is made explicit.
    let next = match &pending {
        Some(_) => (
            "agentstack trust .".to_string(),
            "calls were refused here — review this project and say yes",
        ),
        None => next,
    };

    // W4: the delivery planner's routing for this project — computed once here
    // so the screen, the JSON body, and the next step cannot disagree about it.
    //
    // Scoped to the manifest's OWN targets, not the whole registry. The mode
    // and gateway readings above deliberately span every adapter (a rendered
    // file or a registered bridge is a fact wherever it is); routing is the
    // opposite question — where THIS project's capabilities go — and answering
    // it for eleven CLIs the project never named would bury the two it did.
    let delivery_targets = crate::render::resolve_targets(m, &ctx.registry, &[], &ctx.dir)
        .unwrap_or_else(|_| m.targets.default.clone());
    let delivery_plan = crate::delivery::Plan::build(&m.delivery, &ctx.registry, &delivery_targets);
    let delivery_has_live = crate::commands::delivery::declares_something_live(m, &delivery_plan);
    // Only harnesses that could actually host a bridge here. `doctor` applies
    // exactly this filter before it raises the bridge finding (see the
    // `bridge_capable_here` filter there); without it, `status` recommended
    // `gateway connect --all --write` on a machine where no CLI can host the
    // gateway — a command the user cannot obey, while `doctor` reported the
    // same state as "nothing to connect" and named a different next step. Two
    // surfaces, one state, two answers, and the recommended one is a no-op.
    let delivery_bridge_gaps: Vec<String> =
        crate::commands::delivery::unconnected_live(&delivery_plan, &ctx.registry)
            .into_iter()
            .filter(|display| {
                delivery_plan
                    .live_harnesses()
                    .iter()
                    .any(|h| &h.display == display && bridge_capable_here(&ctx.registry, &h.id))
            })
            .collect();

    // Routing says what `apply` writes from now on; the state ledger plus disk
    // say what an earlier apply already wrote and left. A failed state read is
    // no reason to claim there is nothing there — but there is also nothing
    // honest to report from it, so an empty list is the only safe fallback and
    // the ledger's own health is `doctor`'s finding, not this screen's.
    let delivery_abandoned = crate::state::State::load()
        .map(|state| {
            crate::commands::apply::abandoned_live_renders(
                &ctx,
                &delivery_plan,
                &state,
                &[crate::scope::Scope::Project, crate::scope::Scope::Global],
            )
        })
        .unwrap_or_default();

    // An abandoned render IS a rung, not a footnote. `doctor` already ends on
    // it (its `↳` fix is the first warning-level command in the report), while
    // `status` used to print the warning and then recommend
    // `toolset create …` — two surfaces, one state, two different "one next
    // action"s. The file is live: a config a harness reads that AgentStack no
    // longer maintains outranks anything about growing the project. Consent
    // and the waiting-drop funnel still outrank it, and the bridge ERROR below
    // overrides it, matching doctor's error-over-warning order.
    // Only the RECORDED ones. `recorded == false` means the file is on disk
    // and AgentStack did not write it — a clone, a hand edit, or the gateway's
    // own bridge registration. Those are reported (they are live files), but
    // `x unrender --write` is a removal, and putting a removal of somebody
    // else's file at the top of the screen as THE next action is a worse claim
    // than the one this rung fixes.
    let next = if delivery_abandoned.iter().any(|a| a.recorded)
        && !next.0.starts_with("agentstack trust")
        && next.0 != "agentstack yes"
    {
        (
            crate::commands::apply::AbandonedRender::REMOVE_IT.to_string(),
            ABANDONED_RENDER_WHY,
        )
    } else {
        next
    };

    // An unregistered bridge over declared live-lane capabilities is doctor's
    // one ERROR-level finding, and `doctor`'s ladder ranks it directly below
    // consent. `status` must rank it the same way or the two surfaces name
    // different next actions for one state (round-2 finding 3, in reverse):
    // the screen would print the "register the bridge" hint and then recommend
    // `apply --write`. Consent and the waiting-drop funnel still outrank it —
    // nothing is served live from a project that has not been reviewed, so the
    // review is genuinely first.
    let next = if delivery_has_live
        && !delivery_bridge_gaps.is_empty()
        && !next.0.starts_with("agentstack trust")
        && next.0 != "agentstack yes"
    {
        (
            "agentstack x gateway connect --all --write".to_string(),
            "nothing routed live is reaching those tools until the bridge is registered",
        )
    } else {
        next
    };

    // Drifted content outranks every rung above it, for the same reason
    // `TrustState::Changed` does: bytes the human already approved have moved,
    // and nothing downstream will serve them until they are re-pinned and
    // re-reviewed. It names `lock --write` rather than `trust .` because the
    // grant REFUSES over drift — sending a driver to `trust .` here is the
    // two-surfaces-disagree loop, one step longer.
    let next = if content_drift.is_empty() {
        next
    } else {
        (
            crate::commands::trust::DRIFT_FIX.to_string(),
            crate::commands::trust::DRIFT_WHY,
        )
    };

    // A never-pinned surface outranks the consent rung for exactly the reason
    // drift does, one step earlier: the grant refuses with "its loadable
    // surface isn't fully pinned", so answering `agentstack trust .` here
    // names step 2 while step 1 is outstanding — the dead end a program driving
    // this field verbatim can never leave. Ranked BELOW drift only because
    // both name the same command; the reason string differs so the human is
    // told which of the two they are in. Same command and same reason string
    // as `doctor`.
    // …and it names a command only when a reported blocker carries one, from
    // the findings themselves — see `unpinned_next_action`.
    let next = if !content_drift.is_empty() {
        next
    } else {
        unpinned_next_action(&surface_unpinned).unwrap_or(next)
    };

    // LAST, and deliberately so: every rung above may name `agentstack trust
    // .`, and on a project that has never been pinned all of them are one step
    // early — the grant binds to a lockfile that does not exist yet, so `use
    // --write` mints one and voids it. See `correct_trust_rung` for the
    // measurement. Applied to the ANSWER rather than to each arm, and applied
    // identically by `doctor`, so the two surfaces cannot disagree about the
    // order of the ceremony.
    let next = correct_trust_rung(next, locked, lock_pins_something(m));

    Ok(Orientation {
        catalog_size: ctx.registry.ids().count(),
        detected_clis,
        native,
        intake,
        manifest_path,
        manifest: ManifestState::Loaded(Box::new(ProjectFacts {
            // Named, not defined-inline: a library-first manifest keeps its
            // servers in `[toolsets.*]`, and counting `[servers]` reported
            // "0 servers" one line under six pinned ones.
            servers: m.declared_server_names().len(),
            skills: m.skills.len(),
            instructions: m.instructions.len(),
            settings: m.settings.len(),
            hooks: m.hooks.len(),
            extensions: m.extensions.len(),
            pinned_targets: m.targets.default.clone(),
            fanout_targets,
            fanout_detected: any_detected,
            toolsets: m.profiles.keys().cloned().collect(),
            session,
            locked,
            trust,
            content_drift,
            surface_unpinned,
            trust_relevant,
            trust_blocks_delivery,
            mode,
            gateway_connected: gateway,
            gateway_outages: crate::commands::connect::gateway_outages(&ctx.registry, &target_ids),
            shadowed_names: shadowed_name_lines(),
            // PLAN versus STATE: with no bridge registered, the live lane is
            // routing that has not started. Read PER HARNESS — one connected
            // CLI must never make the other four claim live delivery.
            delivery: crate::commands::delivery::summary_lines_for(&delivery_plan, &ctx.registry),
            instruction_channels: crate::instructions::channels(
                m,
                &ctx.registry,
                &delivery_targets,
                crate::scope::Scope::default_for(&ctx.dir),
                &ctx.dir,
                &crate::library::Library::load_default_or_warn(),
                // `status` names no toolset, so the model comes from
                // `[settings.<cli>] model` or is honestly unknown.
                None,
            ),
            delivery_rendered_lane: crate::delivery::rendered_lane_line(&delivery_plan),
            // Not `has_dynamic_lane()`: the plan reports a live lane for what
            // a harness CAN take, so a project declaring only instructions and
            // settings — served entirely from files — was being told to
            // register a bridge it does not need. The predicate doctor's
            // finding uses is the one answer.
            delivery_has_live,
            delivery_bridge_gaps,
            delivery_abandoned,
            rendered,
            secrets: if deep_reads { secret_facts(&ctx) } else { None },
            updates: if deep_reads {
                super::updates::available_updates(m)
            } else {
                Vec::new()
            },
            needs_your_yes: pending,
            // A malformed lock is doctor's finding, not status's: degrade to
            // "no packages" rather than turning the orientation screen into an
            // error. `locked` above already says whether a lock exists at all.
            packages: crate::lock::Lock::load(&ctx.dir)
                .map(|lock| crate::package::effective_members(&lock).to_vec())
                .unwrap_or_default(),
            // Deep reads only, on the same budget as secrets and updates: this
            // opens one small file per skill and stats one per house rule.
            context: if deep_reads {
                context_cost(&ctx)
            } else {
                ContextCost::default()
            },
        })),
        next,
    })
}

/// The human screen. `status` distinguishes `agentstack status` (which adds
/// the secrets line and the deep-check pointer) from bare `agentstack`.
fn print_orientation(o: &Orientation, status: bool) {
    println!(
        "{} {} — one portable manifest, every agent CLI\n",
        "agentstack".bold(),
        env!("CARGO_PKG_VERSION")
    );

    // Two different facts, never merged into one number: how many CLIs are
    // detected HERE, and how many the catalog supports. Printing "none
    // detected" above "13 detected CLI(s)" was one screen contradicting itself
    // (pilot Run B); the second number was always the catalog.
    if o.detected_clis.is_empty() {
        println!(
            "  {}  none detected here — {} supported",
            "CLIs    ".bold(),
            o.catalog_size
        );
    } else {
        println!(
            "  {}  {} of {} supported detected here: {}",
            "CLIs    ".bold(),
            o.detected_clis.len(),
            o.catalog_size,
            o.detected_clis.join(" · ")
        );
    }

    // Native config found in this directory that our manifest doesn't cover.
    // Naming it is the whole point: `adopt` could always read these files, but
    // no surface said they existed, so an uncoached user never reached it.
    print_native_line(&o.native, matches!(o.manifest, ManifestState::Missing));

    // Files the user dropped into their own `.agentstack/` tree. Same reason
    // the native line exists: the content is sitting right there, and until a
    // surface names it, nothing tells the user it has to be adopted to count.
    print_intake_line(&o.intake);

    match &o.manifest {
        ManifestState::Missing => println!("  {}  none in this directory", "Setup".bold()),
        ManifestState::Broken(err) => println!(
            "  {}  {} — {}",
            "Setup".bold(),
            o.manifest_path.display(),
            format!("failed to load: {err}").red()
        ),
        ManifestState::Loaded(f) => {
            let mut parts = vec![super::count(f.servers, "server")];
            if f.skills > 0 {
                parts.push(super::count(f.skills, "skill"));
            }
            // Every other declared kind, on the same line and in the same
            // shape. Shown only when non-zero, so a plain server project reads
            // exactly as it did before.
            for (n, noun) in [
                (f.instructions, "instruction"),
                (f.settings, "CLI's settings"),
                (f.hooks, "hook"),
                (f.extensions, "extension"),
            ] {
                if n > 0 {
                    parts.push(super::count(n, noun));
                }
            }
            let targets_note = if f.pinned_targets.is_empty() {
                if f.fanout_detected {
                    format!(
                        "{}, no CLIs pinned",
                        super::count(f.fanout_targets, "detected CLI")
                    )
                } else {
                    // Nothing detected: `resolve_targets` falls back to the
                    // whole catalog. Say that, rather than calling the catalog
                    // "detected".
                    format!(
                        "no CLIs pinned and none detected here — would try all {}",
                        super::count(f.fanout_targets, "supported CLI")
                    )
                }
            } else {
                super::count(f.pinned_targets.len(), "target")
            };
            println!(
                "  {}  {} — {} → {}",
                "Setup".bold(),
                o.manifest_path.display(),
                parts.join(" · "),
                targets_note
            );

            // Profiles get their own line, named rather than counted (P18):
            // "which profiles do I have" stops being archaeology through the
            // manifest or a triggered disambiguation error.
            if !f.toolsets.is_empty() {
                println!(
                    "  {}  {}",
                    "Toolsets".bold(),
                    profiles_line(&f.toolsets, f.session.as_ref().map(|s| s.profile.as_str()))
                );
            }

            // A package is only ever named by a toolset, and only a toolset
            // delivers one: `package_runtime_servers` contributes nothing to an
            // unfenced run, so the member set `status --json` publishes is not
            // what a plain `agentstack run <cli>` loads (invariant 8 — claims
            // match enforcement, `docs/design/package-layer.md`). Printed only
            // when this project pinned a package, so a project without one
            // reads exactly as it did before.
            if !f.packages.is_empty() {
                println!(
                    "            {}",
                    "package servers reach a run only when the run names a toolset that selects the package"
                        .dimmed()
                );
            }

            // Stage 2.2: an active temporary session is a first-class fact of
            // the default status surface — its own line, not just the (active)
            // marker inside the profiles list, with the command that reverts it.
            if let Some(sess) = &f.session {
                let (headline, hint) =
                    session_status_line(&sess.profile, sess.age_secs, sess.abandoned);
                println!("  {}  {} — {}", "Session ".bold(), headline, hint.dimmed());
            }

            println!(
                "  {}  {}{}",
                "Status  ".bold(),
                if f.locked {
                    "locked"
                } else {
                    "not locked (never activated)"
                },
                match f.trust {
                    crate::trust::TrustState::Trusted => " · trusted",
                    crate::trust::TrustState::Changed => " · trust stale (content changed)",
                    crate::trust::TrustState::Untrusted => " · untrusted",
                }
            );

            // Untrusted (or trust-stale) teaches the human what that state
            // *means*, not just the label (P16). Only shown when trust is
            // relevant — the sentence names the gateway, and a static,
            // no-gateway project has no gateway to describe, so the honest
            // `· untrusted` Status label stands alone. Not because its servers
            // are live: they are inert too, refused at `apply`/`use` instead.
            if f.trust_relevant {
                if let Some(note) = orientation_trust_note(f.trust) {
                    println!("            {}", note.dimmed());
                }
            }

            // W1. The line above says what an untrusted project *means*; this
            // one says what it already cost, so it is not dimmed — it is the
            // louder, evidence-bearing version of the same fact, and it is
            // shown whether or not trust is "relevant" here, because a recorded
            // refusal is not a judgement about relevance. `agentstack trust .`
            // is deliberately the same command the Next line prints: the JSON
            // `fix` carries the resolved path for a caller working elsewhere,
            // the screen keeps the phrasing the human is about to read again.
            if let Some(pending) = &f.needs_your_yes {
                println!(
                    "            needs your yes: {} refused since you last said yes · review with `agentstack trust .`",
                    super::count(pending.refused, "call")
                );
            }

            // The Mode row used to sit here. It retired with STRATEGY.md v3
            // (TODO.md item 9): Mode named a choice — static, clean-at-rest,
            // zero-files — that the user no longer makes, so printing it
            // taught a concept the product had deleted, and it said what the
            // project IS where Delivery below says what actually HAPPENED.
            // `Mode` survives as an internal reading (it still decides the
            // clean-at-rest next step, and `doctor --json` still carries it
            // under `doctor-mode-v1`); it is only no longer a thing this
            // screen asks a person to hold in their head.

            // W4: the planner routes silently, so this is where a person finds
            // out what it decided — per CLI, both lanes, in plain language.
            // The two binding honesty rules follow it on their own lines: never
            // a bare "0 files", and a separate `rendered lane:` naming what is
            // really written.
            for (i, line) in f.delivery.iter().enumerate() {
                let label = if i == 0 { "Delivery" } else { "        " };
                println!("  {}  {}", label.bold(), line);
            }
            if !f.delivery.is_empty() {
                if f.delivery_has_live && !f.delivery_bridge_gaps.is_empty() {
                    println!(
                        "            {}",
                        crate::commands::delivery::CONNECT_THE_BRIDGE
                    );
                }
                if f.delivery_has_live {
                    // Disk-checked: the "0 project artifacts" wording is only
                    // true when the walk found nothing an earlier render left.
                    println!(
                        "            {}",
                        crate::commands::apply::live_lane_artifacts_line(&f.delivery_abandoned)
                            .dimmed()
                    );
                }
                if let Some(lane) = &f.delivery_rendered_lane {
                    println!("            {}", lane.dimmed());
                }
                // Directly under the zero-artifacts sentence, which is true of
                // the routing and false of the machine while one of these
                // exists. Not dimmed: a config file the harness reads that
                // AgentStack no longer maintains is not a footnote.
                for found in &f.delivery_abandoned {
                    println!("            {}  {}", "⚠".yellow(), found.sentence());
                    // The remedy comes from the find, exactly as `apply` and
                    // `why` print it: "remove it: x unrender --write" only for
                    // a file the ledger records as ours. `status` used to
                    // promise that removal for every find, including files
                    // AgentStack never wrote, where the command answers
                    // "nothing in 1 file is ours to remove".
                    println!("            {}  {}", "→".cyan(), found.remedy());
                }
            }

            // W4 precondition 6 — the gateway is registered somewhere and
            // cannot run. Not dimmed: nothing this project declares reaches
            // that harness, and AgentStack deliberately does NOT render files
            // to cover for it (a static fallback is always an explicit user
            // action). One sentence, naming the one recovery command.
            for outage in &f.gateway_outages {
                println!("  {}  {}", "Gateway ".bold(), outage.sentence());
            }

            // Shadowed library names. Printed only when one exists — a name
            // resolving to a copy the user did not mean is the one thing the
            // precedence rule must never do quietly, and silence here is what
            // "hidden" would look like.
            for line in &f.shadowed_names {
                println!("  {}  {}", "Library ".bold(), line);
            }

            // The house-rules honesty matrix. Printed only when this project
            // actually has house rules — an orientation screen stays four ideas
            // wide until there is something to say — but then it names EVERY
            // targeted harness, including the ones that cannot receive them.
            if f.instructions > 0 {
                for (i, row) in f.instruction_channels.iter().enumerate() {
                    let label = if i == 0 { "House   " } else { "        " };
                    println!("  {}  {}", label.bold(), row.sentence());
                }
            }

            if status {
                print_context_line(&f.context);
                print_updates_line(&f.updates);
                if let Some(secrets) = &f.secrets {
                    print_secrets_line(secrets);
                }
            }
        }
    }

    println!(
        "\n  {}  {}   {}",
        "Next:".bold(),
        o.next.0.green(),
        o.next.1.dimmed()
    );
    println!("  {}", "All commands: agentstack --help".dimmed());
    // The deep-check pointer is redundant when `doctor` is already the one next
    // step: printing the same command twice, described two different ways, made
    // it look like two different things and undercut the single-next-step rule.
    if status && !o.next.0.starts_with("agentstack doctor") {
        println!(
            "  {}",
            "Deep check (drift, quirks, supply chain): agentstack doctor".dimmed()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // P4 witness: mode is derived from observable signals, with the priority
    // the doc lays out. Rendered artifacts always read as static (even if the
    // gateway is also connected); a trust-gated gateway with nothing rendered is
    // zero-files; a lockfile alone is clean-at-rest; a bare project defaults to
    // static.
    #[test]
    fn mode_derivation_follows_signal_priority() {
        // rendered wins over everything, including a connected+trusted gateway.
        assert_eq!(mode_from_signals(true, true, true, true), Mode::Static);
        assert_eq!(mode_from_signals(true, false, false, false), Mode::Static);
        // zero-files: gateway registered AND trusted, nothing rendered.
        assert_eq!(mode_from_signals(false, true, true, true), Mode::ZeroFiles);
        // connected but not trusted is not yet zero-files; falls to clean-at-rest
        // when locked.
        assert_eq!(
            mode_from_signals(false, true, false, true),
            Mode::CleanAtRest
        );
        // locked, nothing rendered, no gateway → clean-at-rest.
        assert_eq!(
            mode_from_signals(false, false, false, true),
            Mode::CleanAtRest
        );
        // bare, never activated → the default (static).
        assert_eq!(mode_from_signals(false, false, false, false), Mode::Static);
    }

    #[test]
    fn clean_at_rest_next_step_teaches_the_session_rhythm() {
        use crate::trust::TrustState;

        let one = [String::from("dev")];

        let start =
            clean_at_rest_next_step(Mode::CleanAtRest, TrustState::Trusted, true, false, &one)
                .expect("trusted clean-at-rest starts a session");
        assert_eq!(start.0, "agentstack x session start dev");

        let end = clean_at_rest_next_step(Mode::CleanAtRest, TrustState::Trusted, true, true, &one)
            .expect("active clean-at-rest session points at its close");
        assert_eq!(end.0, "agentstack x session end");

        assert!(
            clean_at_rest_next_step(Mode::Static, TrustState::Trusted, true, false, &one).is_none()
        );
    }

    /// **G28**, the clean-at-rest rung — the one of the three where a concrete
    /// command sometimes DOES exist, so "null by design" has to be earned per
    /// state rather than declared once.
    ///
    /// Before this, 0 and 2+ toolsets both printed the literal
    /// `agentstack x session start <toolset>`, and `machine_command` dropped it
    /// for the brackets, so a panel got `null` with nothing saying why. The
    /// zero case was the worse of the two: `session start` takes `<TOOLSET>` as
    /// a REQUIRED positional (measured — it exits 2 without one), so the
    /// sentence named a command that state refuses outright.
    ///
    /// The sentence and the machine field are asserted together, per state.
    #[test]
    fn the_session_rung_names_a_toolset_only_when_the_manifest_settles_which() {
        use crate::trust::TrustState;

        let step = |names: &[&str]| {
            let owned: Vec<String> = names.iter().map(|s| (*s).to_string()).collect();
            clean_at_rest_next_step(Mode::CleanAtRest, TrustState::Trusted, true, false, &owned)
                .expect("trusted, locked, clean-at-rest, no session")
        };

        // Exactly one: nothing to choose, so a driver gets a real command.
        let (sentence, _) = step(&["dev"]);
        assert_eq!(sentence, "agentstack x session start dev");
        assert!(
            machine_command(&sentence).is_some(),
            "the one state with a concrete answer must reach the machine field"
        );

        // Two or more: the pick is the user's, and the options are named so it
        // is one glance rather than a trip to the manifest.
        let (sentence, why) = step(&["dev", "prod"]);
        assert!(
            sentence.contains("dev") && sentence.contains("prod"),
            "name the real choices: {sentence}"
        );
        assert!(
            machine_command(&sentence).is_none(),
            "picking for the user would start the wrong toolset: {sentence}"
        );
        assert!(
            why.contains("agentstack x session start <toolset>"),
            "the shape stays where a human reads it: {why}"
        );

        // None: `session start` cannot run at all here.
        let (sentence, _) = step(&[]);
        assert!(
            !sentence.starts_with("agentstack x session start"),
            "no toolset exists for it to load — naming it is a command that \
             refuses: {sentence}"
        );
        assert!(
            machine_command(&sentence).is_none(),
            "null by design: {sentence}"
        );
    }

    /// **G29**, as a property of the rewrite itself: every rung that names the
    /// review becomes the pin while the project is unlocked, and nothing else
    /// is touched.
    #[test]
    fn the_lock_rung_replaces_only_the_review_and_only_while_unlocked() {
        let review = || ("agentstack trust .".to_string(), "review it");

        // Unlocked, with something to pin: the review is one rung too early.
        assert_eq!(
            correct_trust_rung(review(), false, true).0,
            LOCK_RUNG_FIX,
            "a grant made before the pins exist is void once `use --write` writes them"
        );
        // Locked: the review is exactly right, and must survive untouched.
        assert_eq!(
            correct_trust_rung(review(), true, true).0,
            "agentstack trust ."
        );
        // Unlocked, but locking would write nothing (a hooks-only manifest —
        // measured: exit 0, no `agentstack.lock`). Naming the pin here is a
        // rung that can never be satisfied.
        assert_eq!(
            correct_trust_rung(review(), false, false).0,
            "agentstack trust .",
            "the rung must terminate, or it is the dead end it was written to remove"
        );
        // Every other rung passes through. `correct_trust_rung` is applied to
        // the ANSWER on both surfaces, so a greedy rewrite here would silently
        // eat the drift, bridge and apply rungs.
        for other in [
            "agentstack lock --write",
            "agentstack apply --write",
            "agentstack yes",
            "agentstack x gateway connect --all --write",
            "find a server or skill to add — only you know what this project needs",
        ] {
            assert_eq!(
                correct_trust_rung((other.to_string(), "why"), false, true).0,
                other,
                "only the review rung moves"
            );
        }
    }

    /// The predicate behind that rung, stated over manifests: it must be true
    /// exactly where `lock --write` leaves a lockfile behind. Hooks are the
    /// trap — they make `declares_capabilities` true and pin nothing.
    #[test]
    fn lock_pins_something_matches_what_lock_actually_writes() {
        let parse = |s: &str| toml::from_str::<agentstack_core::manifest::Manifest>(s).unwrap();

        assert!(!lock_pins_something(&parse("version = 1\n")));
        assert!(!lock_pins_something(&parse(
            "version = 1\n[hooks.pre]\nevent = \"PreToolUse\"\ncommand = \"/bin/echo\"\n"
        )));
        // …and `declares_capabilities` disagrees on exactly that manifest,
        // which is why the two readings cannot be collapsed into one.
        assert!(declares_capabilities(&parse(
            "version = 1\n[hooks.pre]\nevent = \"PreToolUse\"\ncommand = \"/bin/echo\"\n"
        )));

        for pins in [
            "version = 1\n[servers.demo]\ntype = \"stdio\"\ncommand = \"/bin/echo\"\n",
            "version = 1\n[skills.s]\npath = \"./s\"\n",
            "version = 1\n[instructions.hr]\npath = \"./hr.md\"\n",
            "version = 1\n[settings.claude-code]\npermissions = { allow = [] }\n",
        ] {
            assert!(
                lock_pins_something(&parse(pins)),
                "lock writes a lockfile for this manifest: {pins}"
            );
        }
    }

    // P16 witness (refined): trust is the headline next-step only when trusting
    // buys something here — a bridge is registered, or the mode depends on the
    // trust gate (`trust_relevant`). When it does, an untrusted or trust-stale
    // manifest routes to `trust .` ahead of `init`/`doctor` and teaches what the
    // state means. When it does not (a static, no-gateway project, whose SHAPE
    // trusting does not change), the trust route is NOT the headline: the next
    // step falls through to the normal ladder, and the "inert servers" note is
    // withheld — because that sentence names a gateway this project has not
    // got — leaving only the true Status label. Its writes are still refused;
    // `the_trust_relevance_truth_table` below is where that is pinned.
    #[test]
    fn untrusted_orientation_teaches_and_routes_to_trust() {
        use crate::trust::TrustState;

        // The one-line note appears for untrusted AND trust-stale, and explains
        // the *consequence* (inert servers), not just the label. (Its caller
        // gates it on trust relevance; the sentence itself is unchanged.)
        for st in [TrustState::Untrusted, TrustState::Changed] {
            let note = orientation_trust_note(st).expect("untrusted states teach");
            assert!(note.contains("inert"), "explains the consequence: {note}");
            assert!(
                note.contains("control-plane tools only"),
                "names the reduced surface: {note}"
            );
        }
        // A trusted manifest has nothing to teach here.
        assert_eq!(orientation_trust_note(TrustState::Trusted), None);

        // Trust-relevant (bridge registered / gate-dependent mode): untrusted
        // and stale both send the human to `trust .`, whatever is rendered.
        assert_eq!(
            next_step(
                TrustState::Untrusted,
                false,
                true,
                true,
                false,
                false,
                false,
                true
            )
            .0,
            "agentstack trust ."
        );
        assert_eq!(
            next_step(
                TrustState::Untrusted,
                true,
                false,
                true,
                false,
                false,
                false,
                true
            )
            .0,
            "agentstack trust ."
        );
        assert_eq!(
            next_step(
                TrustState::Changed,
                true,
                true,
                true,
                false,
                false,
                false,
                true
            )
            .0,
            "agentstack trust ."
        );

        // Static, no-gateway (trust irrelevant), and DECLARING something: the
        // gate rung answers, because `apply --write` here is refused outright
        // (G27). This used to read `agentstack apply --write` — the command the
        // state cannot run. The never-converging trust nag it was written
        // against is still fixed: this rung is guarded on declared content, and
        // it converges, because granting turns it off.
        assert_eq!(
            next_step(
                TrustState::Untrusted,
                false,
                true,
                false,
                false,
                false,
                false,
                true
            )
            .0,
            "agentstack trust ."
        );
        // Declaring NOTHING → the Empty rung: there is nothing to render,
        // group, or verify, and nothing for the gate to hold either. The
        // expectation used to be the literal `agentstack search <query>`; that
        // was a shape, and a shape read as a runnable command on screen while
        // `machine_command` dropped it. The rung is now prose, with the shapes
        // taught in the `why` and a `null` machine field.
        assert_eq!(
            next_step(
                TrustState::Untrusted,
                true,
                false,
                false,
                false,
                false,
                false,
                true
            )
            .0,
            EMPTY_PROJECT_NEXT.0
        );

        // Trust-STALE is different, and routes to the review whatever the
        // relevance flag says (v0.17.1): content the user already approved has
        // changed, `status` is already reporting it, and sending them to
        // `doctor` first made the cue cost two commands instead of one.
        assert_eq!(
            next_step(
                TrustState::Changed,
                true,
                true,
                false,
                false,
                false,
                false,
                true
            )
            .0,
            "agentstack trust ."
        );
        assert_eq!(
            next_step(
                TrustState::Changed,
                false,
                false,
                false,
                false,
                false,
                false,
                true
            )
            .0,
            "agentstack trust ."
        );

        // The wiring is done and nothing is grouped yet → the Group rung.
        // `doctor` here was a dead end: a user who ran it clean was offered it
        // again (pilot Run A). The expectation used to be the literal
        // `agentstack toolset create <name> --server <server>`; a toolset's name
        // is the one argument nothing on disk can supply, so written as a
        // command it was a shape `machine_command` then dropped (G28). The shape
        // survives in the `why`, asserted below.
        let (cmd, why) = next_step(
            TrustState::Trusted,
            true,
            true,
            false,
            true,
            false,
            false,
            true,
        );
        assert_eq!(
            cmd,
            "name a toolset to group these servers — the name is yours to choose"
        );
        assert!(
            why.contains("agentstack toolset create <name> --server <server>"),
            "the fillable shape stays where a human reads it: {why}"
        );

        // Servers configured here that the manifest doesn't cover outrank both
        // — rendering a manifest that omits half the setup is not the step
        // that helps.
        assert_eq!(
            next_step(
                TrustState::Trusted,
                false,
                true,
                false,
                false,
                true,
                false,
                true
            )
            .0,
            "agentstack adopt"
        );

        // Once trusted the trust-relevance flag is moot: the render vs. verify
        // ladder applies either way.
        for relevant in [true, false] {
            assert_eq!(
                next_step(
                    TrustState::Trusted,
                    false,
                    true,
                    relevant,
                    false,
                    false,
                    false,
                    true
                )
                .0,
                "agentstack apply --write"
            );
            assert_eq!(
                next_step(
                    TrustState::Trusted,
                    true,
                    false,
                    relevant,
                    false,
                    false,
                    false,
                    true
                )
                .0,
                EMPTY_PROJECT_NEXT.0
            );
            assert_eq!(
                next_step(
                    TrustState::Trusted,
                    false,
                    false,
                    relevant,
                    false,
                    false,
                    false,
                    true
                )
                .0,
                EMPTY_PROJECT_NEXT.0
            );
        }
    }

    /// The Empty rung is prose on screen and `null` for a driver, and every
    /// command its `why` teaches is either a shape or one that runs.
    ///
    /// This used to assert the opposite — that the field stayed a runnable
    /// command — on the belief that a `null` here was a `status-v1` schema
    /// change. It is not: the Group and Verified rungs already answer `null`
    /// from this field, so a consumer handles it or was already broken on the
    /// commonest healthy projects. What the old mapping did produce was a loop:
    /// `agentstack search` is a read-only browse that changes nothing, so a
    /// driver ran it and was handed it again forever.
    ///
    /// So the property flipped, and the guard has to be stricter than "is it
    /// null", or nulling everything would satisfy it. It pins the REASON: the
    /// sentence carries no shape (it is not pretending to be a command), and
    /// every backticked command in the `why` either carries a placeholder — a
    /// shape, which is right for a human and filtered from the machine field —
    /// or survives `machine_command`, which is what "it runs" looks like here.
    #[test]
    fn the_empty_rung_is_prose_on_screen_and_null_for_a_driver() {
        assert!(
            !EMPTY_PROJECT_NEXT.0.contains('<'),
            "the sentence must not look like a command it is not: {}",
            EMPTY_PROJECT_NEXT.0
        );
        assert_eq!(
            machine_command(EMPTY_PROJECT_NEXT.0),
            None,
            "a read-only browse is not a next ACTION — a driver that runs it is \
             handed it again forever"
        );
        // The `why` still has to be honest, or "null" just moved the dead end
        // one field over: each command it teaches is a shape, or it runs.
        for cmd in EMPTY_PROJECT_NEXT
            .1
            .split('`')
            .skip(1)
            .step_by(2)
            .filter(|s| s.starts_with("agentstack "))
        {
            assert!(
                cmd.contains('<') || machine_command(cmd).is_some(),
                "`{cmd}` is offered as neither a shape nor a runnable command"
            );
        }
    }

    /// F9 witness (FINDINGS.md, rc.1 review): a dropped-but-undeclared file
    /// must route the one next action to `agentstack yes` — the funnel's
    /// activation verb — not to `adopt` or `trust .`. Before this, `yes`
    /// appeared on no detection surface at all: a participant who dropped a
    /// file was told to `trust .` a project with zero servers, and the funnel
    /// the study exists to measure was unreachable. The one state that still
    /// outranks a drop is trust-stale: content the user already approved has
    /// changed, and that re-review keeps the headline.
    #[test]
    fn a_waiting_drop_routes_to_yes() {
        use crate::trust::TrustState;
        // Every non-stale combination of the other signals: the drop wins —
        // including over unimported native servers and the trust unlock.
        for trust in [TrustState::Trusted, TrustState::Untrusted] {
            for rendered in [true, false] {
                for has_capabilities in [true, false] {
                    for trust_relevant in [true, false] {
                        for unimported in [true, false] {
                            let (cmd, _) = next_step(
                                trust,
                                rendered,
                                has_capabilities,
                                trust_relevant,
                                false,
                                unimported,
                                true,
                                true,
                            );
                            assert_eq!(
                                cmd, "agentstack yes",
                                "a waiting drop must route to the funnel \
                                 (trust={trust:?} rendered={rendered} caps={has_capabilities} \
                                 relevant={trust_relevant} unimported={unimported})"
                            );
                        }
                    }
                }
            }
        }
        // Trust-stale keeps the headline; the drop is offered after re-review.
        assert_eq!(
            next_step(
                TrustState::Changed,
                true,
                true,
                false,
                false,
                false,
                true,
                true
            )
            .0,
            "agentstack trust ."
        );
    }

    /// F02 regression: the recommended next step must never be a command that
    /// refuses. `init` errors out once a manifest exists ("init has nothing
    /// left to do here"), and `next_step` only runs when one has loaded — so
    /// no combination of its inputs may produce it. Before this, a normal
    /// finished setup (imported, applied, never `use`d) recommended exactly
    /// that, and following the advice printed an error.
    #[test]
    fn next_step_never_recommends_a_command_that_refuses() {
        use crate::trust::TrustState;
        for trust in [
            TrustState::Trusted,
            TrustState::Untrusted,
            TrustState::Changed,
        ] {
            for rendered in [true, false] {
                for has_capabilities in [true, false] {
                    for trust_relevant in [true, false] {
                        for drops in [true, false] {
                            let (cmd, why) = next_step(
                                trust,
                                rendered,
                                has_capabilities,
                                trust_relevant,
                                false,
                                false,
                                drops,
                                true,
                            );
                            assert!(
                                !cmd.contains("init"),
                                "a loaded manifest must never be sent back to init \
                                 (trust={trust:?} rendered={rendered} caps={has_capabilities} \
                                 relevant={trust_relevant} drops={drops}) → {cmd} / {why}"
                            );
                        }
                    }
                }
            }
        }

        // And the specific shape that used to break: a project holding
        // capabilities that are already on disk is set up — verify it, don't
        // re-import it. Stated over the TRUSTED project, because the untrusted
        // one now stops one rung earlier at the gate (G27) and never reaches
        // the verify rung at all.
        assert_eq!(
            next_step(
                TrustState::Trusted,
                true,
                true,
                false,
                false,
                false,
                false,
                true
            )
            .0,
            "agentstack doctor"
        );
        assert_eq!(
            next_step(
                TrustState::Untrusted,
                true,
                true,
                false,
                false,
                false,
                false,
                true
            )
            .0,
            "agentstack trust .",
            "the gate refuses every write this rung's commands would make"
        );
    }

    // P18(a) witness: orientation names profiles rather than counting them, one
    // line for a small set, truncated beyond four, with the active one marked.
    #[test]
    fn profiles_line_names_and_marks_active() {
        let two = vec!["dev".to_string(), "prod".to_string()];
        assert_eq!(profiles_line(&two, None), "dev, prod");
        assert_eq!(profiles_line(&two, Some("dev")), "dev (active), prod");

        // Exactly four still lists every name.
        let four: Vec<String> = ["a", "b", "c", "d"].iter().map(|s| s.to_string()).collect();
        assert_eq!(profiles_line(&four, None), "a, b, c, d");

        // Beyond four truncates to the count plus the first three names, and the
        // active marker still shows when it falls inside that window.
        let five: Vec<String> = ["a", "b", "c", "d", "e"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(profiles_line(&five, None), "5 toolsets: a, b, c, …");
        assert_eq!(
            profiles_line(&five, Some("b")),
            "5 toolsets: a, b (active), c, …"
        );
    }

    // Stage 2.2: the Session status line humanizes the session's age.
    #[test]
    fn session_age_buckets() {
        assert_eq!(session_age(5), "started just now");
        assert_eq!(session_age(240), "started 4m ago");
        assert_eq!(session_age(3900), "started 1h 5m ago");
    }

    fn loaded_orientation(secrets: Option<SecretFacts>) -> Orientation {
        Orientation {
            detected_clis: vec!["Claude Code".into()],
            catalog_size: 13,
            native: Vec::new(),
            intake: Vec::new(),
            manifest_path: std::path::PathBuf::from("/repo/.agentstack/agentstack.toml"),
            manifest: ManifestState::Loaded(Box::new(ProjectFacts {
                servers: 2,
                instructions: 0,
                settings: 0,
                hooks: 0,
                extensions: 0,
                packages: Vec::new(),
                skills: 1,
                pinned_targets: vec!["claude-code".into()],
                fanout_targets: 1,
                fanout_detected: true,
                toolsets: vec!["dev".into(), "prod".into()],
                session: Some(SessionFacts {
                    profile: "dev".into(),
                    started_unix: 1_700_000_000,
                    age_secs: 240,
                    abandoned: false,
                }),
                locked: true,
                trust: crate::trust::TrustState::Changed,
                content_drift: Vec::new(),
                surface_unpinned: Vec::new(),
                trust_relevant: true,
                trust_blocks_delivery: true,
                mode: Mode::CleanAtRest,
                gateway_connected: false,
                gateway_outages: Vec::new(),
                shadowed_names: Vec::new(),
                instruction_channels: Vec::new(),
                delivery: Vec::new(),
                delivery_rendered_lane: None,
                delivery_has_live: false,
                delivery_bridge_gaps: Vec::new(),
                delivery_abandoned: Vec::new(),
                rendered: false,
                secrets,
                needs_your_yes: None,
                updates: Vec::new(),
                context: ContextCost::default(),
            })),
            next: ("agentstack trust .".into(), "review and re-trust"),
        }
    }

    /// The two trust readings, pinned as a table rather than described.
    ///
    /// Measured against the shipped binary before it was written, one project
    /// per row, each with a declared server and a declared instruction
    /// fragment, and each row's `apply --write` outcome observed:
    ///
    /// ```text
    ///  mode           trust      gw  | trust_relevant | blocks | apply --write
    ///  static         untrusted  no  | false          | true   | REFUSED
    ///  static         drifted    no  | false          | true   | REFUSED
    ///  static         trusted    no  | false          | false  | wrote
    ///  clean-at-rest  untrusted  no  | true           | true   | REFUSED
    ///  clean-at-rest  drifted    no  | true           | true   | REFUSED
    ///  clean-at-rest  trusted    no  | true           | false  | wrote
    ///  static         untrusted  yes | true           | true   | REFUSED
    ///  zero-files     trusted    yes | true           | false  | wrote
    /// ```
    ///
    /// Three properties this exists to hold still.
    ///
    /// 1. **`trust_relevant` is false in states where trust blocks
    ///    everything.** The three `static` rows are not a defect in the value —
    ///    they are what a delivery-posture hint correctly says — but they are
    ///    exactly why no consumer may read `false` as "trust is not an issue
    ///    here". The doc comments that once claimed such a project "renders
    ///    whatever the trust state" were wrong, and this row set is what
    ///    stops them coming back.
    /// 2. **`trust_blocks_delivery` tracks the gate and nothing else.** It is
    ///    the same answer in all three modes, which is the point: mode is not
    ///    an input.
    /// 3. **The mode axis is not independent of the trust axis.**
    ///    `zero-files` requires `trusted` in [`mode_from_signals`], so
    ///    `zero-files × untrusted` is unreachable — a "for each mode, for each
    ///    trust state" grid has holes, and a consumer must not infer one from
    ///    the other.
    #[test]
    fn the_trust_relevance_truth_table() {
        use crate::trust::TrustState;
        const MODES: [Mode; 3] = [Mode::Static, Mode::CleanAtRest, Mode::ZeroFiles];
        const STATES: [TrustState; 3] = [
            TrustState::Trusted,
            TrustState::Untrusted,
            TrustState::Changed,
        ];

        // ── `trust_relevant`: a posture reading. Blind to trust, sensitive to
        // the bridge and to the two gate-derived modes.
        for mode in MODES {
            for gateway in [true, false] {
                let expected = gateway || mode != Mode::Static;
                assert_eq!(
                    trust_relevant(gateway, mode),
                    expected,
                    "trust_relevant({gateway}, {mode:?})"
                );
            }
        }
        // Stated once more as the literal rows, so a formula change that keeps
        // the shape but moves an answer still breaks here.
        assert!(!trust_relevant(false, Mode::Static));
        assert!(trust_relevant(true, Mode::Static));
        assert!(trust_relevant(false, Mode::CleanAtRest));
        assert!(trust_relevant(false, Mode::ZeroFiles));

        // ── `trust_blocks_delivery`: a gate reading. Sensitive to trust and to
        // whether there is anything to block; blind to mode and to the bridge.
        for trust in STATES {
            let blocked = trust != TrustState::Trusted;
            assert_eq!(
                trust_blocks_delivery(trust, true),
                blocked,
                "declared content is blocked by every non-trusted state ({trust:?})"
            );
            assert!(
                !trust_blocks_delivery(trust, false),
                "a project declaring nothing has nothing for the gate to block ({trust:?})"
            );
        }

        // ── The pairing that motivated the field: in every `static` row the
        // posture hint says false while the gate says true. This is the
        // sentence "an untrusted static project reports trust_relevant: false
        // while apply --write refuses its servers", as an assertion.
        for trust in [TrustState::Untrusted, TrustState::Changed] {
            assert!(
                !trust_relevant(false, Mode::Static),
                "the posture hint stays false for a static, no-gateway project"
            );
            assert!(
                trust_blocks_delivery(trust, true),
                "...while the gate blocks every declared kind ({trust:?})"
            );
        }

        // ── Mode is derived, so the grid has holes. `zero-files` is only ever
        // reached with trust granted; nothing may read a mode as a trust state.
        for trusted in [true, false] {
            let mode = mode_from_signals(false, true, trusted, true);
            if trusted {
                assert_eq!(mode, Mode::ZeroFiles);
            } else {
                assert_ne!(
                    mode,
                    Mode::ZeroFiles,
                    "zero-files x untrusted is unreachable: the bridge serves \
                     control-plane tools only until the digest is pinned"
                );
            }
        }
    }

    /// `context-cost-v1` rule 3, at the serializer: no data is not zero.
    ///
    /// A project with nothing measurable carries no `context_cost` key at all
    /// — absence is the only reading that cannot be rendered as "this project
    /// is free". A project whose servers were never measured DOES carry the
    /// key, with the count of unmeasured servers and a total that excludes
    /// them, because "unknown" and "zero" are different answers.
    #[test]
    fn context_cost_is_absent_rather_than_zero_and_never_counts_the_unmeasured() {
        let silent = status_json(&loaded_orientation(None));
        assert!(
            silent["project"].get("context_cost").is_none(),
            "nothing to measure must print no key: {silent}"
        );

        let mut o = loaded_orientation(None);
        if let ManifestState::Loaded(f) = &mut o.manifest {
            f.context = ContextCost {
                servers: vec![("github".into(), 9_100)],
                servers_unmeasured: 2,
                skills: (12, 3_000),
                house_rules: (1, 2_100),
            };
        }
        let out = status_json(&o);
        let c = &out["project"]["context_cost"];
        assert_eq!(c["estimate"], true, "never presented as measured");
        assert_eq!(c["total_est_tokens"], 14_200, "measured parts only");
        assert_eq!(c["servers_unmeasured"], 2);
        assert_eq!(c["servers"].as_array().unwrap().len(), 1);
        assert_eq!(c["servers"][0]["name"], "github");
        // The ladder is untouched: context cost is information, never a rung.
        assert_eq!(out["next_action"]["command"], "agentstack trust .");
    }

    /// Rule 4: quiet when boring. One contributor gets the headline only; the
    /// breakdown appears when there is something to compare, largest first.
    #[test]
    fn context_rows_stay_quiet_for_one_contributor_and_rank_by_cost() {
        let one = ContextCost {
            skills: (3, 400),
            ..ContextCost::default()
        };
        assert_eq!(one.rows().len(), 1, "one row is not a breakdown");
        assert_eq!(one.total(), 400);

        let many = ContextCost {
            servers: vec![("github".into(), 9_100)],
            servers_unmeasured: 0,
            skills: (12, 3_000),
            house_rules: (2, 2_100),
        };
        let labels: Vec<String> = many.rows().into_iter().map(|(_, l)| l).collect();
        assert_eq!(
            labels,
            vec![
                "github".to_string(),
                "12 skill descriptions".to_string(),
                "2 house rules".to_string(),
            ]
        );
    }

    /// `needs-your-yes-v1`, at the serializer: the key appears only when
    /// something was actually refused, and it carries the fix — never a card.
    /// A project with nothing refused must read byte-for-byte as it did before
    /// the field existed, which is why absence (not `null`) is the contract.
    #[test]
    fn needs_your_yes_appears_only_with_evidence_and_carries_no_card() {
        let clean = status_json(&loaded_orientation(None));
        assert!(
            clean["project"].get("needs_your_yes").is_none(),
            "a project with no recorded refusal must not carry the key: {clean}"
        );

        let mut o = loaded_orientation(None);
        if let ManifestState::Loaded(f) = &mut o.manifest {
            f.needs_your_yes = Some(NeedsYourYes {
                refused: 3,
                last_refused_ts: 1_700_000_100,
                fix: "agentstack trust /repo".to_string(),
            });
        }
        let out = status_json(&o);
        let pending = &out["project"]["needs_your_yes"];
        assert_eq!(pending["refused"], 3);
        assert_eq!(pending["last_refused_ts"], 1_700_000_100u64);
        assert_eq!(pending["fix"], "agentstack trust /repo");
        // The card stays behind `agentstack trust` — one walk, one renderer.
        // Anything resembling a reviewable surface here would be a second one.
        for absent in ["items", "review", "servers", "skills", "surface_digest"] {
            assert!(
                pending.get(absent).is_none(),
                "status must not carry card payload ({absent}): {pending}"
            );
        }
    }

    /// `json-reads-v1`: `status --json` is the orientation screen's own
    /// reading, keyed. `project` carries the per-project facts; `trust` uses
    /// the same vocabulary `use --list --json` does, so a UI holding both
    /// reads needs one lookup table, not two.
    #[test]
    fn status_json_carries_the_orientation_reading() {
        let out = status_json(&loaded_orientation(Some(SecretFacts {
            referenced: 2,
            unresolved: vec!["NOTION_TOKEN".into()],
        })));
        assert_eq!(out["manifest"]["present"], true);
        assert_eq!(out["manifest"]["loaded"], true);
        assert_eq!(out["manifest"]["error"], serde_json::Value::Null);
        let p = &out["project"];
        assert_eq!(p["servers"], 2);
        assert_eq!(p["skills"], 1);
        assert_eq!(p["targets"]["pinned"][0], "claude-code");
        assert_eq!(p["targets"]["fanout"], 1);
        assert_eq!(p["toolsets"][1], "prod");
        assert_eq!(p["session"]["profile"], "dev");
        assert_eq!(p["session"]["abandoned"], false);
        assert_eq!(p["locked"], true);
        assert_eq!(p["trust"], "drifted");
        assert_eq!(p["trust_relevant"], true);
        assert_eq!(p["trust_blocks_delivery"], true);
        assert_eq!(p["mode"], "clean-at-rest");
        assert_eq!(p["secrets"]["referenced"], 2);
        assert_eq!(p["secrets"]["unresolved"][0], "NOTION_TOKEN");
        assert_eq!(out["next_action"]["command"], "agentstack trust .");
    }

    /// The two readings that have no project: `project` is `null`, and a
    /// manifest that exists but will not load says why. A consumer branches on
    /// one field instead of probing a dozen for null.
    #[test]
    fn status_json_distinguishes_missing_from_broken() {
        let missing = status_json(&Orientation {
            detected_clis: Vec::new(),
            catalog_size: 13,
            native: Vec::new(),
            intake: Vec::new(),
            manifest_path: std::path::PathBuf::from("/repo/.agentstack/agentstack.toml"),
            manifest: ManifestState::Missing,
            next: ("agentstack init".into(), "guided one-command setup"),
        });
        assert_eq!(missing["manifest"]["present"], false);
        assert_eq!(missing["manifest"]["error"], serde_json::Value::Null);
        assert_eq!(missing["project"], serde_json::Value::Null);

        let broken = status_json(&Orientation {
            detected_clis: Vec::new(),
            catalog_size: 13,
            native: Vec::new(),
            intake: Vec::new(),
            manifest_path: std::path::PathBuf::from("/repo/.agentstack/agentstack.toml"),
            manifest: ManifestState::Broken("missing field `type`\nin `servers.a`".into()),
            next: ("agentstack doctor".into(), "diagnose the manifest"),
        });
        assert_eq!(broken["manifest"]["present"], true);
        assert_eq!(broken["manifest"]["loaded"], false);
        // Sanitized to one line — the screen may wrap, a JSON string may not
        // smuggle control bytes into a consumer's UI (rule 7).
        assert_eq!(
            broken["manifest"]["error"],
            "missing field `type` in `servers.a`"
        );
        assert_eq!(broken["project"], serde_json::Value::Null);
    }

    /// Invariant 5 witness for this payload: the secrets reading carries
    /// COUNTS and unresolved NAMES. A resolved secret contributes to the count
    /// and nothing else — its name is not even listed, let alone its value.
    #[test]
    fn status_json_secrets_never_carry_a_value() {
        let all_resolve = status_json(&loaded_orientation(Some(SecretFacts {
            referenced: 2,
            unresolved: Vec::new(),
        })));
        let secrets = &all_resolve["project"]["secrets"];
        assert_eq!(secrets["referenced"], 2);
        assert_eq!(secrets["unresolved"].as_array().unwrap().len(), 0);
        assert_eq!(secrets.as_object().unwrap().len(), 2, "two keys, no value");

        // Not asked for (bare `agentstack`) is a third state, distinct from
        // "asked, everything resolves".
        let unasked = status_json(&loaded_orientation(None));
        assert_eq!(unasked["project"]["secrets"], serde_json::Value::Null);
    }

    /// `update-offer-v1`, both directions. The offer carries the three fields
    /// and the SHIPPED command; no offer means the key is absent entirely —
    /// not `null`, not `[]` — because absence must stay unreadable as
    /// "current" (the check is offline and never proves currency).
    #[test]
    fn status_json_offers_updates_and_omits_the_key_when_there_is_none() {
        let none = status_json(&loaded_orientation(None));
        assert!(
            none["project"].get("updates").is_none(),
            "no offer must not materialize the key: {}",
            none["project"]
        );

        let mut o = loaded_orientation(None);
        if let ManifestState::Loaded(f) = &mut o.manifest {
            f.updates = vec![crate::commands::updates::PackUpdate {
                name: "acme".into(),
                current: "v0.1.0".into(),
                available: "v0.2.0".into(),
            }];
        }
        let out = status_json(&o);
        let updates = &out["project"]["updates"];
        assert_eq!(updates["packs"][0]["name"], "acme");
        assert_eq!(updates["packs"][0]["current"], "v0.1.0");
        assert_eq!(updates["packs"][0]["available"], "v0.2.0");
        // `lock` previews by default, so the fix a machine reads must carry
        // `--write` — a bare `lock --upgrade` pins nothing and never converges.
        assert_eq!(updates["fix"], "agentstack lock --upgrade acme --write");
    }

    /// `package-members-v1`, at the serializer. A project selecting no package
    /// must read exactly as it did before the field existed (absence, not
    /// `[]`); a project with one must expose the EFFECTIVE set — both origins
    /// named, the removal named, the override counted, and each member's lane
    /// derived from its kind so an instruction can never be presented as
    /// something the gateway serves.
    #[test]
    fn status_json_carries_the_effective_member_set_and_omits_the_key_when_empty() {
        let none = status_json(&loaded_orientation(None));
        assert!(
            none["project"].get("packages").is_none(),
            "no package must not materialize the key: {}",
            none["project"]
        );

        use agentstack_core::digest::Sha256Hex;
        use agentstack_core::lock::{
            LockedPackage, LockedPackageMember, PackageMemberKind, PackageMemberOrigin,
        };
        let mut o = loaded_orientation(None);
        if let ManifestState::Loaded(f) = &mut o.manifest {
            f.packages = vec![LockedPackage {
                name: "rust-backend".into(),
                version: "1.4.0".into(),
                source: "library:rust-backend".into(),
                rev: None,
                toolsets: vec!["backend".into()],
                removed: vec!["legacy".into()],
                members: vec![
                    LockedPackageMember {
                        name: "house-rules".into(),
                        kind: PackageMemberKind::Instruction,
                        origin: PackageMemberOrigin::Package,
                        checksum: Sha256Hex::of(b"a"),
                        provenance: "package:rust-backend@1.4.0#instructions/house.md".into(),
                    },
                    LockedPackageMember {
                        name: "sql-review".into(),
                        kind: PackageMemberKind::Skill,
                        origin: PackageMemberOrigin::ProjectOverride,
                        checksum: Sha256Hex::of(b"b"),
                        provenance: "project:skills.house-sql-review".into(),
                    },
                ],
            }];
        }
        let out = status_json(&o);
        let pkg = &out["project"]["packages"][0];
        assert_eq!(pkg["name"], "rust-backend");
        assert_eq!(pkg["version"], "1.4.0");
        assert_eq!(pkg["toolsets"][0], "backend");
        assert_eq!(pkg["removed"][0], "legacy");
        assert_eq!(pkg["overrides"], 1);
        assert_eq!(pkg["members"][0]["kind"], "instruction");
        assert_eq!(pkg["members"][0]["lane"], "rendered");
        assert_eq!(pkg["members"][0]["origin"], "package");
        assert_eq!(pkg["members"][1]["lane"], "dynamic");
        assert_eq!(pkg["members"][1]["origin"], "project-override");
        assert_eq!(
            pkg["members"][1]["provenance"],
            "project:skills.house-sql-review"
        );
        assert_eq!(
            pkg["members"][1]["checksum"].as_str().map(str::len),
            Some(64)
        );
    }

    // Stage 2.2: a live session reads as active; an abandoned one is flagged
    // and both offer the same safe `session end` recovery.
    #[test]
    fn session_status_line_flags_abandoned_and_offers_recovery() {
        let (head, hint) = session_status_line("dev", 240, false);
        assert_eq!(head, "'dev' active temporarily (started 4m ago)");
        assert!(hint.contains("agentstack x session end"));
        assert!(!hint.contains("abandoned"));

        let (head, hint) = session_status_line("dev", 14 * 3600, true);
        assert!(head.contains("looks abandoned"), "flags it: {head}");
        assert!(head.contains("started 14h 0m ago"));
        assert!(
            hint.contains("agentstack x session end"),
            "still offers the safe recovery: {hint}"
        );
    }
}
