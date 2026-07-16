//! `agentstack upgrade <vendor>` — re-resolve an installed vendor pack from its
//! recorded source (`catalog:<id>`) and apply any changes to its members
//! (server, skills, house rules), re-pinning the lockfile. The counterpart to
//! `add <pack>` / `remove <pack>` that completes the pack lifecycle (Phase 6 of
//! docs/plans/vendor-packs.md).
//!
//! Safety mirrors `add_pack`: the re-resolved spec is re-checked against
//! `[policy]` before anything is written, instruction-body changes (which steer
//! the user's daily-driver agent) are gated behind `--with-instructions`/`--yes`,
//! and the apply is atomic — a failure restores the prior install from a backup.
//!
//! Known limitation: the catalog is embedded in the binary with a single version
//! per id, so re-resolving an installed pack yields identical content and
//! `upgrade` reports "already current". The command is structurally complete for
//! when the catalog becomes versioned/remote; today its real value is verifying a
//! pack still matches its source and re-pinning the lock.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context as _, Result};
use owo_colors::OwoColorize;

use crate::cli::UpgradeArgs;
use crate::commands::{add, install, remove};
use crate::lock::Lock;
use crate::manifest::{Instruction, PluginRecipe, Skill};
use crate::provider::{self, Candidate, CandidateKind, PackSpec};
use crate::store::{self, Store};
use crate::util::{atomic, diff};

pub fn run(args: &UpgradeArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let ctx = super::load(manifest_dir)?;
    let manifest = &ctx.loaded.manifest;

    let targets: Vec<String> = if args.all {
        manifest
            .plugins
            .iter()
            .filter(|(_, r)| r.kind.as_deref() == Some("pack"))
            .map(|(n, _)| n.clone())
            .collect()
    } else {
        let name = args
            .name
            .as_deref()
            .ok_or_else(|| anyhow!("upgrade needs a pack name (or --all)"))?;
        vec![name.to_string()]
    };

    if targets.is_empty() {
        println!("No vendor packs installed — nothing to upgrade.");
        return Ok(());
    }

    let mut failures = 0;
    for name in &targets {
        let Some(recipe) = remove::pack_ledger(manifest, name) else {
            if args.all {
                continue;
            }
            anyhow::bail!(
                "'{name}' is not an installed vendor pack (no [plugins.{name}] pack ledger). \
                 Use `agentstack remove` for single capabilities."
            );
        };
        if let Err(e) = upgrade_one(&ctx, manifest_dir, name, recipe, args) {
            if args.all {
                eprintln!("{} {name}: {e:#}", "✗".red());
                failures += 1;
            } else {
                return Err(e);
            }
        }
    }
    if failures > 0 {
        anyhow::bail!("{failures} pack(s) failed to upgrade");
    }
    Ok(())
}

/// Re-resolve one pack from its ledger source and hand off to `upgrade_pack`.
fn upgrade_one(
    ctx: &super::Context,
    manifest_dir: Option<&Path>,
    pack: &str,
    recipe: &PluginRecipe,
    args: &UpgradeArgs,
) -> Result<()> {
    let source = recipe
        .source
        .as_deref()
        .ok_or_else(|| anyhow!("pack '{pack}' has no recorded source to re-resolve"))?;

    // Git pack: re-resolve at the newest version tag on the remote (policy
    // gates the source again before any fetch).
    if let Some(git_ref) = crate::provider::gitpack::GitPackRef::parse(source) {
        let current_tag = git_ref.tag.clone().ok_or_else(|| {
            anyhow!("pack '{pack}' git source '{source}' has no tag to compare against")
        })?;
        let newest = crate::provider::gitpack::GitPackRef {
            tag: None, // resolve() selects the newest version tag
            ..git_ref.clone()
        };
        let (mut resolved, mut origin) = add::resolve_git_pack_gated(ctx, &newest)?;
        // Never downgrade: if the newest version tag is not newer than the
        // installed one, re-resolve at the installed tag (content-diff still
        // catches a moved tag).
        let newer = match (
            crate::provider::gitpack::version_key(&resolved.tag),
            crate::provider::gitpack::version_key(&current_tag),
        ) {
            (Some(n), Some(c)) => n > c,
            _ => false,
        };
        if !newer && resolved.tag != current_tag {
            (resolved, origin) = add::resolve_git_pack_gated(
                ctx,
                &crate::provider::gitpack::GitPackRef {
                    tag: Some(current_tag.clone()),
                    ..git_ref.clone()
                },
            )?;
        }
        if resolved.tag != current_tag {
            println!(
                "{} '{pack}': {} -> {} ({})",
                "newer tag".cyan(),
                current_tag,
                resolved.tag.bold(),
                &resolved.commit[..resolved.commit.len().min(12)]
            );
        }
        let spec = resolved.spec.clone();
        return upgrade_pack(
            ctx,
            manifest_dir,
            pack,
            recipe,
            &resolved.candidate,
            &spec,
            args,
            &origin,
        );
    }

    let id = source.strip_prefix("catalog:").ok_or_else(|| {
        anyhow!(
            "pack '{pack}' source '{source}' is not a catalog or git source; it cannot be upgraded"
        )
    })?;
    let candidate = provider::resolve(id)
        .ok_or_else(|| anyhow!("pack '{pack}' source id '{id}' is no longer in the catalog"))?;
    let CandidateKind::Pack(spec) = &candidate.kind else {
        anyhow::bail!("catalog id '{id}' is no longer a pack");
    };
    let origin = add::PackOrigin {
        assets: add::AssetSource::Embedded,
        source: format!("catalog:{id}"),
        version: recipe.version.clone(),
        rev: None,
    };
    upgrade_pack(
        ctx,
        manifest_dir,
        pack,
        recipe,
        &candidate,
        spec,
        args,
        &origin,
    )
}

