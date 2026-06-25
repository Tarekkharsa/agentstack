//! `agentstack add server|skill` — add a capability to the manifest. Flag-driven
//! (scriptable, agent-operable), writing into `agentstack.toml` via the TOML
//! merger so comments/formatting survive.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use indexmap::IndexMap;
use owo_colors::OwoColorize;
use serde_json::Value;
use toml_edit::{Array, DocumentMut};

use crate::cli::{AddArgs, AddFromArgs, AddKind, AddServerArgs, AddSkillArgs};
use crate::manifest::{Server, ServerType, Skill};
use crate::provider;
use crate::render::merge_toml;
use crate::util::diff;

pub fn run(args: &AddArgs, manifest_dir: Option<&Path>) -> Result<()> {
    match &args.kind {
        AddKind::From(a) => add_from(a, manifest_dir),
        AddKind::Server(a) => add_server(a, manifest_dir),
        AddKind::Skill(a) => add_skill(a, manifest_dir),
    }
}

fn add_from(a: &AddFromArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let ctx = super::load(manifest_dir)?;
    let candidate = provider::resolve(&a.id)
        .with_context(|| format!("no capability '{}' in the catalog or registry", a.id))?;
    if ctx.loaded.manifest.servers.contains_key(&candidate.name) {
        anyhow::bail!("server '{}' already exists in the manifest", candidate.name);
    }
    println!(
        "{} {} ({}) — {}",
        "found".green(),
        candidate.name.bold(),
        candidate.source,
        candidate.id
    );
    let server = candidate.to_server();
    write_manifest(
        &ctx,
        "servers",
        &serde_json::to_value(&server)?,
        a.profile.as_deref(),
        &candidate.name,
        a.write,
    )?;
    if a.write {
        println!(
            "{} review secrets with `agentstack secret list`, then `agentstack apply`.",
            "↳".cyan()
        );
    }
    Ok(())
}

fn add_server(a: &AddServerArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let ctx = super::load(manifest_dir)?;
    if ctx.loaded.manifest.servers.contains_key(&a.name) {
        anyhow::bail!("server '{}' already exists in the manifest", a.name);
    }

    let server = Server {
        server_type: a.transport,
        url: a.url.clone(),
        command: a.command.clone(),
        args: a.args.clone(),
        headers: parse_kv(&a.headers)?,
        env: parse_kv(&a.env)?,
    };
    match a.transport {
        ServerType::Http if server.url.is_none() => {
            anyhow::bail!("http server needs --url")
        }
        ServerType::Stdio if server.command.is_none() => {
            anyhow::bail!("stdio server needs --command")
        }
        _ => {}
    }

    write_manifest(
        &ctx,
        "servers",
        &serde_json::to_value(&server)?,
        a.profile.as_deref(),
        &a.name,
        a.write,
    )
}

fn add_skill(a: &AddSkillArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let ctx = super::load(manifest_dir)?;
    if ctx.loaded.manifest.skills.contains_key(&a.name) {
        anyhow::bail!("skill '{}' already exists in the manifest", a.name);
    }
    let skill = Skill {
        path: Some(a.path.clone()),
        git: None,
        rev: None,
    };
    write_manifest(
        &ctx,
        "skills",
        &serde_json::to_value(&skill)?,
        a.profile.as_deref(),
        &a.name,
        a.write,
    )
}

fn write_manifest(
    ctx: &super::Context,
    location: &str,
    body: &Value,
    profile: Option<&str>,
    name: &str,
    write: bool,
) -> Result<()> {
    let original = fs::read_to_string(&ctx.loaded.manifest_path)
        .with_context(|| format!("reading {}", ctx.loaded.manifest_path.display()))?;
    let new_text = build_manifest_with(&original, location, name, body, profile)?;

    println!(
        "{} add '{name}' to {}",
        "→".cyan(),
        ctx.loaded.manifest_path.display()
    );
    print!(
        "{}",
        diff::render(&original, &new_text)
            .lines()
            .map(|l| format!("  {l}\n"))
            .collect::<String>()
    );

    if write {
        fs::write(&ctx.loaded.manifest_path, &new_text)
            .with_context(|| format!("writing {}", ctx.loaded.manifest_path.display()))?;
        println!("{} added '{name}'.", "✓".green());
    } else {
        println!(
            "\nDry run. Re-run with {} to update the manifest.",
            "--write".bold()
        );
    }
    Ok(())
}

