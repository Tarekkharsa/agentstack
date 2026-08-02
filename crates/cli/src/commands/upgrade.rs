//! `agentstack lock --upgrade <vendor>` — re-resolve an installed vendor pack from its
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
//! W3 widened what "atomic" covers. Per
//! `docs/design/automatic-delivery.md` §"Mixed-lane upgrades are
//! transactional", a package whose members span both delivery lanes "updates
//! the lock **and** re-renders the managed instruction region, or it does
//! neither" — so the lock re-pin (skills *and* instruction pins) and the
//! managed-region re-render now happen INSIDE the same backup/rollback
//! envelope as the manifest and asset writes, not as separate steps after it.
//! The re-render is deliberately conservative: it refreshes only instruction
//! files that already carry the managed region, because a package upgrade must
//! never be the reason a file appears in a project.
//!
//! The result report names each lane on its own line, and never describes an
//! instruction as going live "via gateway" — it went to a file, and the
//! sentence says which one.
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
use crate::manifest::{Instruction, PackInstall, Skill};
use crate::provider::{self, Candidate, CandidateKind, PackSpec};
use crate::render::instructions::{manages_file, plan_instructions};
use crate::render::resolve_targets;
use crate::scope::Scope;
use crate::store::{self, Store};
use crate::util::{atomic, diff};

pub fn run(args: &UpgradeArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let ctx = super::load(manifest_dir)?;
    let manifest = &ctx.loaded.manifest;

    let targets: Vec<String> = if args.all {
        manifest.packs.keys().cloned().collect()
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
                "'{name}' is not an installed vendor pack (no [packs.{name}] ledger). \
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
        anyhow::bail!("{} failed to upgrade", super::count(failures, "pack"));
    }
    Ok(())
}

/// Re-resolve one pack from its ledger source and hand off to `upgrade_pack`.
fn upgrade_one(
    ctx: &super::Context,
    manifest_dir: Option<&Path>,
    pack: &str,
    recipe: &PackInstall,
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
    recipe: &PackInstall,
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

    // One transaction, both lanes (design §"Mixed-lane upgrades are
    // transactional"): the manifest write, the asset swap, the lock re-pin,
    // and the managed-region re-render either all land or all revert. The lock
    // and the region used to be separate steps AFTER this call, which is
    // exactly the state the contract forbids — a lock that moved for a
    // manifest that rolled back.
    let outcome = apply_upgrade(
        ctx,
        manifest_dir,
        pack,
        recipe,
        spec,
        want_instructions,
        &original,
        &new_text,
        origin,
    )?;

    print_result_report(ctx, pack, recipe, origin, &diff_result, &outcome);
    Ok(())
}

