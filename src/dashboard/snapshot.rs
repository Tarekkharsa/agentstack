//! Builds the read-only JSON snapshot the dashboard renders — the cross-harness
//! matrix plus secrets/skills/stats — aggregated from the core library. Secret
//! *values* are never included, only resolved/unresolved status (PLAN §9f).

use std::path::Path;

use anyhow::Result;
use serde_json::{json, Value};

use crate::scope::Scope;
use crate::secret::Resolver;
use crate::state::{target_key, State};
use crate::store::{local_source_dir, Store};
use crate::usage::Usage;

pub fn build(manifest_dir: Option<&Path>) -> Result<Value> {
    let ctx = crate::commands::load(manifest_dir)?;
    let manifest = &ctx.loaded.manifest;
    let state = State::load().unwrap_or_default();
    let usage = Usage::load().unwrap_or_default();
    let store = Store::default_store();

    // Adapters (columns of the matrix).
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
            })
        })
        .collect();
    let adapter_ids: Vec<String> = ctx.registry.ids().map(String::from).collect();

    // Servers × adapters matrix, using state for "currently active".
    let servers: Vec<Value> = manifest
        .servers
        .iter()
        .map(|(name, s)| {
            let cells: Vec<Value> = adapter_ids
                .iter()
                .map(|id| {
                    let g = state
                        .managed_servers(&target_key(id, Scope::Global))
                        .contains(name);
                    let p = state
                        .managed_servers(&target_key(id, Scope::Project))
                        .contains(name);
                    json!({ "adapter": id, "global": g, "project": p })
                })
                .collect();
            json!({
                "name": name,
                "type": match s.server_type {
                    crate::manifest::ServerType::Http => "http",
                    crate::manifest::ServerType::Stdio => "stdio",
                },
                "url": s.url,
                "cells": cells,
            })
        })
        .collect();

    // Skills: source kind + whether installed locally.
    let skills: Vec<Value> = manifest
        .skills
        .iter()
        .map(|(name, sk)| {
            let kind = match sk.source() {
                Ok(crate::manifest::SkillSource::Git { .. }) => "git",
                Ok(crate::manifest::SkillSource::Path(_)) => "path",
                Err(_) => "invalid",
            };
            json!({
                "name": name,
                "source": kind,
                "installed": local_source_dir(&store, sk, &ctx.dir).is_some(),
            })
        })
        .collect();

    // Secrets: resolved status only (never the value).
    let secrets: Vec<Value> = manifest
        .referenced_secrets()
        .into_iter()
        .map(|name| {
            let resolved = ctx.resolver.resolve(&name).is_some();
            json!({ "name": name, "resolved": resolved })
        })
        .collect();

    // Profiles.
    let profiles: Vec<Value> = manifest
        .profiles
        .iter()
        .map(|(name, p)| json!({ "name": name, "servers": p.servers, "skills": p.skills }))
        .collect();

    // Usage stats.
    let stats: Vec<Value> = {
        let mut v: Vec<(&String, &u64)> = usage.activations.iter().collect();
        v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        v.into_iter()
            .map(|(name, count)| json!({ "name": name, "activations": count }))
            .collect()
    };

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
        "secrets": secrets,
        "profiles": profiles,
        "stats": stats,
    }))
}
