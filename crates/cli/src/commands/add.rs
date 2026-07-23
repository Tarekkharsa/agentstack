//! `agentstack add server|skill` — add a capability to the manifest. Flag-driven
//! (scriptable, agent-operable), writing into `agentstack.toml` via the TOML
//! merger so comments/formatting survive.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use indexmap::IndexMap;
use owo_colors::OwoColorize;
use serde_json::Value;
use toml_edit::{Array, DocumentMut};

use crate::cli::{AddArgs, AddFromArgs, AddKind, AddServerArgs, AddSkillArgs};
use crate::manifest::{Manifest, PackInstall, Server, ServerType, Skill};
use crate::provider::{self, Candidate, CandidateKind, InstrRef, PackSpec, SkillRef};
use crate::render::merge_toml;
use crate::scope::Scope;
use crate::util::diff;

/// Provenance header prepended to a pack's extracted instruction file so its
/// origin survives into the merged CLAUDE.md/AGENTS.md region — and so `remove`
/// can tell a pack-written file from a user-authored one before deleting it.
fn vendor_marker(pack: &str) -> String {
    format!("<!-- agentstack:vendor {pack} (unofficial) -->")
}

pub fn run(args: &AddArgs, manifest_dir: Option<&Path>) -> Result<()> {
    match &args.kind {
        AddKind::From(a) => add_from(a, manifest_dir),
        AddKind::Server(a) => add_server(a, manifest_dir),
        AddKind::Skill(a) => add_skill(a, manifest_dir),
    }
}

/// `agentstack set …` — the idempotent counterpart of `add`: create-or-update
/// in place, so a validation error's fix command works whether or not the
/// entry already exists.
pub fn run_set(args: &crate::cli::SetArgs, manifest_dir: Option<&Path>) -> Result<()> {
    match &args.kind {
        crate::cli::SetKind::Server(a) => upsert_server(a, manifest_dir, true),
    }
}

fn add_from(a: &AddFromArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let ctx = super::load(manifest_dir)?;

    // `git:<url>[@<tag>][#subdir]` — a versioned pack from any git host.
    if let Some(git_ref) = crate::provider::gitpack::GitPackRef::parse(&a.id) {
        return add_git_pack(a, &ctx, &git_ref);
    }

    let candidate = provider::resolve(&a.id).with_context(|| {
        format!(
            "no capability '{}' in the catalog or registry — run `agentstack search {}` to find one",
            a.id, a.id
        )
    })?;
    // Candidate name/id can be remote (registry id is the raw API string;
    // pack names come from pack.toml) and print on a DRY RUN, before any
    // gate — sanitize at this first human-read surface (design §A.2 #8).
    println!(
        "{} {} ({}) — {}",
        "found".green(),
        crate::text::sanitize_line(&candidate.name).bold(),
        candidate.source,
        crate::text::sanitize_line(&candidate.id)
    );
    match &candidate.kind {
        CandidateKind::Server(_) => add_from_server(a, &ctx, &candidate),
        CandidateKind::Skill(skill) => add_from_skill(a, &ctx, &candidate, skill),
        CandidateKind::Pack(spec) => add_pack(
            a,
            &ctx,
            &candidate,
            spec,
            &PackOrigin::catalog(&candidate.id),
        ),
        CandidateKind::Hook(_) => add_from_hook(a, &ctx, &candidate),
        CandidateKind::Extension(ext) => anyhow::bail!(
            "'{}' is a native extension — executable in-process code that `add from` does not \
             install. Reference it in [extensions.{}] with target = \"{}\", then run \
             `agentstack lock` so the code is pinned and re-gates trust.",
            candidate.name,
            candidate.name,
            ext.target
        ),
    }
}

/// Install a pack from a git repo at a version tag. Policy gates the source
/// *before* anything is fetched; the clone's skill/instruction content passes
/// the same scan gate as `install` before any manifest planning happens.
fn add_git_pack(
    a: &AddFromArgs,
    ctx: &super::Context,
    git_ref: &crate::provider::gitpack::GitPackRef,
) -> Result<()> {
    let (resolved, origin) = resolve_git_pack_gated(ctx, git_ref)?;
    println!(
        "{} {} (git) — {} at {} ({})",
        "found".green(),
        crate::text::sanitize_line(&resolved.candidate.name).bold(),
        crate::text::sanitize_line(&git_ref.url),
        crate::text::sanitize_line(&resolved.tag).bold(),
        &resolved.commit[..resolved.commit.len().min(12)],
    );
    add_pack(a, ctx, &resolved.candidate, &resolved.spec, &origin)
}

/// Gate `[policy] allowed_sources` (before any network), then clone + parse +
/// content-scan the pack. Shared by the CLI, t3code, and MCP install paths.
pub(crate) fn resolve_git_pack_gated(
    ctx: &super::Context,
    git_ref: &crate::provider::gitpack::GitPackRef,
) -> Result<(crate::provider::gitpack::ResolvedGitPack, PackOrigin)> {
    let policy = &ctx.loaded.manifest.policy;
    if !policy.allowed_sources.is_empty()
        && !git_ref
            .policy_sources()
            .iter()
            .any(|s| policy.source_allowed(s))
    {
        anyhow::bail!(
            "policy allowed_sources rejects '{}' — nothing fetched",
            git_ref.policy_sources().last().expect("label")
        );
    }
    let resolved = crate::provider::gitpack::resolve(git_ref)?;
    let origin = PackOrigin {
        assets: AssetSource::Dir(resolved.root.clone()),
        source: resolved.source_id.clone(),
        version: resolved.tag.clone(),
        rev: Some(resolved.commit.clone()),
    };
    Ok((resolved, origin))
}

fn add_from_server(a: &AddFromArgs, ctx: &super::Context, candidate: &Candidate) -> Result<()> {
    if ctx.loaded.manifest.servers.contains_key(&candidate.name) {
        anyhow::bail!(
            "server '{}' already exists in the manifest — run `agentstack remove {}` first, or rename it",
            candidate.name,
            candidate.name
        );
    }
    let server = candidate.to_server();
    write_manifest(
        ctx,
        "servers",
        &serde_json::to_value(&server)?,
        a.profile.as_deref(),
        &candidate.name,
        a.write,
        "add",
    )?;
    if a.write {
        println!(
            "{} review secrets with `agentstack secret list`, then `agentstack apply`.",
            "↳".cyan()
        );
    }
    Ok(())
}

/// Install a library hook by copying its definition into the project's inline
/// `[hooks.<name>]` table. Hooks always render from the manifest (see
/// `render/hooks.rs`), so this is a plain copy — no runtime library indirection —
/// and the definition becomes part of the manifest bytes the trust digest covers.
/// Hooks are global (not profile-scoped), so `--profile` does not apply.
fn add_from_hook(a: &AddFromArgs, ctx: &super::Context, candidate: &Candidate) -> Result<()> {
    if ctx.loaded.manifest.hooks.contains_key(&candidate.name) {
        // Unlike servers/skills, `agentstack remove` does not recognize hooks by
        // name — the fix is a manifest edit, not a remove command.
        anyhow::bail!(
            "hook '{}' already exists in the manifest — edit `[hooks.{}]` in the manifest to change it, or delete that entry by hand",
            candidate.name,
            candidate.name
        );
    }
    write_manifest(
        ctx,
        "hooks",
        &serde_json::to_value(candidate.to_hook())?,
        None,
        &candidate.name,
        a.write,
        "add",
    )?;
    if a.write {
        println!(
            "{} run `agentstack apply` to compile the hook into each harness.",
            "↳".cyan()
        );
    }
    Ok(())
}

fn add_from_skill(
    a: &AddFromArgs,
    ctx: &super::Context,
    candidate: &Candidate,
    skill: &SkillRef,
) -> Result<()> {
    if ctx.loaded.manifest.skills.contains_key(&candidate.name) {
        anyhow::bail!(
            "skill '{}' already exists in the manifest — run `agentstack remove {}` first, or rename it",
            candidate.name,
            candidate.name
        );
    }
    let (entry, asset) = skill_entry(skill)?;
    if let Some(asset) = &asset {
        let dest = ctx.dir.join(asset);
        if dest.exists() {
            anyhow::bail!(
                "destination '{}' already exists — remove it first",
                dest.display()
            );
        }
    }

    let original = fs::read_to_string(&ctx.loaded.manifest_path)
        .with_context(|| format!("reading {}", ctx.loaded.manifest_path.display()))?;
    let new_text = build_manifest_with(
        &original,
        "skills",
        &candidate.name,
        &serde_json::to_value(&entry)?,
        a.profile.as_deref(),
    )?;

    println!(
        "{} add skill '{}' to {}",
        "→".cyan(),
        candidate.name.bold(),
        ctx.loaded.manifest_path.display()
    );
    print!(
        "{}",
        diff::render(&original, &new_text)
            .lines()
            .map(|l| format!("  {l}\n"))
            .collect::<String>()
    );

    if a.write {
        crate::util::atomic::write(&ctx.loaded.manifest_path, &new_text)
            .with_context(|| format!("writing {}", ctx.loaded.manifest_path.display()))?;
        if let Some(asset) = &asset {
            crate::catalog::extract_asset_dir(asset, &ctx.dir.join(asset))
                .with_context(|| format!("extracting bundled skill '{}'", candidate.name))?;
        }
        println!("{} added skill '{}'.", "✓".green(), candidate.name);
    } else {
        println!(
            "\nDry run. Re-run with {} to update the manifest.",
            "--write".bold()
        );
    }
    Ok(())
}

