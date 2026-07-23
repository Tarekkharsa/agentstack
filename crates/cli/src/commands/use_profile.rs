//! `agentstack use <profile>` — activate a profile: render its servers into each
//! target's config and materialize its skills into the target's skills dir, for
//! the chosen scope. Dry-run by default; `--write` performs changes.

use agentstack_core::digest::Sha256Hex;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use indexmap::IndexMap;
use owo_colors::OwoColorize;

use crate::cli::UseArgs;
use crate::library::Library;
use crate::lock::{Lock, LockedServer, LockedSkill, ServerSource, SkillLockSource};
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
    /// The profile this activation resolved to. `None` is the implicit
    /// default: the manifest declares no profiles, so the full inline set
    /// (every `[skills.*]` and `[servers.*]`) is what activates.
    pub profile: Option<String>,
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

    // Which profile drives this activation (early, so a bad name fails before
    // anything resolves). `None` = the implicit default set.
    let profile = selected_profile(manifest, args.profile.as_deref())?;

    let mode = if args.write {
        ResolveMode::Fetch
    } else {
        ResolveMode::NoFetch
    };
    // Activation and its dry run reproduce existing lock pins. Without
    // threading these commits into resolution, a rev-less or branch-based
    // manifest could follow a shared clone that another skill has churned.
    let lock = Lock::load(&ctx.dir)?;
    let resolved_skills = resolve_active_skills_with_pins(
        manifest,
        profile.as_deref(),
        &ctx.dir,
        &libctx.library,
        &libctx.lib_home,
        &libctx.store,
        mode,
        Some(&lock),
    )?;

    // `${REF}`s stay intact; they are resolved per-target at render time, not
    // here. The resolved list is kept for lock recording; the `name -> Server`
    // map drives rendering.
    let selection = match &profile {
        Some(p) => Selection::Profile(p.clone()),
        None => Selection::All,
    };
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
        args.scope.unwrap_or_else(|| Scope::default_for(&ctx.dir)),
        &ctx.dir,
    );

    Ok(Prepared {
        resolved_skills,
        resolved_servers,
        server_map,
        profile,
    })
}

/// Which profile drives an activation. A named profile must exist. With no
/// name given: the single declared profile is unambiguous; several need a
/// name; **none declared** selects the implicit default — every inline skill
/// and server in the manifest (`Ok(None)`). Profiles are opt-in selectivity,
/// not a prerequisite for activation.
pub(crate) fn selected_profile(
    manifest: &Manifest,
    requested: Option<&str>,
) -> Result<Option<String>> {
    match requested {
        Some(p) => {
            manifest
                .profiles
                .get(p)
                .with_context(|| {
                    format!("no profile '{p}' in manifest — check the `[profiles.*]` tables there for the exact name")
                })?;
            Ok(Some(p.to_string()))
        }
        None => {
            let mut names = manifest.profiles.keys();
            match (names.next(), names.next()) {
                (None, _) => Ok(None),
                (Some(only), None) => Ok(Some(only.clone())),
                // Several declared: the error *is* the profile listing (P18) —
                // each name with its server + skill counts and the exact
                // command to pick it, so disambiguating and discovering "what's
                // in each profile" are the same step.
                (Some(_), Some(_)) => anyhow::bail!(profile_disambiguation(manifest)),
            }
        }
    }
}