/// What re-resolving the pack changed, relative to the installed ledger + disk.
#[derive(Default)]
struct PackDiff {
    server_changed: bool,
    skills_added: Vec<String>,
    skills_removed: Vec<String>,
    skills_changed: Vec<String>,
    instr_added: Vec<String>,
    instr_removed: Vec<String>,
    instr_body_changed: Vec<String>,
}

impl PackDiff {
    fn is_empty(&self) -> bool {
        !self.server_changed
            && self.skills_added.is_empty()
            && self.skills_removed.is_empty()
            && self.skills_changed.is_empty()
            && self.instr_added.is_empty()
            && self.instr_removed.is_empty()
            && self.instr_body_changed.is_empty()
    }
    /// Instruction prose that is added or rewritten steers the agent and needs
    /// explicit acceptance. Removing prose is safe and does not.
    fn has_steering(&self) -> bool {
        !self.instr_added.is_empty() || !self.instr_body_changed.is_empty()
    }
}

#[allow(clippy::too_many_arguments)]
fn upgrade_pack(
    ctx: &super::Context,
    manifest_dir: Option<&Path>,
    pack: &str,
    recipe: &PluginRecipe,
    candidate: &Candidate,
    spec: &PackSpec,
    args: &UpgradeArgs,
    origin: &add::PackOrigin,
) -> Result<()> {
    let manifest = &ctx.loaded.manifest;

    // Re-gate the freshly resolved spec on [policy] BEFORE planning any write, so
    // an upgrade can't smuggle in a now-forbidden member or disallowed source.
    add::check_pack_policy(manifest, pack, spec, &origin.source)?;

    // Instructions are part of the desired state if the pack already has them
    // installed, or the user opts in now with --with-instructions.
    let want_instructions = !recipe.instructions.is_empty() || args.with_instructions;

    let diff_result = diff_pack(
        ctx,
        pack,
        recipe,
        candidate,
        spec,
        want_instructions,
        origin,
    )?;

    if diff_result.is_empty() {
        println!("{} pack '{}' already current.", "✓".green(), pack);
        return Ok(());
    }

    // Build the post-upgrade manifest text (used for the diff preview and write).
    let original = fs::read_to_string(&ctx.loaded.manifest_path)
        .with_context(|| format!("reading {}", ctx.loaded.manifest_path.display()))?;
    let new_text = build_upgraded_manifest(
        &original,
        pack,
        recipe,
        candidate,
        spec,
        want_instructions,
        origin,
    )?;

    println!(
        "{} upgrade pack '{}' in {}",
        "↑".cyan(),
        pack.bold(),
        ctx.loaded.manifest_path.display()
    );
    print!(
        "{}",
        diff::render(&original, &new_text)
            .lines()
            .map(|l| format!("  {l}\n"))
            .collect::<String>()
    );
    print_change_summary(&diff_result);

    // Steering gate: instruction prose changed/added but the user hasn't accepted
    // it. Refuse the whole upgrade (atomic) — nothing is written.
    let accepted = args.yes || args.with_instructions;
    if diff_result.has_steering() && !accepted {
        println!(
            "\n{} house rules changed — re-run with {} (or {}) to accept them. Nothing written.",
            "↳".cyan(),
            "--with-instructions".bold(),
            "--yes".bold()
        );
        return Ok(());
    }

    if !args.write {
        println!(
            "\nDry run. Re-run with {} to apply the upgrade.",
            "--write".bold()
        );
        return Ok(());
    }

    apply_upgrade(
        ctx,
        pack,
        recipe,
        spec,
        want_instructions,
        &original,
        &new_text,
        origin,
    )?;
    repin_lock(ctx, manifest_dir, recipe, spec)?;

    println!("{} upgraded pack '{}'.", "✓".green(), pack);
    Ok(())
}

