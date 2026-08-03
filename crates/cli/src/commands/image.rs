//! `agentstack image` — compose one toolset and its pinned capabilities into a
//! self-run container image.
//!
//! `docs/design/packaging.md` is the contract. Two halves, in the shape the
//! rest of the CLI uses: the **plan** ([`crate::image::plan`]) is a read that
//! touches no disk and no daemon and is what a bare invocation prints, and
//! `--write` is the only thing that stages a build context or speaks to
//! Docker.
//!
//! Three properties this file exists to keep true:
//!
//! - **Nothing is written when anything is unclear.** Blockers are collected
//!   for the whole plan, printed with the member each belongs to, and then the
//!   command exits non-zero — before a context directory exists.
//! - **No secret is ever resolved.** There is no resolver in this path. Server
//!   definitions are staged with their `${REF}` placeholders verbatim and the
//!   image carries only the NAMES it will require at run time.
//! - **Docker's absence is reported, not hidden.** The context is staged in
//!   full first, so a machine with no daemon still produces a complete,
//!   finishable artifact and is told exactly what is missing and what to run.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use owo_colors::OwoColorize;

use agentstack_runtime::image::{self as backend, ImageSpec};

use crate::cli::ImageArgs;
use crate::image::{
    ImagePlan, ENTRYPOINT_SH, IMAGE_HOME, PAYLOAD_DIR, PAYLOAD_ROOT, REQUIRED_SECRETS_FILE,
};

pub fn run(args: &ImageArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let ctx = super::load(manifest_dir)?;
    let libctx = ctx.library_ctx();
    let manifest = &ctx.loaded.manifest;

    let toolset = crate::image::resolve_toolset(manifest, args.toolset.as_deref())?;
    let harness = crate::image::resolve_harness(
        manifest,
        &toolset,
        args.harness.as_deref(),
        &ctx.registry,
        &ctx.dir,
    )?;
    let plan = crate::image::plan(
        manifest,
        &ctx.dir,
        &libctx,
        &ctx.registry,
        &toolset,
        &harness,
        args.tag.as_deref(),
        args.from.as_deref(),
    )?;

    let project_root = agentstack_core::manifest::project_root_of(&ctx.dir);
    let trust = crate::trust::check(&project_root);

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&crate::ui_contract::envelope(plan.to_json()))?
        );
    } else {
        show(&plan, trust, args.write);
    }

    // Fail closed on the whole plan before anything is staged: an image is
    // built from reviewed digests, and a member we cannot account for must
    // never be baked in "mostly".
    if !plan.buildable() {
        anyhow::bail!(
            "refusing to build: {} cannot be accounted for from reviewed, pinned bytes",
            super::count(plan.blockers.len(), "member")
        );
    }

    if !args.write {
        if !args.json {
            println!(
                "  {} nothing has been written and Docker has not been contacted — {} builds it.",
                "·".dimmed(),
                "agentstack image --write".bold()
            );
        }
        return Ok(());
    }

    // Invariant 3: baking skill bytes into an image puts them where an agent
    // reads them, which is exactly what an untrusted project may not do.
    anyhow::ensure!(
        trust == crate::trust::TrustState::Trusted,
        "refusing to build: this project's content is {} — an image puts reviewed bytes where \
         an agent reads them, so review it first with `agentstack trust`",
        match trust {
            crate::trust::TrustState::Changed => "changed since you approved it",
            _ => "not approved on this machine",
        }
    );

    stage(&plan, &ctx.loaded.manifest_path, &ctx.dir)?;

    let status = backend::probe();
    if !status.is_available() {
        let argv = backend::build_argv(&plan.context_dir, &plan.tag).join(" ");
        anyhow::bail!(
            "{}\n  the build context is staged and complete, so you can finish it yourself \
             once Docker is available:\n  {argv}",
            status.sentence()
        );
    }

    backend::build(&plan.context_dir, &plan.tag)?;
    println!(
        "  {} built {} — one toolset, {}",
        "✓".green(),
        plan.tag.bold(),
        super::count(plan.members.len(), "pinned member")
    );
    println!(
        "  {} run it under the sandbox contract: {}",
        "·".dimmed(),
        format!(
            "AGENTSTACK_SANDBOX_IMAGE={} agentstack run {} --sandbox",
            plan.tag, plan.harness
        )
        .dimmed()
    );
    Ok(())
}

