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
                    format!("no toolset '{p}' in this project — `agentstack toolset list` shows the ones declared here")
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
            let counts = format!(
                "{} · {}",
                super::count(servers, "server"),
                super::count(skills, "skill")
            );
            (name, counts)
        })
        .collect();
    let name_w = rows.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
    let counts_w = rows
        .iter()
        .map(|(_, c)| c.chars().count())
        .max()
        .unwrap_or(0);

    let mut out = String::from("several toolsets declared — name one:");
    for (name, counts) in &rows {
        out.push_str(&format!(
            "\n  {name:<name_w$}   {counts:<counts_w$}   agentstack use {name}"
        ));
    }
    out
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
                        crate::verify::Verdict::Unpinned => blockers.push((
                            r.name.clone(),
                            "unpinned — run `agentstack lock --write`".into(),
                        )),
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
                        crate::verify::Verdict::Unpinned => blockers.push((
                            r.name.clone(),
                            "unpinned — run `agentstack lock --write`".into(),
                        )),
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
        // The manifest FILE, not just the project base. A UI that wants to
        // open the source of truth has to know whether this project uses the
        // `.agentstack/` layout or a legacy root manifest, and only the code
        // that resolves that layout can answer without guessing. `ctx.dir` is
        // already the resolved manifest dir.
        "manifest_path": ctx.dir.join(agentstack_core::manifest::MANIFEST_FILE)
            .display()
            .to_string(),
        "trust": trust_state,
        "profiles": profiles,
        // Null when nothing is active; a UI renders the end/recovery action
        // from this object, never from its own remembered state. `abandoned`
        // (Stage 2.2) carries the CLI's own age-based judgment so the panel
        // highlights an interrupted session for recovery without duplicating
        // the threshold in TypeScript.
        "session": active_session.map(|s| {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            serde_json::json!({
                "profile": crate::text::sanitize_line(&s.profile),
                "scope": s.scope,
                "started_unix": s.started_unix,
                "abandoned": s.is_abandoned(now),
            })
        }),
    }))
}