/// Compute the member-level diff between the installed ledger/on-disk state and
/// the re-resolved spec.
#[allow(clippy::too_many_arguments)]
fn diff_pack(
    ctx: &super::Context,
    pack: &str,
    recipe: &PluginRecipe,
    candidate: &Candidate,
    spec: &PackSpec,
    want_instructions: bool,
    origin: &add::PackOrigin,
) -> Result<PackDiff> {
    let manifest = &ctx.loaded.manifest;
    let mut d = PackDiff::default();

    // Server: compare desired vs current (transport/url/header-keys — secret
    // values are ${REF}s, never literals).
    let desired_server = spec.server.as_ref().map(|_| candidate.to_server());
    let current_server = manifest.servers.get(pack).cloned();
    d.server_changed = desired_server != current_server;

    // Skills: name set-diff, plus content re-checksum for the common set.
    let desired_skills: Vec<String> = spec.skills.iter().map(|s| s.name.clone()).collect();
    for s in &spec.skills {
        if !recipe.skills.contains(&s.name) {
            d.skills_added.push(s.name.clone());
        } else if let Some(asset) = &s.path {
            if skill_content_changed(ctx, &s.name, asset, &origin.assets)? {
                d.skills_changed.push(s.name.clone());
            }
        }
    }
    for name in &recipe.skills {
        if !desired_skills.contains(name) {
            d.skills_removed.push(name.clone());
        }
    }

    // Instructions: only meaningful when instructions are part of desired state.
    let desired_instr: Vec<String> = if want_instructions {
        spec.instructions.iter().map(|i| i.name.clone()).collect()
    } else {
        Vec::new()
    };
    if want_instructions {
        for instr in &spec.instructions {
            let body = add::stamped_instruction_from(pack, instr, &origin.assets)?;
            let on_disk = ctx.dir.join(format!("instructions/{}.md", instr.name));
            if !recipe.instructions.contains(&instr.name) {
                d.instr_added.push(instr.name.clone());
            } else {
                let current = fs::read_to_string(&on_disk).unwrap_or_default();
                if current != body {
                    d.instr_body_changed.push(instr.name.clone());
                }
            }
        }
    }
    for name in &recipe.instructions {
        if !desired_instr.contains(name) {
            d.instr_removed.push(name.clone());
        }
    }

    Ok(d)
}

/// Has the pack's asset for `skill` diverged from what's installed on disk?
fn skill_content_changed(
    ctx: &super::Context,
    skill: &str,
    asset: &str,
    assets: &add::AssetSource,
) -> Result<bool> {
    let on_disk = ctx
        .loaded
        .manifest
        .skills
        .get(skill)
        .and_then(|s| s.path.as_deref());
    let Some(on_disk) = on_disk else {
        return Ok(true);
    };
    let installed = ctx.dir.join(on_disk.trim_start_matches("./"));
    if !installed.exists() {
        return Ok(true);
    }
    // Extract the pack asset to a scratch dir and compare content digests.
    let tmp = ctx.dir.join(format!(".agentstack-cmp-{}", sanitize(skill)));
    let _ = fs::remove_dir_all(&tmp);
    let extracted = assets.extract_dir(asset, &tmp);
    let changed = match extracted {
        Ok(()) => store::dir_digest(&tmp).ok() != store::dir_digest(&installed).ok(),
        Err(_) => true,
    };
    let _ = fs::remove_dir_all(&tmp);
    Ok(changed)
}