/// The human plan: everything that would enter the image, then the honesty
/// lines. Deliberately prints members BEFORE blockers — a reader needs to see
/// the composition to understand what the blocker is about.
fn show(plan: &ImagePlan, trust: crate::trust::TrustState, write: bool) {
    let posture = plan.posture();
    println!(
        "  {}  toolset {} for {} → {}",
        "Image".bold(),
        plan.toolset.bold(),
        plan.harness_display,
        plan.tag.bold()
    );
    println!("  {} from {}", "·".dimmed(), plan.base.dimmed());

    if plan.members.is_empty() {
        println!("  {} this toolset selects nothing yet", "·".dimmed());
    }
    let width = plan
        .members
        .iter()
        .map(|m| m.name.len())
        .max()
        .unwrap_or(0)
        .min(32);
    for m in &plan.members {
        let short = &m.digest[..m.digest.len().min(12)];
        println!(
            "  {:<8} {:<width$}  {}  {}",
            m.kind.as_str(),
            m.name,
            short.dimmed(),
            if m.compiled {
                m.dest.dimmed().to_string()
            } else {
                format!("{} (carried)", m.dest).dimmed().to_string()
            }
        );
    }

    if plan.required_secrets.is_empty() {
        println!("  {} no secrets are required at run time", "·".dimmed());
    } else {
        println!(
            "  {} required at run time, never in the image: {}",
            "·".dimmed(),
            plan.required_secrets.join(", ").bold()
        );
    }

    // The posture label, in its shipped spelling, with the caveat attached in
    // the same breath — the two must never be printed apart.
    println!("  {} posture {}", "·".dimmed(), posture.to_string().bold());
    println!("  {} {}", "·".dimmed(), plan.posture_caveat().dimmed());

    if !plan.blockers.is_empty() {
        println!("  {} this plan cannot be built:", "⊘".dimmed());
        for b in &plan.blockers {
            println!("    {} {} — {}", b.kind, b.name.bold(), b.reason);
        }
    }

    if write {
        return;
    }
    match trust {
        crate::trust::TrustState::Trusted => {}
        _ => println!(
            "  {} this project is not approved on this machine — the build will refuse until \
             {}",
            "·".dimmed(),
            "agentstack trust".bold()
        ),
    }
}

/// Stage the complete build context. Every byte written here is either a
/// pinned member copied out of the content store, a fixed constant, or a value
/// derived from the manifest and the lock — never a resolved secret.
fn stage(plan: &ImagePlan, manifest_path: &Path, manifest_dir: &Path) -> Result<()> {
    let ctx_dir = &plan.context_dir;
    // Rebuilt from scratch each time, so a stale member from a previous lock
    // can never ride along. Bounded by construction: `context_dir_for` only
    // ever returns a path under `~/.agentstack/images/<validated-name>/`.
    if ctx_dir.exists() {
        fs::remove_dir_all(ctx_dir).with_context(|| format!("clearing {}", ctx_dir.display()))?;
    }
    let payload = ctx_dir.join(PAYLOAD_DIR);
    fs::create_dir_all(&payload).with_context(|| format!("creating {}", payload.display()))?;

    // The guard, and the data it reads. Names only — `refs_in` yields names and
    // the plan re-validated every one of them.
    let entry_path = payload.join("entrypoint.sh");
    fs::write(&entry_path, ENTRYPOINT_SH)
        .with_context(|| format!("writing {}", entry_path.display()))?;
    make_executable(&entry_path)?;
    let mut refs = plan.required_secrets.join("\n");
    if !refs.is_empty() {
        refs.push('\n');
    }
    fs::write(payload.join(REQUIRED_SECRETS_FILE), refs)?;

    // The manifest and the lock: the declaration and the pins the artifact was
    // built from. Both hold `${REF}` placeholders and no values — that is the
    // property that makes a manifest commit-safe, and it makes it image-safe
    // for the same reason.
    let manifest_out = payload.join("manifest");
    fs::create_dir_all(&manifest_out)?;
    fs::copy(manifest_path, manifest_out.join("agentstack.toml"))
        .with_context(|| format!("copying {}", manifest_path.display()))?;
    let lock_path = agentstack_core::lock::Lock::path(manifest_dir);
    let lock_bytes = fs::read(&lock_path).unwrap_or_default();
    if !lock_bytes.is_empty() {
        fs::write(manifest_out.join("agentstack.lock"), &lock_bytes)?;
    }

    // Server definitions, `${REF}` intact. Data for the runner, deliberately
    // NOT native harness configuration: the one path that writes that resolves
    // secrets into the file it writes.
    let servers_out = payload.join("servers");
    fs::create_dir_all(&servers_out)?;
    for (name, resolved) in &plan.servers {
        let Ok(rs) = resolved else { continue };
        crate::text::validate_name(name)
            .with_context(|| format!("refusing to stage server '{}'", name.escape_debug()))?;
        let text = serde_json::to_string_pretty(&rs.server)?;
        fs::write(servers_out.join(format!("{name}.json")), text)?;
    }

    // Package instruction members: carried, not compiled.
    if !plan.instruction_sources.is_empty() {
        let out = payload.join("instructions");
        fs::create_dir_all(&out)?;
        for (name, src) in &plan.instruction_sources {
            crate::text::validate_name(name).with_context(|| {
                format!("refusing to stage instruction '{}'", name.escape_debug())
            })?;
            fs::copy(src, out.join(name)).with_context(|| format!("copying {}", src.display()))?;
        }
    }

    // Skills, through the SAME materialization seam `use --write` runs — name
    // validation, no-clobber, and pruning semantics come with it rather than
    // being re-implemented. Every name is passed as a `pinned_copy` so the
    // strategy is Copy regardless of what the adapter prefers: a symlink into
    // the host's content store is a dangling path inside an image.
    if let Some(dest) = &plan.skills_dest {
        let rel = dest
            .strip_prefix(&format!("{PAYLOAD_ROOT}/"))
            .with_context(|| format!("skills destination {dest} is outside {PAYLOAD_ROOT}"))?;
        let skills_dir = payload.join(rel);
        let names: Vec<String> = plan.skill_sources.iter().map(|(n, _)| n.clone()).collect();
        let skill_plan = crate::render::skills::plan_with_pinned(
            skills_dir,
            crate::adapter::descriptor::SkillStrategy::Copy,
            plan.skill_sources.clone(),
            &[],
            names,
        )?;
        crate::render::skills::materialize(&skill_plan)?;
    }

    // The descriptor: what is inside, verifiable member by member without
    // running the image.
    let descriptor = descriptor_json(plan, &lock_bytes);
    fs::write(
        payload.join("image.json"),
        serde_json::to_string_pretty(&descriptor)?,
    )?;

    // The Dockerfile — rendered from the backend-agnostic `ImageSpec`, which
    // refuses any value it cannot prove safe rather than escaping it.
    let spec = spec_for(plan, &lock_bytes);
    let dockerfile = spec.dockerfile()?;
    fs::write(ctx_dir.join("Dockerfile"), dockerfile)?;
    Ok(())
}

