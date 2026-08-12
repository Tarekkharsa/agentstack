//! The funnel's activation half (STRATEGY.md Phase 1, slice B): one action
//! that takes locally-authored dropped files from "sitting in a directory" to
//! "live in every CLI", behind one combined preview and one confirmation.
//!
//! **The collapse is presentation, not semantics.** Internally this is still
//! declare → lock → trust → render, in that order, through the same functions
//! the explicit path uses. Nothing here constructs a grant, computes a digest,
//! or writes a pin of its own: `trust::grant_with_card` reaches the same
//! `grant_gated` that `agentstack trust` does, so the audit trail this leaves
//! is byte-identical to the trail the explicit sequence leaves. That is a
//! witness (`crates/cli/tests/funnel_activation.rs`), not an intention.
//!
//! **What may take this path.** Only first-time adoption of inert, locally
//! authored content:
//!
//! - Provenance must pass — [`crate::intake::Provenance::is_local`]. Content
//!   that arrived with the project keeps the full staged review.
//! - A name the manifest already declares is never in scope (slice A reports
//!   the collision instead of adopting it).
//! - Servers are excluded: they carry commands, env, and secrets, and there is
//!   no file to drop. Hooks and extensions are excluded entirely — executable
//!   kinds always get the full ceremony.
//! - Re-gates of changed content are NOT compressed. This path runs when there
//!   is qualifying new content to declare; a project whose *existing* content
//!   drifted is sent to the explicit path until the Phase 2 diff card exists.
//!
//! Disqualified content is named in the preview with the reason and the exact
//! command that reviews it properly — routing around it silently would make
//! the provenance split invisible, which is the opposite of the point.

use std::path::Path;

use std::io::IsTerminal;

use agentstack_core::paint::OwoColorize;
use anyhow::{Context, Result};

use crate::cli::YesArgs;
use crate::intake;

/// What the funnel decided it may act on, and what it refused.
struct Plan {
    /// Content that may take the compressed path.
    qualified: Vec<intake::Item>,
    /// Content held back, each with the reason the user is shown.
    held: Vec<(String, String)>,
}

fn plan(found: &intake::Found) -> Plan {
    let mut qualified = Vec::new();
    let mut held = Vec::new();
    for item in &found.items {
        if item.provenance.is_local() {
            qualified.push(item.clone());
        } else {
            held.push((
                format!("{} {}", item.kind.noun(), item.name),
                format!("{} — full review required", item.provenance.reason()),
            ));
        }
    }
    for c in &found.collisions {
        held.push((
            format!("{} {}", c.kind.noun(), c.name),
            "that name is already declared — rename the file or remove the existing entry"
                .to_string(),
        ));
    }
    Plan { qualified, held }
}

pub fn run(args: &YesArgs, manifest_dir: Option<&Path>) -> Result<()> {
    run_gated(args, manifest_dir, std::io::stdin().is_terminal())
}

/// The funnel with the TTY probe injected, mirroring how `trust` makes its own
/// consent gate testable.
///
/// The funnel is deliberately an **interactive** verb. Its whole design is a
/// review printed to a human followed by that human's yes; a headless caller
/// has no one to show it to, and letting `--yes` alone satisfy the gate here
/// would reopen exactly what §7.2 closed on `trust` — a caller asserting
/// consent nobody gave. Headless callers keep the explicit path, where
/// `--consented` binds the acknowledgement to previewed bytes.
pub fn run_gated(args: &YesArgs, manifest_dir: Option<&Path>, interactive: bool) -> Result<()> {
    run_answered(args, manifest_dir, interactive, None)
}

