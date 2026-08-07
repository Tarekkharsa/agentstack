//! `agentstack up` — one command, and the environment materializes.
//!
//! Strategy v2 Phase 4, "one-command materialization". The scenario is a fresh
//! machine holding a checkout and nothing else: the manifest and lock are
//! there, the harnesses are installed, and none of it is wired together. Every
//! piece needed to fix that already existed — `install --locked` to verify the
//! environment against the lock, `apply --write` to render each CLI's config,
//! `doctor` to say where things stand. What did not exist was one command that
//! ran them in the right order and left the user with a single true sentence.
//!
//! # This composes; it does not write
//!
//! Everything here that touches disk does so through a command that already
//! owned that write: [`super::install::run`] for the lock reconciliation and
//! [`super::apply::write_quiet`] for the render. There is deliberately no
//! filesystem call in this module — a witness asserts it — because a second
//! writing path would be a second place for the consent and undo work of the
//! last three phases to be got wrong.
//!
//! # Its exit code is `apply`'s, and the lock's
//!
//! Because the render IS [`super::apply::write_quiet`], `up` reports whatever
//! that call reported: a refused write is a refused write whichever command a
//! script ran. It finishes the transcript first — the closing next step prints
//! even on a failure, since an exit code with no way forward is worse than none
//! — and then exits nonzero.
//!
//! The lock verification is judged the same way, and used not to be. It printed
//! a yellow "could not verify against lock" and left the exit code at 0, four
//! lines below a comment calling it "the one step of `up` that must not be
//! best-effort" — so a CI job that ran `up` and read success had an environment
//! nothing had ever checked against `agentstack.lock`, which is the single
//! guarantee `--locked` exists to give. CONTINUING after a failure and
//! SUCCEEDING are separable: `up` still runs the render and still prints the
//! closing next step, and then reports the failure it saw.
//!
//! # It ends through the next-action seam, not a summary line
//!
//! The last line is [`super::doctor`]'s `next_action`, unmodified. That matters
//! more than it looks: `up` characteristically finishes in a state that is
//! *not* finished — the secrets are this machine's to set, and a checkout the
//! user has not reviewed is not trusted. A bespoke "✓ ready" would be the
//! false-ready bug all over again, invented fresh at the one moment a user is
//! least equipped to notice. Whatever `doctor` would honestly tell them to do
//! next is what `up` says, because it *is* what `doctor` says.

use agentstack_core::paint::OwoColorize;
use anyhow::Result;

use std::path::Path;

use crate::cli::{ApplyArgs, InstallArgs, UpArgs};
use crate::scope::Scope;

