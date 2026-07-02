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

use crate::render::{effective_servers, plan_target_with_servers, resolve_targets, Selection};
use crate::scope::Scope;
use crate::state::{target_key, State};
use crate::util::paths;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillAdd {
    pub dir: String,
    pub names: Vec<String>,
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
    fs::canonicalize(dir)
        .unwrap_or_else(|_| dir.to_path_buf())
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

    let selection = Selection::Profile(profile.to_string());
    let state = State::load().unwrap_or_default();
    let target_ids = resolve_targets(manifest, &ctx.registry, &[]);

    // Library-aware effective server set (inline-first, then central library).
    let libctx = ctx.library_ctx();
    let server_map = effective_servers(manifest, &libctx.library, &libctx.lib_home, &selection)?;

    // Snapshot: server config files (for undo) + skills dirs (to detect adds).
    let mut backups: Vec<crate::history::FileChange> = Vec::new();
    let mut touched: BTreeSet<String> = BTreeSet::new();
    let mut skill_before: Vec<(PathBuf, BTreeSet<String>)> = Vec::new();
    for id in &target_ids {
        let Some(desc) = ctx.registry.get(id) else {
            continue;
        };
        let prev = state.managed_servers(&target_key(id, scope, &ctx.dir));
        if let Some(plan) =
            plan_target_with_servers(desc, &ctx.resolver, &server_map, &prev, scope, &ctx.dir)?
        {
            backups.push(crate::history::capture(
                &plan.config_path,
                format!("{} · servers", desc.display),
            ));
            touched.insert(desc.display.clone());
        }
        if let Some(sd) = desc.skills_dir_for(scope, &ctx.dir) {
            skill_before.push((sd.clone(), dir_entries(&sd)));
        }
    }

    // Activate the profile (servers + skills) in this scope.
    let use_args = crate::cli::UseArgs {
        profile: profile.to_string(),
        targets: vec![],
        scope: Some(scope),
        write: true,
        allow_unresolved: false,
        no_gitignore: false,
    };
    crate::commands::use_profile::run(&use_args, manifest_dir)?;

    // Record the server snapshots as one undoable history entry.
    let history_id = crate::history::record(scope.as_str(), touched.into_iter().collect(), backups)
        .unwrap_or(None);

    // Which skills did activation add? (so end removes exactly those)
    let mut skill_adds = Vec::new();
    for (sd, before) in skill_before {
        let added: Vec<String> = dir_entries(&sd).difference(&before).cloned().collect();
        if !added.is_empty() {
            skill_adds.push(SkillAdd {
                dir: sd.to_string_lossy().into_owned(),
                names: added,
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
    let dir = match manifest_dir {
        Some(d) => d.to_path_buf(),
        None => std::env::current_dir()?,
    };
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
    // 2. Remove the skills the session materialized.
    for sa in &sess.skill_adds {
        for name in &sa.names {
            remove_entry(&Path::new(&sa.dir).join(name));
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