/// Human rendering of the listing body (the non-`--json` branch).
fn print_profile_listing(out: &serde_json::Value) {
    let trust_state = out["trust"].as_str().unwrap_or("?");
    let profiles = out["profiles"].as_array().map_or(&[][..], Vec::as_slice);
    if profiles.is_empty() {
        // F17: the user-facing word is "toolset" (t3code says it too); the
        // manifest table stays `[profiles.*]` and is named as such where the
        // user would have to type it. An empty list is also the one moment
        // where naming a first toolset is obviously useful, so it offers the
        // command rather than only reporting the absence.
        println!(
            "No toolsets yet — the implicit default (every inline skill and server) is what activates."
        );
        println!("  Name one:  agentstack toolset create <name> --server <server>");
        return;
    }
    println!("Declared toolsets (project trust: {trust_state}):");
    for p in profiles {
        let name = p["name"].as_str().unwrap_or("?");
        let ready = if p["pinned"].as_bool().unwrap_or(false) {
            "pinned".to_string()
        } else {
            super::count(p["blockers"].as_array().map_or(0, Vec::len), "blocker")
        };
        let in_use = if p["active"].as_bool().unwrap_or(false) {
            "  · in use (agentstack x session end reverts it)"
        } else {
            ""
        };
        println!(
            "  {name}  —  {}, {}  [{ready}]{in_use}",
            super::count(p["skills"].as_array().map_or(0, Vec::len), "skill"),
            super::count(p["servers"].as_array().map_or(0, Vec::len), "server"),
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
    // Same effective setting every other write path reads: the caller's
    // per-run flag OR the project's durable opt-out.
    let gitignore_off = no_gitignore || !ctx.loaded.manifest.meta.manages_gitignore();
    if scope == Scope::Project && !gitignore_off && !out.written.is_empty() {
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
    // Standing re-gate answers reshape delivery before anything is verified or
    // materialized (`docs/design/consent-card.md`, §b). Two effects, both of
    // which must happen HERE so every downstream consumer — the verify gate,
    // the printed counts, the plan — sees the same set:
    //
    //   blocked      → the item is dropped entirely. It fails closed, exactly
    //                  as drift does, but as a standing state rather than a
    //                  question re-asked on every command.
    //   keep-pinned  → the item is delivered FROM THE CONTENT STORE, not from
    //                  the project. This is what makes keep-pinned mean "the
    //                  approved bytes are what agents load"; delivering the
    //                  live path would ship the very change the user declined.
    let decisions = crate::trust::decisions_for(&crate::manifest::project_root_of(&ctx.dir));
    let store_root = crate::store::Store::default_store().root().to_path_buf();
    let mut blocked_names: Vec<String> = Vec::new();
    let mut pinned_copies: Vec<String> = Vec::new();
    let mut unverified_pins: Vec<String> = Vec::new();
    let mut active_skills: Vec<(String, PathBuf)> = resolved_skills
        .iter()
        .filter_map(|r| {
            match decisions
                .iter()
                .find(|d| d.kind == "skill" && d.name == r.name)
                .map(|d| &d.answer)
            {
                Some(crate::trust::Decision::Blocked) => {
                    blocked_names.push(r.name.clone());
                    None
                }
                Some(crate::trust::Decision::KeepPinned { pin }) => {
                    let hex = pin.rsplit(':').next().unwrap_or(pin);
                    let snapshot = store_root.join("content").join(hex);
                    // The snapshot must still hash to the approved digest and
                    // hold no symlinks before it is delivered (F4). A bare
                    // `is_dir()` here was the one read on the one path whose
                    // entire purpose is "the approved bytes are what agents
                    // load": the store dir is writable, so a planted symlink
                    // or edited file under this name would be copied into
                    // every harness as though the user had approved it.
                    if crate::store::verified_snapshot(&snapshot, hex) {
                        pinned_copies.push(r.name.clone());
                        Some((r.name.clone(), snapshot))
                    } else {
                        // The approved bytes are gone or no longer verify.
                        // Fail closed rather than silently falling back to
                        // the live path, which is the content the human
                        // declined.
                        unverified_pins.push(r.name.clone());
                        blocked_names.push(r.name.clone());
                        None
                    }
                }
                None => Some((r.name.clone(), r.path.clone())),
            }
        })
        .collect();
    if !args.quiet && !blocked_names.is_empty() {
        println!(
            "  {} {} excluded by a standing decision: {} — revisit with `agentstack trust`",
            "⊘".dimmed(),
            super::count(blocked_names.len(), "skill"),
            blocked_names.join(", ")
        );
    }
    if !args.quiet && !unverified_pins.is_empty() {
        // Named separately from the plain exclusions: the user asked to keep
        // approved bytes that this machine can no longer produce, and "excluded
        // by a standing decision" alone would hide that the store copy is the
        // thing that failed.
        println!(
            "  {} the approved copy of {} is missing or failed verification — excluded until \
             you review the live content with `agentstack trust`",
            "⊘".dimmed(),
            unverified_pins.join(", ")
        );
    }

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
            // A keep-pinned skill is drifted BY DEFINITION — that is what the
            // human was asked about — and it is not being delivered from the
            // drifted path anyway, so the fail-closed gate must not stop the
            // activation over it. A blocked skill is not being delivered at
            // all. Both were already answered at the consent gate; re-blocking
            // here would make an answered question unanswerable.
            .filter(|r| !blocked_names.contains(&r.name) && !pinned_copies.contains(&r.name))
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

        // CONTENT BINDING ON THE RENDERED LANE (invariant 4). The gates above
        // proved the LIVE bytes still hash to the lock pin. What gets rendered
        // must nevertheless be the immutable, content-addressed snapshot named
        // by that pin — never the mutable directory the bytes were read from.
        //
        // The hole this closes: `render::skills` symlinks the delivered
        // artifact at its source dir, and for a central-library skill that dir
        // is exactly what `lib sync` rewrites. The link's TARGET STRING never
        // changes, so nothing re-gates, yet the bytes a harness reads *through*
        // the link become the library's new ones. Unreviewed content reaching
        // agent context with no re-gate is precisely what invariant 4 forbids,
        // and it is the same rule the serving lane already follows in
        // `mcp_server::load_skill`: resolve from the lock, read from the store
        // by digest.
        //
        // Deliberately AFTER `ensure_activatable`, not folded into the
        // `active_skills` construction above: the drift gate owns the "these
        // bytes moved" refusal and must keep owning it. Reaching this line
        // means the live bytes match the pin, which is also what makes
        // `pinned_content`'s repair-from-live leg safe — it can only ever
        // deposit the pinned, reviewed content, and `snapshot_content`
        // re-proves the address as it lands.
        for (name, path) in active_skills.iter_mut() {
            // Keep-pinned names already point at their approved snapshot (and
            // were excluded from the gate above), so there is nothing to
            // redirect and no live copy to repair from.
            if pinned_copies.contains(name) {
                continue;
            }
            // Unpinned: no digest to serve by, so behaviour is unchanged —
            // the live path, plus the pin-me warning the resolver already
            // emitted. Fabricating a pin here would be inventing consent.
            let Some(entry) = lock.get(name) else {
                continue;
            };
            *path = libctx
                .store
                .pinned_content(entry.checksum.hex(), path)
                .map_err(|e| {
                    // Fail closed. Falling back to the live directory would
                    // restore the exact hole above, so a store that cannot
                    // produce verified bytes refuses the whole activation.
                    anyhow::anyhow!(
                        "refusing to render skill '{name}': its approved bytes could not be \
                         served from the content store ({e}) — run `agentstack lock --write` to \
                         re-pin and review it"
                    )
                })?;
        }
    }

    let target_ids = resolve_targets(manifest, &ctx.registry, &args.targets, &ctx.dir)?;
    let ruleset = crate::render::ruleset_for(manifest)?;
    // `use` ACTIVATES a toolset; it is not a second writer with its own idea
    // of where capabilities go. Under the default routing an MCP-capable
    // harness's servers travel the live lane, so activation must not also
    // write a server config — that would put the same servers in two places,
    // one of them direct and unbrokered, while every reading surface reports
    // the live lane. Same `delivery::Plan` reading `apply` uses; everything
    // else this command does (skills, extensions, the .gitignore block) is
    // untouched, and servers are still written when `[delivery]
    // render_locally` is set or the harness has no live channel.
    let plan_delivery =
        crate::delivery::Plan::build(&manifest.delivery, &ctx.registry, &target_ids);
    // Harnesses whose servers this activation deliberately did not write, and
    // the subset whose bridge is not registered — named at the end in the
    // same voice `apply` uses.
    let mut live_withheld: std::collections::BTreeSet<String> = Default::default();
    let mut live_unconnected: std::collections::BTreeSet<String> = Default::default();
    if !args.quiet {
        println!(
            "Activating toolset '{}' (scope: {scope}) — {}, {}",
            label.bold(),
            super::count(server_map.len(), "server"),
            super::count(active_skills.len(), "skill")
        );
        // Activation delivers what the manifest declares;
        // dropped-but-undeclared content is not part of this toolset until it
        // is adopted.
        crate::intake::print_notice(
            &ctx.dir,
            &crate::manifest::project_root_of(&ctx.dir),
            manifest,
        );
        // Moment 5: name the undo before the first byte changes. `use --write`
        // has no confirmation step by design — `--write` is an imperative
        // contract, not a proposal — so the requirement is satisfied by saying
        // the way back at the top of the write phase, not by inventing a gate.
        if args.write {
            println!(
                "  {} undo: `agentstack x restore --last --write`",
                "↩".dimmed()
            );
        }
    }

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
    // Targets this activation covers — written or already carrying the right
    // servers. Needed separately from `wrote` for the same reason as in `apply`:
    // activating a toolset whose servers are already on disk changes nothing, and
    // "on 0 target(s)" printed above the list of files it now manages reads as
    // failure.
    let mut covered_targets: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
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
    // Derived once, from the manifest this activation already loaded, so the
    // preview arm and the write arm below read the same answer.
    let gitignore_off = args.no_gitignore || !manifest.meta.manages_gitignore();
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

        // Dry-run counterparts of the state records the managed .gitignore
        // block reads. A dry run records nothing, so those reads report an
        // empty block and the preview would omit the .gitignore edit this
        // activation is about to make — see the block at the end of this loop.
        let mut would_manage_config = false;
        let mut would_manage_skills = false;

        // --- servers ---
        // Route servers through the same delivery plan `apply` reads.
        let servers_live = plan_delivery.servers_route_live(id);
        if servers_live {
            // No config path is named on purpose: naming the file is what made
            // this read as "these servers are about to be written here".
            // The verb comes from this harness's own bridge state, never from
            // the routing alone: with no gateway registered nothing is served,
            // and `status`, `doctor`, `delivery` and `apply` all say "planned
            // live (not connected)" about this harness at this moment.
            let bridged = super::overview::bridge_registered(&ctx.registry, id);
            let lane = if bridged {
                "are served live"
            } else {
                "are planned live (not connected)"
            };
            println!(
                "  {} MCP servers {lane}, not written — nothing for `use` to render here",
                "·".dimmed()
            );
            live_withheld.insert(desc.display.clone());
            if !bridged {
                live_unconnected.insert(desc.display.clone());
            }
            // This target IS covered by the activation — its servers travel
            // the live lane. Counting it keeps the closing summary from
            // reading "0 target(s)" for a project that is fully activated.
            if args.write {
                covered_targets.insert(desc.display.clone());
            }
        } else {
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
                    // What a write WOULD leave managed here. `blocked` gates it
                    // exactly as it gates the write below: an activation that
                    // writes nothing must not preview hiding a hand-maintained
                    // config.
                    would_manage_config =
                        !blocked && (!plan.managed.is_empty() || !foreign.is_empty());
                    if plan.changed() {
                        if args.write && blocked {
                            blocked_targets.push(desc.display.clone());
                            let reason = if plan.unresolved.is_empty() {
                                "secret read failures; set them"
                            } else {
                                "unresolved secrets; set them"
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
                            state.record_active_profile(&key, prepared.profile.clone());
                            // Track what this guarded write kept on disk (empty
                            // after a --prune-foreign actually pruned them).
                            state.record_kept_foreign(&key, foreign.clone());
                            crate::usage::bump(&plan.managed);
                            wrote += 1;
                            covered_targets.insert(desc.display.clone());
                            if plan.remove_if_empty_shell(desc) {
                                println!(
                                    "  {} removed empty {}",
                                    "−".yellow(),
                                    plan.config_path.display()
                                );
                            } else {
                                println!(
                                    "  {} servers → {}",
                                    "✓".green(),
                                    plan.config_path.display()
                                );
                            }
                        } else {
                            println!(
                                "  {} {} to write",
                                "→".cyan(),
                                super::count(plan.managed.len(), "server")
                            );
                        }
                    } else {
                        // Even with no file change, keep state in sync with
                        // reality (mirrors `apply`) — prune tracking and the
                        // .gitignore block depend on it.
                        if args.write && !blocked {
                            state.record(&key, plan.managed.clone(), &plan.proposed, &identity);
                            state.record_active_profile(&key, prepared.profile.clone());
                            state.record_kept_foreign(&key, foreign.clone());
                            covered_targets.insert(desc.display.clone());
                        }
                        println!("  {} servers up to date", "✓".green());
                    }
                }
            }
        }

        // --- skills --- (config-only adapters have no skills dir; they still
        // reach the managed .gitignore block below for their config entry).
        if let Some(skills_dir) = desc.skills_dir_for(scope, &ctx.dir) {
            let strategy = desc.skills.as_ref().map(|s| s.strategy).unwrap_or_default();
            let prev_skills = state.managed_skills(&key);
            // Keep-pinned names are COPIED even where this adapter symlinks:
            // their source is the content-store snapshot, and a link would
            // re-attach the delivered artifact to the live file.
            let plan = skills::plan_with_pinned(
                skills_dir.clone(),
                strategy,
                active_skills.clone(),
                &prev_skills,
                pinned_copies.clone(),
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
                        "  {} {} → {}",
                        "✓".green(),
                        super::count(plan.managed_names().len(), "skill"),
                        skills_dir.display()
                    );
                } else {
                    would_manage_skills = !plan.managed_names().is_empty();
                    println!(
                        "  {} {} to {} into {}",
                        "→".cyan(),
                        super::count(plan.active.len(), "skill"),
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
                "  {} ({reason} — {} not materialized)",
                "·".dimmed(),
                super::count(active_skills.len(), "skill")
            );
        }

        // Managed .gitignore block: emit an entry only for an artifact this
        // target manages now (after the write sections above). `use` never
        // compiles instructions, so its instruction flag is the on-disk managed
        // marker `apply` leaves — the record that keeps the two commands'
        // blocks byte-identical.
        //
        // The dry-run arm derives the same flags prospectively (what this
        // activation would leave managed) OR'd with what is already recorded,
        // so the preview names the edit. The write arm is untouched: its
        // byte-for-byte agreement with `apply` is a pinned witness.
        if scope == Scope::Project {
            let instr_path = desc
                .instructions
                .as_ref()
                .and_then(|s| s.path_for(scope, &ctx.dir));
            // `use` never compiles instructions in either mode, so this stays
            // the on-disk marker `apply` leaves.
            let instr_managed = instr_path
                .as_deref()
                .is_some_and(crate::render::instructions::manages_file);
            let managed = crate::render::gitignore::Managed {
                config: (!args.write && would_manage_config)
                    || !state.managed_servers(&key).is_empty()
                    || !state.kept_foreign(&key).is_empty(),
                skills: (!args.write && would_manage_skills)
                    || !state.managed_skills(&key).is_empty(),
                instructions: instr_managed,
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
        && !gitignore_off
        && crate::render::gitignore::ensure_block(&project_root, &ignore_entries, true)?
    {
        println!(
            "\n{} .gitignore: managed block updated — generated artifacts stay out of git ({} to commit them instead)",
            "✓".green(),
            "--no-gitignore".bold()
        );
    } else if !args.write
        && scope == Scope::Project
        && !gitignore_off
        && crate::render::gitignore::ensure_block(&project_root, &ignore_entries, false)?
    {
        // `write: false` answers "would this change?" without touching the
        // file, so preview and write agree by construction rather than by a
        // second implementation of the block.
        println!(
            "\n{} .gitignore: would add a managed block so generated artifacts stay out of git",
            "→".cyan()
        );
        // Sorted + deduped to match `splice`, so the previewed lines are the
        // lines that land.
        let mut shown: Vec<&str> = ignore_entries.iter().map(String::as_str).collect();
        shown.sort_unstable();
        shown.dedup();
        for entry in shown {
            println!("  {} {entry}", "+".green());
        }
        println!(
            "  {} {} to leave .gitignore alone and commit them instead",
            "·".dimmed(),
            "--no-gitignore".bold()
        );
    } else if scope == Scope::Project
        && gitignore_off
        && crate::render::gitignore::has_block(&project_root)
    {
        // Opted out with a block already on disk. Activation never strips it,
        // so report the leftover rather than let the user believe these files
        // show up in `git status`.
        println!(
            "\n{} .gitignore: this project opted out, but a managed block is still present — \
             delete the marked lines to un-hide the generated files",
            "·".dimmed()
        );
    }

    // What this activation deliberately did NOT write, said plainly, with both
    // ways forward — the same voice and the same shared recovery constant
    // `apply`, `status`, `doctor` and `delivery` use. A user who ran
    // `use --write` expecting `.mcp.json` learns here why it is not there.
    if !live_withheld.is_empty() && !args.quiet {
        let names: Vec<&str> = live_withheld.iter().map(String::as_str).collect();
        println!(
            "\n{} MCP servers for {} are routed to the live lane — `use` does not write them.",
            "ℹ".cyan(),
            names.join(", ")
        );
        if live_unconnected.is_empty() {
            println!(
                "  {} they are served on demand through the registered bridge.",
                "·".dimmed()
            );
        } else {
            println!(
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
            println!("  {} {}", "→".cyan(), super::delivery::CONNECT_THE_BRIDGE);
            println!(
                "  {} or write files anyway: agentstack x delivery render-locally --write",
                "→".cyan()
            );
        }
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
            let _ = crate::history::record(
                scope.as_str(),
                format!("use '{label}'"),
                history_targets.clone(),
                backups,
            );
        }
        if !total_failure {
            // Record each activated skill + server's resolved digest so a fresh
            // checkout resolves the same content (and `doctor`/`explain` can
            // flag drift). Server locks store the definition digest only — never
            // a resolved secret value.
            //
            // (Skills under a standing re-gate decision are skipped inside
            // `record_lock` itself — see the comment there.)
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
                    "\n{} activated '{}' — wrote skills to {}; no server configs changed.",
                    "✓".green(),
                    label,
                    super::count(wrote_skill_dirs, "location")
                );
            } else {
                // Coverage first, changes second — see `covered_targets`. A
                // re-activation of an already-current toolset changes nothing,
                // and the bare changed-count read as "nothing happened".
                let covered = covered_targets.len().max(wrote);
                if wrote == 0 {
                    println!(
                        "\n{} '{}' already active on {} — nothing to change.",
                        "✓".green(),
                        label,
                        super::count(covered, "target")
                    );
                } else {
                    println!(
                        "\n{} activated '{}' on {} — wrote {wrote}.",
                        "✓".green(),
                        label,
                        super::count(covered, "target")
                    );
                }
            }
            // Only claim undoability for what restore actually covers: the
            // server configs captured above (skills revert via `session end`).
            // Name the exact command — bare `agentstack restore` lists the
            // ledger instead of undoing, so it read as a broken instruction.
            if wrote > 0 {
                println!("  {}", "undo: agentstack x restore --last --write".dimmed());
            }
        } else {
            // A blocked target is a failure, not a footnote: report it in the
            // summary and exit nonzero so scripts can't mistake this for done.
            println!(
                "\n{} activated '{}' on {} (wrote {wrote}); {} BLOCKED: {}",
                "⚠".yellow(),
                label,
                super::count(covered_targets.len().max(wrote), "target"),
                super::count(blocked_targets.len(), "target"),
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
                "unresolved secrets blocked {} — {fix}",
                super::count(blocked_targets.len(), "target")
            );
        }
    } else if !args.quiet {
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
    // `None` is NOT a toolset named "default" — it is the whole declared
    // `[skills]` table. Saying "toolset 'default'" in an error was a false
    // statement in a consent-pipeline error path (a project can have no such
    // toolset, and the user may have typed `--profile docs`), so the label
    // names what was actually walked.
    let plabel = profile_name.unwrap_or("declared");

    let mut out = Vec::new();
    for name in names {
        let pinned_rev = pins
            .and_then(|lock| lock.get(&name))
            .and_then(|entry| entry.rev.as_deref());
        let resolved = crate::resolve::resolve_skill_with_pin(
            manifest, dir, library, lib_home, store, &name, mode, pinned_rev,
        )
        .with_context(|| format!("resolving skill '{name}' for toolset '{plabel}'"))?;
        if !resolved.path.exists() {
            anyhow::bail!(
                "skill '{name}' (toolset '{plabel}') resolved to {} but it is not present on disk — run `agentstack install`",
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
    // A skill under a standing re-gate answer keeps the pin the answer named
    // — it is never re-pinned here. A keep-pinned item's live checksum is
    // precisely the change the human declined, and upserting it would move
    // the lock to the declined bytes with no consent moment: the next review
    // would read "matches" and the decline would be quietly gone. Enforced at
    // this choke point (not per call site) so `use`, `lock`, and every future
    // caller inherit it; the only path that may move a decided pin is the
    // trust commit point, where accepting IS the consent — and accepting
    // clears the decision first.
    let decided = decided_names(dir, "skill");
    for r in skills {
        if decided.contains(&r.name) {
            continue;
        }
        lock.upsert(locked_from_resolved(r, manifest, library));
    }
    for r in servers {
        lock.upsert_server(LockedServer {
            name: r.name.clone(),
            source: match r.origin {
                ServerOrigin::Inline => ServerSource::Inline,
                ServerOrigin::Library => ServerSource::Library,
                // A package member is pinned as a `[[package.member]]` row by
                // `record_package_pins`, and must never ALSO become a
                // `[[server]]` row — two lock rows for one name are two pins
                // that can disagree. Nothing routes a package member here
                // today; if something ever does, it must fail loudly rather
                // than write a second pin under a wrong source label.
                ServerOrigin::Package => anyhow::bail!(
                    "server '{}' came from a package and cannot be pinned as a project \
                     server row — package members are pinned by `agentstack lock` as \
                     package members. This is a bug; please report it.",
                    crate::text::sanitize_line(&r.name)
                ),
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

/// Names of one kind's items with a standing re-gate answer for this
/// project. The lock-recording paths use it to leave answered pins alone.
pub(crate) fn decided_names(dir: &Path, kind: &str) -> std::collections::HashSet<String> {
    let base = crate::manifest::project_root_of(dir);
    crate::trust::decisions_for(&base)
        .into_iter()
        .filter(|d| d.kind == kind)
        .map(|d| d.name)
        .collect()
}

/// Build a lockfile entry from a resolved skill, recovering the source locator
/// (`path`/`git`) from wherever the name resolved.
/// The pin comes from [`crate::store::Store::pin`], which deposits a path
/// source's bytes into the content store as part of producing it — see that
/// function for why capturing and pinning are deliberately one act.
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
        checksum: crate::store::Store::default_store()
            .pin(&crate::store::Resolved {
                path: resolved.path.clone(),
                rev: resolved.rev.clone(),
                checksum: resolved.checksum.clone(),
                fetched: false,
                source_kind: resolved.source_kind,
            })
            .expect("a resolved skill checksum is a digest this process computed"),
        // Not known from resolved state — `Lock::upsert` carries forward
        // whatever intake recorded, so a re-lock cannot launder it away.
        license: None,
        origin: None,
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
            listing.starts_with("several toolsets declared"),
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
            license: None,
            origin: None,
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
