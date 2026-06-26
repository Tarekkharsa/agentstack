//! Builds the JSON the dashboard renders — a full, read-only view of the
//! manifest aggregated from the core library (PLAN §9f). Secret *values* are
//! never included, only `${REF}` names and resolved/unresolved status.

use std::path::Path;

use anyhow::Result;
use serde_json::{json, Value};

use crate::lock::Lock;
use crate::manifest::{ServerType, SkillSource};
use crate::scope::Scope;
use crate::secret::{DotEnvResolver, EnvResolver, KeychainResolver, Resolver, VarlockResolver};
use crate::state::{target_key, State};
use crate::store::{local_source_dir, Store};
use crate::usage::Usage;

/// The state the dashboard loads: a full snapshot, or a "needs init" welcome
/// payload when no manifest exists yet (so a brand-new user can start here).
pub fn state(manifest_dir: Option<&Path>) -> Result<Value> {
    let dir = match manifest_dir {
        Some(d) => d.to_path_buf(),
        None => std::env::current_dir()?,
    };
    if !dir.join(crate::manifest::load::MANIFEST_FILE).exists() {
        return welcome(&dir);
    }
    let mut v = build(manifest_dir)?;
    if let Some(o) = v.as_object_mut() {
        o.insert("needsInit".into(), Value::Bool(false));
    }
    Ok(v)
}

/// Pre-manifest welcome: which CLIs we detected, ready to import.
fn welcome(dir: &Path) -> Result<Value> {
    let reg = crate::adapter::Registry::load()?;
    let adapters: Vec<Value> = reg
        .iter()
        .map(|d| {
            json!({
                "id": d.id,
                "display": d.display,
                "installed": d.is_installed(),
                "configPresent": d.config_present(),
            })
        })
        .collect();
    Ok(json!({
        "needsInit": true,
        "meta": { "dir": dir.display().to_string(), "version": env!("CARGO_PKG_VERSION") },
        "adapters": adapters,
    }))
}

