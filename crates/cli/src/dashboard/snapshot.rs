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

/// Plural suffix: `""` for one, `"s"` otherwise. Keeps count strings
/// grammatical (`1 target` / `2 targets`) without per-call branching.
fn s(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// The state the dashboard loads: a full snapshot, or a "needs init" welcome
/// payload when no manifest exists yet (so a brand-new user can start here).
pub fn state(manifest_dir: Option<&Path>) -> Result<Value> {
    let base = crate::commands::project_base(manifest_dir)?;
    let dir = crate::manifest::resolve_manifest_dir(&base);
    if !dir.join(crate::manifest::load::MANIFEST_FILE).exists() {
        return welcome(&dir);
    }
    let mut v = build(manifest_dir)?;
    if let Some(o) = v.as_object_mut() {
        o.insert("needsInit".into(), Value::Bool(false));
    }
    Ok(v)
}

/// Pre-manifest welcome: which CLIs we detected and, for each, the MCP servers
/// it already has — so the UI can show where the CLIs disagree before unifying.
fn welcome(dir: &Path) -> Result<Value> {
    let reg = crate::adapter::Registry::load()?;
    let mut union: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let adapters: Vec<Value> = reg
        .iter()
        .map(|d| {
            let detected = d.detected();
            // Read each detected CLI's existing MCP servers (names only).
            let servers: Vec<String> = if detected {
                d.read_config_value()
                    .ok()
                    .flatten()
                    .map(|v| {
                        crate::adapter::extract_servers(d, &v)
                            .into_iter()
                            .map(|(name, _)| name)
                            .collect()
                    })
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            for s in &servers {
                union.insert(s.clone());
            }
            json!({
                "id": d.id,
                "display": d.display,
                "installed": d.is_installed(),
                "configPresent": d.config_present(),
                "detected": detected,
                "mcp": d.mcp.is_some(),
                "servers": servers,
            })
        })
        .collect();
    Ok(json!({
        "needsInit": true,
        "meta": { "dir": dir.display().to_string(), "version": env!("CARGO_PKG_VERSION") },
        "adapters": adapters,
        "serverUnion": union.into_iter().collect::<Vec<_>>(),
    }))
}

/// The wire-proxy telemetry as JSON for the dashboard's Proxy panel — the same
/// ranked, per-capability view as `agentstack proxy report`. An empty log
/// yields `requests: 0` with an empty `capabilities` list (an explicit
/// empty-state the UI renders, never an error).
fn proxy_report() -> Value {
    let report = crate::proxy::aggregate(&crate::proxy::read_all());
    json!({
        "requests": report.requests,
        "totalTools": report.total_tools,
        "totalEstTokens": report.total_est_tokens,
        "totalLabel": crate::footprint::fmt_tokens(report.total_est_tokens),
        "capabilities": report.capabilities.iter().map(|c| json!({
            "capability": c.capability,
            "tools": c.tools,
            "avgEstTokens": c.avg_est_tokens,
            "avgLabel": crate::footprint::fmt_tokens(c.avg_est_tokens),
            "calls": c.calls,
            "hint": c.hint,
        })).collect::<Vec<_>>(),
    })
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
                "mcp": d.mcp.is_some(),
                "configPath": d.config.as_ref().map(|c| c.path.clone()),
                "projectConfig": d.project.as_ref().map(|p| p.config.clone()),
                "skillsDir": d.skills.as_ref().map(|s| s.dir.clone()),
            })
        })
        .collect();

    // Servers × adapters matrix + full (commit-safe) config.
    let footprints = crate::footprint::Footprints::load().unwrap_or_default();
    let servers: Vec<Value> = manifest
        .servers
        .iter()
        .map(|(name, s)| {
            let cells: Vec<Value> = adapter_ids
                .iter()
                .map(|id| {
                    json!({
                        "adapter": id,
                        "global": state.managed_servers(&target_key(id, Scope::Global, &ctx.dir)).contains(name),
                        "project": state.managed_servers(&target_key(id, Scope::Project, &ctx.dir)).contains(name),
                    })
                })
                .collect();
            json!({
                "name": name,
                "type": match s.server_type { ServerType::Http => "http", ServerType::Stdio => "stdio" },
                "url": s.url,
                "command": s.command,
                "args": s.args,
                "cwd": s.cwd,
                "headers": s.headers.iter().map(|(k, v)| json!({"key": k, "value": v})).collect::<Vec<_>>(),
                "env": s.env.iter().map(|(k, v)| json!({"key": k, "value": v})).collect::<Vec<_>>(),
                "cells": cells,
                // Context-cost lens (measured via `stats --live`, cached).
                "footprint": footprints.get(name).map(|f| json!({
                    "tools": f.tools,
                    "estTokens": f.est_tokens,
                    "label": crate::footprint::fmt_tokens(f.est_tokens),
                })),
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
                Ok(SkillSource::Git { url, rev, subpath }) => {
                    ("git", json!({"git": url, "rev": rev, "subpath": subpath}))
                }
                Ok(SkillSource::Path(p)) => ("path", json!({"path": p})),
                Err(_) => ("invalid", Value::Null),
            };
            let locked = lock.get(name);
            let cells: Vec<Value> = skill_adapter_ids
                .iter()
                .map(|id| {
                    json!({
                        "adapter": id,
                        "global": state.managed_skills(&target_key(id, Scope::Global, &ctx.dir)).contains(name),
                        "project": state.managed_skills(&target_key(id, Scope::Project, &ctx.dir)).contains(name),
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

    // Skills present on disk in each CLI's skills dir. Every entry is surfaced —
    // including broken links / non-skill dirs — with a status, so nothing is
    // silently hidden; only `valid` ones can be adopted or added to the library.
    struct Disc {
        source: String,
        is_symlink: bool,
        valid: bool,
        broken: bool,
        present_in: Vec<String>,
    }
    let mut disc: std::collections::BTreeMap<String, Disc> = std::collections::BTreeMap::new();
    for d in ctx.registry.iter() {
        for sk in d.discover_skills(Scope::Global, &ctx.dir) {
            let e = disc.entry(sk.name.clone()).or_insert_with(|| Disc {
                source: sk.source.display().to_string(),
                is_symlink: sk.is_symlink,
                valid: sk.valid,
                broken: sk.broken,
                present_in: Vec::new(),
            });
            if !e.present_in.contains(&d.id) {
                e.present_in.push(d.id.clone());
            }
        }
    }
    let discovered_skills: Vec<Value> = disc
        .into_iter()
        .map(|(name, e)| {
            let status = if e.broken {
                "broken link"
            } else if !e.valid {
                "no SKILL.md"
            } else {
                "ok"
            };
            json!({
                "name": name,
                "source": e.source,
                "isSymlink": e.is_symlink,
                "valid": e.valid,
                "broken": e.broken,
                "status": status,
                "presentIn": e.present_in,
                "inManifest": manifest.skills.contains_key(&name),
            })
        })
        .collect();

    // Native extensions/add-ons in each CLI's extensions dir — both the global
    // dir (~/.pi/agent/extensions) and the project dir (.pi/extensions).
    let mut extensions: Vec<Value> = Vec::new();
    for d in ctx.registry.iter() {
        for scope in [Scope::Global, Scope::Project] {
            let label = match scope {
                Scope::Global => "global",
                Scope::Project => "project",
            };
            // The ownership-ledger artifacts in this dir, so a discovered entry
            // agentstack rendered can be labelled managed-by-agentstack (label
            // only — read-only contract, no write path from the dashboard). A
            // missing/unreadable ledger just yields no managed names.
            let managed: std::collections::BTreeSet<String> = d
                .extensions_dir_for(scope, &ctx.dir)
                .and_then(|dir| crate::render::extensions::managed_artifacts(&dir).ok())
                .into_iter()
                .flatten()
                .map(|m| m.filename)
                .collect();
            for e in d.discover_extensions(scope, &ctx.dir) {
                extensions.push(json!({
                    "harness": d.id,
                    "name": e.name,
                    "kind": e.kind,
                    "isSymlink": e.is_symlink,
                    "broken": e.broken,
                    "scope": label,
                    "managedByAgentstack": managed.contains(&e.name),
                }));
            }
        }
    }

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
    let next_actions = next_actions(&secrets, &skills, &global_drift, &health);

    // Live tracked harness runs (separate agentstack processes the dashboard can
    // observe and kill). For a profile-bound run, surface its trust footprint —
    // the servers + skills that live process can reach — inline.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let runs: Vec<Value> = crate::runs::list()
        .into_iter()
        .map(|r| {
            let (p_servers, p_skills) = r
                .profile
                .as_ref()
                .and_then(|p| manifest.profiles.get(p))
                .map(|p| (p.servers.clone(), p.skills.clone()))
                .unwrap_or_default();
            json!({
                "id": r.id,
                "harness": r.harness,
                "display": r.display,
                "pid": r.pid,
                "command": r.command,
                "args": r.args,
                "cwd": r.cwd,
                "profile": r.profile,
                "scope": r.scope,
                "startedUnix": r.started_unix,
                "uptimeSecs": now.saturating_sub(r.started_unix),
                "revertsOnExit": r.started_session,
                "servers": p_servers,
                "skills": p_skills,
            })
        })
        .collect();

    // Zero-files gateway: which detected harnesses carry the global
    // `agentstack mcp --auto-project` entry, and where this project stands with
    // the trust gate. Read-only mirror of `doctor`'s bridge section — the
    // dashboard shows it; granting trust stays a terminal act.
    let bridge_harnesses: Vec<Value> = ctx
        .registry
        .iter()
        .filter(|d| d.detected())
        // let-else binds both options together, so the presence check and the
        // use are one expression — no filter-then-unwrap coupling to keep in
        // sync (an entry missing either field is simply skipped).
        .filter_map(|d| {
            let (Some(cfg), Some(mcp)) = (d.config.as_ref(), d.mcp.as_ref()) else {
                return None;
            };
            let path = crate::util::paths::expand_tilde(&cfg.path);
            let existing = std::fs::read_to_string(&path).unwrap_or_default();
            let connected =
                crate::commands::connect::has_bridge_entry(&existing, &mcp.location, cfg.format);
            Some(json!({ "id": d.id, "display": d.display, "connected": connected }))
        })
        .collect();
    let project_base = crate::manifest::project_root_of(&ctx.dir);
    let bridge = json!({
        "harnesses": bridge_harnesses,
        "project": project_base.display().to_string(),
        "trust": match crate::trust::check(&project_base) {
            crate::trust::TrustState::Trusted => "trusted",
            crate::trust::TrustState::Changed => "changed",
            crate::trust::TrustState::Untrusted => "untrusted",
        },
    });

    // Read-only analysis panels — the same numbers the CLI's `proxy report`,
    // `analyze`, `optimize`, and `stats` print, embedded for the dashboard.
    // Each collector is best-effort (no `?`): a failure degrades to an empty
    // panel rather than sinking the whole snapshot. `statsReport` avoids the
    // existing `stats` key (activation counts consumed by the Usage card).
    let proxy = proxy_report();
    let analyze = crate::commands::analyze::collect();
    let optimize = crate::commands::optimize::collect(manifest_dir);
    let stats_report = crate::commands::stats::collect(manifest_dir);

    Ok(json!({
        "meta": {
            "name": manifest.meta.name,
            "dir": ctx.dir.display().to_string(),
            "version": env!("CARGO_PKG_VERSION"),
            "defaultTargets": manifest.targets.default,
        },
        "bridge": bridge,
        "adapters": adapters,
        "servers": servers,
        "skills": skills,
        "skillAdapters": skill_adapters,
        "discoveredSkills": discovered_skills,
        "settingsAdapters": settings_adapters,
        "hooks": hooks,
        "hookAdapters": hook_adapters,
        "extensions": extensions,
        "instructions": instructions,
        "secrets": secrets,
        "profiles": profiles,
        "stats": stats,
        "proxy": proxy,
        "analyze": analyze,
        "optimize": optimize,
        "statsReport": stats_report,
        "health": health,
        "nextActions": next_actions,
        "runs": runs,
        "session": crate::session::active(&ctx.dir).map(|s| json!({
            "profile": s.profile,
            "scope": s.scope,
            "startedUnix": s.started_unix,
            "loads": s.loads.iter().map(|l| json!({
                "name": l.name, "reason": l.reason, "ts": l.ts,
            })).collect::<Vec<_>>(),
        })),
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
    let selected = crate::render::resolve_targets(manifest, &ctx.registry, &[])?;
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
        let key = target_key(&id, scope, &ctx.dir);
        let mut prev = state.managed_servers(&key);
        // Match apply's cross-manifest guard so this preview shows what a
        // write would actually do: foreign-recorded entries are kept.
        state.foreign_prunes(&key, scope, &ctx.dir, &mut prev, |n| {
            manifest.servers.contains_key(n)
        });
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
                diff: plan.diff_plain(),
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
            format!("{} secret{} resolved", refs.len(), s(refs.len())),
            None,
        );
    } else {
        push(
            &mut out,
            "error",
            format!(
                "{} secret{} unresolved: {}",
                unresolved.len(),
                s(unresolved.len()),
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
        push(out, "ok", "your tools are in sync".into(), None);
    } else {
        push(
            out,
            "warn",
            format!(
                "{drift} selected target{} drifted (global) — Preview to reconcile",
                s(drift)
            ),
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
                "{extra_drift} installed non-default target{} with renderable drift — preview all to reconcile",
                s(extra_drift)
            ),
            Some(json!({ "type": "preview", "scope": "global", "all": true })),
        );
    }
}

fn next_actions(
    secrets: &[Value],
    skills: &[Value],
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
            "Your tools are out of sync".into(),
            format!(
                "{drift} tool{} out of sync with your saved stack — review and apply.",
                s(drift)
            ),
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
            format!(
                "{} skill source{} not installed.",
                missing_skills.len(),
                s(missing_skills.len())
            ),
            "skills",
            "Review skills",
            json!({ "type": "section", "section": "skills" }),
        ));
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
        assert!(messages.contains(&"your tools are in sync"));
        assert!(messages
            .iter()
            .any(|msg| msg.contains("2 installed non-default targets")));
        assert!(!messages
            .iter()
            .any(|msg| msg.contains("2 selected targets drifted")));
    }

    #[test]
    fn next_actions_include_missing_secret_and_selected_drift() {
        let secrets = vec![json!({ "name": "KIBANA_TOKEN", "resolved": false })];
        let skills = Vec::new();
        let selected = vec![drift("codex", true, true)];
        let health = Vec::new();

        let actions = next_actions(&secrets, &skills, &selected, &health);
        let ids: Vec<&str> = actions
            .iter()
            .filter_map(|a| a.get("id").and_then(Value::as_str))
            .collect();

        assert!(ids.contains(&"missing-secret:KIBANA_TOKEN"));
        assert!(ids.contains(&"drift:global"));
    }

    #[test]
    fn proxy_report_is_a_well_formed_empty_state_when_no_telemetry() {
        // `proxy_report` never touches a manifest, so its empty-state shape is
        // deterministic regardless of the machine's telemetry: a numeric
        // request count and a capabilities array (never an error).
        let p = proxy_report();
        assert!(p["requests"].is_u64(), "requests is a number");
        assert!(p["totalTools"].is_u64());
        assert!(
            p["totalLabel"].is_string(),
            "pre-formatted token label for the UI"
        );
        assert!(
            p["capabilities"].is_array(),
            "capabilities is always an array, empty when no wire activity"
        );
        for cap in p["capabilities"].as_array().unwrap() {
            assert!(cap["capability"].is_string());
            assert!(cap["tools"].is_u64());
            assert!(cap["avgLabel"].is_string());
            assert!(cap["calls"].is_u64());
            assert!(cap["hint"].is_string());
        }
    }

    #[test]
    fn build_embeds_the_four_analysis_panels() {
        // A minimal real manifest on disk is enough to exercise `build`; the
        // four analysis collectors read global state best-effort, so the panels
        // are present and well-formed even on a machine with no history.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(crate::manifest::load::MANIFEST_FILE),
            "version = 1\n[meta]\nname = \"test\"\n",
        )
        .unwrap();

        let v = build(Some(dir.path())).expect("snapshot builds");

        // Proxy: ranked wire report with an explicit empty state.
        assert!(v["proxy"]["requests"].is_u64());
        assert!(v["proxy"]["capabilities"].is_array());

        // Analyze: call activity + library dead weight, both objects.
        assert!(v["analyze"]["calls"].is_object());
        assert!(v["analyze"]["dead_weight"].is_object());

        // Optimize: the recommendation list (empty is well-formed).
        assert!(v["optimize"]["recommendations"].is_array());

        // Stats: per-capability report under `statsReport` (the legacy `stats`
        // key stays the activation-count list the Usage card consumes).
        assert!(v["statsReport"]["capabilities"].is_array());
        assert!(v["statsReport"]["anyMeasured"].is_boolean());
        assert!(v["stats"].is_array(), "legacy activation list is untouched");
    }
}
