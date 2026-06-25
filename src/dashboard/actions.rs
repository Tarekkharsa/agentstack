//! Dashboard write actions that don't map 1:1 to a CLI command: per-CLI
//! enable/disable of a server, and adding a custom server to the manifest.

use std::path::Path;

use anyhow::{Context, Result};
use indexmap::IndexMap;
use serde_json::Value;

use crate::manifest::{Server, ServerType};
use crate::render::{plan_target, Selection};
use crate::scope::Scope;
use crate::state::{target_key, State};

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
    std::fs::write(&manifest_path, &new_text)
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
