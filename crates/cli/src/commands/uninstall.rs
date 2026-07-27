//! `agentstack uninstall` — take everything AgentStack put on this machine
//! back off it, previewing first.
//!
//! Reversibility is the product's central promise, and until this existed the
//! promise had a hole: `restore` undoes *one recorded write*, but nothing
//! answered "remove all of it". A tool that asks to manage nine other tools'
//! configuration needs a guaranteed exit, or trying it is not a small decision
//! (review finding F06).
//!
//! **The mechanism is the ordinary render path, run against an empty
//! manifest.** Every removal here comes from `plan_*(Manifest::default(),
//! previously-managed…)` — the same four planners `apply` uses, given nothing
//! to declare. That is deliberate: they already remove exactly our entries and
//! leave foreign ones alone, they already produce reviewable diffs, and their
//! writes go through the same history capture, so an uninstall is itself
//! undoable with `agentstack restore` right up until the ledger is removed.
//! Hand-rolled deletion would have been a second write path with none of those
//! properties.
//!
//! What it does NOT touch, on purpose: the project's `agentstack.toml` (the
//! thing you would want to keep or commit), any capability's own installed
//! files outside our managed regions, and anything a *different* manifest
//! manages at global scope — `previously_managed` is read per manifest-scoped
//! state key, so another project's global entries are simply not in the list.

use std::path::{Path, PathBuf};

use anyhow::Result;
use owo_colors::OwoColorize;

use crate::cli::UninstallArgs;
use crate::manifest::Manifest;
use crate::render::instructions::plan_instructions;
use crate::render::{plan_hooks, plan_settings, plan_target_with_servers};
use crate::scope::Scope;
use crate::state::{target_key, State};

/// One thing found on disk that this command can take back off. `write` is a
/// boxed `FnOnce` because each planner returns its own plan type and all this
/// command needs from them is "show me the diff" and "do it".
struct Removal {
    /// What a human calls it: `Claude Code · servers (this project)`.
    label: String,
    path: PathBuf,
    diff: String,
    write: Box<dyn FnOnce() -> Result<()>>,
}

pub fn run(args: &UninstallArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let ctx = crate::commands::load(manifest_dir)?;
    let state = State::load()?;
    // An empty manifest is the whole trick: every planner reads it as "declare
    // nothing" and removes precisely what it previously managed. Parsed rather
    // than constructed so it picks up every `#[serde(default)]` in the model —
    // a field added later cannot silently arrive here as something non-empty.
    let empty: Manifest = toml::from_str("version = 1\n").expect("the empty manifest parses");
    let ruleset = crate::render::ruleset_for(&empty)?;

    let scopes: Vec<Scope> = match args.scope.as_str() {
        "project" => vec![Scope::Project],
        "global" => vec![Scope::Global],
        _ => vec![Scope::Project, Scope::Global],
    };

    let target_ids: Vec<String> = ctx.registry.ids().map(str::to_string).collect();
    let mut removals: Vec<Removal> = Vec::new();
    for id in &target_ids {
        let Some(desc) = ctx.registry.get(id) else {
            continue;
        };
        for &scope in &scopes {
            let key = target_key(id, scope, &ctx.dir);
            let at = if scope == Scope::Project {
                "this project"
            } else {
                "global"
            };
            let name = |what: &str| format!("{} · {what} ({at})", desc.display);

            let managed = state.managed_servers(&key);
            if !managed.is_empty() {
                if let Some(plan) = plan_target_with_servers(
                    desc,
                    &ctx.resolver,
                    &ruleset,
                    &Default::default(),
                    &managed,
                    scope,
                    &ctx.dir,
                )? {
                    if plan.changed() {
                        // Reuse `apply`'s own rule for a file that now holds
                        // nothing but an empty container: delete it rather
                        // than leave a `{"mcpServers": {}}` husk behind. An
                        // uninstall that leaves litter is not an uninstall.
                        let desc = desc.clone();
                        let path = plan.config_path.clone();
                        removals.push(Removal {
                            label: name("servers"),
                            path: path.clone(),
                            diff: plan.diff(),
                            write: Box::new(move || {
                                plan.write()?;
                                if plan.remove_if_empty_shell(&desc) {
                                    // It took the file; take the directory it
                                    // was alone in.
                                    prune_empty_dir(&path);
                                } else if scope == Scope::Project {
                                    // Formats `is_empty_shell` doesn't cover
                                    // (a TOML config whose only content was
                                    // our tables reduces to zero bytes).
                                    prune_if_blank(&path);
                                }
                                Ok(())
                            }),
                        });
                    }
                }
            }

            let settings = state.managed_settings(&key);
            if !settings.is_empty() {
                if let Some(plan) =
                    plan_settings(&empty, desc, &ctx.resolver, &settings, scope, &ctx.dir)?
                {
                    if plan.changed() {
                        removals.push(Removal {
                            label: name("settings"),
                            path: plan.settings_path.clone(),
                            diff: plan.diff(),
                            write: Box::new(move || plan.write()),
                        });
                    }
                }
            }

            // Hooks — including anything `guard install` compiled in. The
            // empty `machine_hooks` slice is what makes this a removal: the
            // machine layer is exactly what `apply` would otherwise re-add.
            if !state.managed_hooks(&key).is_empty() {
                if let Some(plan) =
                    plan_hooks(&empty, desc, &ctx.resolver, true, scope, &ctx.dir, &[])?
                {
                    if plan.changed() {
                        removals.push(Removal {
                            label: name("hooks"),
                            path: plan.path.clone(),
                            diff: plan.diff(),
                            write: Box::new(move || plan.write()),
                        });
                    }
                }
            }

            // Managed regions in CLAUDE.md / AGENTS.md. No state gate: the
            // region is self-delimiting, so an empty manifest plans it away
            // whether or not this machine's state file remembers writing it.
            if let Some(plan) = plan_instructions(&empty, desc, scope, &ctx.dir) {
                if plan.changed() {
                    removals.push(Removal {
                        label: name("instructions"),
                        path: plan.path.clone(),
                        diff: plan.diff(),
                        write: Box::new(move || plan.write()),
                    });
                }
            }
        }
    }

    let root = crate::manifest::project_root_of(&ctx.dir);

    // N3: the `.gitignore` managed block is a managed region like any other,
    // and it names generated files the removals above just deleted. Leaving it
    // would mean the repo carries dead AgentStack config after being told
    // AgentStack was uninstalled. Project scope only — the block is a
    // project-root artifact.
    if scopes.contains(&Scope::Project) {
        let path = root.join(".gitignore");
        if let Ok(existing) = std::fs::read_to_string(&path) {
            if let Some(updated) = crate::render::gitignore::remove_block(&existing) {
                removals.push(Removal {
                    label: "Generated-artifact ignores (this project)".to_string(),
                    path: path.clone(),
                    diff: crate::util::diff::render(&existing, &updated),
                    write: Box::new(move || {
                        if updated.is_empty() {
                            // Our block was the whole file: take the file too
                            // rather than leave an empty `.gitignore` behind.
                            let _ = std::fs::remove_file(&path);
                        } else {
                            crate::util::atomic::write(&path, &updated)?;
                        }
                        Ok(())
                    }),
                });
            }
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
    }

    println!("\n{}", "AgentStack is uninstalled.".bold());
    if !remove_home {
        println!(
            "  {}",
            "Its own state is still under ~/.agentstack (undo with `agentstack restore`).".dimmed()
        );
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
            "Kept: {} — holds {secrets} secret value(s) in plaintext. Delete it yourself \
             to fully reset.",
            crate::commands::init::display_path(&env, manifest_dir)
        ));
    }
    out
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
    for r in removals {
        let capture = crate::history::capture(&r.path, r.label.clone());
        (r.write)()?;
        println!(
            "  {} {} {}",
            "✓".green(),
            "reverted".dimmed(),
            crate::commands::init::display_path(&r.path, root)
        );
        backups.push(capture);
        labels.push(r.label);
    }
    let n = backups.len();
    crate::history::record("uninstall", labels, backups)?;
    println!("\n{n} file(s) reverted.");
    Ok(())
}

