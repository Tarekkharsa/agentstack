//! Ephemeral sessions: load a profile *for now*, then revert everything when
//! you're done. agent CLIs read their config at launch, so "for this session"
//! means: write the config before you start the agent and restore it after.
//! Start snapshots the affected server files (via the same history engine
//! `apply` uses) and the skills directories, activates the profile, and
//! remembers what it added. End restores the server files and removes the
//! skills it added — leaving things exactly as they were. Sessions default to
//! project scope so they stay contained to a repo.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::render::{plan_target_with_servers, resolve_targets};
use crate::scope::Scope;
use crate::state::{target_key, State};
use crate::util::paths;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillAdd {
    pub dir: String,
    pub names: Vec<String>,
    /// Whether the skills dir already existed before the session started.
    /// `end` removes the emptied dir only when the session created it —
    /// exact restore, never deleting a dir the user made. Defaults to true
    /// so records from older versions conservatively never rmdir.
    #[serde(default = "default_true")]
    pub dir_preexisted: bool,
}

fn default_true() -> bool {
    true
}

/// One on-demand capability load the agent performed during a session — the
/// replay trail (what was pulled, and the reason it gave).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadEntry {
    pub name: String,
    pub reason: String,
    pub ts: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub dir: String,
    pub profile: String,
    pub scope: String,
    pub started_unix: u64,
    /// History entry id holding the pre-session server-file snapshots.
    pub history_id: Option<String>,
    pub skill_adds: Vec<SkillAdd>,
    /// On-demand loads the agent made within this session (progressive
    /// disclosure). Sticky within the session, gone at exit.
    #[serde(default)]
    pub loads: Vec<LoadEntry>,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// A session older than this is treated as abandoned (Stage 2.2). AgentStack
/// does not supervise the agent process — `start` writes the config, the agent
/// CLI reads it at launch, and the session persists purely as the undo record
/// until `end`. So there is no process to poll for liveness at this layer; age
/// is the only honest signal. Twelve hours spans a full working day plus a
/// buffer, so a session past it has almost certainly outlived whatever launched
/// it (a closed terminal, a killed panel, a reboot). Offering `session end` is
/// always safe — it only restores the pre-session bytes — so a rare false
/// positive costs a nudge, nothing more.
pub const ABANDONED_AFTER_SECS: u64 = 12 * 3600;

/// Has a session running since `started_unix` outlived any plausible working
/// session as of `now`? Pure over its inputs so the boundary is unit-tested.
pub fn is_abandoned(started_unix: u64, now: u64) -> bool {
    now.saturating_sub(started_unix) >= ABANDONED_AFTER_SECS
}

impl Session {
    /// Whether this session reads as abandoned as of `now` (see
    /// [`is_abandoned`]).
    pub fn is_abandoned(&self, now: u64) -> bool {
        is_abandoned(self.started_unix, now)
    }
}

/// Record an on-demand load against the active session for `dir`. Idempotent
/// (sticky): a second load of the same name is a no-op. Returns whether it was
/// newly recorded. Errors if no session is active here.
pub fn record_load(dir: &Path, name: &str, reason: &str) -> Result<bool> {
    let key = dir_key(dir);
    let mut map = load_all();
    let sess = map
        .get_mut(&key)
        .context("no active session in this directory")?;
    if sess.loads.iter().any(|l| l.name == name) {
        return Ok(false);
    }
    sess.loads.push(LoadEntry {
        name: name.to_string(),
        reason: reason.to_string(),
        ts: now_secs(),
    });
    save_all(&map)?;
    Ok(true)
}

fn pointer_path() -> PathBuf {
    paths::agentstack_home().join("sessions.json")
}

fn dir_key(dir: &Path) -> String {
    // Normalize to the manifest dir: `start` holds ctx.dir (`.agentstack/`),
    // while `end`/`active` callers may hold the project root — both must
    // agree on the key or a started session can never be found again.
    let dir = crate::manifest::resolve_manifest_dir(dir);
    fs::canonicalize(&dir)
        .unwrap_or(dir)
        .to_string_lossy()
        .into_owned()
}