/// Compute the member-level diff between the installed ledger/on-disk state and
/// the re-resolved spec.
#[allow(clippy::too_many_arguments)]
fn diff_pack(
    ctx: &super::Context,
    pack: &str,
    recipe: &PackInstall,
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
    recipe: &PackInstall,
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
    text = remove::remove_entry(&text, "packs", pack)?;

    // 2. Re-add the desired members, recording a fresh ledger.
    let mut ledger = PackInstall {
        rev: origin.rev.clone(),
        source: Some(origin.source.clone()),
        version: origin.version.clone(),
        description: candidate.description.clone(),
        targets: spec.targets.clone(),
        servers: Vec::new(),
        skills: Vec::new(),
        hooks: recipe.hooks.clone(),
        instructions: Vec::new(),
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

    text = add::build_manifest_with(&text, "packs", pack, &serde_json::to_value(&ledger)?, None)?;
    Ok(text)
}

/// What the applied upgrade actually did, per delivery lane. Only facts the
/// report is allowed to state — every field is filled in from a completed
/// write, never from the plan.
#[derive(Default)]
struct LaneOutcome {
    /// Skills re-pinned in the lock (dynamic lane).
    skills_repinned: usize,
    /// Instruction fragments re-pinned in the lock (rendered lane).
    instr_pinned: usize,
    /// Instruction **fragment source** files this upgrade wrote, under the
    /// manifest dir.
    fragments: Vec<PathBuf>,
    /// Instruction files (`CLAUDE.md` / `AGENTS.md`) whose managed region was
    /// rewritten. Empty means **no file was written**, which the report says
    /// out loud rather than implying a write happened.
    rendered: Vec<PathBuf>,
}

/// Apply the upgrade atomically: back up everything the run can touch, write
/// the new manifest, swap the on-disk assets, re-pin the lock, re-render the
/// managed instruction regions — and on any failure restore all of it.
///
/// The backup set is the transaction's scope, so it is worth naming: the
/// manifest, the pack's old skill dirs, the pack's old instruction fragment
/// files, **the lockfile** (or its absence), and **every instruction file that
/// already carries a managed region**. The last two are the W3 additions; the
/// first three were already covered.
#[allow(clippy::too_many_arguments)]
fn apply_upgrade(
    ctx: &super::Context,
    manifest_dir: Option<&Path>,
    pack: &str,
    recipe: &PackInstall,
    spec: &PackSpec,
    want_instructions: bool,
    original: &str,
    new_text: &str,
    origin: &add::PackOrigin,
) -> Result<LaneOutcome> {
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

    // The lock joins the transaction. `None` records "there was no lock here",
    // so a rollback DELETES the one this run created rather than leaving pins
    // for a manifest that no longer exists — held in memory because a lockfile
    // is small and the restore must not itself depend on the backup dir.
    let lock_path = Lock::path(&ctx.dir);
    let lock_before: Option<Vec<u8>> = fs::read(&lock_path).ok();

    // Instruction files that ALREADY carry a managed region — the only ones
    // the re-render below may touch. Backed up by content, across every
    // registered adapter rather than only the resolved targets, so the backup
    // set is a superset of the write set by construction: a file can never be
    // rendered without a way back.
    let region_before = managed_region_snapshot(ctx);

    // Mutate. On the first error, restore from the backups and bail.
    let result = (|| -> Result<LaneOutcome> {
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

        // Everything below reads the manifest we just wrote, so reload through
        // the ordinary command loader rather than parsing the text by hand:
        // it applies the same machine-layer instruction merge every other
        // command sees. Compiling the region from an unmerged manifest would
        // silently drop the user's personal fragments out of a global-scope
        // CLAUDE.md.
        let fresh = super::load(manifest_dir).context("re-reading the upgraded manifest")?;
        let mut outcome = LaneOutcome {
            fragments: new_instr.iter().map(|(p, _)| p.clone()).collect(),
            ..LaneOutcome::default()
        };
        outcome.skills_repinned = repin_lock(&fresh, recipe, spec)?;
        // Instruction pins go through `lock.rs`'s recorder — the single
        // pinning path — rather than a second one here. Strict, because a pack
        // upgrade that drops an instruction must prune its pin too; the
        // rollback below is what makes strictness safe.
        if fresh
            .loaded
            .manifest
            .instructions
            .values()
            .any(|i| !i.from_user_layer)
        {
            outcome.instr_pinned =
                super::lock::record_instruction_pins(&fresh.dir, &fresh.loaded.manifest, true)?;
        }
        outcome.rendered = rerender_managed_regions(&fresh)?;
        Ok(outcome)
    })();

    let outcome = match result {
        Ok(outcome) => outcome,
        Err(e) => {
            // Roll back: clear whatever the failed apply produced, restore
            // manifest and every backed-up file/dir.
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
            // The lock is restored to its exact prior bytes, or removed when
            // there was none — "or it does neither" has to include the lock,
            // otherwise a rolled-back upgrade still moved a pin.
            match &lock_before {
                Some(bytes) => {
                    let _ = fs::write(&lock_path, bytes);
                }
                None => {
                    let _ = fs::remove_file(&lock_path);
                }
            }
            // Managed regions: exact prior bytes. These files are never
            // created by the upgrade, so restoring by write is always exact.
            for (path, before) in &region_before {
                let _ = fs::write(path, before);
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
    };

    cleanup(&backup_root);
    Ok(outcome)
}

/// The current bytes of every instruction file that carries agentstack's
/// managed region, across every registered adapter at this manifest's default
/// scope. Read before the mutation so the rollback can put them back exactly;
/// a file with no region is not in here because it is not ours to write.
fn managed_region_snapshot(ctx: &super::Context) -> Vec<(PathBuf, Vec<u8>)> {
    let scope = Scope::default_for(&ctx.dir);
    let mut out: Vec<(PathBuf, Vec<u8>)> = Vec::new();
    for desc in ctx.registry.iter() {
        let Some(spec) = desc.instructions.as_ref() else {
            continue;
        };
        let Some(path) = spec.path_for(scope, &ctx.dir) else {
            continue;
        };
        // Several adapters legitimately share one file (AGENTS.md); back it
        // up once.
        if out.iter().any(|(p, _)| *p == path) {
            continue;
        }
        if manages_file(&path) {
            if let Ok(bytes) = fs::read(&path) {
                out.push((path, bytes));
            }
        }
    }
    out
}

/// Re-render the managed instruction region for every target that **already
/// has one on disk**, and report which files were written.
///
/// The scoping rule is conservative on purpose: an upgrade may refresh a
/// region a human already accepted, but it must never be the reason an
/// instruction file — or a managed region inside one — first appears in a
/// project. `manages_file` is that whole rule, in one predicate; a target
/// without the marker is skipped, and the report says plainly that nothing was
/// written and which command would render it.
///
/// Region merging is `render::merge_md`'s job via `plan_instructions`, not
/// reimplemented here: prose outside the markers must survive untouched.
fn rerender_managed_regions(ctx: &super::Context) -> Result<Vec<PathBuf>> {
    let manifest = &ctx.loaded.manifest;
    // W5: a package's instruction members compile into the same region, so an
    // upgrade in a project whose house rules arrive only through a package
    // still has a region to refresh.
    let pinned = Lock::load(&ctx.dir).unwrap_or_default();
    let packages = crate::package::effective_members(&pinned);
    if manifest.instructions.is_empty() && packages.iter().all(|p| p.members.is_empty()) {
        return Ok(Vec::new());
    }
    let scope = Scope::default_for(&ctx.dir);
    let target_ids = resolve_targets(manifest, &ctx.registry, &[], &ctx.dir)?;
    let mut written = Vec::new();
    for id in &target_ids {
        let Some(desc) = ctx.registry.get(id) else {
            continue;
        };
        let Some(plan) = plan_instructions(manifest, desc, scope, &ctx.dir, packages) else {
            continue;
        };
        if !manages_file(&plan.path) {
            continue;
        }
        // A missing fragment source would silently delete that fragment's
        // compiled prose from the region — the same block `instructions
        // --write` applies. The pack's own fragments were just written, so in
        // practice this only guards a machine-layer fragment that vanished.
        if !plan.missing.is_empty() || !plan.changed() {
            continue;
        }
        plan.write()
            .with_context(|| format!("re-rendering {}", plan.path.display()))?;
        written.push(plan.path.clone());
    }
    Ok(written)
}

/// Re-pin the lockfile for the pack's skills after an upgrade; returns how many
/// skills were pinned. Called from inside the transaction with a context
/// reloaded from the upgraded manifest.
fn repin_lock(ctx: &super::Context, recipe: &PackInstall, spec: &PackSpec) -> Result<usize> {
    let mut lock = Lock::load(&ctx.dir)?;
    let store = Store::default_store();
    let desired: Vec<String> = spec.skills.iter().map(|s| s.name.clone()).collect();

    let mut pinned = 0usize;
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
        pinned += 1;
    }
    // Drop lock rows for skills the upgrade removed.
    for old in &recipe.skills {
        if !desired.contains(old) {
            lock.remove(old);
        }
    }
    lock.save(&ctx.dir)?;
    Ok(pinned)
}

/// The pre-write **plan**, already partitioned by delivery lane so the preview
/// and the result speak the same vocabulary. Nothing here is past tense —
/// nothing has been written yet, and the steering gate below may still refuse
/// the whole upgrade.
///
/// Skills and the server are the dynamic lane; instructions are the rendered
/// lane. A lane with no members gets no line at all rather than an empty one.
fn print_change_summary(d: &PackDiff) {
    let members = |pairs: &[(&str, &[String])]| -> Vec<String> {
        pairs
            .iter()
            .filter(|(_, items)| !items.is_empty())
            .map(|(label, items)| format!("{label}: {}", items.join(", ")))
            .collect()
    };

    let mut dynamic = members(&[
        ("skills added", &d.skills_added),
        ("skills changed", &d.skills_changed),
        ("skills removed", &d.skills_removed),
    ]);
    if d.server_changed {
        dynamic.push("server definition changed".to_string());
    }
    if !dynamic.is_empty() {
        println!("  {} {}", "dynamic lane:".cyan(), dynamic.join(" · "));
    }

    let rendered = members(&[
        ("house rules added", &d.instr_added),
        ("house rules changed", &d.instr_body_changed),
        ("house rules removed", &d.instr_removed),
    ]);
    if !rendered.is_empty() {
        println!("  {} {}", "rendered lane:".cyan(), rendered.join(" · "));
    }
}

/// The per-lane **result** report (design §"Mixed-lane upgrades are
/// transactional, and report per lane"). Two rules here are binding copy, not
/// style:
///
/// 1. The lanes get separate lines. Never one blended sentence — that is how a
///    user comes to believe no file was touched when one was.
/// 2. An instruction is never described as going live "via gateway". It went
///    to a file, and the sentence names which file.
///
/// A lane with nothing in it prints nothing; a rendered lane that wrote no
/// instruction file says so out loud instead of implying a write.
fn print_result_report(
    ctx: &super::Context,
    pack: &str,
    recipe: &PackInstall,
    origin: &add::PackOrigin,
    d: &PackDiff,
    out: &LaneOutcome,
) {
    if recipe.version != origin.version {
        println!(
            "{} upgraded {pack} {} → {}",
            "✓".green(),
            recipe.version,
            origin.version.bold()
        );
    } else {
        println!("{} upgraded pack '{pack}'.", "✓".green());
    }
    if let Some(line) = dynamic_lane_result(d, out) {
        println!("  {line}");
    }
    for line in rendered_lane_result(&ctx.dir, d, out) {
        println!("  {line}");
    }
}

/// The dynamic-lane result line, or `None` when the lane had no members.
///
/// Deliberately NOT the contract's literal "live via gateway now": whether
/// these bytes are served over the gateway depends on the project's delivery
/// mode, and today's default still renders statically. Invariant 8 ("claims
/// match enforcement") outranks the example's wording, so the line states the
/// thing that is true in every mode — the lock now names the new bytes.
fn dynamic_lane_result(d: &PackDiff, out: &LaneOutcome) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if out.skills_repinned > 0 {
        parts.push(format!(
            "{} re-pinned",
            super::count(out.skills_repinned, "skill")
        ));
    }
    if !d.skills_removed.is_empty() {
        parts.push(format!(
            "{} removed",
            super::count(d.skills_removed.len(), "skill")
        ));
    }
    if d.server_changed {
        parts.push("server definition changed".to_string());
    }
    if parts.is_empty() {
        return None;
    }
    Some(format!(
        "dynamic lane: {} — the lock now names the new bytes",
        parts.join(", ")
    ))
}

/// The rendered-lane result lines, naming **what was written and where**.
/// Empty when the pack has no rendered members at all.
fn rendered_lane_result(dir: &Path, d: &PackDiff, out: &LaneOutcome) -> Vec<String> {
    if out.fragments.is_empty() && d.instr_removed.is_empty() {
        return Vec::new();
    }
    let list = |paths: &[PathBuf]| -> String {
        paths
            .iter()
            .map(|p| rel_display(dir, p))
            .collect::<Vec<_>>()
            .join(", ")
    };

    let mut lines = Vec::new();
    if out.fragments.is_empty() {
        // Only removals: the fragments are gone from the manifest and disk,
        // and no instruction file gained anything.
        lines.push(format!(
            "rendered lane: {} removed from this project's fragments; no instruction file gained \
             new prose",
            super::count(d.instr_removed.len(), "house rule")
        ));
        return lines;
    }

    let wrote = format!(
        "{} written to {}",
        super::count(out.fragments.len(), "house-rule fragment"),
        list(&out.fragments)
    );
    if out.rendered.is_empty() {
        // The honest negative: fragments moved, but nothing rendered. Say it,
        // and name the command that would.
        lines.push(format!(
            "rendered lane: {wrote}; no instruction file here carries agentstack's managed region, \
             so no file was rendered"
        ));
        lines.push(
            "  ↳ `agentstack instructions --write` renders the region into CLAUDE.md / AGENTS.md"
                .to_string(),
        );
    } else {
        lines.push(format!(
            "rendered lane: {wrote}; managed region updated in {}",
            list(&out.rendered)
        ));
    }
    if out.instr_pinned > 0 {
        lines.push(format!(
            "  ↳ {} pinned in the lock",
            super::count(out.instr_pinned, "instruction fragment")
        ));
    }
    lines
}

/// A path shown relative to the manifest dir when it lives under it, absolute
/// otherwise — short enough to read, never ambiguous about which file.
fn rel_display(dir: &Path, path: &Path) -> String {
    path.strip_prefix(dir).unwrap_or(path).display().to_string()
}

/// Filesystem-safe slug for scratch/backup directory names.
fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}