/// Rebuild the manifest with the re-resolved members: drop every current member,
/// then re-add the desired server + skills + (opt-in) instructions + ledger.
#[allow(clippy::too_many_arguments)]
fn build_upgraded_manifest(
    original: &str,
    pack: &str,
    recipe: &PluginRecipe,
    candidate: &Candidate,
    spec: &PackSpec,
    want_instructions: bool,
    origin: &add::PackOrigin,
) -> Result<String> {
    let mut text = original.to_string();

    // 1. Remove the current members (and the ledger).
    for server in &recipe.servers {
        text = remove::remove_entry(&text, "servers", server)?;
    }
    for skill in &recipe.skills {
        text = remove::remove_entry(&text, "skills", skill)?;
    }
    for instr in &recipe.instructions {
        text = remove::remove_entry(&text, "instructions", instr)?;
    }
    text = remove::remove_entry(&text, "plugins", pack)?;

    // 2. Re-add the desired members, recording a fresh ledger.
    let mut ledger = PluginRecipe {
        kind: Some("pack".into()),
        rev: origin.rev.clone(),
        source: Some(origin.source.clone()),
        version: origin.version.clone(),
        description: candidate.description.clone(),
        display: recipe.display.clone(),
        category: None,
        targets: spec.targets.clone(),
        default_enabled: None,
        servers: Vec::new(),
        skills: Vec::new(),
        hooks: recipe.hooks.clone(),
        instructions: Vec::new(),
        homepage: recipe.homepage.clone(),
        repository: None,
        license: None,
        author: None,
    };

    if spec.server.is_some() {
        let server = candidate.to_server();
        text = add::build_manifest_with(
            &text,
            "servers",
            pack,
            &serde_json::to_value(&server)?,
            None,
        )?;
        ledger.servers.push(pack.to_string());
    }

    for skill in &spec.skills {
        let asset = skill
            .path
            .as_ref()
            .ok_or_else(|| anyhow!("pack skill '{}' has no bundled path", skill.name))?;
        let entry = Skill {
            path: Some(format!("./{asset}")),
            git: None,
            rev: None,
            subpath: None,
        };
        text = add::build_manifest_with(
            &text,
            "skills",
            &skill.name,
            &serde_json::to_value(&entry)?,
            None,
        )?;
        ledger.skills.push(skill.name.clone());
    }

    if want_instructions {
        for instr in &spec.instructions {
            let entry = Instruction {
                path: format!("./instructions/{}.md", instr.name),
                targets: vec!["*".into()],
                from_user_layer: false,
            };
            text = add::build_manifest_with(
                &text,
                "instructions",
                &instr.name,
                &serde_json::to_value(&entry)?,
                None,
            )?;
            ledger.instructions.push(instr.name.clone());
        }
    }

    text = add::build_manifest_with(
        &text,
        "plugins",
        pack,
        &serde_json::to_value(&ledger)?,
        None,
    )?;
    Ok(text)
}