/// Translate a provider [`SkillRef`] into the manifest [`Skill`] entry to write
/// plus the embedded asset path to extract (`None` for git sources). A bundled
/// (path) skill is stored as a manifest-relative `./<asset>` pointing at the
/// extracted copy under the manifest dir.
fn skill_entry(skill: &SkillRef) -> Result<(Skill, Option<String>)> {
    match (&skill.path, &skill.git) {
        (Some(asset), _) => Ok((
            Skill {
                path: Some(format!("./{asset}")),
                git: None,
                rev: None,
                subpath: None,
            },
            Some(asset.clone()),
        )),
        (None, Some(_)) => Ok((
            Skill {
                path: None,
                git: skill.git.clone(),
                rev: skill.rev.clone(),
                subpath: None,
            },
            None,
        )),
        (None, None) => anyhow::bail!("skill '{}' has no path or git source", skill.name),
    }
}

/// A bundled-asset extraction queued during pack planning, applied only on
/// `--write` (so a dry run touches no files on disk).
enum Extraction {
    /// Copy a pack skill dir to a destination relative to the manifest.
    SkillDir { asset: String, dest: String },
    /// Write an instruction file (already provenance-stamped) to a destination
    /// relative to the manifest.
    InstrFile { dest: String, body: String },
}

/// Where a pack's content and identity come from — the embedded catalog or a
/// cloned git checkout. Carried through `add_pack` so the same install path
/// serves both.
pub(crate) struct PackOrigin {
    pub assets: AssetSource,
    /// Ledger id: `catalog:<id>` or `git:<url>@<tag>[#subdir]`.
    pub source: String,
    /// Ledger version: the git tag, or the catalog's static version.
    pub version: String,
    /// The commit a git tag resolved to (provenance).
    pub rev: Option<String>,
}

pub(crate) enum AssetSource {
    /// Assets compiled into the binary under `catalog/`.
    Embedded,
    /// Assets on disk under a cloned pack root.
    Dir(PathBuf),
}

impl PackOrigin {
    pub(crate) fn catalog(id: &str) -> Self {
        PackOrigin {
            assets: AssetSource::Embedded,
            source: format!("catalog:{id}"),
            version: "0.1.0".into(),
            rev: None,
        }
    }
}

impl AssetSource {
    /// Copy one skill dir asset to `out`.
    pub(crate) fn extract_dir(&self, asset: &str, out: &Path) -> Result<()> {
        match self {
            AssetSource::Embedded => crate::catalog::extract_asset_dir(asset, out),
            AssetSource::Dir(root) => crate::util::fsx::copy_dir_all(&root.join(asset), out),
        }
    }

    /// Read one instruction file asset.
    pub(crate) fn read_file(&self, asset: &str) -> Result<String> {
        match self {
            AssetSource::Embedded => crate::catalog::read_asset_file(asset),
            AssetSource::Dir(root) => fs::read_to_string(root.join(asset))
                .with_context(|| format!("reading pack file '{asset}'")),
        }
    }
}

/// Install a vendor pack: server + skill(s) + (opt-in) house-rule instructions,
/// composed into the manifest as one atomic write. Each member lands in its
/// normal section; a `[packs.<name>]` ledger records them so `remove` can undo
/// the install. NOT a runtime concept (PLAN: packs ride existing rails).
fn add_pack(
    a: &AddFromArgs,
    ctx: &super::Context,
    candidate: &Candidate,
    spec: &PackSpec,
    origin: &PackOrigin,
) -> Result<()> {
    let pack = &candidate.name;
    let manifest = &ctx.loaded.manifest;

    // 0. Policy gate FIRST — before planning any write. Evaluate every member
    //    name + source against [policy]; bail atomically on the first violation.
    check_pack_policy(manifest, pack, spec, &origin.source)?;

    // 1. Collision check across every target key + the ledger key. Atomic.
    if spec.server.is_some() && manifest.servers.contains_key(pack) {
        anyhow::bail!(
            "server '{pack}' already exists in the manifest (pack '{pack}' would clash); \
             remove it first or rename"
        );
    }
    for skill in &spec.skills {
        if manifest.skills.contains_key(&skill.name) {
            anyhow::bail!(
                "skill '{}' already exists in the manifest (from pack '{pack}'); \
                 remove it first or rename",
                skill.name
            );
        }
    }
    let want_instructions = a.with_instructions;
    if want_instructions {
        for instr in &spec.instructions {
            if manifest.instructions.contains_key(&instr.name) {
                anyhow::bail!(
                    "instruction '{}' already exists in the manifest (from pack '{pack}')",
                    instr.name
                );
            }
        }
    }
    if manifest.packs.contains_key(pack) {
        anyhow::bail!(
            "a pack '{pack}' is already installed in the manifest — run `agentstack remove {pack}` first to reinstall"
        );
    }

    // On-disk collision: an extraction must never overwrite files a user already
    // has at our destinations (the manifest-key checks above don't see disk). A
    // fresh install's dests should be clear; bail (atomically) if they aren't.
    for skill in &spec.skills {
        if let Some(asset) = &skill.path {
            let dest = ctx.dir.join(asset);
            if dest.exists() {
                anyhow::bail!(
                    "destination '{}' already exists (pack '{pack}' skill '{}') — remove it first",
                    dest.display(),
                    skill.name
                );
            }
        }
    }
    if want_instructions {
        for instr in &spec.instructions {
            let dest = ctx.dir.join(format!("instructions/{}.md", instr.name));
            if dest.exists() {
                anyhow::bail!(
                    "destination '{}' already exists (pack '{pack}' instruction '{}') — remove it first",
                    dest.display(),
                    instr.name
                );
            }
        }
    }

    // Build the new manifest text member-by-member, preserving comments.
    let original = fs::read_to_string(&ctx.loaded.manifest_path)
        .with_context(|| format!("reading {}", ctx.loaded.manifest_path.display()))?;
    let mut text = original.clone();
    let mut extractions: Vec<Extraction> = Vec::new();
    let mut ledger = PackInstall {
        source: Some(origin.source.clone()),
        rev: origin.rev.clone(),
        version: origin.version.clone(),
        description: candidate.description.clone(),
        targets: spec.targets.clone(),
        servers: Vec::new(),
        skills: Vec::new(),
        hooks: Vec::new(),
        instructions: Vec::new(),
    };

    // 2. Server.
    if spec.server.is_some() {
        let server = candidate.to_server();
        text = build_manifest_with(
            &text,
            "servers",
            pack,
            &serde_json::to_value(&server)?,
            a.profile.as_deref(),
        )?;
        ledger.servers.push(pack.clone());
    }

    // 3. Skills — extract each embedded asset dir under the manifest, write the
    //    `[skills.<name>]` path entry.
    for skill in &spec.skills {
        let Some(asset) = &skill.path else {
            // Pack skills are always bundled content (embedded or cloned) —
            // extracted to a path the lock can digest-pin.
            anyhow::bail!("pack skill '{}' has no bundled path", skill.name);
        };
        let dest = format!("./{asset}");
        let entry = Skill {
            path: Some(dest.clone()),
            git: None,
            rev: None,
            subpath: None,
        };
        text = build_manifest_with(
            &text,
            "skills",
            &skill.name,
            &serde_json::to_value(&entry)?,
            None,
        )?;
        extractions.push(Extraction::SkillDir {
            asset: asset.clone(),
            dest: asset.clone(),
        });
        ledger.skills.push(skill.name.clone());
    }

    // 4. Instructions — opt-in (they steer the user's daily-driver agent). When
    //    enabled, extract the markdown, prepend a provenance header, write a
    //    flat `instructions/<name>.md`. When not, skip but tell the user how.
    if want_instructions {
        for instr in &spec.instructions {
            let body = stamped_instruction_from(pack, instr, &origin.assets)?;
            // Defense in depth: `instr.name` is remote (pack.toml) and becomes
            // a path component. The parse gate already enforced the contract;
            // re-assert here so this join can never traverse even if a new
            // pack source forgets the gate (design §C.1).
            crate::text::validate_name(&instr.name)
                .with_context(|| format!("pack instruction '{}'", instr.name.escape_debug()))?;
            let dest = format!("instructions/{}.md", instr.name);
            let entry = crate::manifest::Instruction {
                path: format!("./{dest}"),
                targets: vec!["*".into()],
                from_user_layer: false,
            };
            text = build_manifest_with(
                &text,
                "instructions",
                &instr.name,
                &serde_json::to_value(&entry)?,
                None,
            )?;
            extractions.push(Extraction::InstrFile { dest, body });
            ledger.instructions.push(instr.name.clone());
        }
    }

    // 5. Pack install ledger.
    text = build_manifest_with(&text, "packs", pack, &serde_json::to_value(&ledger)?, None)?;

    // Show the plan.
    println!(
        "{} install pack '{}' into {}",
        "→".cyan(),
        pack.bold(),
        ctx.loaded.manifest_path.display()
    );
    print!(
        "{}",
        diff::render(&original, &text)
            .lines()
            .map(|l| format!("  {l}\n"))
            .collect::<String>()
    );
    print_pack_members(spec, &ledger, want_instructions);

    if !a.write {
        if !spec.instructions.is_empty() && !want_instructions {
            println!(
                "\n{} house rules skipped. Re-run with {} to install them.",
                "↳".cyan(),
                "--with-instructions".bold()
            );
        }
        println!(
            "\nDry run. Re-run with {} to install the pack.",
            "--write".bold()
        );
        return Ok(());
    }

    // Apply: write the manifest (the ledger that lets `remove` undo us) first,
    // then extract bundled assets. The on-disk collision check above guarantees
    // we never clobber a user's files. If an extraction fails partway, roll the
    // manifest back and remove what we created so the install stays all-or-nothing.
    crate::util::atomic::write(&ctx.loaded.manifest_path, &text)
        .with_context(|| format!("writing {}", ctx.loaded.manifest_path.display()))?;
    let mut created: Vec<(PathBuf, bool)> = Vec::new(); // (path, is_dir)
    for ex in &extractions {
        let result = match ex {
            Extraction::SkillDir { asset, dest } => {
                let out = ctx.dir.join(dest);
                origin
                    .assets
                    .extract_dir(asset, &out)
                    .map(|()| created.push((out, true)))
            }
            Extraction::InstrFile { dest, body } => {
                let out = ctx.dir.join(dest);
                (|| {
                    if let Some(parent) = out.parent() {
                        fs::create_dir_all(parent)
                            .with_context(|| format!("creating {}", parent.display()))?;
                    }
                    fs::write(&out, body).with_context(|| format!("writing {}", out.display()))
                })()
                .map(|()| created.push((out, false)))
            }
        };
        if let Err(e) = result {
            // Roll back: restore the original manifest and drop created files.
            // The manifest restore is load-bearing — a silent failure would
            // leave a possibly-corrupt manifest with no signal, so surface it
            // (unlike the best-effort file cleanup below).
            if let Err(restore_err) =
                crate::util::atomic::write(&ctx.loaded.manifest_path, &original)
            {
                eprintln!(
                    "warning: rollback could not restore {} ({restore_err:#}); \
                     the manifest may be inconsistent — check it before re-running",
                    ctx.loaded.manifest_path.display()
                );
            }
            for (path, is_dir) in created.iter().rev() {
                if *is_dir {
                    let _ = fs::remove_dir_all(path);
                } else {
                    let _ = fs::remove_file(path);
                }
            }
            return Err(e).context("extracting pack assets (install rolled back)");
        }
    }
    println!("{} installed pack '{}'.", "✓".green(), pack);
    print_pack_next_steps(pack, spec, want_instructions);
    Ok(())
}