/// Build updated manifest text with `name` (a server or skill) inserted under
/// `location`, optionally enrolled in `profile`. Shared by the CLI and the MCP
/// server; preserves comments via the TOML merger.
pub fn build_manifest_with(
    original: &str,
    location: &str,
    name: &str,
    body: &Value,
    profile: Option<&str>,
) -> Result<String> {
    let entries = vec![(name.to_string(), body.clone())];
    let mut new_text = merge_toml::merge(original, location, &entries, true)?;
    if let Some(p) = profile {
        new_text = add_to_profile(&new_text, p, location, name)?;
    }
    Ok(new_text)
}

/// Append `name` to `profiles.<profile>.<field>` (creating the array if needed).
fn add_to_profile(text: &str, profile: &str, field: &str, name: &str) -> Result<String> {
    use toml_edit::{Item, Table};
    let mut doc: DocumentMut = text.parse().context("parsing manifest as TOML")?;

    // Ensure `[profiles]` and `[profiles.<profile>]` exist as standalone tables
    // (not inline) so freshly-created profiles render cleanly.
    if doc.get("profiles").is_none() {
        let mut t = Table::new();
        t.set_implicit(true);
        doc.insert("profiles", Item::Table(t));
    }
    let profiles = doc["profiles"]
        .as_table_mut()
        .context("`profiles` is not a table")?;
    if profiles.get(profile).is_none() {
        profiles.insert(profile, Item::Table(Table::new()));
    }
    let ptable = profiles[profile]
        .as_table_mut()
        .with_context(|| format!("profiles.{profile} is not a table"))?;

    let slot = &mut ptable[field];
    if slot.is_none() {
        *slot = toml_edit::value(Array::new());
    }
    let arr = slot
        .as_array_mut()
        .with_context(|| format!("profiles.{profile}.{field} is not an array"))?;
    if !arr.iter().any(|v| v.as_str() == Some(name)) {
        arr.push(name);
    }
    Ok(doc.to_string())
}

/// Parse `Key=Value` strings into an ordered map.
fn parse_kv(pairs: &[String]) -> Result<IndexMap<String, String>> {
    let mut map = IndexMap::new();
    for p in pairs {
        let (k, v) = p
            .split_once('=')
            .with_context(|| format!("expected Key=Value, got '{p}'"))?;
        map.insert(k.trim().to_string(), v.to_string());
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_kv_with_equals_in_value() {
        let m = parse_kv(&["A=1".into(), "B=x=y".into()]).unwrap();
        assert_eq!(m["A"], "1");
        assert_eq!(m["B"], "x=y");
    }

    #[test]
    fn appends_to_existing_profile_array() {
        let text = "version = 1\n[profiles.backend]\nservers = [\"a\"]\n";
        let out = add_to_profile(text, "backend", "servers", "b").unwrap();
        assert!(out.contains("\"a\""));
        assert!(out.contains("\"b\""));
        // Idempotent.
        let again = add_to_profile(&out, "backend", "servers", "b").unwrap();
        assert_eq!(again.matches("\"b\"").count(), 1);
    }

    #[test]
    fn creates_profile_array_when_absent() {
        let out = add_to_profile("version = 1\n", "new", "skills", "x").unwrap();
        let doc: DocumentMut = out.parse().unwrap();
        assert!(doc["profiles"]["new"]["skills"].is_array());
    }
}
