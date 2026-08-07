//! `agentstack x uninstall` — take everything AgentStack put on this machine
//! back off it, previewing first.
//!
//! Reversibility is the product's central promise, and until this existed the
//! promise had a hole: `restore` undoes *one recorded write*, but nothing
//! answered "remove all of it". A tool that asks to manage nine other tools'
//! configuration needs a guaranteed exit, or trying it is not a small decision
//! (review finding F06).
//!
//! The removal planning lives in [`super::unrender`] — the ordinary render
//! path run against an empty manifest, shared with `set-mode`'s un-render leg
//! so the machine exit and a delivery-mode switch can never disagree about
//! what "nothing of ours rendered here" means. Every FILE edit goes through
//! the same history capture, so those are undoable with `agentstack restore`
//! right up until the ledger is removed. The skills leg is the exception —
//! `capture: false`, because the ledger holds bytes and a delivered skill is a
//! linked directory (G31; see [`crate::history`]) — so the closing copy says
//! which half that restore covers: [`print_skills_bound`].
//!
//! What it does NOT touch, on purpose: the project's `agentstack.toml` (the
//! thing you would want to keep or commit), any capability's own installed
//! files outside our managed regions, and anything a *different* manifest
//! manages at global scope — `previously_managed` is read per manifest-scoped
//! state key, so another project's global entries are simply not in the list.

use std::path::Path;

use agentstack_core::paint::OwoColorize;
use anyhow::Result;

use crate::cli::UninstallArgs;
use crate::scope::Scope;
use crate::state::State;

use super::unrender::{self, Removal};

pub fn run(args: &UninstallArgs, manifest_dir: Option<&Path>) -> Result<()> {
    // No manifest here is not necessarily a mistake. "Reset everything
    // AgentStack put on this machine" is a real thing to want — after deleting
    // a project, or from any directory at all — and until this branch existed
    // the only command that could do it refused to start without the one file
    // it does not remove. Answering "there is no manifest" to someone asking
    // to be rid of the tool is the worst moment to be pedantic (review F07).
    let ctx = match crate::commands::load(manifest_dir) {
        Ok(ctx) => ctx,
        Err(no_manifest) => return machine_state_only(args, no_manifest),
    };
    let state = State::load()?;

    let scopes: Vec<Scope> = match args.scope.as_str() {
        "project" => vec![Scope::Project],
        "global" => vec![Scope::Global],
        _ => vec![Scope::Project, Scope::Global],
    };

    let plan = unrender::plan(&ctx, &state, &scopes, /*own_global_only=*/ false)?;
    let mut removals = plan.removals;

    // Read now, while the records still exist: the write below either clears
    // them (`clear_managed_state`) or deletes the ledger holding them, and by
    // the closing line there would be nothing left to name.
    let pruned_skills = pruned_skills(&ctx, &state, &scopes);

    let root = crate::manifest::project_root_of(&ctx.dir);

    // N3: the `.gitignore` managed block is a managed region like any other,
    // and it names generated files the removals above just deleted. Leaving it
    // would mean the repo carries dead AgentStack config after being told
    // AgentStack was uninstalled. Project scope only — the block is a
    // project-root artifact.
    if scopes.contains(&Scope::Project) {
        if let Some(removal) = unrender::plan_gitignore_removal(&root) {
            removals.push(removal);
        }
    }

    let home = crate::util::paths::agentstack_home();
    let remove_home = home.exists() && !args.keep_home;

    if removals.is_empty() && !remove_home {
        println!("Nothing to remove — AgentStack manages no files here.");
        return Ok(());
    }

    print_plan(
        args,
        &root,
        &removals,
        remove_home.then_some(home.as_path()),
    );

    if !args.write {
        print_dry_run_footer(remove_home);
        return Ok(());
    }

    apply_removals(&root, removals)?;
    if remove_home {
        // Last, and separately: this is the undo ledger, so everything above
        // stays revertible until the moment it goes.
        std::fs::remove_dir_all(&home)
            .with_context_path(&home)
            .map(|_| println!("  {} removed {}", "✓".green(), home.display()))?;
    } else {
        // The ledger survives, so it must stop claiming the renders that are
        // now off disk — otherwise the derived delivery mode and the drift
        // report keep describing files that no longer exist.
        unrender::clear_managed_state(&plan.touched_keys)?;
    }

    println!("\n{}", "AgentStack is uninstalled.".bold());
    if !remove_home {
        println!(
            "  {}",
            "Its own state is still under ~/.agentstack (undo with `agentstack restore`).".dimmed()
        );
        print_skills_bound(&pruned_skills);
    }
    for line in kept_notes(&ctx.dir) {
        println!("  {}", line.dimmed());
    }
    println!(
        "  {}",
        "The binary itself is still on PATH — remove it the way you installed it \
         (`rm $(command -v agentstack)` for the installer or `self link`)."
            .dimmed()
    );
    Ok(())
}

