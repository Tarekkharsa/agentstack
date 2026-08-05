//! `agentstack apply` — render the manifest into each target's native config.
//! Shows a preview first; TTY users can confirm, and `--write` applies directly.

use std::path::Path;

use anyhow::{Context, Result};
use owo_colors::OwoColorize;

use crate::cli::ApplyArgs;
use crate::manifest::{validate_with_context, ValidateCtx};
use crate::render::instructions::plan_instructions;
use crate::render::{
    effective_servers, plan_hooks, plan_settings, plan_target_with_servers, resolve_targets,
    Selection,
};
use crate::scope::Scope;
use crate::state::{target_key, State};

/// What a render pass observed, so callers can decide whether to prompt.
pub(crate) struct Outcome {
    /// How many targets (across servers/settings/hooks) have pending changes.
    pub changed_count: usize,
    /// Structural validation errors — nothing will be written until fixed, so
    /// there is nothing to confirm.
    pub validation_errors: bool,
    /// Unresolved secrets that would block at least one write. `apply` may still
    /// let a user confirm a partial write; setup uses this to stop before any
    /// newcomer wizard write.
    pub write_blockers: usize,
    /// Targets actually written this pass (0 on any dry run) — drives the
    /// "restart your CLI" hint, which only matters once something changed on disk.
    pub written_count: usize,
    /// Whether this pass would add or update the managed `.gitignore` block.
    ///
    /// Only ever true on a dry run — a write has already made the change, so
    /// there is nothing pending. The wizard reads it to decide whether its
    /// confirm needs to offer the opt-out at all: asking about a file edit
    /// that isn't happening is exactly the kind of question progressive
    /// disclosure exists to suppress.
    pub gitignore_pending: bool,
}

pub fn run(args: &ApplyArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let requested = args.write && !args.dry_run;
    if requested {
        // `--write`: apply directly. The scripting / CI escape hatch — never
        // prompts, whatever the terminal is.
        let outcome = render(args, manifest_dir, true, false, true)?;
        // A refused write is a failed command: scripts must never read a
        // validation-blocked `apply --write` as success (exit 0 was audit
        // finding A0).
        if outcome.validation_errors {
            anyhow::bail!(
                "manifest has validation errors — nothing was written; fix the ✗ above, then re-run `agentstack apply --write`"
            );
        }
        restart_hint(&outcome);
        return Ok(());
    }
    // No `--write`. An explicit `--dry-run`, or any non-interactive shell (CI,
    // pipes, redirects), keeps the classic read-only behavior exactly: show the
    // diff and the "re-run with --write" hint, write nothing, never block on
    // input.
    if args.dry_run || !crate::util::confirm::is_interactive() {
        render(args, manifest_dir, false, false, true)?;
        return Ok(());
    }
    // Interactive default: show the dry-run diff (no re-run hint), then offer to
    // apply it in place.
    let outcome = render(args, manifest_dir, false, false, false)?;
    if outcome.validation_errors {
        // The would-be write is already off the table; say so with a nonzero
        // exit instead of silently returning success.
        anyhow::bail!("manifest has validation errors — fix the ✗ above");
    }
    if outcome.changed_count == 0 {
        return Ok(());
    }
    // Moment 5: the undo is named HERE — inside the preview, before the
    // confirm — not only in `restart_hint` after the write. A user deciding
    // whether to say yes needs to know the way back at the moment they decide,
    // not once the configs have already moved.
    crate::outln!(
        "\n{} undo: every file above is snapshotted first — `agentstack restore` puts them back.",
        "↩".dimmed()
    );
    if crate::util::confirm::confirm("\nApply these changes?")? {
        // Confirmed: a quiet second pass writes without re-printing the diff.
        let outcome = render(args, manifest_dir, true, true, true)?;
        restart_hint(&outcome);
    } else {
        crate::outln!("Not written. Re-run with {} to apply.", "--write".bold());
    }
    Ok(())
}

/// Configs changed on disk, but a harness only reads them at startup — say so.
/// Standalone `apply` only; `setup` prints its own closing "Next" block.
/// The undo pointer rides along: every write is snapshotted into history, and
/// telling the user so at the moment of mutation is the cheapest trust there is.
fn restart_hint(outcome: &Outcome) {
    if outcome.written_count > 0 {
        // written_count counts touched targets, i.e. CLIs whose config changed.
        let advice = if outcome.written_count == 1 {
            "Restart or reopen your agent CLI so it picks up the new config."
        } else {
            "Restart or reopen your agent CLIs so they pick up the new config."
        };
        crate::outln!("{} {advice}", "→".cyan());
        // Name the exact command, not the bare verb: `agentstack restore` alone
        // lists the ledger, so a user following this line got a list to read
        // rather than the undo they were promised.
        crate::outln!("  {}", "undo: agentstack x restore --last --write".dimmed());
    }
}

/// A native server config that is on disk while this project routes that
/// harness's MCP servers through the live lane: `apply` writes nothing here,
/// but the harness still reads the file at startup.
///
/// **Disk is the trigger.** The detector reads the file and lists the servers
/// it actually declares; the state ledger only decides the WORDING (did
/// AgentStack write this, or did it arrive some other way — a clone, a git
/// checkout, `x restore`, a hand edit). Binding the trigger to the ledger was
/// the defect: any route that puts the file back without the record hid a live
/// file behind a green tick, which is invariant 8 in the most ordinary team
/// scenario there is.
///
/// One exception, argued at the gate in [`abandoned_at`]: a GLOBAL config with
/// no ledger record is the user's own machine environment — the file `init`
/// imports FROM — so it is not reported. Global configs AgentStack did write
/// still are, and so is every project-scope file either way.
pub(crate) struct AbandonedRender {
    /// Human name of the harness: `Claude Code`.
    pub display: String,
    /// The config file still on disk.
    pub path: std::path::PathBuf,
    /// Server names the file ON DISK declares — read from the file, never from
    /// the ledger.
    pub servers: Vec<String>,
    /// Whether the state ledger claims this key: true → AgentStack wrote it,
    /// false → it is here and AgentStack did not write it. Wording only.
    pub recorded: bool,
    /// Names an earlier guarded write kept because ANOTHER manifest applied
    /// them. Routing never made these ours to stop reporting.
    pub foreign: Vec<String>,
}

impl AbandonedRender {
    /// The one command that takes the file back off disk. Named identically by
    /// `apply`, `doctor` and `status` — rule (e) wants one runnable answer, not
    /// three spellings of it.
    pub const REMOVE_IT: &'static str = "agentstack x unrender --write";

    /// One line a person can act on, shared by every surface.
    pub fn sentence(&self) -> String {
        let who = if self.servers.is_empty() {
            String::new()
        } else {
            format!(" (it holds {})", self.servers.join(", "))
        };
        if self.recorded {
            format!(
                "{} is still on disk{who} — AgentStack wrote it, no longer manages it, and {} may \
                 still be reading it",
                self.path.display(),
                self.display
            )
        } else {
            format!(
                "{} is on disk{who} — AgentStack did not write it, and {} may still be reading it",
                self.path.display(),
                self.display
            )
        }
    }

    /// The runnable answer for THIS file. `x unrender` removes only entries the
    /// ledger records as ours, so promising it for a file AgentStack never
    /// wrote would be a claim its enforcement cannot keep.
    pub fn remedy(&self) -> String {
        if self.recorded {
            format!("remove it: {}", Self::REMOVE_IT)
        } else {
            "review it: it is not this project's render — adopt it (agentstack adopt) or delete \
             it by hand"
                .to_string()
        }
    }

    /// The bare command for THIS file, with no prose around it.
    ///
    /// `doctor` and `status` lift the `↳` slot verbatim into their one
    /// `next:` line, so that slot may hold a command and nothing else. It is
    /// the same choice [`Self::remedy`] makes in prose: `x unrender` for a
    /// file the ledger records as ours, `adopt` for one it does not — because
    /// `x unrender` answers "nothing in 1 file is ours to remove" there, and a
    /// named command that cannot make progress is the dead end this whole
    /// engagement is about.
    pub fn remedy_command(&self) -> &'static str {
        if self.recorded {
            Self::REMOVE_IT
        } else {
            "agentstack adopt"
        }
    }
}

/// The disk-checked form of [`crate::delivery::ZERO_ARTIFACTS`].
///
/// `ZERO_ARTIFACTS` is a claim about the ROUTING: it says what the live lane
/// keeps off disk from now on. It is not a claim about the machine, and
/// printing it beside a config file an earlier `apply` wrote is exactly the
/// mistake `commands::delivery`'s prose warns about — a delivery claim computed
/// from [`crate::delivery::Plan`] alone. Every surface that wants to say how
/// little the live lane leaves behind asks this function, which takes the
/// already-walked disk reading and only says "0" when disk agrees.
pub(crate) fn live_lane_artifacts_line(abandoned: &[AbandonedRender]) -> String {
    if abandoned.is_empty() {
        return crate::delivery::ZERO_ARTIFACTS.to_string();
    }
    let n = abandoned.len();
    let (files, are, them) = if n == 1 {
        ("file", "is", "it")
    } else {
        ("files", "are", "them")
    };
    // Only claim authorship when the ledger claims every one of them: a file
    // that arrived by clone or checkout is equally live and equally reported,
    // but AgentStack did not write it.
    let whose = if abandoned.iter().all(|a| a.recorded) {
        " AgentStack wrote"
    } else {
        ""
    };
    // And the command follows the same reading. `x unrender` removes only
    // ledger-recorded entries, so a set with none of them must not be sent
    // there — it would answer "nothing is ours to remove".
    let review = if abandoned.iter().any(|a| a.recorded) {
        AbandonedRender::REMOVE_IT
    } else {
        "agentstack adopt"
    };
    format!(
        "{n} server config {files}{whose} {are} on disk for the capabilities served \
         live — nothing NEW is written there, but the tools may still be reading {them} \
         (review: {review})"
    )
}