pub fn build(manifest_dir: Option<&Path>) -> Result<Value> {
    let ctx = crate::commands::load(manifest_dir)?;
    let manifest = &ctx.loaded.manifest;
    let state = State::load().unwrap_or_default();
    let usage = Usage::load().unwrap_or_default();
    let store = Store::default_store();
    let lock = Lock::load(&ctx.dir).unwrap_or_default();

    let adapter_ids: Vec<String> = ctx.registry.ids().map(String::from).collect();

    // Adapters (columns of the matrix), with their config/skill locations.
    let adapters: Vec<Value> = ctx
        .registry
        .iter()
        .map(|d| {
            json!({
                "id": d.id,
                "display": d.display,
                "installed": d.is_installed(),
                "configPresent": d.config_present(),
                "supportsProject": d.supports_scope(Scope::Project),
                "configPath": d.config.path,
                "projectConfig": d.project.as_ref().map(|p| p.config.clone()),
                "skillsDir": d.skills.as_ref().map(|s| s.dir.clone()),
            })
        })
        .collect();

    // Servers × adapters matrix + full (commit-safe) config.
    let servers: Vec<Value> = manifest
        .servers
        .iter()
        .map(|(name, s)| {
            let cells: Vec<Value> = adapter_ids
                .iter()
                .map(|id| {
                    json!({
                        "adapter": id,
                        "global": state.managed_servers(&target_key(id, Scope::Global)).contains(name),
                        "project": state.managed_servers(&target_key(id, Scope::Project)).contains(name),
                    })
                })
                .collect();
            json!({
                "name": name,
                "type": match s.server_type { ServerType::Http => "http", ServerType::Stdio => "stdio" },
                "url": s.url,
                "command": s.command,
                "args": s.args,
                "headers": s.headers.iter().map(|(k, v)| json!({"key": k, "value": v})).collect::<Vec<_>>(),
                "env": s.env.iter().map(|(k, v)| json!({"key": k, "value": v})).collect::<Vec<_>>(),
                "cells": cells,
            })
        })
        .collect();

    // Which adapters have a skills directory (the columns of the skills matrix).
    let skill_adapters: Vec<Value> = ctx
        .registry
        .iter()
        .filter(|d| {
            d.skills_dir_for(Scope::Global, &ctx.dir).is_some()
                || d.skills_dir_for(Scope::Project, &ctx.dir).is_some()
        })
        .map(|d| {
            json!({
                "id": d.id,
                "display": d.display,
                "supportsProject": d.skills_dir_for(Scope::Project, &ctx.dir).is_some(),
            })
        })
        .collect();
    let skill_adapter_ids: Vec<String> = skill_adapters
        .iter()
        .map(|a| a["id"].as_str().unwrap_or("").to_string())
        .collect();

    // Skills: source + lock detail + installed + per-CLI active cells.
    let skills: Vec<Value> = manifest
        .skills
        .iter()
        .map(|(name, sk)| {
            let (kind, src) = match sk.source() {
                Ok(SkillSource::Git { url, rev }) => ("git", json!({"git": url, "rev": rev})),
                Ok(SkillSource::Path(p)) => ("path", json!({"path": p})),
                Err(_) => ("invalid", Value::Null),
            };
            let locked = lock.get(name);
            let cells: Vec<Value> = skill_adapter_ids
                .iter()
                .map(|id| {
                    json!({
                        "adapter": id,
                        "global": state.managed_skills(&target_key(id, Scope::Global)).contains(name),
                        "project": state.managed_skills(&target_key(id, Scope::Project)).contains(name),
                    })
                })
                .collect();
            json!({
                "name": name,
                "source": kind,
                "src": src,
                "installed": local_source_dir(&store, sk, &ctx.dir).is_some(),
                "lockedRev": locked.and_then(|l| l.rev.clone()),
                "checksum": locked.map(|l| l.checksum.clone()),
                "cells": cells,
            })
        })
        .collect();

    // Lifecycle hooks.
    let hooks: Vec<Value> = manifest
        .hooks
        .iter()
        .map(|(name, h)| {
            json!({
                "name": name,
                "event": h.event,
                "matcher": h.matcher,
                "command": h.command,
                "args": h.args,
                "timeout": h.timeout,
                "targets": h.targets,
            })
        })
        .collect();
    // Which adapters can run hooks (the targets a hook can be aimed at).
    let hook_adapters: Vec<Value> = ctx
        .registry
        .iter()
        .filter(|d| d.hooks.is_some())
        .map(|d| json!({"id": d.id, "display": d.display}))
        .collect();

    // Instruction fragments.
    let instructions: Vec<Value> = manifest
        .instructions
        .iter()
        .map(|(name, instr)| {
            let path = if Path::new(&instr.path).is_absolute() {
                std::path::PathBuf::from(&instr.path)
            } else {
                ctx.dir.join(&instr.path)
            };
            json!({
                "name": name,
                "path": instr.path,
                "targets": instr.targets,
                "exists": path.exists(),
            })
        })
        .collect();

    // Secrets: resolved status + which source resolved it (never the value).
    let env = EnvResolver;
    let varlock = VarlockResolver::detect(&ctx.dir);
    let keychain = KeychainResolver;
    let dotenv = DotEnvResolver::from_dir(&ctx.dir);
    let secrets: Vec<Value> = manifest
        .referenced_secrets()
        .into_iter()
        .map(|name| {
            let source = if env.resolve(&name).is_some() {
                Some("env")
            } else if varlock.as_ref().and_then(|v| v.resolve(&name)).is_some() {
                Some("varlock")
            } else if keychain.resolve(&name).is_some() {
                Some("keychain")
            } else if dotenv.as_ref().and_then(|d| d.resolve(&name)).is_some() {
                Some(".env")
            } else {
                None
            };
            json!({ "name": name, "resolved": source.is_some(), "source": source })
        })
        .collect();

    let profiles: Vec<Value> = manifest
        .profiles
        .iter()
        .map(|(name, p)| json!({ "name": name, "servers": p.servers, "skills": p.skills }))
        .collect();

    let stats: Vec<Value> = {
        let mut v: Vec<(&String, &u64)> = usage.activations.iter().collect();
        v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        v.into_iter()
            .map(|(name, count)| json!({ "name": name, "activations": count }))
            .collect()
    };

    // Settings: one card per CLI that has a native settings file, prefilled with
    // the manifest's current [settings.<id>] block (or empty).
    let settings_adapters: Vec<Value> = ctx
        .registry
        .iter()
        .filter(|d| d.settings.is_some())
        .map(|d| {
            let current = manifest
                .settings
                .get(&d.id)
                .cloned()
                .unwrap_or_else(|| json!({}));
            let path = d
                .settings_for(Scope::Global, &ctx.dir)
                .map(|(p, _)| p.display().to_string())
                .unwrap_or_default();
            let fields = d
                .settings
                .as_ref()
                .map(|s| s.fields.clone())
                .unwrap_or_default();
            // The catalog keys actually present in the CLI's settings file right
            // now, so the dashboard reflects reality by default (no manual import).
            let live = d
                .read_settings_value(&ctx.dir)
                .ok()
                .flatten()
                .map(|v| Value::Object(crate::adapter::extract_settings(d, &v)))
                .unwrap_or_else(|| json!({}));
            json!({
                "id": d.id,
                "display": d.display,
                "path": path,
                "current": current,
                "live": live,
                "fields": fields,
            })
        })
        .collect();

    // Skills already present on disk in each CLI's skills dir but not yet in the
    // manifest — so the user can see and adopt what they already have.
    let mut disc: std::collections::BTreeMap<String, (String, bool, Vec<String>)> =
        std::collections::BTreeMap::new();
    for d in ctx.registry.iter() {
        for sk in d.discover_skills(Scope::Global, &ctx.dir) {
            let e = disc
                .entry(sk.name.clone())
                .or_insert_with(|| (sk.source.display().to_string(), sk.is_symlink, Vec::new()));
            if !e.2.contains(&d.id) {
                e.2.push(d.id.clone());
            }
        }
    }
    let discovered_skills: Vec<Value> = disc
        .into_iter()
        .map(|(name, (source, is_symlink, present_in))| {
            json!({
                "name": name,
                "source": source,
                "isSymlink": is_symlink,
                "presentIn": present_in,
                "inManifest": manifest.skills.contains_key(&name),
            })
        })
        .collect();

    // Native harness plugins already installed on this machine (read-only view).
    let (plugin_list, marketplace_list) = crate::plugins::all_plugins();
    let plugins: Vec<Value> = plugin_list
        .iter()
        .map(|p| {
            json!({
                "harness": p.harness,
                "name": p.name,
                "marketplace": p.marketplace,
                "scope": p.scope,
                "projects": p.projects,
                "version": p.version,
                "enabled": p.enabled,
                "status": p.status,
                "source": p.source,
            })
        })
        .collect();
    let marketplaces: Vec<Value> = marketplace_list
        .iter()
        .map(|m| json!({ "harness": m.harness, "name": m.name, "source": m.source }))
        .collect();
    let plugin_recipes: Vec<Value> =
        crate::plugin_recipes::statuses(manifest, &ctx.registry, &ctx.dir)
            .iter()
            .map(|r| {
                json!({
                        "name": r.name,
                        "display": r.display,
                        "version": r.version,
                        "description": r.description,
                        "category": r.category,
                        "targets": r.targets,
                        "servers": r.servers,
                        "skills": r.skills,
                        "hooks": r.hooks,
                    "packagePath": r.package_path.display().to_string(),
                    "generated": r.generated,
                    "stale": r.stale,
                    "conflict": r.conflict,
                    "missingSkills": r.missing_skills,
                    "marketplaces": r.marketplaces.iter().map(|m| json!({
                        "target": m.target,
                        "path": m.path.display().to_string(),
                        "present": m.present,
                        "stale": m.stale,
                        "nativeVisible": m.native_visible,
                        "nativeSource": m.native_source,
                    })).collect::<Vec<_>>(),
                    "installs": r.installs.iter().map(|i| json!({
                        "target": i.target,
                        "installed": i.installed,
                        "enabled": i.enabled,
                        "status": i.status,
                    })).collect::<Vec<_>>(),
                    "guidance": r.guidance.iter().map(|g| json!({
                        "target": g.target,
                        "nextAction": g.next_action,
                    })).collect::<Vec<_>>(),
                    "requiredSecrets": r.required_secrets,
                })
            })
            .collect();

    let global_drift =
        render_drift_targets(&ctx, manifest, &state, Scope::Global, DriftSet::Selected)?;
    let global_non_selected_drift = render_drift_targets(
        &ctx,
        manifest,
        &state,
        Scope::Global,
        DriftSet::InstalledNonSelected,
    )?;
    let health = health_checks(&ctx, manifest, &global_drift, &global_non_selected_drift);
    let next_actions = next_actions(&secrets, &skills, &plugin_recipes, &global_drift, &health);

    Ok(json!({
        "meta": {
            "name": manifest.meta.name,
            "dir": ctx.dir.display().to_string(),
            "version": env!("CARGO_PKG_VERSION"),
            "defaultTargets": manifest.targets.default,
        },
        "adapters": adapters,
        "servers": servers,
        "skills": skills,
        "skillAdapters": skill_adapters,
        "discoveredSkills": discovered_skills,
        "settingsAdapters": settings_adapters,
        "hooks": hooks,
        "hookAdapters": hook_adapters,
        "pluginRecipes": plugin_recipes,
        "plugins": plugins,
        "marketplaces": marketplaces,
        "instructions": instructions,
        "secrets": secrets,
        "profiles": profiles,
        "stats": stats,
        "health": health,
        "nextActions": next_actions,
    }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DriftSet {
    Selected,
    InstalledNonSelected,
}

#[derive(Debug, Clone)]
struct DriftTarget {
    id: String,
    display: String,
    config_path: String,
    selected_by_manifest: bool,
    installed: bool,
    config_present: bool,
    changed: bool,
    diff: String,
    reason_skipped: Option<String>,
}

impl DriftTarget {
    fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "display": self.display,
            "path": self.config_path,
            "selectedByManifest": self.selected_by_manifest,
            "installed": self.installed,
            "configPresent": self.config_present,
            "changed": self.changed,
            "diff": self.diff,
            "reasonSkipped": self.reason_skipped,
        })
    }
}