/// The materialized skills this uninstall prunes, by name.
///
/// Read from the same `managed_skills` records [`unrender::plan`] builds its
/// skills leg from, over the scopes this run planned — so the notice below
/// appears exactly when that leg does, and is empty otherwise. Deduplicated
/// across targets and scopes: one skill delivered to three CLIs is one thing
/// the user thinks about.
fn pruned_skills(ctx: &super::Context, state: &State, scopes: &[Scope]) -> Vec<String> {
    let mut names = Vec::new();
    for id in ctx.registry.ids() {
        for &scope in scopes {
            names.extend(state.managed_skills(&crate::state::target_key(id, scope, &ctx.dir)));
        }
    }
    names.sort_unstable();
    names.dedup();
    names
}

/// Bound the `restore` line printed above this one.
///
/// That ledger replays the FILE edits this uninstall made — every removal is
/// captured into it first — but the skills leg is `capture: false` (see
/// [`Removal::capture`], and [`crate::history`] for why: the ledger stores
/// bytes and a delivered skill is a linked directory). So "undo with
/// `agentstack restore`" was offering an undo that would bring the configs
/// back and silently leave the skills off. Say which half is which, and name
/// the command that does put them back.
///
/// [`crate::history::SKILLS_COME_OFF_WITH`] is deliberately not reused here:
/// it names `x uninstall --write`, which is the command the reader just ran.
/// The reason sentence is shared, because that is the part that must never
/// drift between Undo surfaces.
///
/// Conditional, like the other Undo surfaces: a project that materialized no
/// skills prints nothing, so this stays a fact about this project rather than
/// a disclaimer people learn to skip.
fn print_skills_bound(names: &[String]) {
    if names.is_empty() {
        return;
    }
    println!(
        "  {} {} ({})",
        "·".dimmed(),
        crate::history::SKILLS_ARE_NOT_RECORDED.dimmed(),
        names
            .iter()
            .map(|n| crate::text::sanitize_line(n))
            .collect::<Vec<_>>()
            .join(", ")
            .dimmed()
    );
    println!(
        "  {} {}",
        "·".dimmed(),
        "so that restore puts the files above back, not these — re-materialize them by \
         activating a toolset that includes them (`agentstack use --write`)"
            .dimmed()
    );
}

/// N2: name what uninstall deliberately KEPT, and say when one of those files
/// holds secret values.
///
/// Keeping `agentstack.toml` and its `.env` is the documented rule — this
/// removes rendered output, not your setup, so you can re-`apply` — but
/// "AgentStack is uninstalled" reads as "nothing of mine is left", and a
/// plaintext credential is precisely the leftover someone uninstalling a
/// security-adjacent tool would want named. Counts the `.env` assignments
/// rather than showing them: the point is that values are there, never what
/// they are.
fn kept_notes(manifest_dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let manifest = manifest_dir.join("agentstack.toml");
    if !manifest.exists() {
        return out;
    }
    out.push(format!(
        "Kept: {} — your setup, so `agentstack apply --write` can rebuild it.",
        crate::commands::init::display_path(&manifest, manifest_dir)
    ));
    let env = manifest_dir.join(".env");
    let secrets = std::fs::read_to_string(&env)
        .map(|s| {
            s.lines()
                .filter(|l| {
                    let l = l.trim();
                    !l.is_empty() && !l.starts_with('#') && l.contains('=')
                })
                .count()
        })
        .unwrap_or(0);
    if secrets > 0 {
        out.push(format!(
            "Kept: {} — holds {} in plaintext. Delete it yourself \
             to fully reset.",
            crate::commands::init::display_path(&env, manifest_dir),
            super::count(secrets, "secret value")
        ));
    }
    out
}