/// [`run_gated`] with the confirmation's answer supplied rather than read from
/// stdin. `None` prompts, which is what production does; the witnesses use it
/// to exercise the decline path, whose whole job is to leave nothing behind.
pub fn run_answered(
    args: &YesArgs,
    manifest_dir: Option<&Path>,
    interactive: bool,
    answer: Option<bool>,
) -> Result<()> {
    if !interactive {
        anyhow::bail!(
            "`agentstack yes` needs a terminal — it is a review you read and answer. \
             Headlessly, use the explicit path: `agentstack adopt --write`, \
             `agentstack lock --write`, then `agentstack trust --yes --consented <digest>` \
             (from `agentstack trust --preview`), then `agentstack use --write`."
        );
    }
    let ctx = super::load(manifest_dir)?;
    let base = crate::manifest::project_root_of(&ctx.dir);
    let found = intake::scan(&ctx.dir, &base, &ctx.loaded.manifest);
    let plan = plan(&found);

    if plan.qualified.is_empty() {
        report_nothing_to_do(&plan, &base);
        return Ok(());
    }

    // ── The combined preview, part one: what will be declared and pinned.
    //
    // Declaring and pinning deliver nothing (they are the inert half, witnessed
    // in slice A), but they still write files, so they are disclosed before
    // they run and their undo is named here rather than after the fact.
    println!(
        "{} {} ready to go live in this project:\n",
        "→".cyan(),
        super::count(plan.qualified.len(), "dropped file")
    );
    for item in &plan.qualified {
        println!(
            "  {} {} {} {}",
            "+".green(),
            item.kind.noun(),
            item.name.bold(),
            format!("({})", item.rel_path).dimmed()
        );
        if let Some(summary) = &item.summary {
            println!("      {}", summary.dimmed());
        }
        // Rider 2: the provenance line travels with the item into the combined
        // preview. Why this content earned the short path is part of what the
        // user is agreeing to.
        println!(
            "      {}",
            format!("your own work — {}", item.provenance.reason()).dimmed()
        );
    }
    print_held(&plan);
    // The undo named here is the one the ledger row below actually contains:
    // the manifest declaration and the lock pin. It deliberately does not claim
    // to revert everything the funnel writes — the files delivered into each
    // CLI are not in that row, and a promise wider than the mechanism is worse
    // than a narrow true one. (The earlier, wider wording is asserted absent by
    // the witness, so it cannot be quoted here either.)
    // `use --write` is what reconciles the delivered files afterwards, and it
    // is named so the sentence ends somewhere useful rather than in a caveat.
    println!(
        "\n{}",
        "This declares them in the manifest and pins their bytes, then shows you the \
         full review below. Undo the declaration and pin with `agentstack x restore \
         --last --write`; `agentstack use --write` then reconciles what each CLI holds."
            .dimmed()
    );

    // ── declare (the same single insertion path `add`/`adopt` use)
    let manifest_text = std::fs::read_to_string(&ctx.loaded.manifest_path)
        .with_context(|| format!("reading {}", ctx.loaded.manifest_path.display()))?;
    let mut new_text = manifest_text.clone();
    for item in &plan.qualified {
        new_text = super::add::build_manifest_with(
            &new_text,
            item.kind.section(),
            &item.name,
            &super::adopt::intake_entry(item),
            None,
        )
        .with_context(|| {
            format!(
                "cannot declare {} '{}' — fix the manifest's TOML syntax with `agentstack doctor`",
                item.kind.noun(),
                item.name
            )
        })?;
    }
    // Everything the inert half is about to change, remembered so that saying
    // no can put it all back. "One action" has to mean one *reversible* action:
    // a preview that already edited the manifest by the time it asks is not a
    // preview, and "cancelled — nothing happened" has to be literally true.
    let before = Rollback::capture(
        &ctx.loaded.manifest_path,
        &crate::lock::Lock::path(&ctx.dir),
    );

    // F12: Ctrl-C between the declare below and the closing yes must be
    // transactional. Without this guard, SIGINT's default disposition kills
    // the process mid-`activate` — after the manifest write, before the
    // history row — leaving a declared, locked project that `undo` says
    // "nothing recorded" about. The guard turns Ctrl-C into an observable
    // cancellation (the blocked prompt read returns instead of the process
    // dying), so the same rollback a typed `n` takes runs here too, and
    // "cancelled — nothing happened" stays literally true. Interactive only:
    // a headless caller has no prompt to interrupt.
    let sigint = interactive
        .then(crate::sys::SigintGuard::install)
        .transpose()?;

    crate::util::atomic::write(&ctx.loaded.manifest_path, &new_text)
        .with_context(|| format!("writing {}", ctx.loaded.manifest_path.display()))?;

    // One yes is one undoable action, and the activation below is the other
    // half of it. `use --write` records its own row for what it renders (the
    // managed `.gitignore` block, each CLI's own files), so two rows land for
    // one yes — and without a batch the `restore --last` promised twice on this
    // screen would reverse only the newest of them, leaving the declaration it
    // was activating in place. The batch makes `--last` reverse the whole
    // funnel, newest phase first: the same seam `setup` uses, for the same
    // reason. Installed here so the funnel's own row below joins it too.
    let _history_batch = crate::history::begin_batch("yes");

    // The rest runs inside a closure so one `?` cannot escape past the
    // rollback. Only a granted, rendered run leaves the writes in place.
    let outcome = activate(&ctx, &base, args, interactive, answer);
    let interrupted = sigint.as_ref().is_some_and(|g| g.interrupted());
    if outcome.is_err() || interrupted {
        before.restore();
    }
    if interrupted {
        // A cancel, not a failure: the project is byte-identical to before
        // this ran, and there is nothing to record or report as an error.
        println!(
            "\n{} cancelled — nothing was declared or activated.",
            "·".dimmed()
        );
        return Ok(());
    }
    outcome?;

    // The write is done and kept — so it becomes an ordinary undoable row.
    //
    // Until this call existed, `yes` promised an undo and then recorded
    // nothing: on a skills-only project the ledger stayed empty, so both
    // `undo` and `restore --list` answered "nothing recorded" and the promised
    // undo did not exist. (The exact old wording is quoted in
    // `tests/yes_is_undoable.rs` and in the changelog — not here, because a
    // witness asserts that phrasing is absent from this file.) The recording
    // seam is the one `apply` and `init` already use; this reuses it rather
    // than adding a second history path.
    //
    // Best-effort, like every other caller: `record` swallows its own failures
    // and a lost ledger row must never fail an activation that already
    // succeeded. It cannot gate — there is nothing after it to gate.
    let names: Vec<String> = plan.qualified.iter().map(|i| i.name.clone()).collect();
    if let Err(err) = crate::history::record("project", "yes", names, before.as_changes()) {
        // Say so rather than swallow it silently: the next line promises an
        // undo, and if the row is missing that promise is false. The activation
        // itself stands.
        println!(
            "  {} could not record this in the undo history ({err:#}) — \
             the change is live, but `agentstack restore` will not list it",
            "⚠".yellow()
        );
    }

    println!(
        "\n{} live. {}",
        "✓".green(),
        "Undo the declaration and pin: `agentstack x restore --last --write`".dimmed()
    );
    Ok(())
}