/// Shared drift planner for Health, Preview, and Next actions. `Selected` is
/// intentionally the same target set `/api/diff` previews and `/api/apply`
/// writes by default.
fn render_drift_targets(
    ctx: &crate::commands::Context,
    manifest: &crate::manifest::Manifest,
    state: &State,
    scope: Scope,
    set: DriftSet,
) -> Result<Vec<DriftTarget>> {
    let selected = crate::render::resolve_targets(manifest, &ctx.registry, &[]);
    let ids: Vec<String> = match set {
        DriftSet::Selected => selected.clone(),
        DriftSet::InstalledNonSelected => ctx
            .registry
            .iter()
            .filter(|d| !selected.iter().any(|id| id == &d.id))
            .filter(|d| d.is_installed() || d.config_present())
            .map(|d| d.id.clone())
            .collect(),
    };

    let mut out = Vec::new();
    for id in ids {
        let selected_by_manifest = selected.iter().any(|t| t == &id);
        let Some(d) = ctx.registry.get(&id) else {
            out.push(DriftTarget {
                id: id.clone(),
                display: id,
                config_path: String::new(),
                selected_by_manifest,
                installed: false,
                config_present: false,
                changed: false,
                diff: String::new(),
                reason_skipped: Some("target is not registered".into()),
            });
            continue;
        };
        let installed = d.is_installed();
        let config_present = d.config_present();
        let prev = state.managed_servers(&target_key(&id, scope));
        match crate::render::plan_target(
            manifest,
            d,
            &ctx.resolver,
            &crate::render::Selection::All,
            &prev,
            scope,
            &ctx.dir,
        )? {
            Some(plan) => out.push(DriftTarget {
                id,
                display: d.display.clone(),
                config_path: plan.config_path.display().to_string(),
                selected_by_manifest,
                installed,
                config_present,
                changed: plan.changed(),
                diff: crate::util::diff::render_plain(&plan.existing, &plan.proposed),
                reason_skipped: None,
            }),
            None => out.push(DriftTarget {
                id,
                display: d.display.clone(),
                config_path: String::new(),
                selected_by_manifest,
                installed,
                config_present,
                changed: false,
                diff: String::new(),
                reason_skipped: Some(format!("{} does not support {scope} scope", d.display)),
            }),
        }
    }
    Ok(out)
}