/// Apply the upgrade atomically: back up the pack's current files, write the new
/// manifest, swap the on-disk assets, and on any failure restore everything.
#[allow(clippy::too_many_arguments)]
fn apply_upgrade(
    ctx: &super::Context,
    pack: &str,
    recipe: &PluginRecipe,
    spec: &PackSpec,
    want_instructions: bool,
    original: &str,
    new_text: &str,
    origin: &add::PackOrigin,
) -> Result<()> {
    let manifest = &ctx.loaded.manifest;

    // Old dirs owned by this pack — contained under the manifest dir only, so a
    // hand-edited/corrupt ledger pointing at an absolute or `../` path can never
    // make us delete outside the managed tree (mirrors the instruction guard).
    let old_skill_dirs: Vec<PathBuf> = remove::safe_skill_dirs(manifest, ctx, recipe);
    // Only delete instruction files we wrote (vendor-marker + containment guard).
    let old_instr_files = remove::safe_instruction_files(manifest, ctx, recipe, pack);

    // Desired on-disk destinations.
    let new_skill_assets: Vec<String> = spec.skills.iter().filter_map(|s| s.path.clone()).collect();
    let new_instr: Vec<(PathBuf, String)> = if want_instructions {
        spec.instructions
            .iter()
            .map(|i| {
                let body = add::stamped_instruction_from(pack, i, &origin.assets)?;
                Ok((ctx.dir.join(format!("instructions/{}.md", i.name)), body))
            })
            .collect::<Result<_>>()?
    } else {
        Vec::new()
    };

    // Back up the manifest + every old pack file so a mid-apply failure reverts.
    let backup_root = ctx
        .dir
        .join(format!(".agentstack-upgrade-{}.bak", sanitize(pack)));
    let _ = fs::remove_dir_all(&backup_root);
    fs::create_dir_all(&backup_root)
        .with_context(|| format!("creating {}", backup_root.display()))?;
    let cleanup = |root: &Path| {
        let _ = fs::remove_dir_all(root);
    };

    let mut backups: Vec<(PathBuf, PathBuf, bool)> = Vec::new(); // (orig, backup, is_dir)
    fs::write(backup_root.join("manifest.toml"), original)
        .with_context(|| "backing up manifest".to_string())?;
    for (i, dir) in old_skill_dirs.iter().enumerate() {
        if dir.exists() {
            let dst = backup_root.join(format!("skill-{i}"));
            crate::util::fsx::copy_dir_all(dir, &dst)?;
            backups.push((dir.clone(), dst, true));
        }
    }
    for (i, file) in old_instr_files.iter().enumerate() {
        if file.exists() {
            let dst = backup_root.join(format!("instr-{i}.md"));
            fs::copy(file, &dst).with_context(|| format!("backing up {}", file.display()))?;
            backups.push((file.clone(), dst, false));
        }
    }

    // Mutate. On the first error, restore from the backups and bail.
    let result = (|| -> Result<()> {
        atomic::write(&ctx.loaded.manifest_path, new_text)
            .with_context(|| format!("writing {}", ctx.loaded.manifest_path.display()))?;
        for dir in &old_skill_dirs {
            if dir.exists() {
                fs::remove_dir_all(dir).with_context(|| format!("removing {}", dir.display()))?;
            }
        }
        for file in &old_instr_files {
            let _ = fs::remove_file(file);
        }
        for asset in &new_skill_assets {
            let out = ctx.dir.join(asset);
            if out.exists() {
                fs::remove_dir_all(&out).ok();
            }
            origin
                .assets
                .extract_dir(asset, &out)
                .with_context(|| format!("extracting skill asset '{asset}'"))?;
        }
        for (out, body) in &new_instr {
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            fs::write(out, body).with_context(|| format!("writing {}", out.display()))?;
        }
        Ok(())
    })();

    if let Err(e) = result {
        // Roll back: clear whatever the failed apply produced, restore manifest
        // and every backed-up file/dir.
        for asset in &new_skill_assets {
            let _ = fs::remove_dir_all(ctx.dir.join(asset));
        }
        for (out, _) in &new_instr {
            let _ = fs::remove_file(out);
        }
        // Restoring the manifest is the load-bearing rollback step — unlike the
        // best-effort file cleanup around it, a silent failure here leaves the
        // user with a possibly-corrupt manifest and no signal, so surface it.
        if let Err(restore_err) = atomic::write(&ctx.loaded.manifest_path, original) {
            eprintln!(
                "warning: rollback could not restore {} ({restore_err:#}); \
                 the manifest may be inconsistent — check it before re-running",
                ctx.loaded.manifest_path.display()
            );
        }
        for (orig, backup, is_dir) in &backups {
            if *is_dir {
                let _ = fs::remove_dir_all(orig);
                let _ = crate::util::fsx::copy_dir_all(backup, orig);
            } else {
                if let Some(parent) = orig.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                let _ = fs::copy(backup, orig);
            }
        }
        cleanup(&backup_root);
        return Err(e).context("upgrade rolled back");
    }

    cleanup(&backup_root);
    Ok(())
}

/// Re-pin the lockfile for the pack's skills after an upgrade.
fn repin_lock(
    ctx: &super::Context,
    manifest_dir: Option<&Path>,
    recipe: &PluginRecipe,
    spec: &PackSpec,
) -> Result<()> {
    let mut lock = Lock::load(&ctx.dir)?;
    let store = Store::default_store();
    let desired: Vec<String> = spec.skills.iter().map(|s| s.name.clone()).collect();

    for skill in &spec.skills {
        let Some(asset) = &skill.path else { continue };
        let entry = Skill {
            path: Some(format!("./{asset}")),
            git: None,
            rev: None,
            subpath: None,
        };
        let resolved = store
            .resolve(&entry, &ctx.dir, None)
            .with_context(|| format!("re-pinning skill '{}'", skill.name))?;
        lock.upsert(install::locked_entry(&skill.name, &entry, &resolved)?);
    }
    // Drop lock rows for skills the upgrade removed.
    for old in &recipe.skills {
        if !desired.contains(old) {
            lock.remove(old);
        }
    }
    lock.save(&ctx.dir)?;
    let _ = manifest_dir; // reserved for future re-resolve via the loaded context
    Ok(())
}

fn print_change_summary(d: &PackDiff) {
    let line = |label: &str, items: &[String]| {
        if !items.is_empty() {
            println!("  {} {}: {}", "•".cyan(), label, items.join(", "));
        }
    };
    if d.server_changed {
        println!("  {} server changed", "•".cyan());
    }
    line("skills added", &d.skills_added);
    line("skills changed", &d.skills_changed);
    line("skills removed", &d.skills_removed);
    line("house rules added", &d.instr_added);
    line("house rules changed", &d.instr_body_changed);
    line("house rules removed", &d.instr_removed);
}

/// Filesystem-safe slug for scratch/backup directory names.
fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}
