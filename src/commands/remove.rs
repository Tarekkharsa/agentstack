//! `agentstack remove <name>` — drop a server or skill from the manifest (and
//! the lockfile for skills), including any profile membership. Comments survive.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use owo_colors::OwoColorize;
use toml_edit::DocumentMut;

use crate::cli::RemoveArgs;
use crate::lock::Lock;
use crate::util::diff;

pub fn run(args: &RemoveArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let ctx = super::load(manifest_dir)?;
    let manifest = &ctx.loaded.manifest;

    let kind = if manifest.servers.contains_key(&args.name) {
        "servers"
    } else if manifest.skills.contains_key(&args.name) {
        "skills"
    } else {
        anyhow::bail!("no server or skill named '{}' in the manifest", args.name);
    };
    let is_skill = kind == "skills";

    let original = fs::read_to_string(&ctx.loaded.manifest_path)
        .with_context(|| format!("reading {}", ctx.loaded.manifest_path.display()))?;
    let new_text = remove_entry(&original, kind, &args.name)?;

    println!(
        "{} remove {} '{}' from {}",
        "−".yellow(),
        kind.trim_end_matches('s'),
        args.name,
        ctx.loaded.manifest_path.display()
    );
    print!(
        "{}",
        diff::render(&original, &new_text)
            .lines()
            .map(|l| format!("  {l}\n"))
            .collect::<String>()
    );

    if args.write {
        fs::write(&ctx.loaded.manifest_path, &new_text)
            .with_context(|| format!("writing {}", ctx.loaded.manifest_path.display()))?;
        if is_skill {
            let mut lock = Lock::load(&ctx.dir)?;
            if lock.remove(&args.name) {
                lock.save(&ctx.dir)?;
            }
        }
        println!("{} removed '{}'.", "✓".green(), args.name);
    } else {
        println!(
            "\nDry run. Re-run with {} to update the manifest.",
            "--write".bold()
        );
    }
    Ok(())
}

/// Remove `name` from the `kind` table and from every profile's `kind` array.
fn remove_entry(text: &str, kind: &str, name: &str) -> Result<String> {
    let mut doc: DocumentMut = text.parse().context("parsing manifest as TOML")?;

    if let Some(tbl) = doc.get_mut(kind).and_then(|i| i.as_table_mut()) {
        tbl.remove(name);
    }
    if let Some(profiles) = doc.get_mut("profiles").and_then(|i| i.as_table_mut()) {
        for (_, item) in profiles.iter_mut() {
            if let Some(arr) = item
                .as_table_mut()
                .and_then(|t| t.get_mut(kind))
                .and_then(|i| i.as_array_mut())
            {
                arr.retain(|v| v.as_str() != Some(name));
            }
        }
    }
    Ok(doc.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_server_table_and_profile_membership() {
        let text = "version = 1\n\n[servers.a]\ntype = \"http\"\nurl = \"u\"\n\n[servers.a.headers]\nX = \"1\"\n\n[servers.b]\ntype = \"http\"\nurl = \"v\"\n\n[profiles.p]\nservers = [\"a\", \"b\"]\n";
        let out = remove_entry(text, "servers", "a").unwrap();
        assert!(!out.contains("[servers.a]"));
        assert!(!out.contains("[servers.a.headers]"));
        assert!(out.contains("[servers.b]"));
        // Removed from the profile array too.
        let doc: DocumentMut = out.parse().unwrap();
        let arr = doc["profiles"]["p"]["servers"].as_array().unwrap();
        let names: Vec<_> = arr.iter().filter_map(|v| v.as_str()).collect();
        assert_eq!(names, vec!["b"]);
    }
}