fn load_all() -> BTreeMap<String, Session> {
    fs::read_to_string(pointer_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_all(map: &BTreeMap<String, Session>) -> Result<()> {
    let path = pointer_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut text = serde_json::to_string_pretty(map)?;
    text.push('\n');
    fs::write(&path, text).with_context(|| format!("writing {}", path.display()))
}

/// The active session for `dir`, if any.
pub fn active(dir: &Path) -> Option<Session> {
    load_all().get(&dir_key(dir)).cloned()
}

/// Every active session on this machine.
pub fn list_all() -> Vec<Session> {
    load_all().into_values().collect()
}

/// Freeze the active session's *resolved* set into a new profile for replay:
/// the original profile's servers + exactly the skills the agent loaded (or the
/// profile's skills if it loaded none). Returns the new profile name.
pub fn freeze(manifest_dir: Option<&Path>, name: Option<&str>) -> Result<String> {
    let ctx = crate::commands::load(manifest_dir)?;
    let sess = active(&ctx.dir).context("no active session in this directory to freeze")?;
    let profile = ctx
        .loaded
        .manifest
        .profiles
        .get(&sess.profile)
        .with_context(|| format!("profile '{}' is gone from the manifest", sess.profile))?;

    let servers = profile.servers.clone();
    // The resolved set is what was actually pulled; fall back to the full
    // profile if the agent loaded nothing on demand.
    let skills: Vec<String> = if sess.loads.is_empty() {
        profile.skills.clone()
    } else {
        sess.loads.iter().map(|l| l.name.clone()).collect()
    };
    let new_name = name
        .map(String::from)
        .unwrap_or_else(|| format!("{}-frozen", sess.profile));

    crate::commands::add::add_profile_json(
        manifest_dir,
        &serde_json::json!({ "name": new_name, "servers": servers, "skills": skills }),
    )
}

/// End every active session, reverting each. Returns how many were ended.
pub fn end_all() -> Result<usize> {
    let mut n = 0;
    for s in list_all() {
        if end(Some(Path::new(&s.dir))).is_ok() {
            n += 1;
        }
    }
    Ok(n)
}

fn dir_entries(dir: &Path) -> BTreeSet<String> {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect()
}

/// What `start` activated, so the caller can state the facts instead of a
/// bare "started" (Stage 2.2): which profile, which native files the session
/// now manages, and which skills it materialized where.
#[derive(Debug)]
pub struct StartReport {
    pub profile: String,
    pub scope: Scope,
    /// (CLI display name, native config file) for each target whose server
    /// config this session manages — the exact files `end` restores.
    pub server_files: Vec<(String, PathBuf)>,
    /// (skills dir, skill names) the activation added — removed again at end.
    pub skill_adds: Vec<(String, Vec<String>)>,
}

/// What `end` reverted, so the caller can report an exact restore instead of
/// a bare "ended" (Stage 2.2).
#[derive(Debug)]
pub struct EndReport {
    pub profile: String,
    /// Files put back to their pre-session bytes, as (path, label) from the
    /// history entry the session recorded at start.
    pub restored: Vec<(String, String)>,
    /// (skills dir, skill names) removed again.
    pub removed_skills: Vec<(String, Vec<String>)>,
}

/// Start a session: snapshot, activate `profile` in `scope`, and remember what
/// to revert.
///
/// Fail-closed (UI control-plane §5): unlike `use --write` — a human's
/// explicit activation, where recording the first pin IS the consent act —
/// `session start` is the verb external UIs drive headlessly, so it refuses
/// an untrusted project and any unpinned/drifted surface outright, routing to
/// the review instead of silently loading content nobody consented to.
pub fn start(manifest_dir: Option<&Path>, profile: &str, scope: Scope) -> Result<StartReport> {
    let ctx = crate::commands::load(manifest_dir)?;
    let key_dir = dir_key(&ctx.dir);
    if load_all().contains_key(&key_dir) {
        anyhow::bail!("a session is already active here — end it first");
    }
    let manifest = &ctx.loaded.manifest;
    manifest
        .profiles
        .get(profile)
        .with_context(|| format!("no profile '{profile}' in the manifest"))?;

    // Untrusted means inert: a session materializes skill content into agent
    // context and server configs the harness will spawn, so the project must
    // be trusted at its CURRENT bytes before any of that happens.
    let base = crate::commands::project_base(manifest_dir)?;
    match crate::trust::check(&base) {
        crate::trust::TrustState::Trusted => {}
        crate::trust::TrustState::Changed => anyhow::bail!(
            "refusing to start a session: the manifest or lockfile changed since this project was trusted — review with `agentstack trust` (or the UI trust review), then retry"
        ),
        crate::trust::TrustState::Untrusted => anyhow::bail!(
            "refusing to start a session: this project is not trusted — review and trust it with `agentstack trust` (or the UI trust review), then retry"
        ),
    }

    let state = State::load().unwrap_or_default();
    let target_ids = resolve_targets(manifest, &ctx.registry, &[])?;

    // Resolve the profile once (library-aware, inline-first) — the same
    // prepared set drives the snapshot planning below AND the activation, so
    // start doesn't load and resolve everything twice.
    let use_args = crate::cli::UseArgs {
        profile: Some(profile.to_string()),
        targets: vec![],
        scope: Some(scope),
        write: true,
        allow_unresolved: false,
        prune_foreign: false,
        no_gitignore: false,
        list: false,
        json: false,
    };
    let libctx = ctx.library_ctx();
    let prepared = crate::commands::use_profile::prepare(&ctx, &libctx, &use_args)?;

    // Strict pin gate: everything the profile resolved must already be pinned
    // in agentstack.lock AND match — before a single byte is materialized.
    // (`activate` below re-checks drift; this adds the no-unpinned rule.)
    {
        let lock = crate::lock::Lock::load(&ctx.dir)?;
        let skill_statuses: Vec<_> = prepared
            .resolved_skills
            .iter()
            .map(|r| {
                let status =
                    crate::resolve::classify_skill(&r.name, &r.checksum, r.rev.as_deref(), &lock);
                (r.name.clone(), status)
            })
            .collect();
        let server_statuses: Vec<_> = prepared
            .resolved_servers
            .iter()
            .map(|r| {
                let status = crate::resolve::classify_server(&r.name, &r.checksum, &lock);
                (r.name.clone(), status)
            })
            .collect();
        crate::verify::ensure_session_startable(
            &format!("profile '{profile}'"),
            &skill_statuses,
            &server_statuses,
        )?;
    }

    // Snapshot: server config files (for undo) + skills dirs (to detect adds,
    // and whether each dir pre-existed so `end` can restore exactly).
    let ruleset = crate::render::ruleset_for(manifest)?;
    let mut backups: Vec<crate::history::FileChange> = Vec::new();
    let mut touched: BTreeSet<String> = BTreeSet::new();
    let mut server_files: Vec<(String, PathBuf)> = Vec::new();
    let mut skill_before: Vec<(PathBuf, bool, BTreeSet<String>)> = Vec::new();
    for id in &target_ids {
        let Some(desc) = ctx.registry.get(id) else {
            continue;
        };
        let prev = state.managed_servers(&target_key(id, scope, &ctx.dir));
        if let Some(plan) = plan_target_with_servers(
            desc,
            &ctx.resolver,
            &ruleset,
            &prepared.server_map,
            &prev,
            scope,
            &ctx.dir,
        )? {
            backups.push(crate::history::capture(
                &plan.config_path,
                format!("{} · servers", desc.display),
            ));
            touched.insert(desc.display.clone());
            server_files.push((desc.display.clone(), plan.config_path.clone()));
        }
        if let Some(sd) = desc.skills_dir_for(scope, &ctx.dir) {
            skill_before.push((sd.clone(), sd.exists(), dir_entries(&sd)));
        }
    }

    // Activate the profile (servers + skills) in this scope.
    crate::commands::use_profile::activate(&ctx, &libctx, &use_args, &prepared)?;

    // Record the server snapshots as one undoable history entry.
    let history_id = crate::history::record(scope.as_str(), touched.into_iter().collect(), backups)
        .unwrap_or(None);

    // Which skills did activation add? (so end removes exactly those)
    let mut skill_adds = Vec::new();
    for (sd, existed, before) in skill_before {
        let added: Vec<String> = dir_entries(&sd).difference(&before).cloned().collect();
        if !added.is_empty() {
            skill_adds.push(SkillAdd {
                dir: sd.to_string_lossy().into_owned(),
                names: added,
                dir_preexisted: existed,
            });
        }
    }

    let mut map = load_all();
    map.insert(
        key_dir.clone(),
        Session {
            dir: key_dir,
            profile: profile.to_string(),
            scope: scope.as_str().to_string(),
            started_unix: now_secs(),
            history_id,
            skill_adds: skill_adds.clone(),
            loads: Vec::new(),
        },
    );
    save_all(&map)?;
    Ok(StartReport {
        profile: profile.to_string(),
        scope,
        server_files,
        skill_adds: skill_adds
            .into_iter()
            .map(|sa| (sa.dir, sa.names))
            .collect(),
    })
}

/// End the active session for `dir`: restore server files and remove the skills
/// it added. Returns exactly what was reverted (Stage 2.2).
pub fn end(manifest_dir: Option<&Path>) -> Result<EndReport> {
    // Walk up like `start` does (via commands::load), or a session started at
    // the project root could never be ended from a subdirectory.
    let dir = crate::commands::project_base(manifest_dir)?;
    let key = dir_key(&dir);
    let mut map = load_all();
    let sess = map
        .get(&key)
        .cloned()
        .context("no active session in this directory")?;

    // 1. Restore server config files to their pre-session content. The
    //    history entry names the files, so the report can state exactly what
    //    went back; a failed/already-done undo reports an empty restore
    //    honestly instead of claiming one.
    let mut restored: Vec<(String, String)> = Vec::new();
    if let Some(hid) = &sess.history_id {
        let files: Vec<(String, String)> = crate::history::list()
            .into_iter()
            .find(|e| &e.id == hid)
            .map(|e| e.files.into_iter().map(|f| (f.path, f.label)).collect())
            .unwrap_or_default();
        if crate::history::undo(hid).is_ok() {
            restored = files;
        }
    }
    // 2. Remove the skills the session materialized. A dir the session itself
    //    created is cleared too when emptied (rmdir semantics: refuses
    //    non-empty dirs) — but a dir that pre-existed the session is left in
    //    place even if empty: `end` promises an exact restore.
    for sa in &sess.skill_adds {
        for name in &sa.names {
            remove_entry(&Path::new(&sa.dir).join(name));
        }
        if !sa.dir_preexisted {
            let _ = fs::remove_dir(Path::new(&sa.dir));
        }
    }
    map.remove(&key);
    save_all(&map)?;
    Ok(EndReport {
        profile: sess.profile,
        restored,
        removed_skills: sess
            .skill_adds
            .into_iter()
            .map(|sa| (sa.dir, sa.names))
            .collect(),
    })
}

/// Remove a skill entry whether it's a symlink, file, or directory.
fn remove_entry(p: &Path) {
    let meta = fs::symlink_metadata(p);
    match meta {
        Ok(m) if m.file_type().is_dir() => {
            let _ = fs::remove_dir_all(p);
        }
        Ok(_) => {
            let _ = fs::remove_file(p);
        }
        Err(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::prelude::*;

    // SECURITY WITNESS (UI control-plane §5, invariant 2): `session start` is
    // the verb external UIs drive headlessly, so it must fail closed — an
    // untrusted project refuses, and a trusted project whose profile
    // references UNPINNED content refuses too (no first-pin-on-activation
    // here; that explicit-consent act stays with `use --write`). NEVER delete
    // or weaken this test.
    // Stage 2.2: the abandoned boundary is age-only (no process supervision at
    // this layer) and lives in one place so every surface agrees.
    #[test]
    fn is_abandoned_uses_the_twelve_hour_boundary() {
        let start = 1_000_000u64;
        // Fresh and mid-session are live.
        assert!(!is_abandoned(start, start));
        assert!(!is_abandoned(start, start + 3600));
        assert!(!is_abandoned(start, start + ABANDONED_AFTER_SECS - 1));
        // At and past the boundary reads as abandoned.
        assert!(is_abandoned(start, start + ABANDONED_AFTER_SECS));
        assert!(is_abandoned(start, start + 48 * 3600));
        // A clock that went backwards never falsely flags (saturating).
        assert!(!is_abandoned(start, start - 100));
    }

    #[test]
    fn session_start_refuses_untrusted_and_unpinned_surface() {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", home.path());

        let proj = assert_fs::TempDir::new().unwrap();
        proj.child(".agentstack/agentstack.toml")
            .write_str(
                "version = 1\n[skills.greet]\npath = \"./skills/greet\"\n[profiles.dev]\nskills = [\"greet\"]\n",
            )
            .unwrap();
        proj.child(".agentstack/skills/greet/SKILL.md")
            .write_str("# greet\n")
            .unwrap();

        // (a) Untrusted project: refuse before anything resolves or writes.
        let err = start(Some(proj.path()), "dev", Scope::Project).unwrap_err();
        assert!(
            format!("{err:#}").contains("not trusted"),
            "routes to the trust review: {err:#}"
        );
        assert!(active(proj.path()).is_none());

        // (b) Trusted but UNPINNED: the skill has no lock entry, so the
        // strict gate refuses — session start never records a first pin.
        crate::trust::trust_unreviewed(proj.path()).unwrap();
        let err = start(Some(proj.path()), "dev", Scope::Project).unwrap_err();
        assert!(
            format!("{err:#}").contains("unpinned"),
            "names the unpinned surface: {err:#}"
        );
        assert!(active(proj.path()).is_none());

        std::env::remove_var("AGENTSTACK_HOME");
    }
}
