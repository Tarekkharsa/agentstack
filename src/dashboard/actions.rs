//! Dashboard write actions that don't map 1:1 to a CLI command: per-CLI
//! enable/disable of a server, and adding a custom server to the manifest.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use indexmap::IndexMap;
use serde_json::Value;

use crate::manifest::{Server, ServerType, Skill};
use crate::render::{plan_target, skills, Selection};
use crate::scope::Scope;
use crate::state::{target_key, State};
use crate::store::{local_source_dir, Store};

/// Enable or disable one server for one target CLI in a scope. Writes that CLI's
/// config (rendering its full enabled set) and updates state.
pub fn toggle(
    manifest_dir: Option<&Path>,
    server: &str,
    target: &str,
    scope: Scope,
    enable: bool,
) -> Result<()> {
    let ctx = crate::commands::load(manifest_dir)?;
    let manifest = &ctx.loaded.manifest;
    if !manifest.servers.contains_key(server) {
        anyhow::bail!("no server '{server}' in the manifest");
    }
    let desc = ctx
        .registry
        .get(target)
        .with_context(|| format!("unknown target '{target}'"))?;

    let key = target_key(target, scope);
    let mut state = State::load()?;
    let previously = state.managed_servers(&key);

    let mut wanted = previously.clone();
    if enable {
        if !wanted.iter().any(|s| s == server) {
            wanted.push(server.to_string());
        }
    } else {
        wanted.retain(|s| s != server);
    }

    let plan = plan_target(
        manifest,
        desc,
        &ctx.resolver,
        &Selection::Explicit(wanted),
        &previously,
        scope,
        &ctx.dir,
    )?
    .with_context(|| format!("{} does not support {scope} scope", desc.display))?;

    plan.write()?;
    state.record(&key, plan.managed.clone(), &plan.proposed);
    state.save()?;
    crate::usage::bump(&[server.to_string()]);
    Ok(())
}

/// Enable or disable one skill for one target CLI in a scope by materializing
/// (symlink/copy) or pruning it in that CLI's skills directory.
pub fn toggle_skill(
    manifest_dir: Option<&Path>,
    skill: &str,
    target: &str,
    scope: Scope,
    enable: bool,
) -> Result<()> {
    let ctx = crate::commands::load(manifest_dir)?;
    let manifest = &ctx.loaded.manifest;
    if !manifest.skills.contains_key(skill) {
        anyhow::bail!("no skill '{skill}' in the manifest");
    }
    let desc = ctx
        .registry
        .get(target)
        .with_context(|| format!("unknown target '{target}'"))?;
    let skills_dir = desc
        .skills_dir_for(scope, &ctx.dir)
        .with_context(|| format!("{} has no {scope} skills directory", desc.display))?;
    let strategy = desc.skills.as_ref().map(|s| s.strategy).unwrap_or_default();
    let store = Store::default_store();

    let key = target_key(target, scope);
    let mut state = State::load()?;
    let previously = state.managed_skills(&key);

    let mut wanted = previously.clone();
    if enable {
        if local_source_dir(&store, &manifest.skills[skill], &ctx.dir).is_none() {
            anyhow::bail!("skill '{skill}' is not installed — run Install first");
        }
        if !wanted.iter().any(|s| s == skill) {
            wanted.push(skill.to_string());
        }
    } else {
        wanted.retain(|s| s != skill);
    }

    // Resolve the wanted skills to their local source dirs (installed only).
    let active: Vec<(String, PathBuf)> = wanted
        .iter()
        .filter_map(|n| {
            let sk = manifest.skills.get(n)?;
            local_source_dir(&store, sk, &ctx.dir).map(|p| (n.clone(), p))
        })
        .collect();

    let plan = skills::plan(skills_dir, strategy, active, &previously);
    skills::materialize(&plan)?;
    state.record_skills(&key, plan.managed_names());
    state.save()?;
    crate::usage::bump(&[skill.to_string()]);
    Ok(())
}