/// Mirror doctor's policy check across every pack member before writing. Bails
/// (atomically) on the first violation, naming the member + the offending rule.
pub(crate) fn check_pack_policy(
    manifest: &Manifest,
    pack: &str,
    spec: &PackSpec,
    pack_source: &str,
) -> Result<()> {
    let policy = &manifest.policy;
    if policy.is_empty() {
        return Ok(());
    }
    // Every member name (vendor-prefixed) the install would introduce.
    let mut names: Vec<&str> = Vec::new();
    if spec.server.is_some() {
        names.push(pack);
    }
    for s in &spec.skills {
        names.push(&s.name);
    }
    for i in &spec.instructions {
        names.push(&i.name);
    }
    // forbid: no introduced member may be forbidden.
    for name in &names {
        if policy.forbid.iter().any(|f| f == name) {
            anyhow::bail!("policy forbids '{name}' (a member of pack '{pack}') — nothing written");
        }
    }
    // allowed_sources: each skill's source must be allowed. The pack server's
    // source is the pack's own (`catalog:<id>` or `git:<url>@<tag>`); skills
    // are extracted to a local path under the manifest (`path:...`).
    if !policy.allowed_sources.is_empty() {
        if spec.server.is_some() && !policy.source_allowed(pack_source) {
            anyhow::bail!(
                "policy allowed_sources rejects pack '{pack}' source '{pack_source}' — nothing written"
            );
        }
        for s in &spec.skills {
            let source = match &s.path {
                Some(p) => format!("path:./{p}"),
                None => match &s.git {
                    Some(url) => format!("git:{url}"),
                    None => "invalid".into(),
                },
            };
            if !policy.source_allowed(&source) {
                anyhow::bail!(
                    "policy allowed_sources rejects skill '{}' source '{source}' (pack '{pack}') — nothing written",
                    s.name
                );
            }
        }
    }
    Ok(())
}

/// Extract a pack's bundled instruction and stamp it with a provenance header +
/// a visible heading so the origin survives into the merged CLAUDE.md/AGENTS.md.
pub(crate) fn stamped_instruction_from(
    pack: &str,
    instr: &InstrRef,
    assets: &AssetSource,
) -> Result<String> {
    let raw = assets.read_file(&instr.path)?;
    Ok(format!(
        "{}\n# vendor: {pack} (unofficial)\n\n{}",
        vendor_marker(pack),
        raw.trim_start()
    ))
}

fn print_pack_members(spec: &PackSpec, ledger: &PackInstall, with_instructions: bool) {
    let servers = ledger.servers.len();
    let skills = ledger.skills.len();
    let instrs = if with_instructions {
        ledger.instructions.len()
    } else {
        0
    };
    let mut parts = Vec::new();
    if servers > 0 {
        parts.push(format!("{servers} server"));
    }
    if skills > 0 {
        parts.push(format!("{skills} skill"));
    }
    if instrs > 0 {
        parts.push(format!("{instrs} instruction"));
    } else if !spec.instructions.is_empty() {
        parts.push(format!("{} instruction (skipped)", spec.instructions.len()));
    }
    if !parts.is_empty() {
        println!("  {} {}", "contains:".dimmed(), parts.join(" · "));
    }
}

fn print_pack_next_steps(pack: &str, spec: &PackSpec, with_instructions: bool) {
    if spec.server.is_some() {
        let reference = format!("{}_TOKEN", sanitize_ref(pack));
        println!(
            "{} set the server secret: `agentstack secret set {reference}` (or via varlock), then `agentstack apply`.",
            "↳".cyan()
        );
    }
    if !spec.instructions.is_empty() && with_instructions {
        // The fragments are declared but not compiled yet — hand over the
        // exact command that lands them in each CLI's CLAUDE.md / AGENTS.md.
        println!(
            "{} compile the installed house rules into your agents' instruction files: `agentstack instructions --write`.",
            "↳".cyan()
        );
    }
    if !spec.instructions.is_empty() && !with_instructions {
        println!(
            "{} house rules were skipped — re-run with `--with-instructions` to install them.",
            "↳".cyan()
        );
    }
}

/// Uppercase, identifier-safe ref base from a name (mirrors the provider's
/// secret-ref convention so the printed next-step matches the lifted `${REF}`).
fn sanitize_ref(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect::<String>()
        .to_ascii_uppercase()
}

fn add_server(a: &AddServerArgs, manifest_dir: Option<&Path>) -> Result<()> {
    upsert_server(a, manifest_dir, false)
}