fn spec_for(plan: &ImagePlan, lock_bytes: &[u8]) -> ImageSpec {
    let posture = plan.posture();
    ImageSpec {
        base: plan.base.clone(),
        tag: plan.tag.clone(),
        payload_dir: PAYLOAD_DIR.to_string(),
        payload_dest: PAYLOAD_ROOT.to_string(),
        labels: vec![
            ("org.agentstack.toolset".into(), plan.toolset.clone()),
            ("org.agentstack.harness".into(), plan.harness.clone()),
            // The honest label, in its shipped machine spelling. A reader who
            // wants the sentence goes to `image.json` or to the design doc —
            // the slug is here so `docker inspect` can classify the artifact.
            ("org.agentstack.posture".into(), posture.slug().to_string()),
            ("org.agentstack.lock".into(), lock_digest(lock_bytes)),
            (
                "org.agentstack.version".into(),
                env!("CARGO_PKG_VERSION").to_string(),
            ),
        ],
        env: vec![("HOME".into(), IMAGE_HOME.to_string())],
        // The same mount point a sandbox run uses, so the image behaves
        // identically whether or not a workspace is mounted over it.
        workdir: crate::commands::sandbox::WORKSPACE.to_string(),
        // Through `/bin/sh` explicitly rather than relying on the file's mode
        // bit: a context staged on a filesystem with no executable bit (or on
        // Windows) would otherwise produce an image whose entrypoint cannot
        // run. The script's own shebang already requires a POSIX shell in the
        // base image, so this adds no dependency.
        entrypoint: vec![
            "/bin/sh".to_string(),
            format!("{PAYLOAD_ROOT}/entrypoint.sh"),
        ],
        cmd: plan.cmd.clone(),
    }
}

fn descriptor_json(plan: &ImagePlan, lock_bytes: &[u8]) -> serde_json::Value {
    let mut body = plan.to_json();
    body["image"]["lock_digest"] = lock_digest(lock_bytes).into();
    body["image"]["built_by"] = env!("CARGO_PKG_VERSION").into();
    body["image"]["payload_root"] = PAYLOAD_ROOT.into();
    body["image"]["home"] = IMAGE_HOME.into();
    body
}

/// `sha256:<hex>` over the lock bytes, or `unlocked` when the project has no
/// lockfile. Names WHICH pin set the artifact was built from, so an image can
/// be tied back to a consent digest without carrying one.
fn lock_digest(lock_bytes: &[u8]) -> String {
    if lock_bytes.is_empty() {
        return "unlocked".to_string();
    }
    format!("sha256:{}", agentstack_core::digest::sha256_hex(lock_bytes))
}

#[cfg(unix)]
fn make_executable(path: &PathBuf) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn make_executable(_path: &PathBuf) -> Result<()> {
    // No mode bit to set here, and nothing depends on one: the image's
    // ENTRYPOINT invokes the script through `/bin/sh` for exactly this reason.
    Ok(())
}
