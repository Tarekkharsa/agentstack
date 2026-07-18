//! `agentstack remove <name>` — drop a server or skill from the manifest (and
//! the lockfile for skills), including any profile membership. Comments survive.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use owo_colors::OwoColorize;
use toml_edit::DocumentMut;

use crate::cli::RemoveArgs;
use crate::lock::Lock;
use crate::manifest::{Instruction, Manifest, PackInstall};
use crate::util::diff;

pub fn run(args: &RemoveArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let ctx = super::load(manifest_dir)?;
    let manifest = &ctx.loaded.manifest;

    // A pack install ledger takes precedence: removing it tears down every
    // member the pack added (server + skills + instructions).
    if let Some(recipe) = pack_ledger(manifest, &args.name) {
        return remove_pack(&ctx, &args.name, recipe, args.write);
    }

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
        crate::util::atomic::write(&ctx.loaded.manifest_path, &new_text)
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

/// The `[packs.<name>]` install ledger, if one exists.
pub(crate) fn pack_ledger<'a>(manifest: &'a Manifest, name: &str) -> Option<&'a PackInstall> {
    manifest.packs.get(name)
}

/// Tear down a vendor pack: remove every member listed in the ledger from the
/// manifest + profiles, delete pack-written instruction files (only ours — they
/// carry the `agentstack:vendor` header), drop the ledger, and clean lockfile
/// entries for removed skills.
fn remove_pack(ctx: &super::Context, name: &str, recipe: &PackInstall, write: bool) -> Result<()> {
    let original = fs::read_to_string(&ctx.loaded.manifest_path)
        .with_context(|| format!("reading {}", ctx.loaded.manifest_path.display()))?;

    let mut new_text = original.clone();
    for server in &recipe.servers {
        new_text = remove_entry(&new_text, "servers", server)?;
    }
    for skill in &recipe.skills {
        new_text = remove_entry(&new_text, "skills", skill)?;
    }
    for instr in &recipe.instructions {
        new_text = remove_entry(&new_text, "instructions", instr)?;
    }
    for hook in &recipe.hooks {
        new_text = remove_entry(&new_text, "hooks", hook)?;
    }
    new_text = remove_entry(&new_text, "packs", name)?;

    // Instruction files we wrote and may delete (carry the vendor marker).
    let safe_instr_files = safe_instruction_files(&ctx.loaded.manifest, ctx, recipe, name);
    // Pack-owned skill dirs to delete (contained under the manifest dir).
    let skill_dirs = safe_skill_dirs(&ctx.loaded.manifest, ctx, recipe);

    println!(
        "{} remove pack '{}' from {}",
        "−".yellow(),
        name,
        ctx.loaded.manifest_path.display()
    );
    print!(
        "{}",
        diff::render(&original, &new_text)
            .lines()
            .map(|l| format!("  {l}\n"))
            .collect::<String>()
    );
    for path in &skill_dirs {
        if path.exists() {
            println!("  {} delete {}/", "−".yellow(), path.display());
        }
    }
    for path in &safe_instr_files {
        println!("  {} delete {}", "−".yellow(), path.display());
    }

    if write {
        crate::util::atomic::write(&ctx.loaded.manifest_path, &new_text)
            .with_context(|| format!("writing {}", ctx.loaded.manifest_path.display()))?;
        for dir in &skill_dirs {
            remove_skill_dir(dir, &ctx.dir);
        }
        for path in &safe_instr_files {
            let _ = fs::remove_file(path);
        }
        if !recipe.skills.is_empty() {
            let mut lock = Lock::load(&ctx.dir)?;
            let mut changed = false;
            for skill in &recipe.skills {
                changed |= lock.remove(skill);
            }
            if changed {
                lock.save(&ctx.dir)?;
            }
        }
        println!("{} removed pack '{}'.", "✓".green(), name);
    } else {
        println!(
            "\nDry run. Re-run with {} to update the manifest.",
            "--write".bold()
        );
    }
    Ok(())
}

/// Resolve which of the ledger's skill directories are safe to delete. Pack
/// ownership is established by the ledger listing the skill; [`contained_skill_dir`]
/// then caps the blast radius so a hand-edited or corrupt ledger can never make us
/// delete the project root, a top-level dir, or the shared `skills/` tree itself.
pub(crate) fn safe_skill_dirs(
    manifest: &Manifest,
    ctx: &super::Context,
    recipe: &PackInstall,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for name in &recipe.skills {
        let Some(path) = manifest.skills.get(name).and_then(|s| s.path.as_deref()) else {
            continue;
        };
        if let Some(dir) = contained_skill_dir(&ctx.dir, path) {
            out.push(dir);
        }
    }
    out
}