/// Import a CLI's existing native settings file into the manifest's
/// `[settings.<target>]` block (catalog keys only). Returns the number of
/// top-level keys imported. Lets a user adopt settings they already have rather
/// than re-entering them.
pub fn import_settings(manifest_dir: Option<&Path>, target: &str) -> Result<usize> {
    let ctx = crate::commands::load(manifest_dir)?;
    let desc = ctx
        .registry
        .get(target)
        .with_context(|| format!("unknown target '{target}'"))?;
    if desc.settings.is_none() {
        anyhow::bail!("{} has no native settings file", desc.display);
    }
    let value = desc
        .read_settings_value(&ctx.dir)?
        .with_context(|| format!("{} has no settings file to import yet", desc.display))?;
    let imported = crate::adapter::extract_settings(desc, &value);
    if imported.is_empty() {
        anyhow::bail!(
            "no recognized settings found in {}'s settings file",
            desc.display
        );
    }
    let count = imported.len();
    let body = Value::Object(imported);

    let manifest_path = ctx.loaded.manifest_path.clone();
    let original = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("reading {}", manifest_path.display()))?;
    let new_text = crate::render::merge_toml::merge(
        &original,
        "settings",
        &[(target.to_string(), body)],
        true,
    )?;
    toml::from_str::<crate::manifest::Manifest>(&new_text)
        .context("resulting manifest would be invalid")?;
    crate::util::atomic::write(&manifest_path, &new_text)
        .with_context(|| format!("writing {}", manifest_path.display()))?;
    Ok(count)
}

/// Set (replace) the `[settings.<target>]` block in the manifest from a JSON
/// object supplied by the dashboard. Comments + other settings blocks survive.
pub fn set_settings(manifest_dir: Option<&Path>, args: &Value) -> Result<()> {
    let target = args
        .get("target")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .context("target is required")?;
    let body = args
        .get("settings")
        .cloned()
        .filter(|v| v.is_object())
        .context("settings must be a JSON object")?;

    // The target must be a real adapter id that actually has a settings file.
    let ctx = crate::commands::load(manifest_dir)?;
    let desc = ctx
        .registry
        .get(target)
        .with_context(|| format!("unknown target '{target}'"))?;
    if desc.settings.is_none() {
        anyhow::bail!("{} has no native settings file to manage", desc.display);
    }

    let manifest_path = ctx.loaded.manifest_path.clone();
    let original = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("reading {}", manifest_path.display()))?;
    // Upsert `[settings.<target>]` as a table; nested objects → subtables.
    let new_text = crate::render::merge_toml::merge(
        &original,
        "settings",
        &[(target.to_string(), body)],
        true,
    )?;
    // Guard: the result must still parse as a manifest.
    toml::from_str::<crate::manifest::Manifest>(&new_text)
        .context("resulting manifest would be invalid")?;
    crate::util::atomic::write(&manifest_path, &new_text)
        .with_context(|| format!("writing {}", manifest_path.display()))?;
    Ok(())
}

/// Adopt a skill already present on disk (discovered in a CLI's skills dir) into
/// the manifest as a `path` skill pointing at its real source. Manifest-only —
/// never moves or deletes the skill's files.
pub fn adopt_skill(manifest_dir: Option<&Path>, name: &str) -> Result<()> {
    let ctx = crate::commands::load(manifest_dir)?;
    if ctx.loaded.manifest.skills.contains_key(name) {
        anyhow::bail!("skill '{name}' is already in the manifest");
    }
    // Find the discovered skill's real source across all CLIs.
    let source = ctx
        .registry
        .iter()
        .flat_map(|d| d.discover_skills(Scope::Global, &ctx.dir))
        .find(|s| s.name == name)
        .map(|s| s.source)
        .with_context(|| format!("no skill named '{name}' found on disk"))?;

    let skill = Skill {
        path: Some(source.display().to_string()),
        git: None,
        rev: None,
    };
    let manifest_path = ctx.loaded.manifest_path.clone();
    let original = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("reading {}", manifest_path.display()))?;
    let body = serde_json::to_value(&skill)?;
    let new_text =
        crate::commands::add::build_manifest_with(&original, "skills", name, &body, None)?;
    crate::util::atomic::write(&manifest_path, &new_text)
        .with_context(|| format!("writing {}", manifest_path.display()))?;
    Ok(())
}