/// Shared body of `add server` (refuses an existing name) and `set server`
/// (updates it in place — `merge_toml` already replaces an existing key, so
/// updating is the same write with the guard dropped).
fn upsert_server(a: &AddServerArgs, manifest_dir: Option<&Path>, allow_update: bool) -> Result<()> {
    let ctx = super::load(manifest_dir)?;
    let exists = ctx.loaded.manifest.servers.contains_key(&a.name);
    if exists && !allow_update {
        anyhow::bail!(
            "server '{name}' already exists in the manifest — update it in place: `agentstack set server {name} …` · or remove it first: `agentstack remove {name}`",
            name = a.name
        );
    }

    // Per-CLI scoping straight from the flag. Validate eagerly against the
    // adapter registry — manifest validation would catch a typo later, but
    // "renders nowhere you expected" is cheaper to refuse right here.
    let targets = if a.targets.is_empty() {
        crate::manifest::model::all_targets()
    } else {
        for t in &a.targets {
            if ctx.registry.get(t).is_none() {
                anyhow::bail!(
                    "unknown target '{t}' — valid adapter ids: {}",
                    ctx.registry.ids().collect::<Vec<_>>().join(", ")
                );
            }
        }
        a.targets.clone()
    };
    let server = Server {
        server_type: a.transport,
        url: a.url.clone(),
        command: a.command.clone(),
        args: a.args.clone(),
        cwd: a.cwd.clone(),
        integrity_roots: Vec::new(),
        targets,
        owner: None,
        headers: parse_kv(&a.headers)?,
        env: parse_kv(&a.env)?,
        extra: Default::default(),
    };
    // Validation errors carry the complete retry skeleton — the user should
    // never have to reconstruct the command shape from memory (audit D5).
    match a.transport {
        ServerType::Http if server.url.is_none() => {
            anyhow::bail!(
                "http server needs --url\n  fix: agentstack set server {} --url <URL> --write",
                a.name
            )
        }
        ServerType::Stdio if server.command.is_none() => {
            anyhow::bail!(
                "stdio server needs --command\n  fix: agentstack set server {} --type stdio --command <CMD> --write",
                a.name
            )
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
        if exists { "update" } else { "add" },
    )
}

/// `agentstack add skill <source>` — the ecosystem-grammar acquisition verb.
/// Parse →
/// stage (git) → discover → select → scan → preview; `--write` promotes the
/// staged clone, writes the manifest entries, records the lock pins, and —
/// when the delivery mode is static and the active profile is unambiguous —
/// materializes the new skills into the default targets. Other modes get
/// the honest next-step hint instead.
fn add_skill(a: &AddSkillArgs, manifest_dir: Option<&Path>) -> Result<()> {
    use crate::provider::source::{parse_source, SkillSource};
    let ctx = super::load(manifest_dir)?;
    let parsed = parse_source(&a.source)?;

    // `@skill` is the human alias for --skill; disagreement is an error,
    // never a precedence puzzle.
    let mut requested = a.skill.clone();
    if let Some(alias) = &parsed.skill_alias {
        if requested.is_empty() {
            requested.push(alias.clone());
        } else if !requested.contains(alias) {
            anyhow::bail!(
                "skill given twice and they disagree: @{} vs --skill {}",
                crate::text::sanitize_line(alias),
                requested.join(", ")
            );
        }
    }

    match parsed.source {
        SkillSource::Local { path } => add_skill_from_dir(a, &ctx, &path, &requested),
        SkillSource::Git { url, ref_, subpath } => {
            add_skill_from_git(a, &ctx, &url, ref_, subpath, &requested)
        }
    }
}

/// Activation context for the write's tail, captured ONCE from pre-write
/// disk state (design §3): `detect_mode` reads lock-existence as a
/// clean-at-rest signal, and this command's own `lock.save` would flip a
/// fresh static project's detected mode if recomputed in the tail.
struct ActivationCtx {
    mode: super::overview::Mode,
    target_ids: Vec<String>,
    scope: Scope,
    /// Several profiles declared → which one is live per target is
    /// unknowable; profile fencing wins over "static → activation".
    ambiguous: bool,
    session_active: bool,
}

impl ActivationCtx {
    fn detect(ctx: &super::Context) -> Result<Self> {
        let target_ids = crate::render::resolve_targets(&ctx.loaded.manifest, &ctx.registry, &[])?;
        Ok(ActivationCtx {
            mode: super::overview::detect_mode(ctx, &target_ids),
            scope: Scope::default_for(&ctx.dir),
            ambiguous: ctx.loaded.manifest.profiles.len() > 1,
            session_active: crate::session::active(&ctx.dir).is_some(),
            target_ids,
        })
    }
}

/// One selected skill, ready to preview/write.
struct PlannedSkill {
    name: String,
    entry: Skill,
    /// Where the skill's bytes live for the lock digest (staged dir on
    /// preview; promoted dir on write).
    content_dir: std::path::PathBuf,
    rev: Option<String>,
    source_kind: &'static str,
    scan_warnings: Vec<String>,
}

fn add_skill_from_git(
    a: &AddSkillArgs,
    ctx: &super::Context,
    url: &str,
    url_ref: Option<String>,
    url_subpath: Option<String>,
    requested: &[String],
) -> Result<()> {
    let rev = merge_source_opt("rev", a.rev.as_ref(), url_ref)?;
    let subpath = merge_source_opt("subpath", a.subpath.as_ref(), url_subpath)?;
    if let Some(s) = &subpath {
        crate::provider::source::validate_subpath(s)?;
    }

    // Transient staging on the store's filesystem: a preview fetches, but
    // never touches the persistent store (design §3).
    let stage = crate::store::Stage::create()?;
    let staging = stage.store();
    let (clone_root, head) = crate::store::checkout(&staging, url, rev.as_deref())?;
    let disc_root = match &subpath {
        // An explicit subpath scopes discovery — the user already navigated.
        // Containment-guarded: a checked-out symlink must not route the
        // preview outside the clone (same refusal Store::resolve applies).
        Some(s) => {
            let d = crate::store::contained_content_dir(&clone_root, Some(s))?;
            anyhow::ensure!(
                d.is_dir(),
                "subpath '{}' does not exist in {}",
                crate::text::sanitize_line(s),
                crate::text::sanitize_line(url)
            );
            d
        }
        None => clone_root.clone(),
    };
    let repo_name = crate::provider::source::repo_name(url);
    let discovered = crate::provider::discover::discover_skills(&disc_root, repo_name.as_deref())?;
    println!(
        "{} {} (git) — {} at {}",
        "found".green(),
        crate::text::sanitize_line(repo_name.as_deref().unwrap_or(url)).bold(),
        crate::text::sanitize_line(url),
        &head[..head.len().min(12)]
    );
    anyhow::ensure!(
        !discovered.is_empty(),
        "no SKILL.md found in {} — not a skills repo (or pass --subpath)",
        crate::text::sanitize_line(url)
    );
    if a.list {
        return print_skill_listing(&discovered);
    }

    let selected = select_skills(&discovered, requested)?;
    let names = final_names(a, ctx, &selected)?;
    let profile = choose_profile(a, &ctx.loaded.manifest)?;

    // Scan the STAGED bytes before anything is offered for writing.
    let mut planned = Vec::new();
    for (skill, name) in selected.iter().zip(&names) {
        let full_sub = join_subpath(subpath.as_deref(), &skill.rel_path);
        // Containment-guarded against checked-out symlink escapes.
        let dir = crate::store::contained_content_dir(&clone_root, full_sub.as_deref())?;
        let mut warnings = Vec::new();
        crate::scan::gate(name, &dir, a.allow_flagged, &mut warnings)?;
        planned.push(PlannedSkill {
            name: name.clone(),
            entry: Skill {
                path: None,
                git: Some(url.to_string()),
                rev: rev.clone(),
                subpath: full_sub,
            },
            content_dir: dir,
            rev: Some(head.clone()),
            source_kind: "git",
            scan_warnings: warnings,
        });
    }
    print_selection(&discovered, &selected, &planned);

    if a.write {
        // Promote BEFORE the manifest write (design §4): rename-only; a
        // taken slot (or refused rename) falls back to a commit-pinned
        // re-resolve, which re-clones via git and is re-scanned — never a
        // byte-copy.
        let real = crate::store::Store::default_store();
        let promoted = match real.adopt_clone(url, &clone_root)? {
            Some(root) => root,
            None => {
                let (root, head2) = crate::store::checkout(&real, url, Some(&head))?;
                anyhow::ensure!(
                    head2 == head,
                    "re-resolve landed {head2} but the preview scanned {head} — retry"
                );
                // Freshly-fetched bytes: re-scan before the lock write. The
                // entry's subpath is exactly the clone-root-relative dir.
                for p in &mut planned {
                    let dir =
                        crate::store::contained_content_dir(&root, p.entry.subpath.as_deref())?;
                    let mut warnings = Vec::new();
                    crate::scan::gate(&p.name, &dir, a.allow_flagged, &mut warnings)?;
                    p.scan_warnings = warnings;
                    p.content_dir = dir;
                }
                root
            }
        };
        // Content dirs re-derive under the promoted root (containment-guarded;
        // idempotent for the re-resolve branch, which already did this).
        for p in &mut planned {
            p.content_dir =
                crate::store::contained_content_dir(&promoted, p.entry.subpath.as_deref())?;
        }
    }

    preview_and_commit(ctx, a.write, &planned, profile.as_deref())
}

fn add_skill_from_dir(
    a: &AddSkillArgs,
    ctx: &super::Context,
    path: &Path,
    requested: &[String],
) -> Result<()> {
    anyhow::ensure!(
        a.rev.is_none() && a.subpath.is_none(),
        "--rev/--subpath apply to git sources — point the path at the directory directly"
    );
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    anyhow::ensure!(abs.is_dir(), "no such directory: {}", abs.display());
    let root_name = abs.file_name().map(|n| n.to_string_lossy().into_owned());
    let discovered = crate::provider::discover::discover_skills(&abs, root_name.as_deref())?;
    println!(
        "{} {} (local)",
        "found".green(),
        crate::text::sanitize_line(&a.source).bold()
    );
    anyhow::ensure!(
        !discovered.is_empty(),
        "no SKILL.md found under {}",
        abs.display()
    );
    if a.list {
        return print_skill_listing(&discovered);
    }

    let selected = select_skills(&discovered, requested)?;
    let names = final_names(a, ctx, &selected)?;
    let profile = choose_profile(a, &ctx.loaded.manifest)?;

    let mut planned = Vec::new();
    for (skill, name) in selected.iter().zip(&names) {
        let dir = join_rel(&abs, &skill.rel_path);
        let mut warnings = Vec::new();
        crate::scan::gate(name, &dir, a.allow_flagged, &mut warnings)?;
        // Store the path relative to the MANIFEST dir, not the user's cwd
        // spelling: discovery resolved `./x` against the current directory,
        // but the store later resolves the manifest value against `.agentstack/`
        // (`ctx.dir`). Storing the raw `./x` would resolve to `.agentstack/x`.
        let path_field = manifest_rel_path(&ctx.dir, &dir);
        planned.push(PlannedSkill {
            name: name.clone(),
            entry: Skill {
                path: Some(path_field),
                git: None,
                rev: None,
                subpath: None,
            },
            content_dir: dir,
            rev: None,
            source_kind: "path",
            scan_warnings: warnings,
        });
    }
    print_selection(&discovered, &selected, &planned);
    preview_and_commit(ctx, a.write, &planned, profile.as_deref())
}

/// Express the absolute `target` as a path the manifest can store — one that
/// `store::resolve_path(base, _)` maps back to `target` no matter which
/// directory the command ran from. `base` is the manifest dir. Prefers a
/// relative path (portable across machines); falls back to the absolute
/// target only when the two share no common root.
fn manifest_rel_path(base: &Path, target: &Path) -> String {
    let base = std::fs::canonicalize(base).unwrap_or_else(|_| base.to_path_buf());
    let target = std::fs::canonicalize(target).unwrap_or_else(|_| target.to_path_buf());
    let bc: Vec<_> = base.components().collect();
    let tc: Vec<_> = target.components().collect();
    let common = bc.iter().zip(&tc).take_while(|(a, b)| a == b).count();
    if common == 0 {
        return target.to_string_lossy().into_owned();
    }
    let mut rel = PathBuf::new();
    for _ in 0..(bc.len() - common) {
        rel.push("..");
    }
    for c in &tc[common..] {
        rel.push(c.as_os_str());
    }
    if rel.as_os_str().is_empty() {
        ".".to_string()
    } else {
        rel.to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/")
    }
}

/// A canonical flag beats its in-URL alias; disagreement is an error.
pub(crate) fn merge_source_opt(
    kind: &str,
    flag: Option<&String>,
    from_url: Option<String>,
) -> Result<Option<String>> {
    match (flag, from_url) {
        (Some(f), Some(u)) if *f != u => anyhow::bail!(
            "{kind} given twice and they disagree: --{kind} {} vs {} in the URL",
            crate::text::sanitize_line(f),
            crate::text::sanitize_line(&u)
        ),
        (Some(f), _) => Ok(Some(f.clone())),
        (None, u) => Ok(u),
    }
}

fn join_rel(root: &Path, rel: &str) -> std::path::PathBuf {
    if rel.is_empty() {
        root.to_path_buf()
    } else {
        root.join(rel)
    }
}

pub(crate) fn join_subpath(user: Option<&str>, rel: &str) -> Option<String> {
    match (user, rel.is_empty()) {
        (Some(u), true) => Some(u.to_string()),
        (Some(u), false) => Some(format!("{}/{rel}", u.trim_end_matches('/'))),
        (None, true) => None,
        (None, false) => Some(rel.to_string()),
    }
}

pub(crate) fn print_skill_listing(
    discovered: &[crate::provider::discover::DiscoveredSkill],
) -> Result<()> {
    for s in discovered {
        let mut notes = Vec::new();
        if s.via_fallback {
            notes.push("via recursive fallback".to_string());
        }
        if !s.name_valid {
            notes.push("invalid name — needs --name".to_string());
        }
        let notes = if notes.is_empty() {
            String::new()
        } else {
            format!("  [{}]", notes.join("; "))
        };
        println!(
            "  {} {:<20} {:<28} {}{notes}",
            "·".dimmed(),
            crate::text::sanitize_line(&s.name).bold(),
            crate::text::sanitize_line(if s.rel_path.is_empty() {
                "(root)"
            } else {
                &s.rel_path
            })
            .dimmed(),
            crate::text::truncate_chars(s.description.as_deref().unwrap_or("(no description)"), 60),
        );
    }
    println!("\nUse {} to add specific skills.", "--skill <name>".bold());
    Ok(())
}

pub(crate) fn select_skills(
    discovered: &[crate::provider::discover::DiscoveredSkill],
    requested: &[String],
) -> Result<Vec<crate::provider::discover::DiscoveredSkill>> {
    let names_list = || {
        discovered
            .iter()
            .map(|s| crate::text::sanitize_line(&s.name))
            .collect::<Vec<_>>()
            .join(", ")
    };
    if !requested.is_empty() {
        let mut out = Vec::new();
        for want in requested {
            match discovered.iter().find(|s| s.name == *want) {
                Some(s) => out.push(s.clone()),
                None => anyhow::bail!(
                    "no skill named '{}' in this source — found: {}",
                    crate::text::sanitize_line(want),
                    names_list()
                ),
            }
        }
        return Ok(out);
    }
    // Auto-select only a single conventional-location hit; fallback hits are
    // never auto-selected (design §2).
    if discovered.len() == 1 && !discovered[0].via_fallback {
        return Ok(vec![discovered[0].clone()]);
    }
    if !crate::util::confirm::is_interactive() {
        anyhow::bail!(
            "{} skill(s) in this source — pass --skill <name> to choose: {}",
            discovered.len(),
            names_list()
        );
    }
    // The crate's first multi-select: all entries start UNCHECKED (opt-in).
    let items: Vec<String> = discovered
        .iter()
        .map(|s| {
            format!(
                "{:<20} {:<24} {}",
                crate::text::sanitize_line(&s.name),
                crate::text::sanitize_line(if s.rel_path.is_empty() {
                    "(root)"
                } else {
                    &s.rel_path
                }),
                crate::text::truncate_chars(
                    s.description.as_deref().unwrap_or("(no description)"),
                    48
                ),
            )
        })
        .collect();
    let picks = dialoguer::MultiSelect::with_theme(&dialoguer::theme::ColorfulTheme::default())
        .with_prompt("Select skills to add (space toggles, enter confirms)")
        .items(&items)
        .interact()?;
    if picks.is_empty() {
        anyhow::bail!("nothing selected — nothing to add");
    }
    Ok(picks.into_iter().map(|i| discovered[i].clone()).collect())
}

fn final_names(
    a: &AddSkillArgs,
    ctx: &super::Context,
    selected: &[crate::provider::discover::DiscoveredSkill],
) -> Result<Vec<String>> {
    if a.name.is_some() && selected.len() != 1 {
        anyhow::bail!(
            "--name applies to a single selection; {} skills selected",
            selected.len()
        );
    }
    let mut names = Vec::new();
    for s in selected {
        let name = a.name.clone().unwrap_or_else(|| s.name.clone());
        crate::text::validate_name(&name).with_context(|| {
            if a.name.is_none() {
                format!(
                    "skill directory '{}' — pass --name to choose a conforming manifest name",
                    crate::text::sanitize_line(&s.name)
                )
            } else {
                "--name".to_string()
            }
        })?;
        if ctx.loaded.manifest.skills.contains_key(&name) {
            anyhow::bail!(
                "skill '{name}' already exists in the manifest — run `agentstack remove {name}` first, or rename it"
            );
        }
        names.push(name);
    }
    Ok(names)
}

/// Zero profiles → implicit default (no membership edit). One → automatic.
/// Several → --profile, or an interactive pick. A --profile naming a profile
/// that doesn't exist is an error — never a silent create (design §4).
fn choose_profile(a: &AddSkillArgs, manifest: &Manifest) -> Result<Option<String>> {
    let declared: Vec<String> = manifest.profiles.keys().cloned().collect();
    if let Some(p) = &a.profile {
        if declared.contains(p) {
            return Ok(Some(p.clone()));
        }
        anyhow::bail!(
            "no profile '{}' in the manifest — declared: {}",
            p,
            if declared.is_empty() {
                "(none)".to_string()
            } else {
                declared.join(", ")
            }
        );
    }
    match declared.len() {
        0 => Ok(None),
        1 => Ok(Some(declared[0].clone())),
        _ => {
            if crate::util::confirm::is_interactive() {
                let idx =
                    dialoguer::Select::with_theme(&dialoguer::theme::ColorfulTheme::default())
                        .with_prompt("Add to which profile?")
                        .items(&declared)
                        .default(0)
                        .interact()?;
                Ok(Some(declared[idx].clone()))
            } else {
                anyhow::bail!(
                    "several profiles declared — pass --profile <name>: {}",
                    declared.join(", ")
                )
            }
        }
    }
}

fn print_selection(
    discovered: &[crate::provider::discover::DiscoveredSkill],
    selected: &[crate::provider::discover::DiscoveredSkill],
    planned: &[PlannedSkill],
) {
    println!(
        "  skills discovered: {} ({} selected)",
        discovered.len(),
        selected.len()
    );
    for (s, p) in selected.iter().zip(planned) {
        let status = match p.scan_warnings.len() {
            0 => "scan: clean".to_string(),
            n => format!("scan: {n} warning(s)"),
        };
        println!(
            "  {} {:<20} {:<24} {:<18} {}",
            "✓".green(),
            crate::text::sanitize_line(&p.name).bold(),
            crate::text::sanitize_line(if s.rel_path.is_empty() {
                "(root)"
            } else {
                &s.rel_path
            })
            .dimmed(),
            status,
            crate::text::truncate_chars(s.description.as_deref().unwrap_or(""), 48),
        );
        for w in &p.scan_warnings {
            println!("    {} {w}", "⚠".yellow());
        }
    }
    for s in discovered
        .iter()
        .filter(|s| s.via_fallback && !selected.iter().any(|x| x.rel_path == s.rel_path))
    {
        println!(
            "  {} {} found via recursive fallback at {} — select explicitly to include",
            "·".dimmed(),
            crate::text::sanitize_line(&s.name),
            crate::text::sanitize_line(&s.rel_path)
        );
    }
}

/// The one preview + one write: manifest diff (always), then on `--write`
/// the atomic manifest write, the lock pins, and — mode-aware — activation.
/// The activation context is
/// captured HERE, before any write, so this command's own lock.save can
/// never flip the detected mode.
fn preview_and_commit(
    ctx: &super::Context,
    write: bool,
    planned: &[PlannedSkill],
    profile: Option<&str>,
) -> Result<()> {
    let act = ActivationCtx::detect(ctx)?;
    let manifest_path = &ctx.loaded.manifest_path;
    let original = fs::read_to_string(manifest_path)
        .with_context(|| format!("reading {}", manifest_path.display()))?;
    let mut text = original.clone();
    for p in planned {
        text = build_manifest_with(
            &text,
            "skills",
            &p.name,
            &serde_json::to_value(&p.entry)?,
            profile,
        )?;
    }
    toml::from_str::<Manifest>(&text).context("resulting manifest would be invalid")?;

    if let Some(p) = profile {
        println!("{} add to profile '{p}'", "→".cyan());
    }
    let names = planned
        .iter()
        .map(|p| format!("'{}'", p.name))
        .collect::<Vec<_>>()
        .join(", ");
    println!("{} add {names} in {}", "→".cyan(), manifest_path.display());
    for line in crate::util::diff::render(&original, &text).lines() {
        println!("  {line}");
    }
    if !write {
        print_activation_footer(&act, profile);
        println!(
            "\nDry run. Re-run with {} to update the manifest.",
            "--write".bold()
        );
        return Ok(());
    }
    // Everything fallible — snapshots, digests, lock entries — is prepared
    // BEFORE the manifest is touched, so a failure here leaves the manifest
    // (and lock) untouched: no half-written state to recover from. Git
    // content is snapshotted out of the churning clone (finding 1) so a
    // later add of another revision can't mutate this skill's symlinked
    // bytes; path sources already live in a stable dir.
    let store = crate::store::Store::default_store();
    let mut lock = crate::lock::Lock::load(&ctx.dir)?;
    let mut mat_sources: Vec<(String, PathBuf)> = Vec::new();
    let mut entries = Vec::new();
    for p in planned {
        let checksum = crate::store::dir_digest(&p.content_dir)?.hex().to_string();
        let source = if p.source_kind == "git" {
            store.snapshot_content(&p.content_dir, &checksum)?
        } else {
            p.content_dir.clone()
        };
        let resolved = crate::store::Resolved {
            path: source.clone(),
            rev: p.rev.clone(),
            checksum,
            fetched: true,
            source_kind: p.source_kind,
        };
        entries.push(super::install::locked_entry(&p.name, &p.entry, &resolved)?);
        mat_sources.push((p.name.clone(), source));
    }

    // Now commit: the manifest atomically (source of truth), then the lock
    // derived from it. If only the lock write fails, the manifest still
    // stands and the error names `agentstack lock` to reconcile — the pair
    // is never left with the manifest ahead and no path back to a match.
    crate::util::atomic::write(manifest_path, &text)
        .with_context(|| format!("writing {}", manifest_path.display()))?;
    for entry in entries {
        lock.upsert(entry);
    }
    lock.save(&ctx.dir).with_context(|| {
        format!(
            "the manifest was written but the lockfile at {} could not be — \
             run `agentstack lock` to reconcile",
            crate::lock::Lock::path(&ctx.dir).display()
        )
    })?;
    println!("{} added {names}.", "✓".green());

    // Mode-aware tail (`act` was captured pre-write — see its doc).
    use super::overview::Mode;
    if act.mode == Mode::Static && !act.ambiguous {
        let new_skills: Vec<(String, PathBuf)> = mat_sources.clone();
        let outcome = super::use_profile::materialize_skills_additive(
            ctx,
            act.scope,
            &act.target_ids,
            &new_skills,
            false,
        )?;
        for (id, dir) in &outcome.written {
            println!(
                "  {} {id}: {} skill(s) → {}",
                "✓".green(),
                new_skills.len(),
                dir.display()
            );
        }
        for (id, name) in &outcome.conflicts {
            println!(
                "  {} {id}: skill '{}' already exists (not managed) — left as is",
                "⚠".yellow(),
                crate::text::sanitize_line(name)
            );
        }
        for (id, reason) in &outcome.unsupported {
            println!("  {} {id}: {reason}", "·".dimmed());
        }
        for (id, err) in &outcome.failed {
            println!("  {} {id}: {err}", "✗".red());
        }
        if outcome.written.is_empty() && outcome.failed.is_empty() {
            // Declared and pinned, materialized nowhere: a successful add
            // that must not read as an activation.
            println!(
                "{} no target took the skill files — activate later with `agentstack use{} --target <id> --write`",
                "⚠".yellow(),
                profile.map(|p| format!(" {p}")).unwrap_or_default()
            );
        }
        if !outcome.failed.is_empty() {
            anyhow::bail!(
                "{} target(s) failed to materialize (the manifest and lock writes stand — \
                 retry with `agentstack use{} --write`)",
                outcome.failed.len(),
                profile.map(|p| format!(" {p}")).unwrap_or_default()
            );
        }
    } else {
        print_activation_footer(&act, profile);
    }
    Ok(())
}

/// The §3 footer matrix: what the write will (or did) do about activation,
/// per pre-write mode and profile ambiguity.
fn print_activation_footer(act: &ActivationCtx, profile: Option<&str>) {
    use super::overview::Mode;
    let profile_word = profile.map(|p| format!(" {p}")).unwrap_or_default();
    match act.mode {
        Mode::Static if !act.ambiguous => println!(
            "{} will materialize into {} target(s)",
            "→".cyan(),
            act.target_ids.len()
        ),
        Mode::Static => println!(
            "{} several profiles declared — activate with `agentstack use{profile_word} --write`",
            "·".dimmed()
        ),
        Mode::CleanAtRest => {
            println!(
                "{} next session picks this up: `agentstack session start{}`",
                "·".dimmed(),
                profile
                    .map(|p| format!(" {p}"))
                    .unwrap_or(" <profile>".into())
            );
            if act.session_active {
                println!(
                    "{} a session is active — it won't see this until the next `session start`",
                    "⚠".yellow()
                );
            }
        }
        Mode::ZeroFiles => println!(
            "{} trust re-gates on this edit: run `agentstack trust .` to re-consent",
            "·".dimmed()
        ),
    }
}

fn write_manifest(
    ctx: &super::Context,
    location: &str,
    body: &Value,
    profile: Option<&str>,
    name: &str,
    write: bool,
    verb: &str,
) -> Result<()> {
    let original = fs::read_to_string(&ctx.loaded.manifest_path)
        .with_context(|| format!("reading {}", ctx.loaded.manifest_path.display()))?;
    let new_text = build_manifest_with(&original, location, name, body, profile)?;

    println!(
        "{} {verb} '{name}' in {}",
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
        crate::util::atomic::write(&ctx.loaded.manifest_path, &new_text)
            .with_context(|| format!("writing {}", ctx.loaded.manifest_path.display()))?;
        println!("{} {verb}d '{name}'.", "✓".green());
    } else {
        println!(
            "\nDry run. Re-run with {} to update the manifest.",
            "--write".bold()
        );
    }
    Ok(())
}

/// The manifest members an install added, by section. Shared return shape so the
/// t3code/MCP reach server, skill, and pack installs through the same door.
#[derive(Debug, Clone, Default)]
pub struct AddedMembers {
    pub servers: Vec<String>,
    pub skills: Vec<String>,
    pub instructions: Vec<String>,
    pub hooks: Vec<String>,
}

/// Resolve a provider id and write it into the manifest at `dir` (no dry-run).
/// Shared by t3code and MCP server. Handles servers, standalone skills,
/// and vendor packs; returns the members it added.
///
/// Instructions are NOT installed here (the daily-driver-steering opt-in lives
/// behind the CLI's `--with-instructions`); pack instructions are reported as
/// available but skipped.
pub fn write_from_provider(dir: &Path, id: &str, profile: Option<&str>) -> Result<AddedMembers> {
    let ctx = super::load(Some(dir))?;

    // Git packs come through the same door (t3code discovery, MCP).
    if let Some(git_ref) = crate::provider::gitpack::GitPackRef::parse(id) {
        let (resolved, origin) = resolve_git_pack_gated(&ctx, &git_ref)?;
        let args = AddFromArgs {
            id: id.to_string(),
            profile: profile.map(str::to_string),
            with_instructions: false,
            write: true,
        };
        add_pack(&args, &ctx, &resolved.candidate, &resolved.spec, &origin)?;
        return Ok(AddedMembers {
            servers: resolved
                .spec
                .server
                .iter()
                .map(|_| resolved.candidate.name.clone())
                .collect(),
            skills: resolved
                .spec
                .skills
                .iter()
                .map(|s| s.name.clone())
                .collect(),
            instructions: Vec::new(),
            hooks: Vec::new(),
        });
    }

    let candidate = provider::resolve(id).with_context(|| {
        format!("no capability '{id}' in the catalog or registry — run `agentstack search {id}` to find one")
    })?;
    match &candidate.kind {
        CandidateKind::Server(_) => {
            let manifest = &ctx.loaded.manifest;
            if manifest.servers.contains_key(&candidate.name) {
                anyhow::bail!(
                    "server '{}' already exists — run `agentstack remove {}` first, or rename it",
                    candidate.name,
                    candidate.name
                );
            }
            let original = fs::read_to_string(&ctx.loaded.manifest_path)?;
            let body = serde_json::to_value(candidate.to_server())?;
            let new_text =
                build_manifest_with(&original, "servers", &candidate.name, &body, profile)?;
            crate::util::atomic::write(&ctx.loaded.manifest_path, &new_text)
                .with_context(|| format!("writing {}", ctx.loaded.manifest_path.display()))?;
            Ok(AddedMembers {
                servers: vec![candidate.name.clone()],
                ..Default::default()
            })
        }
        CandidateKind::Skill(skill) => {
            let manifest = &ctx.loaded.manifest;
            if manifest.skills.contains_key(&candidate.name) {
                anyhow::bail!(
                    "skill '{}' already exists — run `agentstack remove {}` first, or rename it",
                    candidate.name,
                    candidate.name
                );
            }
            let (entry, asset) = skill_entry(skill)?;
            if let Some(asset) = &asset {
                if ctx.dir.join(asset).exists() {
                    anyhow::bail!(
                        "destination '{}' already exists — remove it first",
                        ctx.dir.join(asset).display()
                    );
                }
            }
            let original = fs::read_to_string(&ctx.loaded.manifest_path)?;
            let body = serde_json::to_value(&entry)?;
            let new_text =
                build_manifest_with(&original, "skills", &candidate.name, &body, profile)?;
            crate::util::atomic::write(&ctx.loaded.manifest_path, &new_text)
                .with_context(|| format!("writing {}", ctx.loaded.manifest_path.display()))?;
            if let Some(asset) = &asset {
                crate::catalog::extract_asset_dir(asset, &ctx.dir.join(asset))
                    .with_context(|| format!("extracting bundled skill '{}'", candidate.name))?;
            }
            Ok(AddedMembers {
                skills: vec![candidate.name.clone()],
                ..Default::default()
            })
        }
        CandidateKind::Pack(spec) => {
            let args = AddFromArgs {
                id: candidate.id.clone(),
                profile: profile.map(str::to_string),
                with_instructions: false,
                write: true,
            };
            add_pack(
                &args,
                &ctx,
                &candidate,
                spec,
                &PackOrigin::catalog(&candidate.id),
            )?;
            Ok(AddedMembers {
                servers: spec.server.iter().map(|_| candidate.name.clone()).collect(),
                skills: spec.skills.iter().map(|s| s.name.clone()).collect(),
                instructions: Vec::new(),
                hooks: Vec::new(),
            })
        }
        CandidateKind::Hook(_) => {
            let manifest = &ctx.loaded.manifest;
            if manifest.hooks.contains_key(&candidate.name) {
                // `agentstack remove` does not recognize hooks by name — the fix
                // is a manifest edit, not a remove command.
                anyhow::bail!(
                    "hook '{}' already exists — edit `[hooks.{}]` in the manifest to change it, or delete that entry by hand",
                    candidate.name,
                    candidate.name
                );
            }
            let original = fs::read_to_string(&ctx.loaded.manifest_path)?;
            // Hooks are global (not profile-scoped); ignore any passed profile.
            let body = serde_json::to_value(candidate.to_hook())?;
            let new_text = build_manifest_with(&original, "hooks", &candidate.name, &body, None)?;
            crate::util::atomic::write(&ctx.loaded.manifest_path, &new_text)
                .with_context(|| format!("writing {}", ctx.loaded.manifest_path.display()))?;
            Ok(AddedMembers {
                hooks: vec![candidate.name.clone()],
                ..Default::default()
            })
        }
        CandidateKind::Extension(ext) => anyhow::bail!(
            "'{}' is a native extension — executable in-process code that `add from` does not \
             install. Reference it in [extensions.{}] with target = \"{}\", then run \
             `agentstack lock`.",
            candidate.name,
            candidate.name,
            ext.target
        ),
    }
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
pub fn add_to_profile(text: &str, profile: &str, field: &str, name: &str) -> Result<String> {
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

/// Create a profile (a named bundle of servers + skills) in the manifest from a
/// JSON args object. Used by the MCP `create_profile` tool and the session
/// helper — a manifest-only write, no configs rendered.
pub fn add_profile_json(manifest_dir: Option<&Path>, args: &Value) -> Result<String> {
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .context("profile name is required")?;
    let names = |key: &str| -> Vec<String> {
        args.get(key)
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    };
    let servers = names("servers");
    let skills = names("skills");
    if servers.is_empty() && skills.is_empty() {
        anyhow::bail!("pick at least one skill or server for the profile");
    }

    let base = crate::commands::project_base(manifest_dir)?;
    let dir = crate::manifest::resolve_manifest_dir(&base);
    let manifest_path = dir.join(crate::manifest::load::MANIFEST_FILE);
    let original = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("reading {}", manifest_path.display()))?;
    let parsed: Manifest = toml::from_str(&original).context("parsing manifest")?;
    if parsed.profiles.contains_key(name) {
        anyhow::bail!("profile '{name}' already exists");
    }

    // Build the profile table by appending each member; the first append
    // creates [profiles.<name>] and its arrays.
    let mut text = original;
    for s in &servers {
        text = add_to_profile(&text, name, "servers", s)?;
    }
    for s in &skills {
        text = add_to_profile(&text, name, "skills", s)?;
    }
    toml::from_str::<Manifest>(&text).context("resulting manifest would be invalid")?;
    crate::util::atomic::write(&manifest_path, &text)
        .with_context(|| format!("writing {}", manifest_path.display()))?;
    Ok(name.to_string())
}

/// Add a skill to the manifest from a JSON args object (git URL or local path).
/// Manifest-only — the caller then installs it and wires it into a CLI. Used by
/// the MCP `add_skill` tool.
pub fn add_skill_json(manifest_dir: Option<&Path>, args: &Value) -> Result<String> {
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .context("skill name is required")?;
    crate::text::validate_name(name)?;
    let str_field = |key: &str| -> Option<String> {
        args.get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(String::from)
    };
    let source = args.get("source").and_then(Value::as_str).unwrap_or("git");
    let skill = if source == "path" {
        Skill {
            path: str_field("path"),
            git: None,
            rev: None,
            subpath: None,
        }
    } else {
        Skill {
            path: None,
            git: str_field("git"),
            rev: str_field("rev"),
            subpath: str_field("subpath"),
        }
    };
    match source {
        "path" if skill.path.is_none() => anyhow::bail!("a path-sourced skill needs a path"),
        "git" if skill.git.is_none() => anyhow::bail!("a git-sourced skill needs a git URL"),
        _ => {}
    }

    let base = crate::commands::project_base(manifest_dir)?;
    let dir = crate::manifest::resolve_manifest_dir(&base);
    let manifest_path = dir.join(crate::manifest::load::MANIFEST_FILE);
    let original = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("reading {}", manifest_path.display()))?;
    let parsed: Manifest = toml::from_str(&original).context("parsing manifest")?;
    if parsed.skills.contains_key(name) {
        anyhow::bail!("skill '{name}' already exists");
    }
    let body = serde_json::to_value(&skill)?;
    let new_text = build_manifest_with(&original, "skills", name, &body, None)?;
    crate::util::atomic::write(&manifest_path, &new_text)
        .with_context(|| format!("writing {}", manifest_path.display()))?;
    Ok(name.to_string())
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

    /// `--target` scopes the added server to named CLIs (persisted as
    /// `targets = [...]`), and a typo'd adapter id refuses up front instead
    /// of silently rendering the server nowhere.
    #[test]
    fn add_server_target_flag_scopes_and_validates() {
        let _g = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", tmp.path().join("home"));
        fs::write(tmp.path().join("agentstack.toml"), "version = 1\n").unwrap();

        let args = |targets: Vec<String>| crate::cli::AddServerArgs {
            name: "tldraw".into(),
            transport: crate::manifest::ServerType::Stdio,
            url: None,
            headers: vec![],
            command: Some("node".into()),
            args: vec!["dist/index.js".into()],
            cwd: None,
            env: vec![],
            profile: None,
            targets,
            write: true,
        };

        let err = add_server(&args(vec!["claude-kode".into()]), Some(tmp.path())).unwrap_err();
        assert!(
            err.to_string().contains("unknown target 'claude-kode'"),
            "{err:#}"
        );

        add_server(&args(vec!["claude-code".into()]), Some(tmp.path())).unwrap();
        let text = fs::read_to_string(tmp.path().join("agentstack.toml")).unwrap();
        let m: Manifest = toml::from_str(&text).unwrap();
        assert_eq!(m.servers["tldraw"].targets, vec!["claude-code"]);

        std::env::remove_var("AGENTSTACK_HOME");
    }

    /// D5 witness: `set server` is create-or-update — an existing name is
    /// rewritten in place — while `add server` still refuses it, now pointing
    /// at `set server` as the update path.
    #[test]
    fn set_server_updates_in_place_where_add_refuses() {
        let _g = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", tmp.path().join("home"));
        fs::write(
            tmp.path().join("agentstack.toml"),
            "version = 1\n\n[servers.demo]\ntype = \"http\"\nurl = \"https://old.example.com\"\n",
        )
        .unwrap();

        let args = crate::cli::AddServerArgs {
            name: "demo".into(),
            transport: crate::manifest::ServerType::Http,
            url: Some("https://new.example.com".into()),
            headers: vec![],
            command: None,
            args: vec![],
            cwd: None,
            env: vec![],
            profile: None,
            targets: vec![],
            write: true,
        };

        let err = add_server(&args, Some(tmp.path())).unwrap_err();
        assert!(err.to_string().contains("agentstack set server"), "{err:#}");

        upsert_server(&args, Some(tmp.path()), true).unwrap();
        let m: Manifest =
            toml::from_str(&fs::read_to_string(tmp.path().join("agentstack.toml")).unwrap())
                .unwrap();
        assert_eq!(
            m.servers["demo"].url.as_deref(),
            Some("https://new.example.com")
        );

        std::env::remove_var("AGENTSTACK_HOME");
    }

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

    fn linear_pack_spec() -> PackSpec {
        PackSpec {
            server: Some(provider::Install::Http {
                url: "https://mcp.linear.app/mcp".into(),
                secret_headers: vec!["Authorization".into()],
            }),
            skills: vec![SkillRef {
                name: "linear_breakdown".into(),
                path: Some("skills/linear/breakdown".into()),
                git: None,
                rev: None,
            }],
            instructions: vec![InstrRef {
                name: "linear_rules".into(),
                path: "instructions/linear/rules.md".into(),
            }],
            targets: vec!["*".into()],
        }
    }

    #[test]
    fn pack_policy_forbids_member() {
        let manifest: Manifest =
            toml::from_str("version = 1\n[policy]\nforbid = [\"linear_breakdown\"]\n").unwrap();
        let err = check_pack_policy(
            &manifest,
            "linear-pack",
            &linear_pack_spec(),
            "catalog:linear-pack",
        )
        .unwrap_err();
        assert!(err.to_string().contains("linear_breakdown"));
        assert!(err.to_string().contains("forbids"));
    }

    #[test]
    fn pack_policy_rejects_unallowed_source() {
        let manifest: Manifest = toml::from_str(
            "version = 1\n[policy]\nallowed_sources = [\"git:github.com/acme/*\"]\n",
        )
        .unwrap();
        // The pack server source `catalog:linear-pack` isn't in the allowlist.
        let err = check_pack_policy(
            &manifest,
            "linear-pack",
            &linear_pack_spec(),
            "catalog:linear-pack",
        )
        .unwrap_err();
        assert!(err.to_string().contains("allowed_sources"));
    }

    #[test]
    fn pack_policy_allows_when_empty() {
        let manifest: Manifest = toml::from_str("version = 1\n").unwrap();
        check_pack_policy(
            &manifest,
            "linear-pack",
            &linear_pack_spec(),
            "catalog:linear-pack",
        )
        .unwrap();
    }

    #[test]
    fn stamped_instruction_carries_provenance() {
        let instr = InstrRef {
            name: "linear_rules".into(),
            path: "instructions/linear/rules.md".into(),
        };
        let out = stamped_instruction_from("linear-pack", &instr, &AssetSource::Embedded).unwrap();
        assert!(out.starts_with("<!-- agentstack:vendor linear-pack (unofficial) -->"));
        assert!(out.contains("# vendor: linear-pack (unofficial)"));
    }

    /// Finding 3: a local skill path is stored relative to the MANIFEST dir,
    /// so it resolves correctly no matter which cwd the command ran from.
    #[test]
    fn manifest_rel_path_rebases_against_the_manifest_dir() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let proj = tmp.path().join("proj");
        let manifest_dir = proj.join(".agentstack");
        let skill = proj.join("my-skill");
        std::fs::create_dir_all(&manifest_dir).unwrap();
        std::fs::create_dir_all(&skill).unwrap();

        // A skill in the project root is one level up from `.agentstack/`.
        assert_eq!(manifest_rel_path(&manifest_dir, &skill), "../my-skill");
        // A skill under the manifest dir stays a plain relative path.
        let nested = manifest_dir.join("skills/x");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(manifest_rel_path(&manifest_dir, &nested), "skills/x");
    }
}