/// The multi-line disambiguation listing for `agentstack use` with several
/// profiles and no name given (P18): each profile on its own line with its
/// server + skill counts and the command to select it. Counts are the profile's
/// declared servers and its *effective* skills — a `"*"` wildcard expands to
/// every inline skill, so the number reflects what would actually activate.
/// Pure over the manifest so the listing is unit-tested directly.
pub(crate) fn profile_disambiguation(manifest: &Manifest) -> String {
    // Precompute each row's "N servers · M skills" so the name and counts
    // columns can be padded to their widest — the select commands then line up
    // instead of drifting with each row's digit and plural widths. Padding uses
    // char counts (the middle dot is one char) to stay right in a monospace TTY.
    let rows: Vec<(&String, String)> = manifest
        .profiles
        .iter()
        .map(|(name, profile)| {
            let servers = profile.servers.len();
            let skills = if profile.loads_all_skills() {
                manifest.skills.len()
            } else {
                profile.skills.len()
            };
            let counts = format!("{} · {}", count(servers, "server"), count(skills, "skill"));
            (name, counts)
        })
        .collect();
    let name_w = rows.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
    let counts_w = rows
        .iter()
        .map(|(_, c)| c.chars().count())
        .max()
        .unwrap_or(0);

    let mut out = String::from("several profiles declared — name one:");
    for (name, counts) in &rows {
        out.push_str(&format!(
            "\n  {name:<name_w$}   {counts:<counts_w$}   agentstack use {name}"
        ));
    }
    out
}

/// "1 server" / "2 servers" — a count with its correctly pluralized noun.
fn count(n: usize, noun: &str) -> String {
    format!("{n} {noun}{}", if n == 1 { "" } else { "s" })
}

pub fn run(args: &UseArgs, manifest_dir: Option<&Path>) -> Result<()> {
    if args.list {
        return list_profiles(args.json, manifest_dir);
    }
    let ctx = super::load(manifest_dir)?;
    let libctx = ctx.library_ctx();
    let prepared = prepare(&ctx, &libctx, args)?;
    activate(&ctx, &libctx, args, &prepared)
}

/// `use --list [--json]` — the Lane B read primitive (UI control-plane §5):
/// every declared profile with its resolved selection and a readiness flag —
/// is everything the profile references pinned in `agentstack.lock` and
/// matching? Read-only and advisory: the flag tells a picker which profiles
/// are one click from a session and which need `lock`/review first; the
/// ENFORCEMENT lives in `session start`'s fail-closed gate, which refuses an
/// unpinned or untrusted surface regardless of what any UI displayed.
fn list_profiles(json: bool, manifest_dir: Option<&Path>) -> Result<()> {
    let out = list_json_value(manifest_dir)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&crate::ui_contract::envelope(out))?
        );
        return Ok(());
    }
    print_profile_listing(&out);
    Ok(())
}

