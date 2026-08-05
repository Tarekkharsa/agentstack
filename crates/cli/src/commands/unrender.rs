//! The shared un-render engine: plan the removal of everything this manifest
//! rendered into native CLI configs — servers, settings, hooks, instruction
//! regions, materialized skills, and the managed `.gitignore` block.
//!
//! Two commands consume it and they must not drift apart: `uninstall` (the
//! machine exit) and `set-mode` (a delivery-mode switch, whose zero-files and
//! clean-at-rest legs are exactly "nothing of ours stays rendered here").
//! **The mechanism is the ordinary render path, run against an empty
//! manifest.** Every removal comes from `plan_*(Manifest::default(),
//! previously-managed…)` — the same planners `apply` uses, given nothing to
//! declare. That is deliberate: they already remove exactly our entries and
//! leave foreign ones alone, they already produce reviewable diffs, and their
//! writes go through the same history capture, so every un-render is undoable
//! with `agentstack restore`. Hand-rolled deletion would be a second write
//! path with none of those properties.
//!
//! Planning only. Each caller owns its confirmation step, history recording,
//! and closing copy — consent stays where the user is.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::manifest::Manifest;
use crate::render::instructions::plan_instructions;
use crate::render::{plan_hooks, plan_settings, plan_target_with_servers, skills};
use crate::scope::Scope;
use crate::state::{target_key, State};

/// One thing found on disk that an un-render can take back off. `write` is a
/// boxed `FnOnce` because each planner returns its own plan type and all a
/// caller needs from them is "show me the diff" and "do it".
pub(crate) struct Removal {
    /// What a human calls it: `Claude Code · servers (this project)`.
    pub label: String,
    pub path: PathBuf,
    pub diff: String,
    /// Whether the write should be captured into the history ledger first.
    /// True for every file edit; false for the skills leg — its path is a
    /// DIRECTORY of pruned symlinks, which `history::capture` records as
    /// `before: None`, and a later rollback would then try `remove_file` on
    /// the directory and fail. Skills need no ledger anyway: re-activating
    /// the toolset re-materializes them exactly.
    pub capture: bool,
    pub write: Box<dyn FnOnce() -> Result<()>>,
}

/// A full un-render plan, plus the state keys whose managed sets the removals
/// drain — so a caller that keeps the state ledger (`set-mode`; `uninstall
/// --keep-home`) can clear exactly those records afterwards. Leaving them
/// would make the ledger claim renders that are no longer on disk, and the
/// delivery mode (derived from that ledger) would keep reading "static" after
/// the files are gone — the exact lie the mode switch exists to remove.
pub(crate) struct UnrenderPlan {
    pub removals: Vec<Removal>,
    /// State keys with any managed set this plan removes from disk.
    pub touched_keys: Vec<String>,
    /// Whether any removal is a compiled instructions region — zero-files
    /// callers surface this: instructions are not delivered live.
    pub removes_instructions: bool,
}

