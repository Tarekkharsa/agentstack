//! `agentstack use <profile>` — activate a profile: render its servers into each
//! target's config and materialize its skills into the target's skills dir, for
//! the chosen scope. Dry-run by default; `--write` performs changes.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use indexmap::IndexMap;
use owo_colors::OwoColorize;

use crate::cli::UseArgs;
use crate::library::Library;
use crate::lock::{Lock, LockedServer, LockedSkill};
use crate::manifest::Manifest;
use crate::render::skills;
use crate::render::{resolve_targets, Selection};
use crate::resolve::{ResolveMode, ResolvedServer, ResolvedSkill, ServerOrigin, SkillOrigin};
use crate::scope::Scope;
use crate::state::{target_key, State};

/// Everything activation needs, resolved once: the profile's skills and
/// servers through the library-aware resolvers. Produced by [`prepare`],
/// consumed by [`activate`] — callers that already planned against the same
/// data (session start snapshots) reuse it instead of re-resolving.
pub struct Prepared {
    pub resolved_skills: Vec<ResolvedSkill>,
    pub resolved_servers: Vec<ResolvedServer>,
    /// `name -> Server` view of `resolved_servers` — the shape rendering wants.
    pub server_map: IndexMap<String, crate::manifest::Server>,
}

/// Resolve a profile's skills + servers (inline-first, then central library),
/// failing clearly before anything is written. A dry run resolves offline
/// (`NoFetch`) — previewing never touches the network; a real `--write`
/// fetches git-backed sources as needed.
pub fn prepare(
    ctx: &super::Context,
    libctx: &super::LibraryCtx,
    args: &UseArgs,
) -> Result<Prepared> {
    let manifest = &ctx.loaded.manifest;

    // Early guard: the profile must exist (its servers are resolved below).
    manifest
        .profiles
        .get(&args.profile)
        .with_context(|| format!("no profile '{}' in manifest", args.profile))?;

    let mode = if args.write {
        ResolveMode::Fetch
    } else {
        ResolveMode::NoFetch
    };
    let resolved_skills = resolve_active_skills(
        manifest,
        &args.profile,
        &ctx.dir,
        &libctx.library,
        &libctx.lib_home,
        &libctx.store,
        mode,
    )?;

    // `${REF}`s stay intact; they are resolved per-target at render time, not
    // here. The resolved list is kept for lock recording; the `name -> Server`
    // map drives rendering.
    let selection = Selection::Profile(args.profile.clone());
    let resolved_servers = crate::render::resolve_active_servers(
        manifest,
        &libctx.library,
        &libctx.lib_home,
        &selection,
    )?;
    let mut server_map: IndexMap<String, crate::manifest::Server> = resolved_servers
        .iter()
        .map(|r| (r.name.clone(), r.server.clone()))
        .collect();
    // Owner-refreshed servers: fan out the owning app's on-disk values, never
    // the stale manifest ones (see render::owned).
    crate::render::refresh_owned_servers(
        &mut server_map,
        &ctx.registry,
        args.scope.unwrap_or(Scope::Global),
        &ctx.dir,
    );

    Ok(Prepared {
        resolved_skills,
        resolved_servers,
        server_map,
    })
}

pub fn run(args: &UseArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let ctx = super::load(manifest_dir)?;
    let libctx = ctx.library_ctx();
    let prepared = prepare(&ctx, &libctx, args)?;
    activate(&ctx, &libctx, args, &prepared)
}