/// The `use --list` body without the envelope: path, trust, profiles, and the
/// active session (if any). Public read API so integrations and tests
/// exercise the exact production listing instead of re-deriving one — the
/// same seam as `restore::list_json_value` and `init::plan_json`.
pub fn list_json_value(manifest_dir: Option<&Path>) -> Result<serde_json::Value> {
    let ctx = super::load(manifest_dir)?;
    let libctx = ctx.library_ctx();
    let manifest = &ctx.loaded.manifest;
    // A broken lockfile fails the listing loudly — its pins are exactly what
    // the readiness flag reports on.
    let lock = Lock::load(&ctx.dir)?;

    // Trust is keyed by the project BASE (the dir holding `.agentstack/`),
    // not the manifest dir.
    let base = super::project_base(manifest_dir)?;
    let trust_state = match crate::trust::check(&base) {
        crate::trust::TrustState::Trusted => "trusted",
        crate::trust::TrustState::Changed => "drifted",
        crate::trust::TrustState::Untrusted => "untrusted",
    };

    // The active session here, if any — the picker's "in use" state, and the
    // recovery surface when a supervising UI died mid-session: the state
    // comes from the CLI's own session store on every read, so a reopened
    // panel sees the interrupted session and can offer the safe end.
    let active_session = crate::session::active(&ctx.dir);

    let mut profiles: Vec<serde_json::Value> = Vec::new();
    for (name, profile) in &manifest.profiles {
        // (skill name, verdict) over the profile's resolved set. Resolution
        // itself can fail (broken ref, no library); a failed resolution is a
        // blocker, not a listing error — the picker must still render.
        let mut blockers: Vec<(String, String)> = Vec::new();
        let skills: Vec<String> = match resolve_active_skills_with_pins(
            manifest,
            Some(name),
            &ctx.dir,
            &libctx.library,
            &libctx.lib_home,
            &libctx.store,
            ResolveMode::NoFetch,
            Some(&lock),
        ) {
            Ok(resolved) => {
                for r in &resolved {
                    let status = crate::resolve::classify_skill(
                        &r.name,
                        &r.checksum,
                        r.rev.as_deref(),
                        &lock,
                    );
                    match crate::verify::skill_verdict(&status) {
                        crate::verify::Verdict::Ok => {}
                        crate::verify::Verdict::Unpinned => blockers
                            .push((r.name.clone(), "unpinned — run `agentstack lock`".into())),
                        crate::verify::Verdict::Block(why) => blockers.push((r.name.clone(), why)),
                    }
                }
                resolved.into_iter().map(|r| r.name).collect()
            }
            Err(e) => {
                blockers.push((name.clone(), format!("skills unresolvable — {e}")));
                profile.skills.clone()
            }
        };
        let servers: Vec<String> = match crate::render::resolve_active_servers(
            manifest,
            &libctx.library,
            &libctx.lib_home,
            &crate::render::Selection::Profile(name.clone()),
        ) {
            Ok(resolved) => {
                for r in &resolved {
                    let status = crate::resolve::classify_server(&r.name, &r.checksum, &lock);
                    match crate::verify::server_verdict(&status) {
                        crate::verify::Verdict::Ok => {}
                        crate::verify::Verdict::Unpinned => blockers
                            .push((r.name.clone(), "unpinned — run `agentstack lock`".into())),
                        crate::verify::Verdict::Block(why) => blockers.push((r.name.clone(), why)),
                    }
                }
                resolved.into_iter().map(|r| r.name).collect()
            }
            Err(e) => {
                blockers.push((name.clone(), format!("servers unresolvable — {e}")));
                profile.servers.clone()
            }
        };
        // Names come from unreviewed repo content — sanitized for display,
        // exactly like the trust preview.
        profiles.push(serde_json::json!({
            "name": crate::text::sanitize_line(name),
            "skills": skills.iter().map(|s| crate::text::sanitize_line(s)).collect::<Vec<_>>(),
            "servers": servers.iter().map(|s| crate::text::sanitize_line(s)).collect::<Vec<_>>(),
            "harness": profile.harness.as_deref().map(crate::text::sanitize_line),
            "pinned": blockers.is_empty(),
            "active": active_session.as_ref().is_some_and(|s| s.profile == *name),
            "blockers": blockers
                .iter()
                .map(|(n, why)| serde_json::json!({
                    "name": crate::text::sanitize_line(n),
                    "reason": crate::text::sanitize_line(why),
                }))
                .collect::<Vec<_>>(),
        }));
    }

    Ok(serde_json::json!({
        "path": base.display().to_string(),
        "trust": trust_state,
        "profiles": profiles,
        // Null when nothing is active; a UI renders the end/recovery action
        // from this object, never from its own remembered state.
        "session": active_session.map(|s| serde_json::json!({
            "profile": crate::text::sanitize_line(&s.profile),
            "scope": s.scope,
            "started_unix": s.started_unix,
        })),
    }))
}

/// Human rendering of the listing body (the non-`--json` branch).
fn print_profile_listing(out: &serde_json::Value) {
    let trust_state = out["trust"].as_str().unwrap_or("?");
    let profiles = out["profiles"].as_array().map_or(&[][..], Vec::as_slice);
    if profiles.is_empty() {
        println!(
            "No profiles declared — the implicit default (every inline skill and server) is what activates."
        );
        return;
    }
    println!("Declared profiles (project trust: {trust_state}):");
    for p in profiles {
        let name = p["name"].as_str().unwrap_or("?");
        let ready = if p["pinned"].as_bool().unwrap_or(false) {
            "pinned".to_string()
        } else {
            format!(
                "{} blocker(s)",
                p["blockers"].as_array().map_or(0, Vec::len)
            )
        };
        let in_use = if p["active"].as_bool().unwrap_or(false) {
            "  · in use (agentstack session end reverts it)"
        } else {
            ""
        };
        println!(
            "  {name}  —  {} skill(s), {} server(s)  [{ready}]{in_use}",
            p["skills"].as_array().map_or(0, Vec::len),
            p["servers"].as_array().map_or(0, Vec::len),
        );
    }
}