/// Delete a rendered config that is now blank, then its directory if that too
/// is empty — the tail of the same "leave no husk" rule
/// [`crate::render::TargetPlan::remove_if_empty_shell`] applies to the formats
/// it understands.
///
/// Both steps are conservative on purpose. The file is removed only when it is
/// *entirely* whitespace, so a config still holding anything of the user's is
/// left alone; the directory is removed only when `remove_dir` succeeds, which
/// on every supported platform fails unless it is already empty — so this can
/// never take a sibling file with it. Both are project-scope only (the caller
/// checks), because a global config directory is shared with the CLI's own
/// state and is not ours to tidy. Failures are ignored: a leftover empty
/// directory is cosmetic, and there is nothing useful to tell the user.
fn prune_if_blank(path: &Path) {
    match std::fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => {}
        _ => return,
    }
    if std::fs::remove_file(path).is_err() {
        return;
    }
    prune_empty_dir(path);
}

/// Remove the directory a just-deleted config lived in, if nothing else is in
/// it. `remove_dir` (not `remove_dir_all`) is load-bearing: it refuses on a
/// non-empty directory, so this can never take a sibling file with it.
fn prune_empty_dir(path: &Path) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::remove_dir(parent);
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::prelude::*;

    /// The pruning rules are the only place this command deletes on its own
    /// (everything else goes through a planner), so they carry the safety
    /// properties: blank files go, files with any content of the user's stay,
    /// and a directory is only ever removed when it is already empty.
    #[test]
    fn pruning_removes_only_blank_files_and_empty_dirs() {
        let tmp = assert_fs::TempDir::new().unwrap();

        // Whitespace-only → gone, and its now-empty directory with it.
        let blank = tmp.child("codex/config.toml");
        blank.write_str("\n  \n").unwrap();
        prune_if_blank(blank.path());
        assert!(!blank.path().exists(), "a blank config is removed");
        assert!(
            !tmp.child("codex").path().exists(),
            "the directory it was alone in goes too"
        );

        // Anything left in the file → untouched. A user's own setting must
        // survive an uninstall that only owned part of the file.
        let kept = tmp.child("gemini/settings.json");
        kept.write_str("{\n  \"theme\": \"dark\"\n}\n").unwrap();
        prune_if_blank(kept.path());
        assert!(kept.path().exists(), "a config with content is left alone");

        // A directory with a sibling in it survives, even when the config
        // beside it was blank — `remove_dir` refuses non-empty directories.
        let shared = tmp.child("shared/config.toml");
        shared.write_str("").unwrap();
        let sibling = tmp.child("shared/keep-me.txt");
        sibling.write_str("mine").unwrap();
        prune_if_blank(shared.path());
        assert!(!shared.path().exists(), "the blank config still goes");
        assert!(
            sibling.path().exists(),
            "a sibling file is never collateral damage"
        );

        // A missing path is a no-op, not an error.
        prune_if_blank(tmp.child("nope/gone.json").path());
    }
}
