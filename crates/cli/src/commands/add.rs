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

fn add_from(a: &AddFromArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let ctx = super::load(manifest_dir)?;

    // `git:<url>[@<tag>][#subdir]` — a versioned pack from any git host.
    if let Some(git_ref) = crate::provider::gitpack::GitPackRef::parse(&a.id) {
        return add_git_pack(a, &ctx, &git_ref);
    }

    let candidate = provider::resolve(&a.id)
        .with_context(|| format!("no capability '{}' in the catalog or registry", a.id))?;
    println!(
        "{} {} ({}) — {}",
        "found".green(),
        candidate.name.bold(),
        candidate.source,
        candidate.id
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
        resolved.candidate.name.bold(),
        git_ref.url,
        resolved.tag.bold(),
        &resolved.commit[..resolved.commit.len().min(12)],
    );
    add_pack(a, ctx, &resolved.candidate, &resolved.spec, &origin)
}

/// Gate `[policy] allowed_sources` (before any network), then clone + parse +
/// content-scan the pack. Shared by the CLI, dashboard, and MCP install paths.
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
        anyhow::bail!("server '{}' already exists in the manifest", candidate.name);
    }
    let server = candidate.to_server();
    write_manifest(
        ctx,
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

/// Install a library hook by copying its definition into the project's inline
/// `[hooks.<name>]` table. Hooks always render from the manifest (see
/// `render/hooks.rs`), so this is a plain copy — no runtime library indirection —
/// and the definition becomes part of the manifest bytes the trust digest covers.
/// Hooks are global (not profile-scoped), so `--profile` does not apply.
fn add_from_hook(a: &AddFromArgs, ctx: &super::Context, candidate: &Candidate) -> Result<()> {
    if ctx.loaded.manifest.hooks.contains_key(&candidate.name) {
        anyhow::bail!("hook '{}' already exists in the manifest", candidate.name);
    }
    write_manifest(
        ctx,
        "hooks",
        &serde_json::to_value(candidate.to_hook())?,
        None,
        &candidate.name,
        a.write,
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
        anyhow::bail!("skill '{}' already exists in the manifest", candidate.name);
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
        anyhow::bail!("a pack '{pack}' is already installed in the manifest");
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
    let ctx = super::load(manifest_dir)?;
    if ctx.loaded.manifest.servers.contains_key(&a.name) {
        anyhow::bail!("server '{}' already exists in the manifest", a.name);
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
        subpath: None,
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
        crate::util::atomic::write(&ctx.loaded.manifest_path, &new_text)
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

/// The manifest members an install added, by section. Shared return shape so the
/// dashboard/MCP reach server, skill, and pack installs through the same door.
#[derive(Debug, Clone, Default)]
pub struct AddedMembers {
    pub servers: Vec<String>,
    pub skills: Vec<String>,
    pub instructions: Vec<String>,
    pub hooks: Vec<String>,
}

/// Resolve a provider id and write it into the manifest at `dir` (no dry-run).
/// Shared by the dashboard and MCP server. Handles servers, standalone skills,
/// and vendor packs; returns the members it added.
///
/// Instructions are NOT installed here (the daily-driver-steering opt-in lives
/// behind the CLI's `--with-instructions`); pack instructions are reported as
/// available but skipped.
pub fn write_from_provider(dir: &Path, id: &str, profile: Option<&str>) -> Result<AddedMembers> {
    let ctx = super::load(Some(dir))?;

    // Git packs come through the same door (dashboard Discover, MCP).
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

    let candidate = provider::resolve(id)
        .with_context(|| format!("no capability '{id}' in the catalog or registry"))?;
    match &candidate.kind {
        CandidateKind::Server(_) => {
            let manifest = &ctx.loaded.manifest;
            if manifest.servers.contains_key(&candidate.name) {
                anyhow::bail!("server '{}' already exists", candidate.name);
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
                anyhow::bail!("skill '{}' already exists", candidate.name);
            }
            let (entry, asset) = skill_entry(skill)?;
            if let Some(asset) = &asset {
                if ctx.dir.join(asset).exists() {
                    anyhow::bail!(
                        "destination '{}' already exists",
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
                anyhow::bail!("hook '{}' already exists", candidate.name);
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
}