/// lock → combined preview → the one yes → render. Split out so its failures
/// (including a declined confirmation) unwind the declarations above.
fn activate(
    ctx: &super::Context,
    base: &Path,
    args: &YesArgs,
    interactive: bool,
    answer: Option<bool>,
) -> Result<()> {
    // Re-read the manifest: `ctx` was loaded before the declarations above were
    // written, so asking it what this project declares would answer for the
    // state we just left behind.
    let fresh = super::load(Some(&ctx.dir))?;

    // ── lock (a prerequisite of trust: the pins are part of the consent digest)
    super::lock::run(&lock_args(), Some(&ctx.dir))
        .context("cannot pin the new content — fix what `agentstack lock` reports and re-run")?;

    // ── The combined preview, part two: what activation will write, and the
    // full trust surface — both inside the one screen, before the one yes.
    //
    // The render plan is the REAL dry run, not a description of it: the same
    // `use` code path with `--write` off. A preview that cannot drift from what
    // it previews is worth more than a prettier summary that can.
    println!("\n{}", "This will write:".bold());
    super::use_profile::run(&use_args(false), Some(&ctx.dir))
        .context("cannot preview activation — fix the error above and re-run")?;
    // Skills and servers are `use`'s job; instruction fragments compile into
    // the managed regions of CLAUDE.md/AGENTS.md through their own command.
    // Both belong to "live everywhere", so both are previewed and both run —
    // an adopted instruction that was declared, pinned, consented to, and then
    // never compiled would be the funnel quietly stopping one step short.
    let has_instructions = !fresh.loaded.manifest.instructions.is_empty();
    if has_instructions {
        super::instructions::run(&instructions_args(false), Some(&ctx.dir))
            .context("cannot preview the instruction compile — fix the error above and re-run")?;
    }

    let card = super::trust::ConsentCard {
        lines: Vec::new(),
        question: "\nMake this live?".to_string(),
        answer,
    };
    super::trust::grant_with_card(base, args.yes, interactive, &card)?;

    // ── render (the same activation paths `use --write` and
    // `instructions --write` run)
    super::use_profile::run(&use_args(true), Some(&ctx.dir))
        .context("consent was granted, but activation failed — fix the error above and re-run `agentstack use --write`")?;
    if has_instructions {
        super::instructions::run(&instructions_args(true), Some(&ctx.dir))
            .context("consent was granted and skills activated, but compiling instructions failed — re-run `agentstack instructions --write`")?;
    }
    Ok(())
}

