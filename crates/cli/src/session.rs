//! Ephemeral sessions: load a profile (and an optional plugin) *for now*, then
//! revert everything when you're done. agent CLIs read their config at launch,
//! so "for this session" means: write the config before you start the agent and
//! restore it after. Start snapshots the affected server files (via the same
//! history engine `apply` uses) and the skills directories, activates the
//! profile, and remembers what it added. End restores the server files, removes
//! the skills it added, and uninstalls the plugin — leaving things exactly as
//! they were. Sessions default to project scope so they stay contained to a repo.

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
    pub plugin: Option<String>,
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

    crate::dashboard::actions::add_profile(
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

/// Start a session: snapshot, activate `profile` (+ optional `plugin`) in
/// `scope`, and remember what to revert.
pub fn start(
    manifest_dir: Option<&Path>,
    profile: &str,
    scope: Scope,
    plugin: Option<&str>,
) -> Result<()> {
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

    let state = State::load().unwrap_or_default();
    let target_ids = resolve_targets(manifest, &ctx.registry, &[]);

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
    };
    let libctx = ctx.library_ctx();
    let prepared = crate::commands::use_profile::prepare(&ctx, &libctx, &use_args)?;

    // Snapshot: server config files (for undo) + skills dirs (to detect adds,
    // and whether each dir pre-existed so `end` can restore exactly).
    let ruleset = crate::render::ruleset_for(manifest)?;
    let mut backups: Vec<crate::history::FileChange> = Vec::new();
    let mut touched: BTreeSet<String> = BTreeSet::new();
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

    // Optional plugin for the session.
    if let Some(pl) = plugin {
        crate::commands::plugins::install_recipe_native(manifest_dir, pl, &[], true)
            .with_context(|| format!("installing plugin '{pl}' for the session"))?;
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
            skill_adds,
            plugin: plugin.map(String::from),
            loads: Vec::new(),
        },
    );
    save_all(&map)
}

/// End the active session for `dir`: restore server files, remove the skills it
/// added, uninstall its plugin.
pub fn end(manifest_dir: Option<&Path>) -> Result<()> {
    // Walk up like `start` does (via commands::load), or a session started at
    // the project root could never be ended from a subdirectory.
    let dir = crate::commands::project_base(manifest_dir)?;
    let key = dir_key(&dir);
    let mut map = load_all();
    let sess = map
        .get(&key)
        .cloned()
        .context("no active session in this directory")?;

    // 1. Restore server config files to their pre-session content.
    if let Some(hid) = &sess.history_id {
        let _ = crate::history::undo(hid);
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
    // 3. Uninstall the session plugin.
    if let Some(pl) = &sess.plugin {
        let _ = crate::commands::plugins::remove_recipe_native(manifest_dir, pl, &[], true);
    }

    map.remove(&key);
    save_all(&map)
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