pub fn run(args: &UpArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let base = super::project_base(manifest_dir)?;
    let dir = crate::manifest::resolve_manifest_dir(&base);
    let scope = Scope::default_for(&dir);

    // ---------------------------------------------------------- harnesses
    // What this machine actually has. Reported first because it is the one
    // fact the user cannot check for themselves at a glance, and because an
    // empty list changes the meaning of everything below it.
    let harnesses = detected_harnesses(&dir);
    crate::outln!(
        "{:<20}{}",
        "found harnesses".dimmed(),
        match &harnesses {
            // OUR failure, said as ours. An unreadable `~/.agentstack/adapters`
            // — or any other reason the registry will not load — used to be
            // swallowed into the empty list below, so the screen told the user
            // to install a CLI they may well have installed already. "none"
            // is a finding about THEIR machine and must only be printed when
            // the lookup that produced it actually ran.
            Err(err) => format!("unknown — the adapter registry did not load: {err:#}")
                .red()
                .to_string(),
            Ok(found) if found.is_empty() =>
                "none — install a supported CLI, or run `agentstack doctor` to see what is looked for"
                    .dimmed()
                    .to_string(),
            Ok(found) => found.join(" · "),
        }
    );

    // -------------------------------------------------------- environment
    let ctx = super::load(Some(&dir))?;
    let m = &ctx.loaded.manifest;
    let shape = format!(
        "{} · {} · {}",
        count(m.profiles.len(), "toolset"),
        count(m.skills.len(), "skill"),
        count(m.servers.len(), "server"),
    );

    // `--locked` is the whole point of doing this here: it verifies the
    // resolved environment against `agentstack.lock` and REFUSES to change the
    // lockfile rather than quietly re-pinning. On a fresh machine that is the
    // difference between "you got what was reviewed" and "you got whatever
    // upstream is today", and it is the one step of `up` that must not be
    // best-effort.
    let mut lock_failed: Option<anyhow::Error> = None;
    let verified = super::install::run(
        &InstallArgs {
            locked: true,
            allow_flagged: false,
        },
        Some(&dir),
    );
    match verified {
        // "Verified against lock" is a claim about what was EXAMINED, and
        // `install` examines skill sources. A manifest with no skills gives it
        // nothing to check, and printing the green claim anyway would be the
        // same vacuous pass P3.1 removed from `doctor` — a line that looks like
        // evidence and is only an absence. So the claim is made only when
        // something backed it, and the absence is stated in words instead.
        Ok(()) if m.skills.is_empty() => crate::outln!(
            "{:<20}{shape} · {}",
            "your environment".dimmed(),
            "no pinned skill sources to verify".dimmed()
        ),
        Ok(()) => crate::outln!(
            "{:<20}{shape} · {} {}",
            "your environment".dimmed(),
            count(m.skills.len(), "skill source").green(),
            "verified against lock".green()
        ),
        Err(err) => {
            // Not a bail HERE, and not a pass either — the same shape the
            // render step below uses (see the verdict at the end). Bailing on
            // the spot would leave a user on a new machine with one error and
            // no configured CLI at all, so the render is still attempted and
            // the closing next step still prints. What is NOT deferred is the
            // verdict: `--locked` is the whole reason this step exists, and an
            // exit code of 0 over an environment it never managed to check
            // says "you got what was reviewed" when nothing established that.
            crate::outln!("{:<20}{shape}", "your environment".dimmed());
            crate::outln!("{:<20}{} {err:#}", "", "NOT verified against lock —".red());
            lock_failed = Some(err);
        }
    }

    // ----------------------------------------------------------- secrets
    // Reported BEFORE the render, not after, because it is the render's most
    // likely cause of failure on a fresh machine. Said afterwards it reads as
    // a second, unrelated problem; said first it is the explanation the user
    // already has in hand when the render reports blocked targets.
    //
    // This is also the one category of work that is genuinely this machine's
    // and cannot be carried in a manifest by design (invariant 5: manifests
    // hold `${REF}`, never values). Naming the exact command per ref is the
    // difference between a status line and a to-do list.
    let refs = m.referenced_secrets();
    let sources = crate::secret::SecretSources::detect(&dir);
    let missing: Vec<&String> = refs
        .iter()
        .filter(|r| sources.source_of(r).is_none())
        .collect();
    if missing.is_empty() {
        if !refs.is_empty() {
            crate::outln!(
                "{:<20}{}",
                "secrets".dimmed(),
                format!(
                    "{} resolved on this machine",
                    count(refs.len(), "reference")
                )
                .green()
            );
        }
    } else {
        crate::outln!(
            "{:<20}{} need this machine's vault:",
            "secrets".dimmed(),
            missing.len()
        );
        for name in &missing {
            crate::outln!(
                "{:<20}${{{name}}} {} agentstack secret set {name}",
                "",
                "→".dimmed()
            );
        }
        // Describes the SHIPPED rule, not the nicer one.
        //
        // `render` blocks a whole target's SERVER config while any `${REF}` in
        // the selection is unresolved — a documented fail-closed boundary, not
        // an oversight. It is not per-server (a ref-less server in the same
        // project is held back too), and it is not per-CLI-whole either: a
        // target's instructions and settings do not depend on the secret and
        // still render. The sentence below carries all three facts — and the
        // `up_materializes` witness holds it to them: "held back whole" must
        // appear (the ref-less server is held back too — per-server phrasing
        // like "the configs that need them" reads as a rule the product does
        // not have), and per-server pausing language must not.
        //
        // Relaxing the rule to per-server is tracked as its own reviewed item;
        // until then this line says what actually happens.
        crate::outln!(
            "{:<20}{}",
            "",
            "until they are set, each CLI's server config is held back whole — even servers \
             that need no secret; nothing is written with a missing credential (fail \
             closed); instructions and settings still render"
                .dimmed()
        );
    }

    // ------------------------------------------------------------ render
    let apply = ApplyArgs {
        targets: args.targets.clone(),
        profile: args.profile.clone(),
        dry_run: false,
        write: true,
        scope: Some(scope),
        // Fail closed on an unresolved `${REF}`, which on a fresh machine is
        // the EXPECTED case rather than an error: the server is left out of
        // the rendered config instead of being written with a literal
        // `${REF}` a CLI would send upstream. The secrets block below is what
        // turns that refusal into something the user can act on.
        allow_unresolved: false,
        prune_foreign: false,
        no_gitignore: args.no_gitignore,
        verbose: false,
    };
    crate::outln!("{}", "rendered".dimmed());
    // Held, not returned yet: the closing next step below is the whole reason
    // `up` keeps going after a failed render, and it must still print. What is
    // NOT deferred is the verdict — see the bail at the end of this function.
    let render_failed = super::apply::write_quiet(&apply, Some(&dir)).err();

    // ------------------------------------------------------- one next step
    // The P3.1 seam, not a summary of our own. See the module docs: `up` ends
    // in an unfinished state often enough that inventing a closing verdict
    // here is how a false "ready" would get born.
    //
    // `next_step`, not `next_action`: this is a human surface. `next_action`
    // is the machine field and is null whenever the honest next step is not a
    // runnable command (an empty trusted project's is `agentstack search
    // <query>`), which would end `up` with no closing line at all.
    let report = super::doctor::collect(Some(&dir))?;
    if let Some(next) = report["next_step"].as_str() {
        crate::outln!("\n{} {}", "next:".bold(), next.bold());
    }

    // ------------------------------------------------------------ the verdict
    // `up`'s render step IS `apply --write`, so the two may not disagree about
    // whether it worked. They did: on a project where every capability routes
    // live with no bridge registered, `apply` calls that a refused delivery and
    // exits 1, while `up` printed the same refusal and exited 0 — and `up` is
    // the documented new-machine command, so the reading a script gets from it
    // is the one that matters most.
    //
    // Adopting apply's code rather than classifying failures here is deliberate:
    // `apply` refuses a write for a blocked target, a hard IO failure, or a
    // delivery of nothing, and telling those apart from this side means matching
    // on its error text — a second, drifting copy of apply's exit policy inside
    // the command whose design rule is that it composes and owns no policy of
    // its own. Every one of those states means the environment did not
    // materialize, which is the single claim `up` exists to make.
    //
    // The failure is stated ONCE, here, rather than printed above and repeated
    // as the process error: `apply`'s message already names the repair for each
    // of those states (the bridge command, the override, `agentstack secret set
    // <REF>`), and the closing next step has already printed above it, so the
    // user ends with an exit code, a reason, and two things to do.
    //
    // "stopped early", not "nothing rendered": `apply` can fail on one target
    // AFTER writing others (an IO error on the fourth of four), and claiming
    // nothing happened over three written configs would be its own false
    // report. What is on disk is whatever the closing `doctor` just read.
    //
    // The lock verification is judged here for the same reason and by the same
    // rule. It is a SEPARATE claim from the render — "this is the environment
    // that was reviewed" versus "it reached your CLIs" — so when both fail the
    // user is told both rather than the first one to be noticed.
    match (lock_failed, render_failed) {
        (Some(lock), Some(render)) => anyhow::bail!(
            "not verified against the lock ({lock:#}) — and rendering stopped early: {render:#}"
        ),
        (Some(lock), None) => anyhow::bail!(
            "not verified against the lock — this is not established to be the environment that \
             was reviewed: {lock:#}"
        ),
        (None, Some(render)) => anyhow::bail!("rendering stopped early — {render:#}"),
        (None, None) => Ok(()),
    }
}

/// The CLIs this machine has, in registry order, by display name. Uses the
/// same `detected` reading `init` imports from and `doctor` counts, so the
/// three can never disagree about what is installed.
///
/// Returns the registry's error rather than an empty list, because the two mean
/// opposite things and the caller prints a different sentence for each. "no CLI
/// is installed" is a fact about the user's machine that they can act on; "the
/// registry did not load" is a fact about ours, and reporting it as the first
/// was the surface blaming the user for our failure — measured with a
/// `~/.agentstack/adapters` directory this process cannot read, which produced
/// a line byte-identical to a clean machine with no CLI on it.
fn detected_harnesses(dir: &Path) -> Result<Vec<String>> {
    let registry = crate::adapter::Registry::load()?;
    let project = Scope::default_for(dir) == Scope::Project;
    Ok(registry
        .iter()
        .filter(|d| d.detected() || (project && d.project_config_present(dir)))
        .map(|d| d.display.clone())
        .collect())
}

/// "3 toolsets" / "1 skill" — pluralized, because a status line that says
/// "1 skills" reads as generated rather than written, and this is the first
/// sentence a user sees on a new machine.
fn count(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("{n} {noun}")
    } else {
        format!("{n} {noun}s")
    }
}