/// Adopt every discovered-on-disk skill that isn't already in the manifest.
/// Returns the number adopted.
pub fn adopt_all_skills(manifest_dir: Option<&Path>) -> Result<usize> {
    let ctx = crate::commands::load(manifest_dir)?;
    // Unique discovered skills not already in the manifest, by name.
    let mut seen: std::collections::BTreeMap<String, std::path::PathBuf> =
        std::collections::BTreeMap::new();
    for d in ctx.registry.iter() {
        for s in d.discover_skills(Scope::Global, &ctx.dir) {
            if !ctx.loaded.manifest.skills.contains_key(&s.name) {
                seen.entry(s.name).or_insert(s.source);
            }
        }
    }
    if seen.is_empty() {
        anyhow::bail!("no new skills found on disk to adopt");
    }
    let manifest_path = ctx.loaded.manifest_path.clone();
    let mut text = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("reading {}", manifest_path.display()))?;
    let count = seen.len();
    for (name, source) in seen {
        let skill = Skill {
            path: Some(source.display().to_string()),
            git: None,
            rev: None,
        };
        let body = serde_json::to_value(&skill)?;
        text = crate::commands::add::build_manifest_with(&text, "skills", &name, &body, None)?;
    }
    crate::util::atomic::write(&manifest_path, &text)
        .with_context(|| format!("writing {}", manifest_path.display()))?;
    Ok(count)
}

/// Add a skill to the manifest from dashboard form fields (git URL or local
/// path). Mirrors `add_server` — writes the manifest only; the user then clicks
/// Install to fetch it and toggles it into a CLI.
pub fn add_skill(manifest_dir: Option<&Path>, args: &Value) -> Result<String> {
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .context("skill name is required")?;
    let source = args.get("source").and_then(Value::as_str).unwrap_or("git");
    let skill = if source == "path" {
        Skill {
            path: str_field(args, "path"),
            git: None,
            rev: None,
        }
    } else {
        Skill {
            path: None,
            git: str_field(args, "git"),
            rev: str_field(args, "rev"),
        }
    };
    match source {
        "path" if skill.path.is_none() => anyhow::bail!("a path-sourced skill needs a path"),
        "git" if skill.git.is_none() => anyhow::bail!("a git-sourced skill needs a git URL"),
        _ => {}
    }

    let dir = match manifest_dir {
        Some(d) => d.to_path_buf(),
        None => std::env::current_dir()?,
    };
    let manifest_path = dir.join(crate::manifest::load::MANIFEST_FILE);
    let original = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("reading {}", manifest_path.display()))?;
    let parsed: crate::manifest::Manifest =
        toml::from_str(&original).context("parsing manifest")?;
    if parsed.skills.contains_key(name) {
        anyhow::bail!("skill '{name}' already exists");
    }
    let body = serde_json::to_value(&skill)?;
    let new_text =
        crate::commands::add::build_manifest_with(&original, "skills", name, &body, None)?;
    crate::util::atomic::write(&manifest_path, &new_text)
        .with_context(|| format!("writing {}", manifest_path.display()))?;
    Ok(name.to_string())
}

/// Add a custom MCP server to the manifest from dashboard form fields.
pub fn add_server(manifest_dir: Option<&Path>, args: &Value) -> Result<String> {
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .context("server name is required")?;
    let transport = args
        .get("transport")
        .and_then(Value::as_str)
        .unwrap_or("http");
    let server = Server {
        server_type: if transport == "stdio" {
            ServerType::Stdio
        } else {
            ServerType::Http
        },
        url: str_field(args, "url"),
        command: str_field(args, "command"),
        args: args
            .get("args")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        headers: obj_to_map(args.get("headers")),
        env: obj_to_map(args.get("env")),
    };
    match server.server_type {
        ServerType::Http if server.url.is_none() => anyhow::bail!("http server needs a url"),
        ServerType::Stdio if server.command.is_none() => {
            anyhow::bail!("stdio server needs a command")
        }
        _ => {}
    }

    let dir = match manifest_dir {
        Some(d) => d.to_path_buf(),
        None => std::env::current_dir()?,
    };
    let manifest_path = dir.join(crate::manifest::load::MANIFEST_FILE);
    let original = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("reading {}", manifest_path.display()))?;
    let parsed: crate::manifest::Manifest =
        toml::from_str(&original).context("parsing manifest")?;
    if parsed.servers.contains_key(name) {
        anyhow::bail!("server '{name}' already exists");
    }
    let body = serde_json::to_value(&server)?;
    let new_text =
        crate::commands::add::build_manifest_with(&original, "servers", name, &body, None)?;
    crate::util::atomic::write(&manifest_path, &new_text)
        .with_context(|| format!("writing {}", manifest_path.display()))?;
    Ok(name.to_string())
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

fn obj_to_map(v: Option<&Value>) -> IndexMap<String, String> {
    v.and_then(Value::as_object)
        .map(|o| {
            o.iter()
                .filter_map(|(k, val)| val.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default()
}
