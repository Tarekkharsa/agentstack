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
//! # It ends through the next-action seam, not a summary line
//!
//! The last line is [`super::doctor`]'s `next_action`, unmodified. That matters
//! more than it looks: `up` characteristically finishes in a state that is
//! *not* finished — the secrets are this machine's to set, and a checkout the
//! user has not reviewed is not trusted. A bespoke "✓ ready" would be the
//! false-ready bug all over again, invented fresh at the one moment a user is
//! least equipped to notice. Whatever `doctor` would honestly tell them to do
//! next is what `up` says, because it *is* what `doctor` says.

use anyhow::Result;
use owo_colors::OwoColorize;

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
        if harnesses.is_empty() {
            "none — install a supported CLI, or run `agentstack doctor` to see what is looked for"
                .dimmed()
                .to_string()
        } else {
            harnesses.join(" · ")
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
    let verified = super::install::run(
        &InstallArgs {
            locked: true,
            allow_flagged: false,
        },
        Some(&dir),
    );
    match &verified {
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
            // Not a bail: the render below is still worth attempting for the
            // parts that do verify, and the closing next action will name the
            // repair. Failing the whole command here would leave a user on a
            // new machine with one error and no configured CLI at all.
            crate::outln!("{:<20}{shape}", "your environment".dimmed());
            crate::outln!(
                "{:<20}{} {err:#}",
                "",
                "could not verify against lock —".yellow()
            );
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
    if let Err(err) = super::apply::write_quiet(&apply, Some(&dir)) {
        // The most common cause on a fresh machine, by far, is that the
        // checkout has never been reviewed on THIS machine — trust is
        // per-machine by design. Say what happened rather than what we
        // guessed; the closing next action already knows to send them to the
        // review, because `doctor` reads the same trust state.
        //
        // "stopped early" not "nothing rendered": `apply` can fail on one
        // target AFTER writing others (an IO error on the fourth of four),
        // and claiming nothing happened over three written configs is the
        // kind of false all-or-nothing this doc pass exists to remove. The
        // closing `doctor` reads the real on-disk state.
        crate::outln!("  {} {err:#}", "rendering stopped early —".yellow());
    }

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
    Ok(())
}

/// The CLIs this machine has, in registry order, by display name. Uses the
/// same `detected` reading `init` imports from and `doctor` counts, so the
/// three can never disagree about what is installed.
fn detected_harnesses(dir: &Path) -> Vec<String> {
    let Ok(registry) = crate::adapter::Registry::load() else {
        return Vec::new();
    };
    let project = Scope::default_for(dir) == Scope::Project;
    registry
        .iter()
        .filter(|d| d.detected() || (project && d.project_config_present(dir)))
        .map(|d| d.display.clone())
        .collect()
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
