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

    let health = health_checks(&ctx, manifest, &state);

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
        "instructions": instructions,
        "secrets": secrets,
        "profiles": profiles,
        "stats": stats,
        "health": health,
    }))
}

/// A compact doctor-style health summary for the dashboard.
fn health_checks(
    ctx: &crate::commands::Context,
    manifest: &crate::manifest::Manifest,
    state: &State,
) -> Vec<Value> {
    let mut out = Vec::new();
    let push = |out: &mut Vec<Value>, level: &str, msg: String| {
        out.push(json!({ "level": level, "message": msg }));
    };

    for d in ctx.registry.iter() {
        if d.is_installed() {
            match d.read_config_value() {
                Ok(_) => push(
                    &mut out,
                    "ok",
                    format!("{}: installed, config parses", d.display),
                ),
                Err(_) => push(
                    &mut out,
                    "error",
                    format!("{}: config does not parse", d.display),
                ),
            }
        } else if d.config_present() {
            push(
                &mut out,
                "warn",
                format!("{}: config present, binary not on PATH", d.display),
            );
        } else {
            push(&mut out, "warn", format!("{}: not detected", d.display));
        }
    }

    let refs = manifest.referenced_secrets();
    let unresolved: Vec<&String> = refs
        .iter()
        .filter(|n| ctx.resolver.resolve(n).is_none())
        .collect();
    if unresolved.is_empty() {
        push(&mut out, "ok", format!("{} secret(s) resolve", refs.len()));
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
        );
    }

    // Drift (global scope).
    let mut drift = 0;
    for id in ctx.registry.ids() {
        if let Some(d) = ctx.registry.get(id) {
            let prev = state.managed_servers(&target_key(id, Scope::Global));
            if let Ok(Some(plan)) = crate::render::plan_target(
                manifest,
                d,
                &ctx.resolver,
                &crate::render::Selection::All,
                &prev,
                Scope::Global,
                &ctx.dir,
            ) {
                if plan.changed() {
                    drift += 1;
                }
            }
        }
    }
    if drift == 0 {
        push(&mut out, "ok", "global configs in sync".into());
    } else {
        push(
            &mut out,
            "warn",
            format!("{drift} target(s) drifted (global) — Apply to reconcile"),
        );
    }

    out
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
pub fn diffs(manifest_dir: Option<&Path>, scope: Scope) -> Result<Value> {
    let ctx = crate::commands::load(manifest_dir)?;
    let manifest = &ctx.loaded.manifest;
    let state = State::load().unwrap_or_default();
    let targets = crate::render::resolve_targets(manifest, &ctx.registry, &[]);

    let mut out = Vec::new();
    for id in &targets {
        let Some(d) = ctx.registry.get(id) else {
            continue;
        };
        let prev = state.managed_servers(&target_key(id, scope));
        if let Some(plan) = crate::render::plan_target(
            manifest,
            d,
            &ctx.resolver,
            &crate::render::Selection::All,
            &prev,
            scope,
            &ctx.dir,
        )? {
            out.push(json!({
                "display": d.display,
                "path": plan.config_path.display().to_string(),
                "changed": plan.changed(),
                "diff": crate::util::diff::render_plain(&plan.existing, &plan.proposed),
            }));
        }
    }
    Ok(json!({ "scope": scope.as_str(), "targets": out }))
}