/// Validate a ledger skill `path` and resolve it to an absolute dir that is safe
/// to `remove_dir_all`. The path must:
/// - be relative and made only of *normal* components — any `.`, `..`, root, or
///   drive-prefix component is rejected (blocks `"."`, `"./"`, `"../x"`, `"/x"`);
/// - live under the conventional `skills/` asset area (where every pack skill is
///   extracted); and
/// - be nested at least one level deep, so we never target `"."`, a bare
///   top-level dir, or `skills/` itself.
///
/// Anything failing these rules yields `None` and is left on disk untouched.
fn contained_skill_dir(root: &Path, path: &str) -> Option<PathBuf> {
    let rel = Path::new(path.trim_start_matches("./"));
    let mut comps = Vec::new();
    for c in rel.components() {
        match c {
            std::path::Component::Normal(s) => comps.push(s),
            // CurDir / ParentDir / RootDir / Prefix are all unsafe here.
            _ => return None,
        }
    }
    // Must be `skills/<something>[/...]` — never `.`, a top-level dir, or a bare
    // `skills`.
    if comps.len() < 2 || comps[0] != "skills" {
        return None;
    }
    Some(root.join(rel))
}

/// Best-effort: remove `dir`, then walk empty parents upward, stopping before
/// `root`. Keeps the managed tree tidy after a skill dir is deleted without ever
/// touching anything outside `root`.
pub(crate) fn remove_skill_dir(dir: &Path, root: &Path) {
    if fs::remove_dir_all(dir).is_err() {
        return;
    }
    let mut parent = dir.parent();
    while let Some(p) = parent {
        if p == root || !p.starts_with(root) {
            break;
        }
        // `remove_dir` only succeeds on an empty directory — exactly what we want.
        if fs::remove_dir(p).is_err() {
            break;
        }
        parent = p.parent();
    }
}

/// Resolve which of the ledger's instruction files are safe to delete: they
/// must exist, resolve under the manifest dir, and carry the pack's
/// `agentstack:vendor` provenance marker (never touch user-authored files).
pub(crate) fn safe_instruction_files(
    manifest: &Manifest,
    ctx: &super::Context,
    recipe: &PackInstall,
    pack: &str,
) -> Vec<PathBuf> {
    let marker = format!("agentstack:vendor {pack}");
    let mut out = Vec::new();
    for name in &recipe.instructions {
        let Some(Instruction { path, .. }) = manifest.instructions.get(name) else {
            continue;
        };
        let rel = Path::new(path.trim_start_matches("./"));
        // Never follow a path that escapes the manifest dir (e.g. `../`), even
        // if it happened to carry our marker — the marker is the primary guard
        // but containment keeps deletion strictly within the managed area.
        if rel.is_absolute()
            || rel
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            continue;
        }
        let file = ctx.dir.join(rel);
        if let Ok(content) = fs::read_to_string(&file) {
            if content.contains(&marker) {
                out.push(file);
            }
        }
    }
    out
}

/// Remove `name` from the `kind` table and from every profile's `kind` array.
pub(crate) fn remove_entry(text: &str, kind: &str, name: &str) -> Result<String> {
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
    fn pack_ledger_found_by_name() {
        let m: Manifest = toml::from_str(
            r#"
            version = 1
            [packs.linear-pack]
            version = "0.1.0"
            description = "Linear pack"
            "#,
        )
        .unwrap();
        assert!(pack_ledger(&m, "linear-pack").is_some());
        assert!(pack_ledger(&m, "missing").is_none());
    }

    #[test]
    fn contained_skill_dir_only_accepts_nested_paths_under_skills() {
        let root = Path::new("/repo");
        // Legitimate pack skill asset paths resolve under the root.
        assert_eq!(
            contained_skill_dir(root, "./skills/pr-triage"),
            Some(root.join("skills/pr-triage"))
        );
        assert_eq!(
            contained_skill_dir(root, "skills/linear/breakdown"),
            Some(root.join("skills/linear/breakdown"))
        );
        // Dangerous / malformed paths are all rejected — never delete root, a
        // top-level dir, the shared skills/ tree, or anything outside it.
        for bad in [
            ".",
            "./",
            "",
            "skills",
            "skills/",
            "instructions/x",
            "../escape",
            "skills/../..",
            "/abs/skills/x",
            "./../skills/x",
        ] {
            assert_eq!(
                contained_skill_dir(root, bad),
                None,
                "path {bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn removes_pack_ledger_table() {
        let text = "version = 1\n\n[packs.linear-pack]\nversion = \"0.1.0\"\ndescription = \"x\"\n";
        let out = remove_entry(text, "packs", "linear-pack").unwrap();
        assert!(!out.contains("[packs.linear-pack]"));
    }

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