/// Build the un-render plan for `scopes`.
///
/// `own_global_only` scopes the global-scope legs to entries THIS manifest
/// recorded (the same `source_manifest` guard `has_rendered_artifacts` uses),
/// so a mode switch can never plan away another project's global renders.
/// `uninstall` passes `false` — its existing, documented behavior is to plan
/// whatever the state key holds, and the machine exit keeps it.
pub(crate) fn plan(
    ctx: &super::Context,
    state: &State,
    scopes: &[Scope],
    own_global_only: bool,
) -> Result<UnrenderPlan> {
    // An empty manifest is the whole trick: every planner reads it as "declare
    // nothing" and removes precisely what it previously managed. Parsed rather
    // than constructed so it picks up every `#[serde(default)]` in the model —
    // a field added later cannot silently arrive here as something non-empty.
    let empty: Manifest = toml::from_str("version = 1\n").expect("the empty manifest parses");
    let ruleset = crate::render::ruleset_for(&empty)?;
    let identity = crate::state::manifest_identity(&ctx.dir);

    let mut out = UnrenderPlan {
        removals: Vec::new(),
        touched_keys: Vec::new(),
        removes_instructions: false,
    };

    let target_ids: Vec<String> = ctx.registry.ids().map(str::to_string).collect();
    for id in &target_ids {
        let Some(desc) = ctx.registry.get(id) else {
            continue;
        };
        for &scope in scopes {
            let key = target_key(id, scope, &ctx.dir);
            if own_global_only
                && scope == Scope::Global
                && state.manifest_source(&key).is_some_and(|s| s != identity)
            {
                // Another manifest's global bookkeeping — not ours to plan away.
                continue;
            }
            let at = if scope == Scope::Project {
                "this project"
            } else {
                "global"
            };
            let name = |what: &str| format!("{} · {what} ({at})", desc.display);
            let mut touched = false;

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
                    crate::render::PriorTrust::STRICT,
                )? {
                    if plan.changed() {
                        touched = true;
                        // Reuse `apply`'s own rule for a file that now holds
                        // nothing but an empty container: delete it rather
                        // than leave a `{"mcpServers": {}}` husk behind. An
                        // un-render that leaves litter is not an un-render.
                        let desc = desc.clone();
                        let path = plan.config_path.clone();
                        out.removals.push(Removal {
                            label: name("servers"),
                            path: path.clone(),
                            diff: plan.diff(),
                            capture: true,
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
                        touched = true;
                        out.removals.push(Removal {
                            label: name("settings"),
                            path: plan.settings_path.clone(),
                            diff: plan.diff(),
                            capture: true,
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
                        touched = true;
                        out.removals.push(Removal {
                            label: name("hooks"),
                            path: plan.path.clone(),
                            diff: plan.diff(),
                            capture: true,
                            write: Box::new(move || plan.write()),
                        });
                    }
                }
            }

            // Materialized skills: the leg no config planner covers, because
            // `use --write` writes them through `render::skills`, not a config
            // file. A removal-only skill plan (empty active set against the
            // previously-managed names) prunes exactly what we own — the
            // marker/symlink rules in `skills::materialize` keep a user's
            // hand-made directory safe, same as during activation.
            let prev_skills = state.managed_skills(&key);
            if !prev_skills.is_empty() {
                if let Some(skills_dir) = desc.skills_dir_for(scope, &ctx.dir) {
                    let strategy = desc.skills.as_ref().map(|s| s.strategy).unwrap_or_default();
                    // Removal-only (empty active set): the inert direction, so
                    // the plan's trust gate has nothing to refuse and `x
                    // unrender` keeps working on an untrusted project — which
                    // is exactly what a project whose consent went stale needs
                    // to take our artifacts back off its disk.
                    let plan = skills::plan(
                        skills_dir.clone(),
                        strategy,
                        Vec::new(),
                        &prev_skills,
                        &ctx.dir,
                        crate::render::PriorTrust::STRICT,
                    )?;
                    if plan.has_work() {
                        touched = true;
                        let diff = plan
                            .to_remove
                            .iter()
                            .map(|n| format!("- {n}\n"))
                            .collect::<String>();
                        out.removals.push(Removal {
                            label: name("skills"),
                            path: skills_dir,
                            diff,
                            capture: false,
                            write: Box::new(move || skills::materialize(&plan)),
                        });
                    }
                }
            }

            // Managed regions in CLAUDE.md / AGENTS.md. No state gate: the
            // region is self-delimiting, so an empty manifest plans it away
            // whether or not this machine's state file remembers writing it.
            // Empty manifest AND no package members: un-rendering plans the
            // whole region away, and a package's instruction members are part
            // of that region like any other fragment.
            if let Some(plan) = plan_instructions(
                &empty,
                desc,
                scope,
                &ctx.dir,
                &[],
                // Planning the region AWAY: nothing to select, so no library
                // read and no toolset.
                &crate::instructions::Selecting::none(),
            ) {
                if plan.changed() {
                    out.removes_instructions = true;
                    out.removals.push(Removal {
                        label: name("instructions"),
                        path: plan.path.clone(),
                        diff: plan.diff(),
                        capture: true,
                        write: Box::new(move || plan.write()),
                    });
                }
            }

            if touched {
                out.touched_keys.push(key);
            }
        }
    }

    Ok(out)
}

/// The managed `.gitignore` block, planned as a removal like any other managed
/// region — it names generated files an un-render just deleted, and leaving it
/// would mean the repo carries dead AgentStack config. Project-root artifact,
/// so callers include it only for project scope.
pub(crate) fn plan_gitignore_removal(root: &Path) -> Option<Removal> {
    let path = root.join(".gitignore");
    let existing = std::fs::read_to_string(&path).ok()?;
    let updated = crate::render::gitignore::remove_block(&existing)?;
    Some(Removal {
        label: "Generated-artifact ignores (this project)".to_string(),
        path: path.clone(),
        diff: crate::util::diff::render(&existing, &updated),
        capture: true,
        write: Box::new(move || {
            if updated.is_empty() {
                // Our block was the whole file: take the file too rather than
                // leave an empty `.gitignore` behind.
                let _ = std::fs::remove_file(&path);
            } else {
                crate::util::atomic::write(&path, &updated)?;
            }
            Ok(())
        }),
    })
}

/// Clear the managed sets for `keys` in the state ledger, keeping everything
/// else (the active toolset, foreign bookkeeping). Call after the removals ran:
/// the ledger must stop claiming renders that are no longer on disk, or the
/// derived delivery mode and the drift report both keep describing files that
/// do not exist. The active-profile memory survives on purpose — it is what a
/// later switch back to on-disk re-renders.
pub(crate) fn clear_managed_state(keys: &[String]) -> Result<()> {
    if keys.is_empty() {
        return Ok(());
    }
    let mut state = State::load()?;
    for key in keys {
        state.clear_managed(key);
    }
    state.save()
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
pub(crate) fn prune_if_blank(path: &Path) {
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
pub(crate) fn prune_empty_dir(path: &Path) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::remove_dir(parent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::prelude::*;

    /// The pruning rules are the only place this engine deletes on its own
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
        // survive an un-render that only owned part of the file.
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