/// The manifest and lockfile bytes as they were before the funnel touched
/// them. `None` for a file that did not exist, so restoring removes it again
/// rather than leaving an empty one behind.
struct Rollback {
    manifest: crate::history::FileChange,
    lock: crate::history::FileChange,
}

impl Rollback {
    /// Taken through [`crate::history::capture`] itself, at the moment before
    /// the funnel writes. That is the only instant at which the pre-write bytes
    /// AND the not-yet-existing parent directories can both be read; a snapshot
    /// assembled later would record the new bytes as though they were the old
    /// ones, and would see every directory as pre-existing.
    fn capture(manifest: &Path, lock: &Path) -> Self {
        Self {
            manifest: crate::history::capture(manifest, "manifest · yes"),
            lock: crate::history::capture(lock, "lock · yes"),
        }
    }

    /// The same two captures, as ledger rows — so the undo the user is offered
    /// is bound to the identical snapshot the failure path below would restore.
    fn as_changes(&self) -> Vec<crate::history::FileChange> {
        vec![self.manifest.clone(), self.lock.clone()]
    }

    /// Best-effort by design: this runs while another error is already on its
    /// way up, and a failure to restore must not replace that error with a
    /// worse one. `restore --last --write` remains the user-facing undo.
    fn restore(&self) {
        let _ = crate::history::rollback(&self.as_changes());
    }
}

fn print_held(plan: &Plan) {
    if plan.held.is_empty() {
        return;
    }
    println!(
        "\n  {}",
        "Not included — these take the full review:".dimmed()
    );
    for (what, why) in &plan.held {
        println!("  {} {what} — {}", "·".dimmed(), why.dimmed());
    }
    println!(
        "  {}",
        "review them with `agentstack adopt`, then `agentstack adopt --write` → `agentstack lock --write` → `agentstack trust .`".dimmed()
    );
}

fn report_nothing_to_do(plan: &Plan, base: &Path) {
    if plan.held.is_empty() {
        println!(
            "Nothing new to activate — no undeclared files are waiting in this project.\n\
             {}",
            "  (`agentstack status` shows where this project stands.)".dimmed()
        );
        return;
    }
    println!("Nothing here can take the short path — every waiting file needs the full review:\n");
    for (what, why) in &plan.held {
        println!("  {} {what} — {}", "·".dimmed(), why.dimmed());
    }
    println!(
        "\n{}",
        format!(
            "Review them: `agentstack adopt`, then `agentstack adopt --write` → `agentstack lock --write` → `agentstack trust {}`",
            base.display()
        )
        .dimmed()
    );
}

fn lock_args() -> crate::cli::LockArgs {
    crate::cli::LockArgs {
        quiet: true,
        ..Default::default()
    }
}

fn instructions_args(write: bool) -> crate::cli::InstructionsArgs {
    crate::cli::InstructionsArgs {
        write,
        ..Default::default()
    }
}

fn use_args(write: bool) -> crate::cli::UseArgs {
    crate::cli::UseArgs {
        write,
        quiet: true,
        ..Default::default()
    }
}
