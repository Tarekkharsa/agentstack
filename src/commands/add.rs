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

use crate::cli::{AddArgs, AddKind, AddServerArgs, AddSkillArgs};
use crate::manifest::{Server, ServerType, Skill};
use crate::render::merge_toml;
use crate::util::diff;

pub fn run(args: &AddArgs, manifest_dir: Option<&Path>) -> Result<()> {
    match &args.kind {
        AddKind::Server(a) => add_server(a, manifest_dir),
        AddKind::Skill(a) => add_skill(a, manifest_dir),
    }
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

    let entries = vec![(a.name.clone(), serde_json::to_value(&server)?)];
    write_manifest(
        &ctx,
        "servers",
        &entries,
        a.profile.as_deref(),
        "servers",
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
    let entries = vec![(a.name.clone(), serde_json::to_value(&skill)?)];
    write_manifest(
        &ctx,
        "skills",
        &entries,
        a.profile.as_deref(),
        "skills",
        &a.name,
        a.write,
    )
}

#[allow(clippy::too_many_arguments)]
fn write_manifest(
    ctx: &super::Context,
    location: &str,
    entries: &[(String, Value)],
    profile: Option<&str>,
    profile_field: &str,
    name: &str,
    write: bool,
) -> Result<()> {
    let original = fs::read_to_string(&ctx.loaded.manifest_path)
        .with_context(|| format!("reading {}", ctx.loaded.manifest_path.display()))?;
    let mut new_text = merge_toml::merge(&original, location, entries, true)?;
    if let Some(p) = profile {
        new_text = add_to_profile(&new_text, p, profile_field, name)?;
    }

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

/// Append `name` to `profiles.<profile>.<field>` (creating the array if needed).
fn add_to_profile(text: &str, profile: &str, field: &str, name: &str) -> Result<String> {
    let mut doc: DocumentMut = text.parse().context("parsing manifest as TOML")?;
    let slot = &mut doc["profiles"][profile][field];
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