/// A compact doctor-style health summary for the dashboard.
fn health_checks(
    ctx: &crate::commands::Context,
    manifest: &crate::manifest::Manifest,
    selected_global_drift: &[DriftTarget],
    non_selected_global_drift: &[DriftTarget],
) -> Vec<Value> {
    let mut out = Vec::new();
    let push = |out: &mut Vec<Value>, level: &str, msg: String, action: Option<Value>| {
        out.push(json!({ "level": level, "message": msg, "action": action }));
    };

    for d in ctx.registry.iter() {
        if d.is_installed() {
            match d.read_config_value() {
                Ok(_) => push(
                    &mut out,
                    "ok",
                    format!("{}: installed, config parses", d.display),
                    None,
                ),
                Err(_) => push(
                    &mut out,
                    "error",
                    format!("{}: config does not parse", d.display),
                    Some(json!({ "type": "section", "section": "settings" })),
                ),
            }
        } else if d.config_present() {
            push(
                &mut out,
                "warn",
                format!("{}: config present, binary not on PATH", d.display),
                None,
            );
        } else {
            push(
                &mut out,
                "warn",
                format!("{}: not detected", d.display),
                None,
            );
        }
    }

    let refs = manifest.referenced_secrets();
    let unresolved: Vec<&String> = refs
        .iter()
        .filter(|n| ctx.resolver.resolve(n).is_none())
        .collect();
    if unresolved.is_empty() {
        push(
            &mut out,
            "ok",
            format!("{} secret(s) resolve", refs.len()),
            None,
        );
    } else {
        push(
            &mut out,
            "error",
            format!(
                "{} secret(s) unresolved: {}",
                unresolved.len(),
                unresolved
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Some(json!({ "type": "section", "section": "secrets" })),
        );
    }

    push_drift_health(&mut out, selected_global_drift, non_selected_global_drift);

    out
}

fn push_drift_health(
    out: &mut Vec<Value>,
    selected_global_drift: &[DriftTarget],
    non_selected_global_drift: &[DriftTarget],
) {
    let push = |out: &mut Vec<Value>, level: &str, msg: String, action: Option<Value>| {
        out.push(json!({ "level": level, "message": msg, "action": action }));
    };

    // Drift (global scope), using the exact selected target set Preview shows.
    let drift = selected_global_drift.iter().filter(|t| t.changed).count();
    if drift == 0 {
        push(out, "ok", "selected global configs in sync".into(), None);
    } else {
        push(
            out,
            "warn",
            format!("{drift} selected target(s) drifted (global) — Preview to reconcile"),
            Some(json!({ "type": "preview", "scope": "global" })),
        );
    }

    let extra_drift = non_selected_global_drift
        .iter()
        .filter(|t| t.changed)
        .count();
    if extra_drift > 0 {
        push(
            out,
            "warn",
            format!(
                "{extra_drift} installed non-default target(s) have renderable drift — preview all to reconcile"
            ),
            Some(json!({ "type": "preview", "scope": "global", "all": true })),
        );
    }
}

fn next_actions(
    secrets: &[Value],
    skills: &[Value],
    recipes: &[Value],
    selected_global_drift: &[DriftTarget],
    health: &[Value],
) -> Vec<Value> {
    let mut out = Vec::new();

    for secret in secrets
        .iter()
        .filter(|s| !s.get("resolved").and_then(Value::as_bool).unwrap_or(false))
    {
        let Some(name) = secret.get("name").and_then(Value::as_str) else {
            continue;
        };
        out.push(command_action(
            format!("missing-secret:{name}"),
            "error",
            format!("{name} is missing"),
            "A manifest reference cannot resolve on this machine.",
            "secrets",
            "Set secret",
            json!({ "type": "section", "section": "secrets" }),
        ));
    }

    let drift = selected_global_drift.iter().filter(|t| t.changed).count();
    if drift > 0 {
        out.push(command_action(
            "drift:global".into(),
            "warn",
            "Selected targets have drift".into(),
            format!("{drift} global target(s) differ from the manifest render."),
            "health",
            "Preview changes",
            json!({ "type": "preview", "scope": "global" }),
        ));
    }

    let missing_skills: Vec<&str> = skills
        .iter()
        .filter(|s| !s.get("installed").and_then(Value::as_bool).unwrap_or(false))
        .filter_map(|s| s.get("name").and_then(Value::as_str))
        .collect();
    if !missing_skills.is_empty() {
        out.push(command_action(
            "skills:missing".into(),
            "warn",
            "Skill sources are not installed".into(),
            format!("{} skill source(s) need install.", missing_skills.len()),
            "skills",
            "Install skills",
            json!({ "type": "post", "path": "/api/install", "label": "Install" }),
        ));
    }

    for recipe in recipes {
        let Some(name) = recipe.get("name").and_then(Value::as_str) else {
            continue;
        };
        let conflict = recipe.get("conflict").and_then(Value::as_str);
        let missing = recipe
            .get("missingSkills")
            .and_then(Value::as_array)
            .map(|a| a.len())
            .unwrap_or(0);
        let generated = recipe
            .get("generated")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let stale = recipe
            .get("stale")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let marketplace_needs_sync = recipe
            .get("marketplaces")
            .and_then(Value::as_array)
            .map(|items| {
                items.iter().any(|m| {
                    !m.get("present").and_then(Value::as_bool).unwrap_or(false)
                        || m.get("stale").and_then(Value::as_bool).unwrap_or(false)
                })
            })
            .unwrap_or(false);
        let marketplace_hidden = recipe
            .get("marketplaces")
            .and_then(Value::as_array)
            .map(|items| {
                items.iter().any(|m| {
                    !m.get("nativeVisible")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false);
        let install_missing = recipe
            .get("installs")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .any(|i| !i.get("installed").and_then(Value::as_bool).unwrap_or(false))
            })
            .unwrap_or(false);

        if let Some(conflict) = conflict {
            out.push(command_action(
                format!("plugin:{name}:conflict"),
                "error",
                format!("{name} plugin package is blocked"),
                format!("Resolve package conflict: {conflict}"),
                "plugins",
                "Open Plugins",
                json!({ "type": "section", "section": "plugins" }),
            ));
        } else if missing > 0 {
            out.push(command_action(
                format!("plugin:{name}:missing-skills"),
                "warn",
                format!("{name} recipe needs skill sources"),
                format!("{missing} skill source(s) must be installed before sync."),
                "plugins",
                "Install skills",
                json!({ "type": "post", "path": "/api/install", "label": "Install" }),
            ));
        } else if !generated || stale || marketplace_needs_sync {
            out.push(command_action(
                format!("plugin:{name}:sync"),
                "warn",
                format!("{name} recipe needs sync"),
                "Generated package or marketplace entry is missing or stale.",
                "plugins",
                "Sync recipes",
                json!({ "type": "post", "path": "/api/plugins_sync", "label": "Plugin recipe sync" }),
            ));
        } else if marketplace_hidden {
            out.push(command_action(
                format!("plugin:{name}:marketplace"),
                "warn",
                format!("{name} marketplace is not visible natively"),
                "The repo marketplace exists but the native harness has not picked it up.",
                "plugins",
                "Open Plugins",
                json!({ "type": "section", "section": "plugins" }),
            ));
        } else if install_missing {
            out.push(command_action(
                format!("plugin:{name}:install"),
                "warn",
                format!("{name} is not installed natively"),
                "Install the recipe from the native harness marketplace.",
                "plugins",
                "Open Plugins",
                json!({ "type": "section", "section": "plugins" }),
            ));
        }
    }

    for row in health {
        let msg = row
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if row.get("level").and_then(Value::as_str) == Some("error")
            && msg.contains("config does not parse")
        {
            out.push(command_action(
                format!("adapter-config:{msg}"),
                "error",
                msg.to_string(),
                "Inspect or import the native settings before applying changes.",
                "health",
                "Open Health",
                json!({ "type": "section", "section": "health" }),
            ));
        }
    }

    out.sort_by_key(|a| match a.get("level").and_then(Value::as_str) {
        Some("error") => 0,
        Some("warn") => 1,
        _ => 2,
    });
    out
}

fn command_action(
    id: String,
    level: &str,
    title: String,
    detail: impl Into<String>,
    section: &str,
    label: &str,
    action: Value,
) -> Value {
    json!({
        "id": id,
        "level": level,
        "title": title,
        "detail": detail.into(),
        "section": section,
        "primary": {
            "label": label,
            "action": action,
        },
        "secondary": {
            "label": "Open section",
            "action": { "type": "section", "section": section },
        }
    })
}

/// Provider search results for the Discover pane: normalized candidates with
/// trust signals and whether each is already in the manifest.
pub fn search(manifest_dir: Option<&Path>, query: &str) -> Result<Value> {
    let installed: Vec<String> = crate::commands::load(manifest_dir)
        .ok()
        .map(|ctx| ctx.loaded.manifest.servers.keys().cloned().collect())
        .unwrap_or_default();

    let results: Vec<Value> = crate::provider::search_all(query, 25)
        .into_iter()
        .map(|c| {
            let t = c.trust();
            json!({
                "id": c.id,
                "name": c.name,
                "description": c.description,
                "source": c.source,
                "addId": if c.source == "catalog" { c.name.clone() } else { c.id.clone() },
                "installed": installed.contains(&c.name),
                "trust": { "namespaced": t.namespaced, "runsCode": t.runs_code, "needsSecret": t.needs_secret },
            })
        })
        .collect();
    Ok(json!({ "query": query, "results": results }))
}

/// Per-target rendering diffs for a scope (for the "preview before apply" flow).
pub fn diffs(manifest_dir: Option<&Path>, scope: Scope, all: bool) -> Result<Value> {
    let ctx = crate::commands::load(manifest_dir)?;
    let manifest = &ctx.loaded.manifest;
    let state = State::load().unwrap_or_default();
    let mut targets = render_drift_targets(&ctx, manifest, &state, scope, DriftSet::Selected)?;
    if all {
        // Also preview installed CLIs the manifest doesn't target by default.
        targets.extend(render_drift_targets(
            &ctx,
            manifest,
            &state,
            scope,
            DriftSet::InstalledNonSelected,
        )?);
    }
    let out: Vec<Value> = targets.into_iter().map(|t| t.to_json()).collect();
    Ok(json!({ "scope": scope.as_str(), "all": all, "targets": out }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drift(id: &str, selected: bool, changed: bool) -> DriftTarget {
        DriftTarget {
            id: id.into(),
            display: id.into(),
            config_path: format!("/tmp/{id}.json"),
            selected_by_manifest: selected,
            installed: true,
            config_present: true,
            changed,
            diff: String::new(),
            reason_skipped: None,
        }
    }

    #[test]
    fn health_drift_counts_selected_targets_separately_from_installed_extras() {
        let selected = vec![drift("codex", true, false)];
        let extras = vec![
            drift("claude-code", false, true),
            drift("cursor", false, true),
        ];
        let mut rows = Vec::new();
        push_drift_health(&mut rows, &selected, &extras);

        let messages: Vec<&str> = rows
            .iter()
            .filter_map(|row| row.get("message").and_then(Value::as_str))
            .collect();
        assert!(messages.contains(&"selected global configs in sync"));
        assert!(messages
            .iter()
            .any(|msg| msg.contains("2 installed non-default target(s)")));
        assert!(!messages
            .iter()
            .any(|msg| msg.contains("2 selected target(s) drifted")));
    }

    #[test]
    fn next_actions_include_missing_secret_and_selected_drift() {
        let secrets = vec![json!({ "name": "KIBANA_TOKEN", "resolved": false })];
        let skills = Vec::new();
        let recipes = Vec::new();
        let selected = vec![drift("codex", true, true)];
        let health = Vec::new();

        let actions = next_actions(&secrets, &skills, &recipes, &selected, &health);
        let ids: Vec<&str> = actions
            .iter()
            .filter_map(|a| a.get("id").and_then(Value::as_str))
            .collect();

        assert!(ids.contains(&"missing-secret:KIBANA_TOKEN"));
        assert!(ids.contains(&"drift:global"));
    }
}