/// Render the prepared profile into every target (servers + skills), record
/// state, and pin the lockfile. The write half of `run` — takes pre-loaded
/// context and pre-resolved sets so callers like session start don't load and
/// resolve everything twice.
/// Per-target outcomes of an add-only skill materialization, for the caller
/// to print in house style.
pub(crate) struct SkillsActivation {
    /// (target id, skills dir written into).
    pub written: Vec<(String, PathBuf)>,
    /// (target id, skill name) where a user-owned dir was left as is.
    pub conflicts: Vec<(String, String)>,
    /// (target id, reason) — reported, never silently skipped.
    pub unsupported: Vec<(String, &'static str)>,
    /// (target id, sanitized error) — the loop continues past a failure.
    pub failed: Vec<(String, String)>,
}

/// Additive skill materialization for `agentstack add skill --write`: a second
/// path beside
/// `activate()`'s skills block — that block prunes and full-replaces state,
/// which are load-bearing `use` behaviors this helper must NOT share:
/// `plan()` runs with `previously_managed = &[]` (an add never prunes) and
/// state records the UNION of the prior managed set and what materialized
/// (`record_skills` is a full overwrite; recording less would silently
/// untrack live symlinks). Skills-only by construction: no server, hook,
/// settings, or instruction path is touched.
pub(crate) fn materialize_skills_additive(
    ctx: &super::Context,
    scope: Scope,
    target_ids: &[String],
    new_skills: &[(String, PathBuf)],
    no_gitignore: bool,
) -> Result<SkillsActivation> {
    let mut out = SkillsActivation {
        written: Vec::new(),
        conflicts: Vec::new(),
        unsupported: Vec::new(),
        failed: Vec::new(),
    };
    let mut state = State::load()?;
    let mut ignore_entries: Vec<String> = Vec::new();
    for id in target_ids {
        let Some(desc) = ctx.registry.get(id) else {
            // resolve_targets validated ids; a manifest-sourced unknown is
            // reported, not dropped.
            out.unsupported.push((id.clone(), "unknown adapter"));
            continue;
        };
        let Some(skills_dir) = desc.skills_dir_for(scope, &ctx.dir) else {
            // BOTH absent cases are reported (binding decision: never a
            // silent skip) — including the copilot-cli shape (`skills`
            // declared, no project dir), which `use` still skips silently
            // today (named follow-up).
            out.unsupported.push((
                id.clone(),
                if desc.skills.is_none() {
                    "skills not supported by this CLI"
                } else {
                    "no skills dir at this scope for this CLI"
                },
            ));
            continue;
        };
        let strategy = desc.skills.as_ref().map(|s| s.strategy).unwrap_or_default();
        let key = target_key(id, scope, &ctx.dir);
        let plan = match skills::plan(skills_dir.clone(), strategy, new_skills.to_vec(), &[]) {
            Ok(p) => p,
            Err(e) => {
                out.failed
                    .push((id.clone(), crate::text::sanitize_line(&format!("{e:#}"))));
                continue;
            }
        };
        for c in &plan.conflicts {
            out.conflicts.push((id.clone(), c.clone()));
        }
        if let Err(e) = skills::materialize(&plan) {
            out.failed
                .push((id.clone(), crate::text::sanitize_line(&format!("{e:#}"))));
            continue;
        }
        // The union rule: conflicted names are already excluded by
        // managed_names(), so a user-owned dir is never claimed as managed.
        let mut union = state.managed_skills(&key);
        for n in plan.managed_names() {
            if !union.contains(&n) {
                union.push(n);
            }
        }
        state.record_skills(&key, union);
        crate::usage::bump(&plan.managed_names());
        if scope == Scope::Project {
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
        out.written.push((id.clone(), skills_dir));
    }
    if scope == Scope::Project && !no_gitignore && !out.written.is_empty() {
        // The block is one shared artifact: harvest extension entries too
        // (write=false — plan only) so rewriting it never drops them.
        ignore_entries.extend(crate::render::extensions::render(
            &ctx.loaded.manifest,
            &ctx.registry,
            scope,
            &ctx.dir,
            false,
        )?);
        let project_root = crate::manifest::project_root_of(&ctx.dir);
        let _ = crate::render::gitignore::ensure_block(&project_root, &ignore_entries, true)?;
    }
    state.save()?;
    Ok(out)
}

pub fn activate(
    ctx: &super::Context,
    libctx: &super::LibraryCtx,
    args: &UseArgs,
    prepared: &Prepared,
) -> Result<()> {
    let manifest = &ctx.loaded.manifest;
    // Default scope follows the manifest's home: project for a repo manifest,
    // global only for the machine manifest.
    let scope = args.scope.unwrap_or_else(|| Scope::default_for(&ctx.dir));
    let resolved_skills = &prepared.resolved_skills;
    let resolved_servers = &prepared.resolved_servers;
    let server_map = &prepared.server_map;
    // Display label; the implicit no-profiles selection reads as "default".
    let label = prepared.profile.clone().unwrap_or_else(|| "default".into());
    // The exact re-run command: with an implicit default there is no profile
    // word to repeat.
    let use_cmd_profile = prepared
        .profile
        .as_ref()
        .map(|p| format!("{p} "))
        .unwrap_or_default();
    // (name, source dir) pairs drive skill materialization; the richer
    // `ResolvedSkill` list is kept for lockfile recording below.
    let active_skills: Vec<(String, PathBuf)> = resolved_skills
        .iter()
        .map(|r| (r.name.clone(), r.path.clone()))
        .collect();

    // Fail-closed drift gate (--write only): everything resolved above must
    // still match its agentstack.lock pin before a single byte is
    // materialized. Unpinned entries pass — recording the first pin below IS
    // the pinning act, and it re-gates trust via the lock bytes. Drifted or
    // broken entries block: the human reviews, `agentstack lock` accepts, and
    // that lock change flips the trust digest for auto mode. The statuses are
    // classified from the already-resolved sets, so what we verify is exactly
    // what we materialize and record.
    if args.write {
        let lock = Lock::load(&ctx.dir)?;
        let skill_statuses: Vec<_> = resolved_skills
            .iter()
            .map(|r| {
                let status =
                    crate::resolve::classify_skill(&r.name, &r.checksum, r.rev.as_deref(), &lock);
                (r.name.clone(), status)
            })
            .collect();
        let server_statuses: Vec<_> = resolved_servers
            .iter()
            .map(|r| {
                let status = crate::resolve::classify_server(&r.name, &r.checksum, &lock);
                (r.name.clone(), status)
            })
            .collect();
        crate::verify::ensure_activatable(
            &format!("'{label}'"),
            &skill_statuses,
            &server_statuses,
        )?;
        // D3 pre-render gate: an unverifiable local executable (symlink,
        // traversal, non-regular file, broken declared root) must block HERE,
        // before any native config is materialized — record_lock rejects it
        // too, but that runs after targets were already written.
        for r in resolved_servers {
            crate::executable::derive_executable_pins(&ctx.dir, &r.name, &r.server)?;
        }
    }

    let target_ids = resolve_targets(manifest, &ctx.registry, &args.targets)?;
    let ruleset = crate::render::ruleset_for(manifest)?;
    println!(
        "Activating profile '{}' (scope: {scope}) — {} server(s), {} skill(s)",
        label.bold(),
        server_map.len(),
        active_skills.len()
    );

    // P19: shadowing an inline skill over a same-named central-library skill is
    // legal (an inline definition always wins), but silence is not — one warning
    // line per shadow so the operator knows the library copy was set aside for
    // the project's own. The inline skill resolved fine here (it has a source);
    // the empty-block trap is caught earlier, in the resolver.
    for r in resolved_skills.iter() {
        if r.origin == SkillOrigin::Inline && libctx.library.get(&r.name).is_some() {
            println!(
                "  {} skill '{}' is defined inline and shadows a same-named central-library skill — the inline copy is used",
                "⚠".yellow(),
                r.name
            );
        }
    }

    let mut state = State::load()?;
    let identity = crate::state::manifest_identity(&ctx.dir);
    let mut wrote = 0;
    // Skill materializations counted separately: "activated on 0 target(s)"
    // right under a "✓ N skill(s) → …" line reads as a contradiction when no
    // CLI binaries are on PATH but skills were genuinely written.
    let mut wrote_skill_dirs = 0;
    let mut blocked_targets: Vec<String> = Vec::new();
    // Distinct missing secret names across targets — the final blocked error
    // prints their exact `secret set` commands (see the apply counterpart).
    let mut missing_secrets: std::collections::BTreeSet<String> = Default::default();
    // Project-scope artifacts we write are machine-local (absolute-path
    // symlinks, resolved values) — collect them for the managed .gitignore
    // block unless the user opts out. Entries are stable and directory-level
    // (the config file, the whole skills dir) so the block never churns as
    // profile membership changes.
    let project_root = crate::manifest::project_root_of(&ctx.dir);
    let mut ignore_entries: Vec<String> = Vec::new();
    // Pre-write snapshots of every server config this activation touches, so
    // `agentstack restore` can undo a `use --write` exactly like an `apply
    // --write` (skill materializations are additive and reverted by `session
    // end`, so they are not captured here).
    let mut backups: Vec<crate::history::FileChange> = Vec::new();
    let mut history_targets: Vec<String> = Vec::new();

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
            &ruleset,
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
                         prune: agentstack use {}--prune-foreign",
                        "⚠".yellow(),
                        foreign.join(", "),
                        use_cmd_profile
                    );
                }
                for u in &plan.unresolved {
                    // Same ↳ fix convention as doctor: the entry reads
                    // "NAME (server 'x')", so the first token is the ref name.
                    let name = u.split_whitespace().next().unwrap_or(u.as_str());
                    println!(
                        "  {} unresolved secret {u} ↳ agentstack secret set {name}",
                        "✗".red()
                    );
                    missing_secrets.insert(name.to_string());
                }
                for d in &plan.denied {
                    println!("  {} blocked by policy: {}", "✗".red(), d);
                }
                for f in &plan.failed {
                    println!("  {} {}", "✗".red(), crate::render::failed_secret_line(f));
                    // Same `secret set` fix whether missing or unreadable —
                    // keep the closing tail copy-pasteable in both cases.
                    let name = f.split_whitespace().next().unwrap_or(f.as_str());
                    missing_secrets.insert(name.to_string());
                }
                let blocked = ((!plan.unresolved.is_empty() || !plan.failed.is_empty())
                    && !args.allow_unresolved)
                    || !plan.denied.is_empty();
                if plan.changed() {
                    if args.write && blocked {
                        blocked_targets.push(desc.display.clone());
                        let reason = if plan.unresolved.is_empty() {
                            "secret read failure(s); set them"
                        } else {
                            "unresolved secret(s); set them"
                        };
                        println!(
                            "  {} not written — {reason} or pass --allow-unresolved",
                            "✗".red()
                        );
                    } else if args.write {
                        backups.push(crate::history::capture(
                            &plan.config_path,
                            format!("{} · servers", desc.display),
                        ));
                        history_targets.push(desc.display.clone());
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
            )?;

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
                    wrote_skill_dirs += 1;
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
        } else if !active_skills.is_empty() {
            // This CLI can't take the skills at this scope — either it has no
            // skills support at all, or (copilot-cli shape) it declares a
            // global skills dir but no project one. Both are REPORTED: a
            // resolved target that can't be materialized is never silently
            // skipped because profile activation may legitimately omit it.
            let reason = if desc.skills.is_none() {
                "skills not supported by this CLI"
            } else {
                "no skills dir at this scope for this CLI"
            };
            println!(
                "  {} ({reason} — {} skill(s) not materialized)",
                "·".dimmed(),
                active_skills.len()
            );
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

    // Native extensions (D6): copy declared `[extensions.*]` sources into their
    // target harness's extension directory — fail-closed on trust + lock,
    // pruned via an ownership ledger. Independent of the per-target server
    // fan-out; project-scope artifacts join the managed .gitignore block.
    let ext_ignore =
        crate::render::extensions::render(manifest, &ctx.registry, scope, &ctx.dir, args.write)?;
    ignore_entries.extend(ext_ignore);

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
        // Fully-blocked activation is a no-op on disk: every target refused its
        // write, so no server config and no skill dir landed. Pinning the
        // lockfile here would leave a phantom behind — an activation that never
        // happened, yet a lock alone is enough for `overview` to infer a
        // delivery mode ("clean-at-rest") the project never reached. So skip the
        // lock write on total failure; a pre-existing lock keeps its own bytes
        // untouched (record_lock is the only path that would rewrite it).
        // Partial success — at least one server config or skill dir written —
        // genuinely activated, so it still pins.
        let nothing_activated = wrote == 0 && wrote_skill_dirs == 0;
        let total_failure = !blocked_targets.is_empty() && nothing_activated;
        // One undoable history entry for the server configs this activation
        // wrote. Best-effort, like apply: never fail a successful use over it.
        if !backups.is_empty() {
            history_targets.dedup();
            let _ = crate::history::record(scope.as_str(), history_targets.clone(), backups);
        }
        if !total_failure {
            // Record each activated skill + server's resolved digest so a fresh
            // checkout resolves the same content (and `doctor`/`explain` can
            // flag drift). Server locks store the definition digest only — never
            // a resolved secret value.
            record_lock(
                &ctx.dir,
                resolved_skills,
                resolved_servers,
                manifest,
                &libctx.library,
            )?;
        }
        if blocked_targets.is_empty() {
            if wrote == 0 && wrote_skill_dirs > 0 {
                println!(
                    "\n{} activated '{}' — wrote skills to {wrote_skill_dirs} location(s); no server configs changed.",
                    "✓".green(),
                    label
                );
            } else {
                println!(
                    "\n{} activated '{}' on {wrote} target(s).",
                    "✓".green(),
                    label
                );
            }
            // Only claim undoability for what restore actually covers: the
            // server configs captured above (skills revert via `session end`).
            if wrote > 0 {
                println!("  {}", "undo: agentstack restore".dimmed());
            }
        } else {
            // A blocked target is a failure, not a footnote: report it in the
            // summary and exit nonzero so scripts can't mistake this for done.
            println!(
                "\n{} activated '{}' on {wrote} target(s); {} target(s) BLOCKED: {}",
                "⚠".yellow(),
                label,
                blocked_targets.len(),
                blocked_targets.join(", ")
            );
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
                "unresolved secret(s) blocked {} target(s) — {fix}",
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
    profile_name: Option<&str>,
    dir: &Path,
    library: &Library,
    lib_home: &Path,
    store: &crate::store::Store,
    mode: ResolveMode,
) -> Result<Vec<ResolvedSkill>> {
    resolve_active_skills_with_pins(
        manifest,
        profile_name,
        dir,
        library,
        lib_home,
        store,
        mode,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn resolve_active_skills_with_pins(
    manifest: &Manifest,
    profile_name: Option<&str>,
    dir: &Path,
    library: &Library,
    lib_home: &Path,
    store: &crate::store::Store,
    mode: ResolveMode,
    pins: Option<&Lock>,
) -> Result<Vec<ResolvedSkill>> {
    // `None` is the implicit default (no profiles declared): every inline
    // skill — the same inline-only expansion the `"*"` wildcard uses.
    let names: Vec<String> = match profile_name {
        None => manifest.skills.keys().cloned().collect(),
        Some(profile_name) => match manifest.profiles.get(profile_name) {
            None => return Ok(Vec::new()),
            Some(profile) => {
                if profile.loads_all_skills() {
                    manifest.skills.keys().cloned().collect()
                } else {
                    profile.skills.clone()
                }
            }
        },
    };
    let plabel = profile_name.unwrap_or("default");

    let mut out = Vec::new();
    for name in names {
        let pinned_rev = pins
            .and_then(|lock| lock.get(&name))
            .and_then(|entry| entry.rev.as_deref());
        let resolved = crate::resolve::resolve_skill_with_pin(
            manifest, dir, library, lib_home, store, &name, mode, pinned_rev,
        )
        .with_context(|| format!("resolving skill '{name}' for profile '{plabel}'"))?;
        if !resolved.path.exists() {
            anyhow::bail!(
                "skill '{name}' (profile '{plabel}') resolved to {} but it is not present on disk — run `agentstack install`",
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
                ServerOrigin::Inline => ServerSource::Inline,
                ServerOrigin::Library => ServerSource::Library,
            },
            checksum: Sha256Hex::parse(&r.checksum)?,
        });
        // D3: pin the server's repository-local executable surface alongside
        // its definition — auto-detected command/args files plus declared
        // integrity roots. An unverifiable local candidate fails the whole
        // lock write (nothing is saved below on error).
        for pin in crate::executable::derive_executable_pins(dir, &r.name, &r.server)? {
            lock.upsert_executable(pin);
        }
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
        // `source_kind` is an internal `&'static str` tag ("path"/"git");
        // parse it to the typed lockfile source at this boundary.
        source: match resolved.source_kind {
            "git" => SkillLockSource::Git,
            _ => SkillLockSource::Path,
        },
        path,
        git,
        rev: resolved.rev.clone(),
        checksum: Sha256Hex::parse(&resolved.checksum)
            .expect("a resolved skill checksum is a digest this process computed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{Library, LibrarySkill};

    // P18(b) witness: the several-profiles error IS the profile listing — each
    // profile on its own line with server + skill counts (pluralized) and the
    // exact command to select it. A `"*"` skills wildcard counts the manifest's
    // inline skills, not the literal `["*"]`.
    #[test]
    fn disambiguation_lists_each_profile_with_counts() {
        let manifest: Manifest = toml::from_str(
            r#"
            version = 1
            [servers.s1]
            type = "stdio"
            command = "x"
            [skills.only]
            path = "./skills/only"
            [profiles.dev]
            servers = ["s1"]
            skills = ["only"]
            [profiles.prod]
            servers = []
            skills = ["*"]
            "#,
        )
        .unwrap();

        let listing = profile_disambiguation(&manifest);
        // One line per profile, each carrying its select command.
        assert!(listing.contains("agentstack use dev"), "{listing}");
        assert!(listing.contains("agentstack use prod"), "{listing}");
        // dev: one declared server, one declared skill (both singular).
        assert!(listing.contains("1 server · 1 skill"), "{listing}");
        // prod: no servers (plural zero) and the wildcard expands to the single
        // inline skill — not counted as the literal `["*"]` entry.
        assert!(listing.contains("0 servers · 1 skill"), "{listing}");
        // The listing IS the error header, so both are one message.
        assert!(
            listing.starts_with("several profiles declared"),
            "{listing}"
        );
    }
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
            Some("p"),
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
            Some("p"),
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
            Some("p"),
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
            Some("p"),
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
            Some("p"),
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
            Some("p"),
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
        assert_eq!(entry.source, SkillLockSource::Path);
        assert_eq!(entry.path.as_deref(), Some("sql-review"));
        assert_eq!(entry.checksum.hex(), resolved[0].checksum);
        assert_eq!(entry.checksum.hex().len(), 64);
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
            source: SkillLockSource::Path,
            path: Some("other".into()),
            git: None,
            rev: None,
            checksum: Sha256Hex::of(b"beef"),
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
            Some("p"),
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
            Some("p"),
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
            Some("p"),
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
            checksum: Sha256Hex::of(b"cafebabe").hex().to_string(),
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
        assert_eq!(entry.source, ServerSource::Library);
        assert_eq!(entry.checksum, Sha256Hex::of(b"cafebabe"));
        // The lock holds only name/source/checksum — never a secret value.
        let text = std::fs::read_to_string(Lock::path(proj.path())).unwrap();
        assert!(
            !text.contains("Bearer"),
            "no definition body or secret in lock"
        );
    }
}