/// Every target whose MCP servers are routed live while a server config this
/// manifest wrote is still on disk.
///
/// `scopes` is the caller's scope list: `apply` passes the scope it is acting
/// at, `doctor` and `status` pass both, because an abandoned file at either
/// scope is equally live to the harness.
pub(crate) fn abandoned_live_renders(
    ctx: &super::Context,
    plan: &crate::delivery::Plan,
    state: &State,
    scopes: &[Scope],
) -> Vec<AbandonedRender> {
    let mut out = Vec::new();
    for h in &plan.harnesses {
        if !h
            .kinds_in(crate::delivery::Lane::Dynamic)
            .contains(&crate::delivery::Kind::Server)
        {
            continue;
        }
        let Some(desc) = ctx.registry.get(&h.id) else {
            continue;
        };
        for &scope in scopes {
            if let Some(found) = abandoned_at(desc, scope, &ctx.dir, state) {
                out.push(found);
            }
        }
    }
    out
}

/// The single-target half of [`abandoned_live_renders`], so `apply` — which
/// already knows this target routes live — asks the same question the other
/// surfaces ask, rather than a second one that could answer differently.
pub(crate) fn abandoned_at(
    desc: &crate::adapter::AdapterDescriptor,
    scope: Scope,
    dir: &Path,
    state: &State,
) -> Option<AbandonedRender> {
    let (path, mut servers) = servers_on_disk(desc, scope, dir)?;
    // The bridge entry is AgentStack's OWN control plane, not a rendered
    // project artifact: `x gateway connect` writes it globally, on purpose, and
    // it is never in the render ledger — so the disk reading would report it as
    // a file "AgentStack did not write" and send the user to `adopt`, which
    // would pull the tool's own registration into their manifest. Filtered here,
    // in the DETECTOR, rather than in `servers_on_disk`: that function's claim
    // is "what the harness actually reads at startup", and the harness really
    // does read the bridge. The name comes from the gateway's own constant so
    // the two cannot drift apart.
    servers.retain(|n| n != super::connect::BRIDGE_ENTRY);
    // The file exists but declares no server we render: the harness reads
    // nothing of ours from it, so there is nothing to warn about.
    if servers.is_empty() {
        return None;
    }
    let key = target_key(&desc.id, scope, dir);
    let recorded = !state.managed_servers(&key).is_empty();
    // A key another manifest owns, with nothing of ours in it, already has one
    // message: the foreign-keep guard. Reporting the file here as well would
    // say the same thing twice in different words.
    if !recorded
        && state
            .manifest_source(&key)
            .is_some_and(|src| src != crate::state::manifest_identity(dir))
    {
        return None;
    }
    // Foreign names are only worth reporting while they are actually in the
    // file — the ledger can outlive the entry it describes.
    let foreign: Vec<String> = state
        .kept_foreign(&key)
        .into_iter()
        .filter(|n| servers.contains(n))
        .collect();
    // A GLOBAL harness config AgentStack never wrote is the user's own machine
    // environment, not a leftover of ours. It is the normal state of every
    // installed CLI on every machine — and on the most common first run there
    // is, it is the very file `init` just imported the servers OUT of. Saying
    // "AgentStack did not write it" there, and offering `adopt` for a server
    // that is already in the manifest, is a named command that cannot make
    // progress: exactly the dead end this detector exists to prevent.
    //
    // The cut keeps BOTH dimensions, deliberately:
    //   * authorship — `recorded` files are still reported at either scope, so
    //     an abandoned GLOBAL render (`apply --scope global --write`, then the
    //     flip) stays visible. A scope-only cut was rejected before for making
    //     that invisible, and it would be wrong here for the same reason.
    //   * scope — at PROJECT scope a config we never wrote is genuinely worth
    //     naming: it came with the repo (the clone case), it is inside the
    //     boundary this manifest claims to be the source of truth for, and
    //     `adopt` there really does make progress.
    // The foreign-keep list survives the cut on purpose: those names are
    // another manifest's writes that a guarded write of OURS chose to keep, so
    // they are still our report to make wherever they sit.
    //
    // Rejected: keying on IMPORT PROVENANCE ("init read this file, so stay
    // quiet about it"). No such record exists — `init` writes nothing to the
    // state ledger — so it would mean inventing a persisted field, and it
    // would still leave every pre-existing global config on a machine that
    // never ran `init` just as noisy. Scope plus authorship needs no new
    // storage and covers the wider, truer case.
    if scope == Scope::Global && !recorded && foreign.is_empty() {
        return None;
    }
    Some(AbandonedRender {
        display: desc.display.clone(),
        path,
        servers,
        recorded,
        foreign,
    })
}

/// The servers a harness's config file DECLARES ON DISK, at one scope, plus the
/// file's path. `None` when the harness has no config at that scope, the file is
/// absent, or it is unreadable.
///
/// The single disk reading for MCP servers. Every surface that wants to know
/// what the harness will actually read at startup calls this one function; a
/// second reading is exactly how a claim drifts from reality.
pub(crate) fn servers_on_disk(
    desc: &crate::adapter::AdapterDescriptor,
    scope: Scope,
    dir: &Path,
) -> Option<(std::path::PathBuf, Vec<String>)> {
    let mcp = desc.mcp.as_ref()?;
    let (path, format) = desc.config_for(scope, dir)?;
    let content = std::fs::read_to_string(&path).ok()?;
    let servers = crate::render::section_keys(&content, &mcp.location, format);
    Some((path, servers))
}

/// Show the dry-run diff without the "re-run with `--write`" hint, for a caller
/// (e.g. `setup`) that shows this preview and then drives its own confirm.
pub(crate) fn preview(args: &ApplyArgs, manifest_dir: Option<&Path>) -> Result<Outcome> {
    render(args, manifest_dir, false, false, false)
}

/// Apply for real without re-printing the diff — the write half for a caller
/// (e.g. `setup`) that already showed the `preview` and got its own confirm.
/// Prints only the per-target write results, so the diff isn't shown twice.
pub(crate) fn write_quiet(args: &ApplyArgs, manifest_dir: Option<&Path>) -> Result<()> {
    render(args, manifest_dir, true, true, true).map(|_| ())
}