/// Render the prepared profile into every target (servers + skills), record
/// state, and pin the lockfile. The write half of `run` — takes pre-loaded
/// context and pre-resolved sets so callers like session start don't load and
/// resolve everything twice.
pub fn activate(
    ctx: &super::Context,
    libctx: &super::LibraryCtx,
    args: &UseArgs,
    prepared: &Prepared,
) -> Result<()> {
    let manifest = &ctx.loaded.manifest;
    let scope = args.scope.unwrap_or(Scope::Global);
    let resolved_skills = &prepared.resolved_skills;
    let resolved_servers = &prepared.resolved_servers;
    let server_map = &prepared.server_map;
    // (name, source dir) pairs drive skill materialization; the richer
    // `ResolvedSkill` list is kept for lockfile recording below.
    let active_skills: Vec<(String, PathBuf)> = resolved_skills
        .iter()
        .map(|r| (r.name.clone(), r.path.clone()))
        .collect();

    let target_ids = resolve_targets(manifest, &ctx.registry, &args.targets);
    println!(
        "Activating profile '{}' (scope: {scope}) — {} server(s), {} skill(s)",
        args.profile.bold(),
        server_map.len(),
        active_skills.len()
    );

    let mut state = State::load()?;
    let identity = crate::state::manifest_identity(&ctx.dir);
    let mut wrote = 0;
    let mut blocked_targets: Vec<String> = Vec::new();
    // Project-scope artifacts we write are machine-local (absolute-path
    // symlinks, resolved values) — collect them for the managed .gitignore
    // block unless the user opts out. Entries are stable and directory-level
    // (the config file, the whole skills dir) so the block never churns as
    // profile membership changes.
    let project_root = crate::manifest::project_root_of(&ctx.dir);
    let mut ignore_entries: Vec<String> = Vec::new();

    for id in &target_ids {
        let Some(desc) = ctx.registry.get(id) else {
            println!("{} unknown adapter '{id}' — skipping", "⚠".yellow());
            continue;
        };
        let key = target_key(id, scope, &ctx.dir);
        println!("\n{}", desc.display.bold());

        // --- servers ---
        let mut previously = state.managed_servers(&key);
        // Names an earlier guarded write kept on disk (state bookkeeping —
        // they left `managed_servers` when this manifest recorded its own
        // set). Ones the profile now selects become managed again below.
        let kept_before: Vec<String> = state
            .kept_foreign(&key)
            .into_iter()
            .filter(|n| !server_map.contains_key(n))
            .collect();
        // Guard cross-manifest global prunes: entries another manifest applied
        // are kept (and reported below), not deleted, unless --prune-foreign.
        let foreign = if args.prune_foreign {
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
            let mut f = state.foreign_prunes(&key, scope, &ctx.dir, &mut previously, |n| {
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
        match crate::render::plan_target_with_servers(
            desc,
            &ctx.resolver,
            server_map,
            &previously,
            scope,
            &ctx.dir,
        )? {
            None => println!("  servers: no {scope} scope"),
            Some(plan) => {
                if !foreign.is_empty() {
                    println!(
                        "  {} keeping {} — applied by another manifest ↳ keep: agentstack adopt · \
                         prune: agentstack use {} --prune-foreign",
                        "⚠".yellow(),
                        foreign.join(", "),
                        args.profile
                    );
                }
                for u in &plan.unresolved {
                    println!("  {} unresolved secret {}", "✗".red(), u);
                }
                for f in &plan.failed {
                    println!(
                        "  {} secret read failed {} — the secret may be set; retry",
                        "✗".red(),
                        f
                    );
                }
                let blocked = (!plan.unresolved.is_empty() || !plan.failed.is_empty())
                    && !args.allow_unresolved;
                if plan.changed() {
                    if args.write && blocked {
                        blocked_targets.push(desc.display.clone());
                        let reason = if plan.unresolved.is_empty() {
                            "secret read failure(s); retry"
                        } else {
                            "unresolved secret(s); set them"
                        };
                        println!(
                            "  {} not written — {reason} or pass --allow-unresolved",
                            "✗".red()
                        );
                    } else if args.write {
                        plan.write()?;
                        state.record(&key, plan.managed.clone(), &plan.proposed, &identity);
                        // Track what this guarded write kept on disk (empty
                        // after a --prune-foreign actually pruned them).
                        state.record_kept_foreign(&key, foreign.clone());
                        crate::usage::bump(&plan.managed);
                        wrote += 1;
                        if plan.remove_if_empty_shell(desc) {
                            println!(
                                "  {} removed empty {}",
                                "−".yellow(),
                                plan.config_path.display()
                            );
                        } else {
                            println!("  {} servers → {}", "✓".green(), plan.config_path.display());
                        }
                    } else {
                        println!("  {} {} server(s) to write", "→".cyan(), plan.managed.len());
                    }
                } else {
                    // Even with no file change, keep state in sync with
                    // reality (mirrors `apply`) — prune tracking and the
                    // .gitignore block depend on it.
                    if args.write && !blocked {
                        state.record(&key, plan.managed.clone(), &plan.proposed, &identity);
                        state.record_kept_foreign(&key, foreign.clone());
                    }
                    println!("  {} servers up to date", "✓".green());
                }
            }
        }

        // --- skills --- (config-only adapters have no skills dir; they still
        // reach the managed .gitignore block below for their config entry).
        if let Some(skills_dir) = desc.skills_dir_for(scope, &ctx.dir) {
            let strategy = desc.skills.as_ref().map(|s| s.strategy).unwrap_or_default();
            let prev_skills = state.managed_skills(&key);
            let plan = skills::plan(
                skills_dir.clone(),
                strategy,
                active_skills.clone(),
                &prev_skills,
            );

            for c in &plan.conflicts {
                println!(
                    "  {} skill '{c}' already exists (not managed) — left as is",
                    "⚠".yellow()
                );
            }
            for r in &plan.to_remove {
                println!("  {} unlinking skill '{r}'", "−".yellow());
            }
            if plan.has_work() {
                if args.write {
                    skills::materialize(&plan)?;
                    state.record_skills(&key, plan.managed_names());
                    crate::usage::bump(&plan.managed_names());
                    println!(
                        "  {} {} skill(s) → {}",
                        "✓".green(),
                        plan.managed_names().len(),
                        skills_dir.display()
                    );
                } else {
                    println!(
                        "  {} {} skill(s) to {} into {}",
                        "→".cyan(),
                        plan.active.len(),
                        strategy_word(strategy),
                        skills_dir.display()
                    );
                }
            } else {
                println!("  {} skills up to date", "✓".green());
            }
        }

        // Managed .gitignore block: emit an entry only for an artifact this
        // target manages now (after the write sections above). `use` never
        // compiles instructions, so its instruction flag is the on-disk managed
        // marker `apply` leaves — the record that keeps the two commands'
        // blocks byte-identical.
        if scope == Scope::Project && args.write {
            let instr_path = desc
                .instructions
                .as_ref()
                .and_then(|s| s.path_for(scope, &ctx.dir));
            let managed = crate::render::gitignore::Managed {
                config: !state.managed_servers(&key).is_empty()
                    || !state.kept_foreign(&key).is_empty(),
                skills: !state.managed_skills(&key).is_empty(),
                instructions: instr_path
                    .as_deref()
                    .is_some_and(crate::render::instructions::manages_file),
            };
            ignore_entries.extend(crate::render::gitignore::managed_entries(
                desc, scope, &ctx.dir, managed,
            ));
        }
    }

    if args.write
        && scope == Scope::Project
        && !args.no_gitignore
        && crate::render::gitignore::ensure_block(&project_root, &ignore_entries, true)?
    {
        println!(
            "\n{} .gitignore: managed block updated — generated artifacts stay out of git ({} to commit them instead)",
            "✓".green(),
            "--no-gitignore".bold()
        );
    }

    if args.write {
        state.save()?;
        // Record each activated skill + server's resolved digest so a fresh
        // checkout resolves the same content (and `doctor`/`explain` can flag
        // drift). Server locks store the definition digest only — never a
        // resolved secret value.
        record_lock(
            &ctx.dir,
            resolved_skills,
            resolved_servers,
            manifest,
            &libctx.library,
        )?;
        if blocked_targets.is_empty() {
            println!(
                "\n{} activated '{}' on {wrote} target(s).",
                "✓".green(),
                args.profile
            );
        } else {
            // A blocked target is a failure, not a footnote: report it in the
            // summary and exit nonzero so scripts can't mistake this for done.
            println!(
                "\n{} activated '{}' on {wrote} target(s); {} target(s) BLOCKED: {}",
                "⚠".yellow(),
                args.profile,
                blocked_targets.len(),
                blocked_targets.join(", ")
            );
            anyhow::bail!(
                "unresolved secret(s) blocked {} target(s) — `agentstack secret set <NAME>` or pass --allow-unresolved",
                blocked_targets.len()
            );
        }
    } else {
        println!("\nDry run. Re-run with {} to apply.", "--write".bold());
    }
    Ok(())
}

fn strategy_word(s: crate::adapter::descriptor::SkillStrategy) -> &'static str {
    match s {
        crate::adapter::descriptor::SkillStrategy::Symlink => "symlink",
        crate::adapter::descriptor::SkillStrategy::Copy => "copy",
    }
}

/// Resolve a profile's active skills to concrete [`ResolvedSkill`]s through the
/// library-aware resolver.
///
/// Each explicit skill name resolves inline-first, then from the central library
/// (see `crate::resolve`). The `"*"` wildcard stays **inline-only**: it expands
/// to the manifest's inline skills and deliberately does not pull in central
/// library skills, to avoid surprising broad activation.
///
/// Returns an error (before any materialization) if a name resolves nowhere, its
/// source is broken, or it resolves to a path that is not present on disk.
///
/// Shared with `agentstack lock`, which pins the same resolution without
/// materializing anything.
pub(crate) fn resolve_active_skills(
    manifest: &Manifest,
    profile_name: &str,
    dir: &Path,
    library: &Library,
    lib_home: &Path,
    store: &crate::store::Store,
    mode: ResolveMode,
) -> Result<Vec<ResolvedSkill>> {
    let profile = match manifest.profiles.get(profile_name) {
        Some(p) => p,
        None => return Ok(Vec::new()),
    };
    let names: Vec<String> = if profile.loads_all_skills() {
        manifest.skills.keys().cloned().collect()
    } else {
        profile.skills.clone()
    };

    let mut out = Vec::new();
    for name in names {
        let resolved =
            crate::resolve::resolve_skill(manifest, dir, library, lib_home, store, &name, mode)
                .with_context(|| {
                    format!("resolving skill '{name}' for profile '{profile_name}'")
                })?;
        if !resolved.path.exists() {
            anyhow::bail!(
                "skill '{name}' (profile '{profile_name}') resolved to {} but it is not present on disk — run `agentstack install`",
                resolved.path.display()
            );
        }
        out.push(resolved);
    }
    Ok(out)
}

/// Pin each resolved skill + server into the project `agentstack.lock` so the
/// refs resolve to the same content on another machine. Servers lock the
/// **definition digest** only — never a resolved secret value. Existing lock
/// entries for other names are preserved.
///
/// Shared with `agentstack lock` (the lock-only path).
pub(crate) fn record_lock(
    dir: &Path,
    skills: &[ResolvedSkill],
    servers: &[ResolvedServer],
    manifest: &Manifest,
    library: &Library,
) -> Result<()> {
    let mut lock = Lock::load(dir)?;
    let before = lock.clone();
    for r in skills {
        lock.upsert(locked_from_resolved(r, manifest, library));
    }
    for r in servers {
        lock.upsert_server(LockedServer {
            name: r.name.clone(),
            source: match r.origin {
                ServerOrigin::Inline => "inline",
                ServerOrigin::Library => "library",
            }
            .to_string(),
            checksum: r.checksum.clone(),
        });
    }
    // Re-activating an unchanged profile is the common case — don't churn the
    // lockfile's mtime (and anything watching it) for a byte-identical pin.
    if lock == before {
        return Ok(());
    }
    lock.save(dir)
}

/// Build a lockfile entry from a resolved skill, recovering the source locator
/// (`path`/`git`) from wherever the name resolved.
fn locked_from_resolved(
    resolved: &ResolvedSkill,
    manifest: &Manifest,
    library: &Library,
) -> LockedSkill {
    let (path, git) = match resolved.origin {
        SkillOrigin::Inline => manifest
            .skills
            .get(&resolved.name)
            .map(|s| (s.path.clone(), s.git.clone()))
            .unwrap_or((None, None)),
        SkillOrigin::Library => library
            .get(&resolved.name)
            .map(|e| (e.path.clone(), e.git.clone()))
            .unwrap_or((None, None)),
    };
    LockedSkill {
        name: resolved.name.clone(),
        source: resolved.source_kind.to_string(),
        path,
        git,
        rev: resolved.rev.clone(),
        checksum: resolved.checksum.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{Library, LibrarySkill};
    use crate::store::Store;
    use assert_fs::prelude::*;

    fn store_in(dir: &assert_fs::TempDir) -> Store {
        Store::with_root(dir.child("store").path().to_path_buf())
    }

    /// Write a path-source skill body under `<lib_home>/skills/<name>/` and index
    /// it in the returned library.
    fn library_with_skill(lib_home: &assert_fs::TempDir, name: &str, body: &str) -> Library {
        lib_home
            .child(format!("skills/{name}/SKILL.md"))
            .write_str(body)
            .unwrap();
        let mut lib = Library::default();
        lib.upsert(LibrarySkill {
            name: name.into(),
            source: "path".into(),
            path: Some(name.into()),
            git: None,
            rev: None,
            subpath: None,
            checksum: None,
            version: None,
            provenance: Some("consolidated".into()),
        });
        lib
    }

    /// Write an inline skill body under `<proj>/skills/<name>/`.
    fn write_inline_body(proj: &assert_fs::TempDir, name: &str, body: &str) {
        proj.child(format!("skills/{name}/SKILL.md"))
            .write_str(body)
            .unwrap();
    }

    #[test]
    fn library_only_skill_activates_from_lib_home() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = store_in(&proj);
        let library = library_with_skill(&lib_home, "sql-review", "# lib\n");

        let manifest: Manifest = toml::from_str(
            r#"
            version = 1
            [profiles.p]
            skills = ["sql-review"]
            "#,
        )
        .unwrap();

        let active = resolve_active_skills(
            &manifest,
            "p",
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            ResolveMode::Fetch,
        )
        .unwrap();

        assert_eq!(active.len(), 1);
        assert_eq!(active[0].name, "sql-review");
        assert_eq!(active[0].origin, SkillOrigin::Library);
        // Path points into the central library's skills home.
        assert!(active[0].path.starts_with(lib_home.child("skills").path()));
        assert!(active[0].path.join("SKILL.md").exists());
        // A digest is captured for the lockfile.
        assert_eq!(active[0].checksum.len(), 64);
    }

    #[test]
    fn inline_skill_materializes_and_wins_over_library() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = store_in(&proj);
        // Same name in both places, different content.
        write_inline_body(&proj, "review", "# inline\n");
        let library = library_with_skill(&lib_home, "review", "# lib\n");

        let manifest: Manifest = toml::from_str(
            r#"
            version = 1
            [skills.review]
            path = "./skills/review"
            [profiles.p]
            skills = ["review"]
            "#,
        )
        .unwrap();

        let active = resolve_active_skills(
            &manifest,
            "p",
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            ResolveMode::Fetch,
        )
        .unwrap();

        assert_eq!(active.len(), 1);
        assert_eq!(active[0].origin, SkillOrigin::Inline);
        let body = std::fs::read_to_string(active[0].path.join("SKILL.md")).unwrap();
        assert_eq!(body, "# inline\n");
    }

    #[test]
    fn unresolved_library_name_fails() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = store_in(&proj);
        let library = Library::default(); // empty

        let manifest: Manifest = toml::from_str(
            r#"
            version = 1
            [profiles.p]
            skills = ["nope"]
            "#,
        )
        .unwrap();

        let err = resolve_active_skills(
            &manifest,
            "p",
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            ResolveMode::Fetch,
        )
        .unwrap_err();
        assert!(err.to_string().contains("nope"));
    }

    #[test]
    fn broken_library_entry_fails_before_materialization() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = store_in(&proj);
        // Indexed by name but with neither `path` nor `git` — source is broken.
        let mut library = Library::default();
        library.upsert(LibrarySkill {
            name: "sql-review".into(),
            source: "path".into(),
            path: None,
            git: None,
            rev: None,
            subpath: None,
            checksum: None,
            version: None,
            provenance: None,
        });

        let manifest: Manifest = toml::from_str(
            r#"
            version = 1
            [profiles.p]
            skills = ["sql-review"]
            "#,
        )
        .unwrap();

        let err = resolve_active_skills(
            &manifest,
            "p",
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            ResolveMode::Fetch,
        )
        .unwrap_err();
        assert!(err.to_string().contains("sql-review"));
    }

    #[test]
    fn wildcard_expands_inline_only_and_ignores_library() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = store_in(&proj);
        write_inline_body(&proj, "a", "# a\n");
        write_inline_body(&proj, "b", "# b\n");
        // A library-only skill that must NOT be activated by the wildcard.
        let library = library_with_skill(&lib_home, "c", "# c\n");

        let manifest: Manifest = toml::from_str(
            r#"
            version = 1
            [skills.a]
            path = "./skills/a"
            [skills.b]
            path = "./skills/b"
            [profiles.p]
            skills = ["*"]
            "#,
        )
        .unwrap();

        let active = resolve_active_skills(
            &manifest,
            "p",
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            ResolveMode::Fetch,
        )
        .unwrap();

        let names: Vec<&str> = active.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b"]);
        assert!(
            !names.contains(&"c"),
            "wildcard must not pull library skills"
        );
    }

    #[test]
    fn record_lock_writes_resolved_digest_for_library_skill() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = store_in(&proj);
        let library = library_with_skill(&lib_home, "sql-review", "# lib\n");

        let manifest: Manifest = toml::from_str(
            r#"
            version = 1
            [profiles.p]
            skills = ["sql-review"]
            "#,
        )
        .unwrap();

        let resolved = resolve_active_skills(
            &manifest,
            "p",
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            ResolveMode::Fetch,
        )
        .unwrap();

        record_lock(proj.path(), &resolved, &[], &manifest, &library).unwrap();

        // The lock now pins the library skill's resolved digest.
        let lock = Lock::load(proj.path()).unwrap();
        let entry = lock.get("sql-review").expect("lock entry written");
        assert_eq!(entry.source, "path");
        assert_eq!(entry.path.as_deref(), Some("sql-review"));
        assert_eq!(entry.checksum, resolved[0].checksum);
        assert_eq!(entry.checksum.len(), 64);
    }

    #[test]
    fn record_lock_preserves_unrelated_entries() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = store_in(&proj);
        let library = library_with_skill(&lib_home, "sql-review", "# lib\n");

        // A pre-existing lock entry for a different, now-unmanaged skill.
        let mut lock = Lock::default();
        lock.upsert(LockedSkill {
            name: "other".into(),
            source: "path".into(),
            path: Some("other".into()),
            git: None,
            rev: None,
            checksum: "beef".into(),
        });
        lock.save(proj.path()).unwrap();

        let manifest: Manifest = toml::from_str(
            r#"
            version = 1
            [profiles.p]
            skills = ["sql-review"]
            "#,
        )
        .unwrap();
        let resolved = resolve_active_skills(
            &manifest,
            "p",
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            ResolveMode::Fetch,
        )
        .unwrap();
        record_lock(proj.path(), &resolved, &[], &manifest, &library).unwrap();

        let lock = Lock::load(proj.path()).unwrap();
        assert!(lock.get("other").is_some(), "unrelated entry preserved");
        assert!(lock.get("sql-review").is_some(), "new entry added");
    }

    #[test]
    fn record_lock_skips_the_write_when_nothing_changed() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = store_in(&proj);
        let library = library_with_skill(&lib_home, "sql-review", "# lib\n");
        let manifest: Manifest = toml::from_str(
            r#"
            version = 1
            [profiles.p]
            skills = ["sql-review"]
            "#,
        )
        .unwrap();
        let resolved = resolve_active_skills(
            &manifest,
            "p",
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            ResolveMode::Fetch,
        )
        .unwrap();
        record_lock(proj.path(), &resolved, &[], &manifest, &library).unwrap();

        // Plant a marker a rewrite would erase (parsing drops comments): if the
        // pins are byte-identical, record_lock must leave the file alone.
        let path = Lock::path(proj.path());
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("# marker\n");
        std::fs::write(&path, &text).unwrap();

        record_lock(proj.path(), &resolved, &[], &manifest, &library).unwrap();
        assert!(
            std::fs::read_to_string(&path).unwrap().contains("# marker"),
            "unchanged pins must not rewrite the lockfile"
        );

        // A real change (new content digest) does rewrite.
        lib_home
            .child("skills/sql-review/SKILL.md")
            .write_str("# changed\n")
            .unwrap();
        let resolved = resolve_active_skills(
            &manifest,
            "p",
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            ResolveMode::Fetch,
        )
        .unwrap();
        record_lock(proj.path(), &resolved, &[], &manifest, &library).unwrap();
        assert!(
            !std::fs::read_to_string(&path).unwrap().contains("# marker"),
            "changed pins rewrite the lockfile"
        );
    }

    #[test]
    fn record_lock_pins_server_definition_digest() {
        let proj = assert_fs::TempDir::new().unwrap();

        // A resolved library server carrying a ${REF} — its definition digest is
        // what gets locked (never the secret value).
        let resolved_server = ResolvedServer {
            name: "kibana".into(),
            origin: ServerOrigin::Library,
            server: toml::from_str(
                "type = \"http\"\nurl = \"https://x/mcp\"\nheaders = { Authorization = \"Bearer ${TOKEN}\" }\n",
            )
            .unwrap(),
            checksum: "cafebabe".into(),
            provenance: Some("consolidated:codex".into()),
        };

        let manifest: Manifest = toml::from_str("version = 1").unwrap();
        let library = Library::default();
        record_lock(
            proj.path(),
            &[],
            std::slice::from_ref(&resolved_server),
            &manifest,
            &library,
        )
        .unwrap();

        let lock = Lock::load(proj.path()).unwrap();
        let entry = lock
            .get_server("kibana")
            .expect("server lock entry written");
        assert_eq!(entry.source, "library");
        assert_eq!(entry.checksum, "cafebabe");
        // The lock holds only name/source/checksum — never a secret value.
        let text = std::fs::read_to_string(Lock::path(proj.path())).unwrap();
        assert!(
            !text.contains("Bearer"),
            "no definition body or secret in lock"
        );
    }
}