/// The "reset all AgentStack data on this machine" half of the exit, reached
/// when there is no manifest to revert rendered output against.
///
/// It removes `~/.agentstack` and nothing else — deliberately narrow. Rendered
/// native config is removed by planning an empty manifest against the state
/// ledger, and without a manifest there is no such plan; guessing at other
/// projects' files from the ledger alone would be a second removal path with
/// none of the diff/undo properties the module header insists on.
///
/// `--keep-home` here would ask to remove nothing at all, so it is refused with
/// the reason rather than silently succeeding at doing nothing.
fn machine_state_only(args: &UninstallArgs, no_manifest: anyhow::Error) -> Result<()> {
    let home = crate::util::paths::agentstack_home();
    if !home.exists() {
        // Nothing on either side: the original "no manifest" error is still
        // the most useful thing to say.
        return Err(no_manifest);
    }
    if args.keep_home {
        println!(
            "Nothing to do: there is no manifest here to revert rendered output against, \
             and --keep-home asks to leave {} alone.",
            home.display()
        );
        return Ok(());
    }

    println!(
        "{}\n",
        if args.write {
            "Removing AgentStack's machine-local state:".bold()
        } else {
            "No manifest here, so there is no rendered output to revert. \
             This is what AgentStack still holds on this machine:"
                .bold()
        }
    );
    println!(
        "  {}  {}",
        "AgentStack's own state".bold(),
        format!(
            "{} — undo ledger, trust store, central library, sessions",
            home.display()
        )
        .dimmed()
    );

    if !args.write {
        println!("\n{}", "Re-run with --write to remove it.".dimmed());
        println!(
            "{}",
            "This is machine-wide: the undo ledger goes with it, so writes AgentStack \
             made in ANY project stop being undoable."
                .dimmed()
        );
        println!(
            "{}",
            "Rendered native config in a project is removed by running `agentstack x uninstall` \
             in that project, which needs its manifest."
                .dimmed()
        );
        return Ok(());
    }

    std::fs::remove_dir_all(&home)
        .with_context_path(&home)
        .map(|_| println!("  {} removed {}", "✓".green(), home.display()))?;

    println!("\n{}", "AgentStack's machine state is gone.".bold());
    println!(
        "  {}",
        "Any rendered config still in a project is untouched — run `agentstack x uninstall` \
         there, with its manifest, to take that back too."
            .dimmed()
    );
    println!(
        "  {}",
        "The binary itself is still on PATH — remove it the way you installed it \
         (`rm $(command -v agentstack)` for the installer or `self link`)."
            .dimmed()
    );
    Ok(())
}

fn print_plan(args: &UninstallArgs, root: &Path, removals: &[Removal], home: Option<&Path>) {
    println!(
        "{}\n",
        if args.write {
            "Removing everything AgentStack manages:".bold()
        } else {
            "AgentStack manages these. Nothing has been changed yet:".bold()
        }
    );
    for r in removals {
        println!(
            "  {}  {}",
            r.label.bold(),
            crate::commands::init::display_path(&r.path, root).dimmed()
        );
        if args.verbose {
            for line in r.diff.lines() {
                println!("    {line}");
            }
        }
    }
    if let Some(home) = home {
        println!(
            "  {}  {}",
            "AgentStack's own state".bold(),
            format!(
                "{} — undo ledger, trust store, central library",
                crate::commands::init::display_path(home, root)
            )
            .dimmed()
        );
    }
    println!();
}

fn print_dry_run_footer(remove_home: bool) {
    println!(
        "{}",
        "Re-run with --write to remove them; --verbose shows every diff first.".dimmed()
    );
    println!(
        "{}",
        "Your agentstack.toml is never touched — this removes rendered output, not your setup."
            .dimmed()
    );
    if remove_home {
        println!(
            "{}",
            "~/.agentstack holds the undo ledger, so it is removed last. Keep it with --keep-home."
                .dimmed()
        );
    }
}

/// Perform the native-config removals, capturing each into the history ledger
/// first so `agentstack restore` can put any of them back. One ledger entry
/// for the whole uninstall: it was one decision, so it undoes as one.
fn apply_removals(root: &Path, removals: Vec<Removal>) -> Result<()> {
    let mut backups = Vec::new();
    let mut labels = Vec::new();
    let mut n = 0usize;
    for r in removals {
        // Skills removals are not file edits (see `Removal::capture`) — they
        // are pruned symlinks a re-activation recreates, so they carry no
        // ledger entry and rollback never touches their directory.
        let capture = r
            .capture
            .then(|| crate::history::capture(&r.path, r.label.clone()));
        (r.write)()?;
        println!(
            "  {} {} {}",
            "✓".green(),
            "reverted".dimmed(),
            crate::commands::init::display_path(&r.path, root)
        );
        n += 1;
        if let Some(capture) = capture {
            backups.push(capture);
            labels.push(r.label);
        }
    }
    crate::history::record("uninstall", "uninstall", labels, backups)?;
    println!("\n{} reverted.", super::count(n, "file"));
    Ok(())
}

/// Small helper so the one non-planner filesystem call in this module still
/// names the path it failed on.
trait WithPath<T> {
    fn with_context_path(self, path: &Path) -> Result<T>;
}

impl<T> WithPath<T> for std::io::Result<T> {
    fn with_context_path(self, path: &Path) -> Result<T> {
        use anyhow::Context;
        self.with_context(|| format!("could not remove {}", path.display()))
    }
}