/// One render pass. `want_write` requests a write (still gated on validation);
/// `quiet` suppresses the diff bodies for the confirmed second pass; `rerun_hint`
/// controls whether the dry-run summary points at `--write` (off when a prompt
/// follows).
fn render(
    args: &ApplyArgs,
    manifest_dir: Option<&Path>,
    want_write: bool,
    quiet: bool,
    rerun_hint: bool,
) -> Result<Outcome> {
    let ctx = super::load(manifest_dir)?;
    let manifest = &ctx.loaded.manifest;
    // Default scope follows the manifest's home: project for a repo manifest,
    // global only for the machine manifest.
    let scope = args.scope.unwrap_or_else(|| Scope::default_for(&ctx.dir));

    let selection = match &args.profile {
        Some(p) => Selection::Profile(p.clone()),
        None => Selection::All,
    };
    // House-rule variant selection: an explicitly named toolset carries the
    // model, and one library read serves every target below.
    let sel = crate::instructions::Selecting::for_command(args.profile.as_deref());
    // Named once for the history ledger below — `restore`'s listing renders
    // this so an `apply --profile backend` entry reads differently from a
    // plain `apply` (both otherwise touch the same files).
    let operation = match &args.profile {
        Some(p) => format!("apply (profile '{p}')"),
        None => "apply".to_string(),
    };

    // Library-aware validation + the effective server set (inline-first, then
    // central library), shared across targets.
    let libctx = ctx.library_ctx();
    let vctx = libctx.validate_ctx(&ctx.dir);
    let target_ids_for_validation: Vec<&str> = ctx.registry.ids().collect();
    let has_errors = print_validation(manifest, target_ids_for_validation, &vctx, quiet);
    let mut server_map =
        effective_servers(manifest, &libctx.library, &libctx.lib_home, &selection)?;
    // Owner-refreshed servers: the owning app's on-disk config is the source
    // of truth, so the fan-out below uses ITS values — never a downgrade back
    // to a stale manifest (see render::owned). Stale entries get their
    // manifest table rewritten on write.
    let owned =
        crate::render::refresh_owned_servers(&mut server_map, &ctx.registry, scope, &ctx.dir);
    let manifest_refresh = plan_owned_manifest_refresh(&ctx.loaded, &owned);
    for o in owned.iter().filter(|o| o.stale) {
        crate::outln!(
            "{} {}: {} (owner) updated its own entry — fanning out the on-disk values",
            "↻".cyan(),
            o.name,
            o.owner_display
        );
    }

    let mut will_write = want_write;

    // A reader hanging up mid-pass (`apply --write | head`) must not kill the
    // process between two targets — that leaves some CLIs rendered and the
    // rest drifted, the exact state this command exists to remove. The guard
    // suppresses the kill; the `outln!`/`out!` macros this function prints
    // through drop the resulting EPIPE instead of panicking. Held for the
    // whole pass and dropped on return, restoring normal piping behaviour.
    let _sigpipe = want_write.then(crate::sys::SigpipeIgnored::new);

    // Structural validation errors would produce broken/partial config — never
    // write on them.
    if will_write && has_errors {
        if !quiet {
            crate::outln!(
                "\n{} manifest has validation errors — not writing. Fix them first.",
                "✗".red()
            );
        }
        will_write = false;
    }

    // Fail-closed instruction drift gate (--write only): every readable
    // project-declared fragment must still match its agentstack.lock pin
    // before any target's instruction region is compiled. Unpinned fragments
    // pass — the successful write records their first pin below. Missing
    // sources keep the existing per-target blocked-write handling (reported
    // in the loop), and machine-layer fragments are exempt: they are the
    // user's own machine content, never pinned.
    if will_write && !manifest.instructions.is_empty() {
        let lock = crate::lock::Lock::load(&ctx.dir)?;
        // Fragments under a standing re-gate answer are exempt, exactly as in
        // `instructions --write`: keep-pinned is drifted by definition and is
        // compiled from the approved store copy, blocked is not compiled at
        // all — the question was answered at the consent gate.
        let decided = super::use_profile::decided_names(&ctx.dir, "instruction");
        let statuses: Vec<_> = manifest
            .instructions
            .iter()
            .filter(|(n, i)| !i.from_user_layer && !decided.contains(*n))
            .map(|(n, i)| {
                let status = crate::resolve::instruction_lock_status_with(
                    n,
                    i,
                    &ctx.dir,
                    &lock,
                    &sel.library,
                );
                (n.clone(), status)
            })
            .filter(|(_, s)| {
                !matches!(
                    s,
                    crate::resolve::InstructionLockStatus::ResolveFailed { .. }
                )
            })
            .collect();
        crate::verify::ensure_instructions_compilable(&ctx.dir.display().to_string(), &statuses)?;
    }

    let target_ids = resolve_targets(manifest, &ctx.registry, &args.targets, &ctx.dir)?;
    if target_ids.is_empty() {
        if !quiet {
            crate::outln!("No targets to apply to. Set [targets].default or pass --target.");
        }
        return Ok(Outcome {
            changed_count: 0,
            validation_errors: has_errors,
            write_blockers: 0,
            written_count: 0,
            gitignore_pending: false,
        });
    }

    // The delivery planner's routing for exactly these targets. `apply` is the
    // rendered lane's command, so it renders what the planner routes to files
    // and nothing else — never a second routing opinion of its own
    // (`docs/design/automatic-delivery.md` §"The decision"). Built once, read
    // per target below.
    let plan_delivery =
        crate::delivery::Plan::build(&manifest.delivery, &ctx.registry, &target_ids);
    let servers_route_live = |id: &str| -> bool {
        plan_delivery
            .harnesses
            .iter()
            .find(|h| h.id == id)
            .is_some_and(|h| {
                h.kinds_in(crate::delivery::Lane::Dynamic)
                    .contains(&crate::delivery::Kind::Server)
            })
    };
    // Harnesses whose MCP servers this pass deliberately did not write, and
    // whose bridge is not registered — i.e. capabilities that are routed live
    // and are reaching nothing. Named loudly at the end: the contract forbids a
    // silent lane switch, and it equally forbids a silent delivery of nothing.
    let mut live_withheld: std::collections::BTreeSet<String> = Default::default();
    let mut live_unconnected: std::collections::BTreeSet<String> = Default::default();
    // Files a previous rendered-lane write left behind for harnesses that now
    // route live. Routing decides what `apply` WRITES; it cannot un-write what
    // is already there, and the state ledger still records that we wrote it —
    // so every surface names the file rather than reporting "nothing here"
    // over a config the harness is still reading (invariant 8).
    let mut abandoned: Vec<AbandonedRender> = Vec::new();
    // Did anything at all travel the rendered lane this pass — settings, hooks,
    // or instructions with real content? The exit-code decision below rests on
    // it, so it is recorded as an observation rather than inferred from counts
    // (an idempotent re-apply changes nothing and is still a delivered project).
    let mut rendered_content = false;

    crate::outln!("Scope: {scope}");
    // When the target list was implicit (no [targets], no --target), say what
    // it resolved to and how to pin it — the fan-out should never be a surprise.
    if !quiet && args.targets.is_empty() && manifest.targets.default.is_empty() {
        crate::outln!(
            "Targets: {} — no [targets] in the manifest; pin the list with `agentstack init` or a [targets] block",
            super::count(target_ids.len(), "detected CLI")
        );
    }
    // The effective (machine ∩ project) policy artifact, compiled once for
    // every render-time check this pass makes (secret scoping, egress).
    let ruleset = crate::render::ruleset_for(manifest)?;
    let mut state = State::load()?;
    let identity = crate::state::manifest_identity(&ctx.dir);
    let mut changed_count = 0;
    let mut error_count = 0;
    let mut write_blockers = 0;
    // Every distinct missing secret name seen across all targets, so the final
    // blocked-write error can print the exact fix commands instead of a
    // `<NAME>` placeholder. BTreeSet: deduped and deterministic order.
    let mut missing_secrets: std::collections::BTreeSet<String> = Default::default();
    // Server entries a write would delete, across all targets — deletions get
    // called out in the dry-run summary, not folded into "would change".
    let mut removed_count = 0;
    // Pre-write snapshots of every file we touch, grouped into one undoable
    // history entry for this apply.
    let mut backups: Vec<crate::history::FileChange> = Vec::new();
    let project_root = crate::manifest::project_root_of(&ctx.dir);
    // Derived ONCE, from the manifest both arms below already share, so the
    // preview and the write can never disagree about whether this project
    // manages the block — the divergence that made the block un-consented in
    // the first place. `--no-gitignore` overrides in the off direction only;
    // the manifest setting is the durable answer (a per-run flag can't be one:
    // the next `use <toolset>` would re-add the block).
    let gitignore_off = args.no_gitignore || !manifest.meta.manages_gitignore();
    let mut ignore_entries: Vec<String> = Vec::new();
    let mut touched_targets: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    // Every target this scope actually covers — written or already in sync. The
    // summary needs it separately from `touched_targets`: an idempotent re-apply
    // touches nothing, and "Applied to 0 target(s)" under four `✓ up to date`
    // lines reads as failure when it means the opposite.
    let mut in_scope_targets: std::collections::BTreeSet<String> =
        std::collections::BTreeSet::new();
    // Per-target outcome for the write summary: `changed_count` tallies plans
    // (a target can change servers + settings + hooks), so the summary counts
    // targets — and only ones actually written, not ones a gate blocked.
    let mut changed_targets: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut blocked_targets: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    // Targets whose render or write hit a hard error (I/O, unreadable config).
    // Distinct from `blocked_targets` (fail-closed gates): a failure here is
    // unexpected, but it must not hide the targets that DID succeed.
    let mut failed_targets: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    // Toolset selections this write replaced with a wider render — named in
    // the summary so a `use backend --write` is never undone silently.
    let mut replaced_profiles: std::collections::BTreeSet<String> =
        std::collections::BTreeSet::new();

    for id in &target_ids {
        let Some(desc) = ctx.registry.get(id) else {
            crate::outln!("{} unknown adapter '{id}' — skipping", "⚠".yellow());
            error_count += 1;
            continue;
        };

        // The whole per-target render/write runs as one fallible block: a hard
        // error on THIS target must not abort the pass, because earlier
        // targets' writes are already on disk — the honest behavior is to
        // report this target, keep rendering the rest, and record every
        // completed write in history/state at the end (Stage 1.2: a failed
        // target never hides successful targets or leaves ownership
        // ambiguous). Writes inside push their history capture only AFTER the
        // write succeeds, so a failed write never pollutes the undo ledger.
        let target_result = (|| -> Result<()> {
            let key = target_key(id, scope, &ctx.dir);
            // Whether this run compiled the instruction file — one input to the
            // managed .gitignore block computed at the end of this target's loop.
            let mut wrote_instructions = false;
            // The dry-run counterpart: whether a write WOULD compile it. The
            // block's write-mode inputs are all post-write records, which a
            // dry run never sets — so previewing the block needs the
            // prospective answer alongside the recorded one.
            let mut would_write_instructions = false;

            // The planner's answer for THIS harness's MCP servers. When they
            // travel the dynamic lane, `apply` writes no server config for this
            // CLI: writing one would put the same servers in two places, which
            // is the defect this command's routing exists to remove.
            let servers_live = servers_route_live(id);
            // Server-block facts the managed-.gitignore decision below needs.
            // Declared here so the live branch can leave them empty without the
            // rest of the target's work reaching into a skipped block.
            let mut server_managed: Vec<String> = Vec::new();
            let mut foreign: Vec<String> = Vec::new();
            let mut blocked = false;
            if servers_live {
                // No config path is printed on purpose: naming the file is what
                // made `apply` read as "these servers are about to be written
                // here".
                crate::outln!("\n{}", desc.display.bold());
                in_scope_targets.insert(desc.display.clone());
                // The state ledger, not the routing, decides what this line may
                // claim. A previous rendered write is still on disk until
                // somebody takes it off, and "nothing for `apply` to render
                // here" over such a file is exactly the dishonesty invariant 8
                // forbids.
                let left_behind = abandoned_at(desc, scope, &ctx.dir, &state);
                // "Served live" asserts the servers reach this CLI. With no
                // gateway registered they reach nothing, and `status`, `doctor`
                // and `delivery` all say "planned live (not connected)" about
                // this same harness at this same moment — so the verb is
                // chosen from the harness's own bridge state, not the routing.
                let lane = if crate::commands::overview::bridge_registered(&ctx.registry, id) {
                    "are served live"
                } else {
                    "are planned live (not connected)"
                };
                match &left_behind {
                    None => crate::outln!(
                        "  {} MCP servers {lane}, not written — nothing for `apply` to \
                         render here",
                        "·".dimmed()
                    ),
                    Some(found) => {
                        crate::outln!(
                            "  {} MCP servers {lane} — `apply` writes nothing here now.",
                            "·".dimmed()
                        );
                        crate::outln!("  {} {}", "⚠".yellow(), found.sentence());
                        crate::outln!("  {} {}", "→".cyan(), found.remedy());
                    }
                }
                // The foreign-keep guard is not the rendered lane's business —
                // it reports entries ANOTHER manifest applied to a file we
                // share. Routing never made those stop existing, so the warning
                // survives the lane switch.
                if let Some(found) = &left_behind {
                    if !found.foreign.is_empty() {
                        crate::outln!(
                            "  {} keeping {} — applied by another manifest ↳ keep: agentstack \
                             adopt · prune: agentstack apply --prune-foreign",
                            "⚠".yellow(),
                            found.foreign.join(", ")
                        );
                    }
                }
                if let Some(found) = left_behind {
                    abandoned.push(found);
                }
                live_withheld.insert(desc.display.clone());
                if !super::overview::bridge_registered(&ctx.registry, id) {
                    live_unconnected.insert(desc.display.clone());
                }
            } else {
                // A labelled block, so "this CLI has no MCP server config at
                // this scope" can stop the SERVER work without stopping the
                // target. The early `return` that used to sit here skipped
                // settings, instructions and hooks as well, and a manifest
                // with only `[settings.pi]` therefore reported "0 targets
                // already in sync — nothing to change" while writing nothing:
                // the delivery of every file-lane capability was gated on the
                // server config, which is this engagement's root-cause shape
                // (a claim computed from one record instead of from what the
                // target actually has).
                'servers: {
                    let mut previously = state.managed_servers(&key);
                    // Names an earlier guarded write kept on disk (state bookkeeping —
                    // they left `managed_servers` when this manifest recorded its own
                    // set). Ones the manifest now selects become managed again below.
                    let kept_before: Vec<String> = state
                        .kept_foreign(&key)
                        .into_iter()
                        .filter(|n| !server_map.contains_key(n))
                        .collect();
                    // Guard cross-manifest global prunes: entries another manifest applied
                    // are kept (and reported below), not deleted, unless --prune-foreign.
                    foreign = if args.prune_foreign {
                        // Fold previously-kept names into the prune set — the escape
                        // hatch must still reach them after a guarded write re-recorded
                        // this key with only our own managed set.
                        for n in &kept_before {
                            if !previously.contains(n) {
                                previously.push(n.clone());
                            }
                        }
                        Vec::new()
                    } else {
                        let mut f =
                            state.foreign_prunes(&key, scope, &ctx.dir, &mut previously, |n| {
                                server_map.contains_key(n)
                            });
                        // Keep surfacing (and tracking) what earlier runs kept.
                        for n in &kept_before {
                            if !f.contains(n) {
                                f.push(n.clone());
                            }
                        }
                        f
                    };
                    let Some(plan) = plan_target_with_servers(
                        desc,
                        &ctx.resolver,
                        &ruleset,
                        &server_map,
                        &previously,
                        scope,
                        &ctx.dir,
                        crate::render::PriorTrust::STRICT,
                    )?
                    else {
                        crate::outln!(
                            "\n{} — no {scope} MCP server config, skipping servers here",
                            desc.display.bold()
                        );
                        break 'servers;
                    };

                    crate::outln!("\n{} ({})", plan.display.bold(), plan.config_path.display());
                    in_scope_targets.insert(desc.display.clone());

                    if plan.managed.is_empty() && plan.removed.is_empty() && plan.skipped.is_empty()
                    {
                        // Now truthful again: the selection this line reports on reads
                        // the toolsets too, so an empty plan means the manifest really
                        // does select nothing for this CLI rather than that `apply`
                        // could not see a library-first manifest's servers.
                        crate::outln!("  no servers selected");
                    }
                    removed_count += plan.removed.len();
                    for r in &plan.removed {
                        if will_write {
                            crate::outln!(
                                "  {} pruning '{r}' (no longer in manifest)",
                                "−".yellow()
                            );
                        } else {
                            // A deletion deserves louder wording than a diff line: name
                            // the entry and the file it would vanish from.
                            crate::outln!(
                                "  {} would REMOVE '{r}' from {} (no longer in manifest)",
                                "−".red(),
                                plan.config_path.display()
                            );
                        }
                    }
                    if !foreign.is_empty() {
                        crate::outln!(
                        "  {} keeping {} — applied by another manifest ↳ keep: agentstack adopt · \
                 prune: agentstack apply --prune-foreign",
                        "⚠".yellow(),
                        foreign.join(", ")
                    );
                    }
                    for (name, reason) in &plan.skipped {
                        crate::outln!("  {} skipping '{name}' — {reason}", "↳".cyan());
                    }
                    for w in &plan.warnings {
                        crate::outln!(
                            "  {} '{w}' has a cwd that {} can't express — it renders without one \
                 (wrap the command in a shell that cd's if the server needs it)",
                            "⚠".yellow(),
                            plan.display
                        );
                    }
                    for u in &plan.unresolved {
                        // Entries read "NAME (server 'x')" — the first token is the ref
                        // name, which is all `secret set` needs.
                        let name = u.split_whitespace().next().unwrap_or(u.as_str());
                        crate::outln!(
                            "  {} unresolved secret {u} ↳ agentstack secret set {name}",
                            "✗".red()
                        );
                        missing_secrets.insert(name.to_string());
                        error_count += 1;
                    }
                    for d in &plan.denied {
                        crate::outln!("  {} blocked by policy: {}", "✗".red(), d);
                    }
                    for f in &plan.failed {
                        crate::outln!("  {} {}", "✗".red(), crate::render::failed_secret_line(f));
                        // The fix is the same `secret set` whether the secret is missing
                        // or its store failed to read — collect the name so the closing
                        // tail stays copy-pasteable in both cases.
                        let name = f.split_whitespace().next().unwrap_or(f.as_str());
                        missing_secrets.insert(name.to_string());
                        error_count += 1;
                    }
                    // Trust: a server entry is a command line the harness spawns
                    // itself, outside agentstack, so an untrusted or drifted
                    // project renders none of them
                    // (`render::apply::trust_refusal`). Reported like any other
                    // fail-closed gate — the diff is still shown, so what is
                    // being withheld stays reviewable.
                    //
                    // Only where something would actually be delivered: an
                    // unchanged plan writes nothing, and "refusing to render"
                    // printed above "✓ up to date" would be two contradictory
                    // claims about the same file.
                    let refused = plan.refusal.is_some() && plan.changed();
                    if refused {
                        if let Some(why) = &plan.refusal {
                            crate::outln!("  {} {why}", "✗".red());
                        }
                        error_count += 1;
                    }
                    // `${REF}`s that didn't resolve must never reach a live config file —
                    // whether the secret is missing or a store failed to read it.
                    blocked = ((!plan.unresolved.is_empty() || !plan.failed.is_empty()) && !args.allow_unresolved)
                // Policy refusals are not a convenience gap: --allow-unresolved
                // never overrides [policy.secrets]/[policy.egress]. Neither does
                // it reach the trust gate: that flag forgives a missing secret,
                // never a missing consent.
                || !plan.denied.is_empty()
                || refused;
                    if blocked {
                        write_blockers += 1;
                    }

                    if plan.changed() {
                        changed_count += 1;
                        changed_targets.insert(desc.display.clone());
                        if !quiet {
                            print_body_or_summary(args, &plan.diff());
                        }
                        if will_write && blocked {
                            blocked_targets.insert(desc.display.clone());
                            if refused {
                                crate::outln!(
                                    "  {} not written — the project has not been trusted for \
                                     this content",
                                    "✗".red()
                                );
                            } else {
                                let reason = if plan.unresolved.is_empty() {
                                    "secret read failures; set them"
                                } else {
                                    "unresolved secrets; set them"
                                };
                                crate::outln!(
                                    "  {} not written — {reason} or pass --allow-unresolved",
                                    "✗".red()
                                );
                            }
                        } else if will_write {
                            let capture = crate::history::capture(
                                &plan.config_path,
                                format!("{} · servers", desc.display),
                            );
                            plan.write()?;
                            backups.push(capture);
                            touched_targets.insert(desc.display.clone());
                            state.record(&key, plan.managed.clone(), &plan.proposed, &identity);
                            // A full-manifest apply replaces any narrower `use`
                            // selection; name the toolset it just widened away so the
                            // switch isn't undone silently (a bare re-activate gets it
                            // back). A `--toolset` apply records that selection.
                            if let Some(prev) = state.active_profile(&key) {
                                if args.profile.as_deref() != Some(prev.as_str()) {
                                    replaced_profiles.insert(prev);
                                }
                            }
                            state.record_active_profile(&key, args.profile.clone());
                            // Track what this guarded write kept on disk (empty after a
                            // --prune-foreign actually pruned them) — see
                            // TargetState::kept_foreign.
                            state.record_kept_foreign(&key, foreign.clone());
                            crate::usage::bump(&plan.managed);
                            if plan.remove_if_empty_shell(desc) {
                                crate::outln!(
                                    "  {} removed empty {}",
                                    "−".yellow(),
                                    plan.config_path.display()
                                );
                            } else {
                                crate::outln!(
                                    "  {} wrote {}",
                                    "✓".green(),
                                    super::count(plan.managed.len(), "server")
                                );
                            }
                        } else {
                            crate::outln!(
                                "  {} {} to apply",
                                "→".cyan(),
                                super::count(plan.managed.len(), "server")
                            );
                        }
                    } else {
                        // Even with no file change, keep state in sync with reality.
                        if will_write {
                            state.record(&key, plan.managed.clone(), &plan.proposed, &identity);
                            state.record_active_profile(&key, args.profile.clone());
                            state.record_kept_foreign(&key, foreign.clone());
                        }
                        crate::outln!("  {} up to date", "✓".green());
                    }
                    server_managed = plan.managed.clone();
                }
            }

            // Native settings file (permissions, feature flags) — a separate file
            // from the MCP config, merged at the top level.
            let prev_settings = state.managed_settings(&key);
            if let Some(sp) = plan_settings(
                manifest,
                desc,
                &ctx.resolver,
                &prev_settings,
                scope,
                &ctx.dir,
            )? {
                rendered_content = rendered_content || !sp.managed.is_empty();
                for u in &sp.unresolved {
                    let name = u.split_whitespace().next().unwrap_or(u.as_str());
                    crate::outln!(
                        "  {} unresolved secret {u} (settings) ↳ agentstack secret set {name}",
                        "✗".red()
                    );
                    missing_secrets.insert(name.to_string());
                    error_count += 1;
                }
                let sblocked = !sp.unresolved.is_empty() && !args.allow_unresolved;
                if sblocked {
                    write_blockers += 1;
                }
                for r in &sp.removed {
                    crate::outln!(
                        "  {} pruning setting '{r}' (no longer in manifest)",
                        "−".yellow()
                    );
                }
                if sp.changed() {
                    changed_count += 1;
                    changed_targets.insert(desc.display.clone());
                    crate::outln!(
                        "  {} settings → {}",
                        "·".dimmed(),
                        sp.settings_path.display()
                    );
                    if !quiet {
                        print_body_or_summary(args, &sp.diff());
                    }
                    if will_write && sblocked {
                        blocked_targets.insert(desc.display.clone());
                        crate::outln!("  {} settings not written — unresolved secrets", "✗".red());
                    } else if will_write {
                        let capture = crate::history::capture(
                            &sp.settings_path,
                            format!("{} · settings", desc.display),
                        );
                        sp.write()?;
                        backups.push(capture);
                        touched_targets.insert(desc.display.clone());
                        state.record_settings(&key, sp.managed.clone());
                        crate::outln!(
                            "  {} wrote {}",
                            "✓".green(),
                            super::count(sp.managed.len(), "setting")
                        );
                    } else {
                        crate::outln!(
                            "  {} {} to apply",
                            "→".cyan(),
                            super::count(sp.managed.len(), "setting")
                        );
                    }
                } else if will_write && !sblocked {
                    state.record_settings(&key, sp.managed.clone());
                }
            }

            // Lifecycle hooks (compiled into the harness's native hooks config).
            // At global scope the machine's guard hook rides along so owning the
            // whole hooks key doesn't strip it (see `guard::machine_hooks_for_apply`).
            let machine_hooks = if scope == Scope::Global {
                crate::commands::guard::machine_hooks_for_apply()
            } else {
                Vec::new()
            };
            let prev_hooks = !state.managed_hooks(&key).is_empty();
            if let Some(hp) = plan_hooks(
                manifest,
                desc,
                &ctx.resolver,
                prev_hooks,
                scope,
                &ctx.dir,
                &machine_hooks,
            )? {
                rendered_content =
                    rendered_content || (!hp.managed.is_empty() && hp.refusal.is_none());
                for u in &hp.unresolved {
                    let name = u.split_whitespace().next().unwrap_or(u.as_str());
                    crate::outln!(
                        "  {} unresolved secret {u} (hook) ↳ agentstack secret set {name}",
                        "✗".red()
                    );
                    missing_secrets.insert(name.to_string());
                    error_count += 1;
                }
                // Trust: a hook is a command the harness runs at full user
                // permission, so an untrusted or drifted project renders none
                // of them (`render::hooks::trust_refusal`). It blocks the write
                // through the same seam an unresolved secret does — the diff is
                // still shown, so what is being withheld stays reviewable — and
                // `--allow-unresolved` does NOT reach it: that flag forgives a
                // missing secret, never a missing consent.
                if let Some(why) = &hp.refusal {
                    crate::outln!("  {} {why}", "✗".red());
                    error_count += 1;
                }
                let hblocked =
                    (!hp.unresolved.is_empty() && !args.allow_unresolved) || hp.refusal.is_some();
                if hblocked {
                    write_blockers += 1;
                }
                if hp.changed() {
                    changed_count += 1;
                    changed_targets.insert(desc.display.clone());
                    crate::outln!("  {} hooks → {}", "·".dimmed(), hp.path.display());
                    if !quiet {
                        print_body_or_summary(args, &hp.diff());
                    }
                    if will_write && hblocked {
                        blocked_targets.insert(desc.display.clone());
                        crate::outln!(
                            "  {} hooks not written — {}",
                            "✗".red(),
                            if hp.refusal.is_some() {
                                "the project has not been trusted for this content"
                            } else {
                                "unresolved secrets"
                            }
                        );
                    } else if will_write {
                        let capture =
                            crate::history::capture(&hp.path, format!("{} · hooks", desc.display));
                        hp.write()?;
                        backups.push(capture);
                        touched_targets.insert(desc.display.clone());
                        state.record_hooks(&key, hp.managed.clone());
                        crate::outln!(
                            "  {} wrote {}",
                            "✓".green(),
                            super::count(hp.managed.len(), "hook")
                        );
                    } else {
                        crate::outln!(
                            "  {} {} to apply",
                            "→".cyan(),
                            super::count(hp.managed.len(), "hook")
                        );
                    }
                } else if will_write && !hblocked {
                    state.record_hooks(&key, hp.managed.clone());
                }
            }

            // Instruction fragments (the managed region of CLAUDE.md / AGENTS.md).
            // Only when the manifest declares [instructions.*]: a manifest without
            // any must never touch — let alone empty out — a region another layer
            // (e.g. the machine manifest seeded by `init --global`) owns.
            // W5: a package's instruction members compile into the same region,
            // so a project whose house rules arrive only through a package is
            // not "a manifest without any [instructions.*]" for this purpose.
            let pinned = crate::lock::Lock::load(&ctx.dir).unwrap_or_default();
            let pkg_instr = !crate::package::members_of_kind(
                &pinned,
                crate::lock::PackageMemberKind::Instruction,
                None,
            )
            .is_empty();
            if !manifest.instructions.is_empty() || pkg_instr {
                // …and only when fragments actually apply at THIS scope: project
                // scope filters out every machine-layer fragment, so a project
                // with none of its own compiles to an empty string there — writing
                // that would strip a committed managed region from the repo.
                if let Some(ip) = plan_instructions(
                    manifest,
                    desc,
                    scope,
                    &ctx.dir,
                    crate::package::effective_members(&pinned),
                    &sel,
                    // `apply` authors nothing the consent digest covers before
                    // this point — the owned-server manifest refresh and the
                    // instruction pins both happen after the whole render loop
                    // — so it is judged against the state on disk, like every
                    // other gate it runs.
                    crate::render::PriorTrust::STRICT,
                )
                // An excluded-only plan still compiles (to a region without
                // the refused fragment): skipping it would leave a blocked
                // fragment's bytes sitting in the managed region, which is
                // the refusal not holding.
                .filter(|ip| {
                    !ip.fragments.is_empty() || !ip.missing.is_empty() || !ip.excluded.is_empty()
                }) {
                    rendered_content =
                        rendered_content || (!ip.fragments.is_empty() && ip.refusal.is_none());
                    for m in &ip.missing {
                        crate::outln!("  {} instruction fragment '{m}' source missing", "✗".red());
                        error_count += 1;
                    }
                    for (name, why) in &ip.excluded {
                        crate::outln!("  {} instruction fragment '{name}' {why}", "⊘".dimmed());
                    }
                    // Trust: a fragment's bytes land in the managed region a
                    // harness reads straight into a model's context, so an
                    // untrusted or drifted project compiles none of its own
                    // (`render::instructions::trust_refusal`). It blocks the
                    // write through the same seam a missing source does — the
                    // diff is still shown, so what is being withheld stays
                    // reviewable.
                    if let Some(why) = &ip.refusal {
                        crate::outln!("  {} {why}", "✗".red());
                        error_count += 1;
                    }
                    // A missing source already dropped its content from the
                    // compile — writing now would delete previously compiled
                    // fragments (all sources missing empties the whole region).
                    // Block the write, mirroring the unresolved-secret path.
                    let iblocked = !ip.missing.is_empty() || ip.refusal.is_some();
                    if iblocked {
                        write_blockers += 1;
                    }
                    if ip.changed() {
                        changed_count += 1;
                        changed_targets.insert(desc.display.clone());
                        crate::outln!("  {} instructions → {}", "·".dimmed(), ip.path.display());
                        if !quiet {
                            print_body_or_summary(args, &ip.diff());
                        }
                        if will_write && iblocked {
                            blocked_targets.insert(desc.display.clone());
                            crate::outln!(
                                "  {} instructions not written — {}",
                                "✗".red(),
                                if ip.refusal.is_some() {
                                    "the project has not been trusted for this content"
                                } else {
                                    "missing fragment sources"
                                }
                            );
                        } else if will_write {
                            let capture = crate::history::capture(
                                &ip.path,
                                format!("{} · instructions", desc.display),
                            );
                            ip.write()?;
                            backups.push(capture);
                            touched_targets.insert(desc.display.clone());
                            wrote_instructions = true;
                            crate::outln!(
                                "  {} wrote {}",
                                "✓".green(),
                                super::count(ip.fragments.len(), "instruction fragment")
                            );
                        } else {
                            // Dry run: a blocked compile writes nothing, so it
                            // must not contribute an ignore entry either —
                            // mirroring the write path's blocked arm.
                            would_write_instructions = !iblocked;
                            crate::outln!(
                                "  {} {} to apply",
                                "→".cyan(),
                                super::count(ip.fragments.len(), "instruction fragment")
                            );
                        }
                    }
                } else if desc.instructions.is_none() {
                    // This CLI has no instruction file agentstack manages. If
                    // fragments would otherwise compile here, note the silent drop
                    // in its block rather than omitting it entirely.
                    let n = manifest
                        .instructions
                        .values()
                        .filter(|i| i.compiles_at(id, scope))
                        .count();
                    if n > 0 {
                        crate::outln!(
                            "  {} (instructions not supported by this CLI — {} not compiled)",
                            "·".dimmed(),
                            super::count(n, "fragment")
                        );
                    }
                }
            }

            // Managed .gitignore block: emit an entry only for an artifact this
            // target actually manages now — after the write sections above, so a
            // blocked write (nothing recorded) contributes nothing. Both flags read
            // persistent records `use` shares, keeping the block churn-free across
            // the two commands. `apply` never materializes skills, so its skills
            // flag is purely the record a prior `use` left.
            //
            // A dry run records nothing, so those same reads would report an
            // empty block and the preview would omit the .gitignore edit the
            // apply is about to make — which is how this write came to be the
            // one nobody consented to. The dry-run arm therefore derives the
            // SAME flags prospectively, from what this run would write
            // (`plan.managed` / `foreign` / `would_write_instructions`) OR'd
            // with the records already on disk. The write arm is untouched:
            // its byte-for-byte agreement with `use` is a pinned witness.
            if scope == Scope::Project {
                let instr_path = desc
                    .instructions
                    .as_ref()
                    .and_then(|s| s.path_for(scope, &ctx.dir));
                let instr_managed_on_disk = instr_path
                    .as_deref()
                    .is_some_and(crate::render::instructions::manages_file);
                let managed = if will_write {
                    crate::render::gitignore::Managed {
                        config: !state.managed_servers(&key).is_empty()
                            || !state.kept_foreign(&key).is_empty(),
                        skills: !state.managed_skills(&key).is_empty(),
                        instructions: wrote_instructions || instr_managed_on_disk,
                    }
                } else {
                    crate::render::gitignore::Managed {
                        // `blocked` gates the prospective half exactly as it
                        // gates the write: a run that would write nothing must
                        // not preview hiding a hand-maintained .mcp.json.
                        config: (!blocked && (!server_managed.is_empty() || !foreign.is_empty()))
                            || !state.managed_servers(&key).is_empty()
                            || !state.kept_foreign(&key).is_empty(),
                        // `apply` never materializes skills in either mode, so
                        // this stays purely the record a prior `use` left.
                        skills: !state.managed_skills(&key).is_empty(),
                        instructions: would_write_instructions || instr_managed_on_disk,
                    }
                };
                ignore_entries.extend(crate::render::gitignore::managed_entries(
                    desc, scope, &ctx.dir, managed,
                ));
            }
            Ok(())
        })();
        if let Err(err) = target_result {
            crate::outln!(
                "\n{} {} failed — {err:#}\n  {} other targets continue; completed writes stay \
                 recorded and undoable",
                "✗".red(),
                desc.display.bold(),
                "·".dimmed()
            );
            failed_targets.insert(desc.display.clone());
            error_count += 1;
        }
    }

    // Native extensions (D6): copy declared `[extensions.*]` sources into their
    // target harness's extension directory — fail-closed on trust + lock,
    // pruned via an ownership ledger. Rendered here (not in the per-target loop)
    // because an extension names its own target adapter, independent of the MCP
    // fan-out selection. Its project-scope artifacts join the managed
    // .gitignore block below.
    let ext_ignore =
        crate::render::extensions::render(manifest, &ctx.registry, scope, &ctx.dir, will_write)?;
    rendered_content = rendered_content || !ext_ignore.is_empty();
    ignore_entries.extend(ext_ignore);

    // Owned-server manifest refresh: rewrite the stale `[servers.X]` tables in
    // whichever manifest layer declares them, so the manifest catches up with
    // the owning app instead of fighting it. Never the other way around — the
    // fan-out above already used the on-disk values.
    let (refresh_files, refresh_elsewhere) = manifest_refresh;
    for name in &refresh_elsewhere {
        crate::outln!(
            "\n{} {name}: owned definition is declared outside this manifest (central library \
             or inherited layer) — the fresh values still fan out, but refresh the declaring \
             file to clear the stale definition",
            "⚠".yellow()
        );
    }
    for (path, entries) in &refresh_files {
        let names: Vec<&str> = entries.iter().map(|(n, _)| n.as_str()).collect();
        changed_count += 1;
        if !will_write {
            crate::outln!(
                "\n{} manifest refresh pending for {} {} → {}",
                "→".cyan(),
                if names.len() == 1 {
                    "owned server"
                } else {
                    "owned servers"
                },
                names.join(", "),
                path.display()
            );
            continue;
        }
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let json_entries: Vec<(String, serde_json::Value)> = entries
            .iter()
            .map(|(n, s)| {
                let value = serde_json::to_value(s)
                    .expect("an internal derive(Serialize) struct always serializes");
                (n.clone(), value)
            })
            .collect();
        let new_text = crate::render::merge_toml::merge(&text, "servers", &json_entries, true)?;
        if new_text == text {
            continue;
        }
        // Rewriting the manifest changes its trust digest. This change is
        // machine-derived from a config the owner harness already executes —
        // nothing new is being authorized — so trust that was VALID before the
        // rewrite is re-pinned to the new digest. That is the general rule
        // `crate::trust_carry::TrustCarry` states and enforces (including why
        // the capture must precede the write); this is one of its callers.
        let carry = crate::trust_carry::TrustCarry::before_write(&ctx.dir);
        let was_trusted = carry.was_valid();
        backups.push(crate::history::capture(
            path,
            "manifest · owned-server refresh",
        ));
        crate::util::atomic::write(path, &new_text)
            .with_context(|| format!("writing {}", path.display()))?;
        crate::outln!(
            "\n{} refreshed {} {} in {}",
            "✓".green(),
            if names.len() == 1 {
                "owned server"
            } else {
                "owned servers"
            },
            names.join(", "),
            path.display()
        );
        // An owned definition can live in a layer this project's trust does not
        // cover (a central library file, an inherited manifest). `across_write`
        // re-pins nothing for those and the project stays re-gated.
        let repinned = carry.across_write(path, &new_text)?;
        if was_trusted {
            if repinned {
                crate::outln!(
                    "  {} trust re-pinned — the refreshed values came from the owner's own config",
                    "·".dimmed()
                );
            } else {
                crate::outln!(
                    "  {} manifest changed — review and re-run `agentstack trust`",
                    "·".dimmed()
                );
            }
        }
    }

    let written_count = touched_targets.len();
    // A target can be both written and blocked/failed — e.g. its instructions
    // landed while its server config was refused over an unresolved secret, or
    // its settings write threw after its servers landed. Split the overlap out
    // so the summary counts don't cover the same target twice.
    let incomplete_targets: std::collections::BTreeSet<String> =
        blocked_targets.union(&failed_targets).cloned().collect();
    let partially_written = touched_targets.intersection(&incomplete_targets).count();
    if will_write {
        state.save()?;
        // Pin the project-declared instruction fragments that compiled. The
        // gate above already blocked on drift, so every checksum recorded
        // here is either unchanged or a first pin — never absorbed drift.
        // (Non-strict: unreadable fragments were reported per target above.)
        if manifest.instructions.values().any(|i| !i.from_user_layer) {
            super::lock::record_instruction_pins(&ctx.dir, manifest, false)?;
        }
    }

    let history_targets: Vec<String> = touched_targets.iter().cloned().collect();
    let mut gitignore_pending = false;
    if will_write && scope == Scope::Project && !gitignore_off {
        let gitignore = project_root.join(".gitignore");
        let capture = crate::history::capture(&gitignore, ".gitignore · managed artifacts");
        match crate::render::gitignore::ensure_block(&project_root, &ignore_entries, true) {
            Ok(true) => {
                backups.push(capture);
                crate::outln!(
                    "\n{} .gitignore: managed block updated — generated artifacts stay out of git ({} to commit them instead)",
                    "✓".green(),
                    "--no-gitignore".bold()
                );
            }
            Ok(false) => {}
            Err(err) => {
                // Config writes happened before the managed-block update. Keep
                // them recoverable even when this final ancillary write fails.
                let _ = crate::history::record(
                    scope.as_str(),
                    operation.clone(),
                    history_targets.clone(),
                    backups,
                );
                return Err(err);
            }
        }
    } else if scope == Scope::Project && !gitignore_off {
        // The .gitignore edit, named in the preview the user actually reads
        // before consenting. `ensure_block` with `write: false` answers "would
        // this change?" without touching the file, so the preview and the
        // write agree by construction rather than by a second implementation.
        //
        // Listing the entries matters more than the count: ".gitignore will be
        // updated" is a claim about a file the user may have hand-curated,
        // and the only thing that makes it reviewable is which lines land.
        if let Ok(true) =
            crate::render::gitignore::ensure_block(&project_root, &ignore_entries, false)
        {
            gitignore_pending = true;
            crate::outln!(
                "\n{} .gitignore: would add a managed block so generated artifacts stay out of git",
                "→".cyan()
            );
            // Sorted + deduped to match `splice`, so the previewed lines are
            // the lines that land — not a per-target order the block collapses.
            let mut shown: Vec<&str> = ignore_entries.iter().map(String::as_str).collect();
            shown.sort_unstable();
            shown.dedup();
            for entry in shown {
                crate::outln!("  {} {entry}", "+".green());
            }
            crate::outln!(
                "  {} {} to leave .gitignore alone and commit them instead",
                "·".dimmed(),
                "--no-gitignore".bold()
            );
        }
    } else if scope == Scope::Project
        && gitignore_off
        && crate::render::gitignore::has_block(&project_root)
    {
        // Opted out, but a block is already on disk. Routine commands never
        // strip it (a team may have committed it), so the leftover is reported
        // instead — otherwise the user believes these files are visible to
        // `git status` when the stale block is still hiding them.
        crate::outln!(
            "\n{} .gitignore: this project opted out, but a managed block is still present — \
             delete the marked lines to un-hide the generated files",
            "·".dimmed()
        );
    }

    if will_write {
        // Record one undoable history entry for every file this apply wrote,
        // including the project-scope managed gitignore block above.
        // Best-effort: never fail an otherwise-successful apply over history.
        let _ = crate::history::record(scope.as_str(), operation, history_targets, backups);
    }

    // `apply` renders servers/instructions/hooks/settings — never skills.
    // Say so, or a manifest's skills look silently dropped (they activate
    // through a profile via `use`). Name a real profile when one exists;
    // without one, "use <profile>" would send the user to a dead end.
    if !manifest.skills.is_empty() && !quiet {
        let n = manifest.skills.len();
        let verb = if n == 1 { "is" } else { "are" };
        let pronoun = if n == 1 { "it" } else { "them" };
        match manifest.profiles.keys().next() {
            Some(first) => crate::outln!(
                "\n{} {} in the manifest {verb} not rendered by `apply` — activate {pronoun} through a toolset: `agentstack use {first} --write` (or `agentstack init`, which does this for you)",
                "ℹ".cyan(),
                super::count(n, "skill")
            ),
            None => crate::outln!(
                "\n{} {} in the manifest {verb} not rendered by `apply` — activate {pronoun}: `agentstack use --write` (no toolsets declared, so everything inline is the default)",
                "ℹ".cyan(),
                super::count(n, "skill")
            ),
        }
    }

    // An apply that widened the render past an active toolset replaced that
    // selection — say so, with the way back, instead of undoing it silently.
    if will_write && !replaced_profiles.is_empty() && !quiet {
        for p in &replaced_profiles {
            crate::outln!(
                "\n{} this rendered the full manifest, replacing the active toolset '{p}' — switch back: agentstack use {p} --write",
                "ℹ".cyan()
            );
        }
    }

    // The capabilities this pass deliberately did NOT write, said plainly, with
    // both ways forward. A user who came here expecting `.mcp.json` must learn
    // from `apply` itself why it is not there — the contract forbids a silent
    // switch of lanes, and a silent delivery of nothing is the same failure
    // wearing the other face.
    if !live_withheld.is_empty() && !quiet {
        let names: Vec<&str> = live_withheld.iter().map(String::as_str).collect();
        crate::outln!(
            "\n{} MCP servers for {} are routed to the live lane — `apply` does not write them.",
            "ℹ".cyan(),
            names.join(", ")
        );
        if live_unconnected.is_empty() {
            crate::outln!(
                "  {} they are served on demand through the registered bridge.",
                "·".dimmed()
            );
        } else {
            crate::outln!(
                "  {} nothing is being served yet — {} {} no bridge registered.",
                "·".dimmed(),
                live_unconnected
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .join(", "),
                if live_unconnected.len() == 1 {
                    "has"
                } else {
                    "have"
                }
            );
            // One shared constant, so `apply`, `status`, `doctor` and
            // `delivery` cannot name four different recovery commands.
            crate::outln!("  {} {}", "→".cyan(), super::delivery::CONNECT_THE_BRIDGE);
            crate::outln!(
                "  {} or write files anyway: agentstack x delivery render-locally --write",
                "→".cyan()
            );
        }
        // "nothing is being served yet" is true of the live lane and false of
        // the machine: files we wrote are still being read. The per-target
        // block above named each one with its removal command; this line keeps
        // the summary from reading as "and therefore nothing is configured".
        if !abandoned.is_empty() {
            // Authorship is claimed only when the ledger claims every one of
            // them — the same conjugation `live_lane_artifacts_line` makes.
            // A file that arrived by clone or checkout is equally live and
            // equally reported, but saying "AgentStack wrote" over it is a
            // claim about who acted, and it would be false.
            crate::outln!(
                "  {} {}{} {} still on disk and still read — see above.",
                "⚠".yellow(),
                super::count(abandoned.len(), "config file"),
                if abandoned.iter().all(|a| a.recorded) {
                    " AgentStack wrote"
                } else {
                    ""
                },
                if abandoned.len() == 1 { "is" } else { "are" }
            );
        }
    }

    crate::outln!();
    if will_write {
        // Count targets actually written, not pending changes — a gate above
        // (unresolved secret, missing fragment source) may have blocked some
        // of the writes, and a hard error may have failed others.
        if incomplete_targets.is_empty() {
            // Report coverage first and changes second. The old form printed only
            // the changed count, so a clean re-apply said "Applied to 0
            // target(s)" directly under four "✓ up to date" lines.
            let covered = in_scope_targets.len().max(written_count);
            let targets = super::count(covered, "target");
            if written_count == 0 && !live_withheld.is_empty() && !rendered_content {
                // "already in sync" would be a claim of delivery over a project
                // where nothing was delivered — the sync is real, and it is a
                // sync of nothing.
                crate::outln!("Nothing for the rendered lane here — see above.");
            } else if written_count == 0 {
                crate::outln!("{targets} already in sync — nothing to change.");
            } else {
                // The undo pointer belongs to `restart_hint`, which already
                // prints it on the standalone path — don't print a second one.
                crate::outln!("{targets} in sync — wrote {written_count}.");
            }
        } else {
            // Every not-fully-written target is blocked, failed, or both;
            // partially written targets count against the fully-written tally
            // so the numbers never cover a target twice.
            let fully_written = written_count - partially_written;
            let partial_note = if partially_written > 0 {
                format!(" ({partially_written} partially written)")
            } else {
                String::new()
            };
            let mut what = Vec::new();
            if !blocked_targets.is_empty() {
                // Deliberately not a list of causes: a target can be blocked by
                // an unresolved secret, a missing fragment source, a policy
                // refusal, or a trust refusal, and a summary that names only
                // some of them sends the reader looking for the wrong fix. Each
                // ✗ above states its own blocker.
                what.push(format!(
                    "{} blocked — see the ✗ line for each",
                    blocked_targets.len()
                ));
            }
            if !failed_targets.is_empty() {
                what.push(format!(
                    "{} failed ({})",
                    failed_targets.len(),
                    failed_targets
                        .iter()
                        .map(String::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            crate::outln!(
                "Wrote {fully_written} of {}; {}{partial_note}; see {} above.",
                super::count(changed_targets.len(), "target"),
                what.join("; "),
                "✗".red()
            );
        }
    } else if rerun_hint {
        if has_errors {
            // Don't point at `--write` when validation already guarantees it
            // would refuse — the next command is fixing the manifest, not
            // re-running the one that just failed.
            crate::outln!(
                "{} would change{} — fix the {} validation errors above before writing.",
                super::count(changed_count, "target"),
                removal_note(removed_count),
                "✗".red()
            );
        } else {
            crate::outln!(
                "{} would change{}. Re-run with {} to write.",
                super::count(changed_count, "target"),
                removal_note(removed_count),
                "--write".bold()
            );
        }
    } else {
        // A confirm prompt is about to follow — don't tell the user to re-run.
        crate::outln!(
            "{} would change{}.",
            super::count(changed_count, "target"),
            removal_note(removed_count)
        );
    }
    // On a blocked write the count line above plus the bail below already tell
    // the whole story — a third "N issue(s)" line in between was pure repetition.
    let blocked_write = will_write && !blocked_targets.is_empty();
    if error_count > 0 && !quiet && !blocked_write {
        crate::outln!(
            "{} — resolve before writing.",
            super::count(error_count, "issue")
        );
    }

    // A hard per-target failure exits nonzero AFTER state and history are
    // recorded above — the successful targets' writes stay owned and undoable,
    // and the error names exactly which targets did not land.
    if will_write && !failed_targets.is_empty() {
        anyhow::bail!(
            "write failed on {}: {} — each ✗ above has the error; targets written \
             before or after stay recorded (undo: agentstack restore)",
            super::count(failed_targets.len(), "target"),
            failed_targets
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    // A blocked write is a failure, not a footnote: exit nonzero so scripts
    // can't mistake a fail-closed apply for success. Mirrors `use --write`.
    // (`doctor --ci` runs its own checks and never reads apply's exit code,
    // and `setup` stops on `write_blockers` before its write pass.)
    if blocked_write {
        // Name the exact fixes when we know them (missing secrets are by far
        // the common case); anything else keeps its ✗ line above.
        let fix = if missing_secrets.is_empty() {
            "each ✗ above names the blocker".to_string()
        } else {
            let cmds: Vec<String> = missing_secrets
                .iter()
                .map(|n| format!("agentstack secret set {n}"))
                .collect();
            format!("fix: {} (or pass --allow-unresolved)", cmds.join(" · "))
        };
        anyhow::bail!(
            "{} on {} — {fix}",
            super::count(blocked_targets.len(), "blocked write"),
            super::count(blocked_targets.len(), "target")
        );
    }

    // Nothing reached any tool: every capability this project has routes live,
    // no bridge is registered anywhere, and the rendered lane had no content of
    // its own to carry. Exit code 1 (a plain `bail`), deliberately:
    //
    //  * A script that runs `apply --write` and reads exit 0 believes the
    //    project's tools are configured. Here they are not, and will not be
    //    until a second command runs — so 0 would be the same false success the
    //    validation gate above already refuses to give.
    //  * It is NOT nonzero merely because servers went live: a project whose
    //    bridge IS registered, or which still writes instructions/settings/
    //    hooks, delivered something and exits 0 with the notice above. The
    //    failure being reported is "nothing was delivered", not "routing
    //    happened".
    //  * No new exit code is invented. `apply` already fails with 1 on a
    //    refused write, and this is a refused delivery.
    if will_write && !live_unconnected.is_empty() && !rendered_content && written_count == 0 {
        anyhow::bail!(
            "nothing was delivered: every capability here is routed to the live lane and no \
             bridge is registered — {} · or write files anyway: agentstack x delivery \
             render-locally --write",
            super::delivery::CONNECT_THE_BRIDGE
        );
    }

    Ok(Outcome {
        changed_count,
        validation_errors: has_errors,
        write_blockers,
        written_count: if will_write { written_count } else { 0 },
        gitignore_pending,
    })
}

/// Print validation issues (unless `quiet`); return true if any are structural
/// errors. The error check runs regardless of `quiet` so a write is still gated
/// on a clean manifest.
fn print_validation(
    manifest: &crate::manifest::Manifest,
    target_ids: Vec<&str>,
    vctx: &ValidateCtx,
    quiet: bool,
) -> bool {
    let issues = validate_with_context(manifest, target_ids, vctx);
    let mut has_error = false;
    for issue in &issues {
        if issue.kind.is_error() {
            has_error = true;
        }
        if !quiet {
            let mark = if issue.kind.is_error() {
                "✗".red().to_string()
            } else {
                "⚠".yellow().to_string()
            };
            match &issue.fix {
                Some(fix) => crate::outln!("{mark} {} ↳ {fix}", issue.message),
                None => crate::outln!("{mark} {}", issue.message),
            }
        }
    }
    has_error
}

/// M5: the rendered file's body when `--verbose`, otherwise one summary line.
/// Kept next to [`diff_summary`] so the two halves of the same decision live
/// together — the caller only decides WHETHER to describe a change, never how.
fn print_body_or_summary(args: &ApplyArgs, diff: &str) {
    if args.verbose {
        crate::out!("{}", indent(diff));
    } else {
        crate::outln!("  {} {}", "~".cyan(), diff_summary(diff).dimmed());
    }
}

fn indent(s: &str) -> String {
    s.lines().map(|l| format!("  {l}\n")).collect::<String>()
}

/// M5: one line standing in for a rendered file's full contents — `+18 / -0
/// lines (new file)`. Apply used to print every rendered file in full on both
/// the dry run AND the write, which for four targets is ~100 lines of JSON and
/// TOML the user did not ask to read and cannot meaningfully check; the facts
/// they DO need (which file, how much moved, how to undo) were buried in it.
/// `--verbose` still prints the bodies.
///
/// Counted from the diff text rather than from a new plan API, because the diff
/// is already the authoritative rendering of "what changes" — a second
/// computation could disagree with the thing `--verbose` shows.
fn diff_summary(diff: &str) -> String {
    let mut added = 0usize;
    let mut removed = 0usize;
    // The diff arrives colorized, so a line starts with an ANSI escape rather
    // than its `+`/`-` marker. `sanitize_block` is the crate's one
    // escape-stripping path — reuse it instead of hand-rolling a second CSI
    // parser here (counting the raw text silently reported +0 / -0).
    let plain = crate::text::sanitize_block(diff);
    for line in plain.lines() {
        match line.trim_start().chars().next() {
            Some('+') => added += 1,
            Some('-') => removed += 1,
            _ => {}
        }
    }
    // Nothing removed and something added means the whole file is new content:
    // either a file that did not exist, or one whose managed region is being
    // written for the first time. Both read the same way to a user deciding
    // whether to look closer.
    let shape = if removed == 0 && added > 0 {
        " (new content)"
    } else {
        ""
    };
    format!("+{added} / -{removed} lines{shape}")
}

/// Dry-run summary suffix when a write would delete server entries — the
/// per-target "would REMOVE" lines name them; this keeps the count visible
/// next to the "would change" total (and the confirm prompt that follows it).
fn removal_note(removed: usize) -> String {
    if removed > 0 {
        format!(", REMOVING {removed} server entr{}", plural_y(removed))
    } else {
        String::new()
    }
}

fn plural_y(n: usize) -> &'static str {
    if n == 1 {
        "y"
    } else {
        "ies"
    }
}

/// Owned-server entries to rewrite, grouped by manifest layer file.
type OwnedRefreshByFile = Vec<(std::path::PathBuf, Vec<(String, crate::manifest::Server)>)>;

/// Group the stale owned servers by the manifest layer file that declares
/// them — the local overlay wins (it overrides at load time), then the
/// manifest itself. Servers declared elsewhere (central library, inherited
/// user layer) come back separately: this apply can't refresh those files,
/// only report them.
fn plan_owned_manifest_refresh(
    loaded: &crate::manifest::LoadedManifest,
    owned: &[crate::render::OwnedStatus],
) -> (OwnedRefreshByFile, Vec<String>) {
    let declares = |path: &Path, name: &str| -> bool {
        let Ok(text) = std::fs::read_to_string(path) else {
            return false;
        };
        let Ok(v) = text.parse::<toml::Value>() else {
            return false;
        };
        v.get("servers").and_then(|s| s.get(name)).is_some()
    };
    let mut by_file: OwnedRefreshByFile = Vec::new();
    let mut elsewhere: Vec<String> = Vec::new();
    for o in owned.iter().filter(|o| o.stale) {
        let file = loaded
            .local_path
            .as_deref()
            .filter(|p| declares(p, &o.name))
            .or_else(|| {
                declares(&loaded.manifest_path, &o.name).then_some(loaded.manifest_path.as_path())
            });
        match file {
            Some(f) => match by_file.iter_mut().find(|(p, _)| p == f) {
                Some((_, entries)) => entries.push((o.name.clone(), o.server.clone())),
                None => by_file.push((f.to_path_buf(), vec![(o.name.clone(), o.server.clone())])),
            },
            None => elsewhere.push(o.name.clone()),
        }
    }
    (by_file, elsewhere)
}

// ── `agentstack x unrender` ───────────────────────────────────────────────
//
// The command every surface names when it finds an abandoned server config.
// It exists because the honest report needed a runnable answer: `apply` no
// longer writes those files, and until this landed nothing took them off
// disk short of `x uninstall`, which removes everything.
//
// Deliberately narrow. It removes ONLY server configs that (a) this manifest
// recorded as managed and (b) the delivery planner now routes to the live
// lane. Settings, hooks, instructions and the `.gitignore` block are still
// rendered for these harnesses, so removing them here would break the very
// delivery this command exists to clean up after. The machine-wide exit is
// still `agentstack x uninstall`.
//
// The removal itself is `unrender::plan` — the ordinary render path against an
// empty manifest — filtered to those files. No second deletion path, and every
// write is captured into history, so this is undoable with `agentstack
// restore` like any other.

/// `agentstack x unrender` — take an abandoned server config back off disk.
pub fn run_unrender(args: &crate::cli::UnrenderArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let ctx = super::load(manifest_dir)?;
    let state = State::load()?;
    let manifest = &ctx.loaded.manifest;
    let target_ids = resolve_targets(manifest, &ctx.registry, &args.targets, &ctx.dir)?;
    let delivery = crate::delivery::Plan::build(&manifest.delivery, &ctx.registry, &target_ids);
    let scopes = [Scope::Project, Scope::Global];
    let found = abandoned_live_renders(&ctx, &delivery, &state, &scopes);

    if found.is_empty() {
        crate::outln!(
            "Nothing to un-render — no server config AgentStack wrote is left over from the \
             rendered lane here."
        );
        return Ok(());
    }

    // `own_global_only = true`: a global-scope file another manifest recorded
    // is not ours to plan away, exactly as the mode switch treats it.
    let planned = super::unrender::plan(&ctx, &state, &scopes, /*own_global_only=*/ true)?;
    let wanted: std::collections::BTreeSet<std::path::PathBuf> =
        found.iter().map(|f| f.path.clone()).collect();
    // Path AND leg: a couple of adapters keep settings in a neighbouring file,
    // and only the servers leg is abandoned here.
    let removals: Vec<super::unrender::Removal> = planned
        .removals
        .into_iter()
        .filter(|r| wanted.contains(&r.path) && r.label.contains("· servers ("))
        .collect();

    let root = crate::manifest::project_root_of(&ctx.dir);
    crate::outln!(
        "{}\n",
        if args.write {
            "Removing the server configs the live lane left behind:".bold()
        } else {
            "Left behind by the rendered lane. Nothing has been changed yet:".bold()
        }
    );
    for f in &found {
        crate::outln!("  {}", f.sentence());
    }
    if removals.is_empty() {
        // The file is on disk and live, but the planner finds nothing of ours
        // in it — either AgentStack never wrote it (a clone, a checkout) or
        // what is left belongs to someone else. Say exactly that rather than
        // reporting a clean removal.
        crate::outln!(
            "\n  {} nothing in {} is ours to remove — edit it by hand if you want it gone.",
            "⚠".yellow(),
            super::count(found.len(), "file")
        );
        return Ok(());
    }
    crate::outln!();
    for r in &removals {
        crate::outln!(
            "  {}  {}",
            r.label.bold(),
            super::init::display_path(&r.path, &root).dimmed()
        );
        if args.verbose {
            for line in r.diff.lines() {
                crate::outln!("    {line}");
            }
        }
    }

    if !args.write {
        crate::outln!(
            "\n  {} re-run with {} to remove them.",
            "·".dimmed(),
            "--write".bold()
        );
        return Ok(());
    }

    let mut backups = Vec::new();
    let mut labels = Vec::new();
    let mut removed = 0usize;
    let mut pruned = 0usize;
    for r in removals {
        let capture = r
            .capture
            .then(|| crate::history::capture(&r.path, r.label.clone()));
        (r.write)()?;
        // Report what happened, not what was planned: the write only deletes
        // the file when nothing but our entries was in it. A file that still
        // holds a foreign entry survives, and calling that "removed" was the
        // summary contradicting its own diff.
        if r.path.exists() {
            pruned += 1;
            crate::outln!(
                "  {} {} {}",
                "✓".green(),
                "removed our entries from".dimmed(),
                super::init::display_path(&r.path, &root)
            );
        } else {
            removed += 1;
            crate::outln!(
                "  {} {} {}",
                "✓".green(),
                "removed".dimmed(),
                super::init::display_path(&r.path, &root)
            );
        }
        if let Some(capture) = capture {
            backups.push(capture);
            labels.push(r.label);
        }
    }
    crate::history::record("project", "unrender servers", labels, backups)?;

    // DELIBERATELY: the ledger is NOT cleared here.
    //
    // It used to be, to stop the next `apply`/`doctor`/`status` reporting the
    // same abandoned file forever. That reason is gone: the detector
    // (`abandoned_at`) now triggers on the FILE BEING ON DISK, not on the
    // ledger, so a removed file stops being reported because it is removed.
    // Clearing would now do only harm. `unrender::plan` builds its removals
    // from `state.managed_servers`, so a cleared ledger means the file cannot
    // be taken off disk a SECOND time — and it comes back routinely, via
    // `x restore`, `undo`, or a `git checkout` of a committed config. Keeping
    // the record is what makes the second removal possible, and what lets the
    // warning still say "AgentStack wrote it" instead of demoting a file we
    // did write to the foreign wording.
    // The summary counts what the disk shows, not what was planned: files that
    // survived because a foreign entry is still in them were pruned, not
    // removed, and saying "removed" for them contradicts the lines above.
    if pruned > 0 {
        crate::outln!(
            "\n{} removed, {} pruned (a foreign entry keeps them on disk).",
            super::count(removed, "file"),
            super::count(pruned, "file"),
        );
    } else {
        crate::outln!("\n{} removed.", super::count(removed, "file"));
    }
    crate::outln!("  {}", "undo: agentstack x restore --last --write".dimmed());
    Ok(())
}
