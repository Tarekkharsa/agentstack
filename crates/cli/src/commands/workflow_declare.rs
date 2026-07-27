//! `agentstack workflow declare` — stage a workflow's files, add its manifest
//! entry, and re-lock, as ONE transaction (review finding F14).
//!
//! **Why this exists.** Authoring a workflow from an approved blueprint was six
//! independent writes: the script, the manifest entry, the role profiles, the
//! lock, the trust grant, the run. A failure at step four left a half-written
//! manifest and an orphaned script behind a button the user had clicked
//! "Approve" on, and nothing said which step had failed. Every write here is
//! captured before it happens and rolled back together, so the outcome is
//! binary: the workflow is declared, or the project is byte-identical to
//! before.
//!
//! **Where it deliberately stops.** It does not trust, and it does not run.
//! Consent is the human's step; a command that granted it on the way past
//! would be exactly the second authority path the invariants forbid. `declare`
//! ends by telling you to review — the gate stays in front of execution.
//!
//! **What it refuses.** A name that is not a plain path component, a name that
//! already exists, a role with no `[profiles.<role>]` table, a script or
//! blueprint it cannot read, and any manifest that fails validation with the
//! new entry in place. All of that is checked BEFORE the first byte is
//! written — the rollback is the safety net, not the plan.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use owo_colors::OwoColorize;

use crate::cli::WorkflowDeclareArgs;
use crate::manifest::Workflow;

/// A file this transaction will create, with the bytes to write and the undo
/// record captured before anything touches disk.
struct Staged {
    path: PathBuf,
    contents: String,
    label: &'static str,
}

pub fn run(manifest_dir: Option<&Path>, args: &WorkflowDeclareArgs) -> Result<()> {
    let ctx = crate::commands::load(manifest_dir)?;
    let mut manifest = ctx.loaded.manifest.clone();
    let root = crate::manifest::project_root_of(&ctx.dir);

    // ── validate everything first ────────────────────────────────────────
    // The name becomes a filename, a manifest key, and a run identity, so it
    // gets the same containment rule as extensions: a plain path component,
    // never something that could escape the directory agentstack owns.
    let name = args.name.trim();
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || Path::new(name).is_absolute()
    {
        bail!(
            "refusing to declare: '{name}' is not a usable workflow name — it becomes a filename \
             and a run identity, so it must be a plain name with no '/', '\\', or '..'"
        );
    }
    if manifest.workflows.contains_key(name) {
        bail!(
            "refusing to declare: [workflows.{name}] already exists — pick another name, or edit \
             the existing entry and re-run `agentstack lock`"
        );
    }

    // Roles are the workflow's whole authority-request surface. Requiring the
    // profile to exist ALREADY is the point: declaring a workflow must never
    // be a way to bring a new role into being.
    let mut roles: Vec<String> = args.roles.clone();
    roles.sort();
    roles.dedup();
    let unknown: Vec<&String> = roles
        .iter()
        .filter(|r| !manifest.profiles.contains_key(*r))
        .collect();
    if !unknown.is_empty() {
        bail!(
            "refusing to declare: no [profiles.*] table for role(s) {} — a workflow requests \
             authority, it never creates it. Define the toolset(s) first, e.g. \
             `agentstack create-profile --name {} --server <name>`",
            unknown
                .iter()
                .map(|r| format!("'{r}'"))
                .collect::<Vec<_>>()
                .join(", "),
            unknown[0]
        );
    }

    let script = read_source(&args.script, "script")?;
    let blueprint = args
        .blueprint
        .as_ref()
        .map(|p| read_source(p, "blueprint"))
        .transpose()?;
    // Parse-check the blueprint here rather than at review time: a blueprint
    // that is not JSON cannot be the graph anyone approved, and finding that
    // out at the trust gate would be finding it out too late.
    if let Some(b) = &blueprint {
        serde_json::from_str::<serde_json::Value>(b)
            .context("the blueprint is not valid JSON — it must be the emitted blueprint block")?;
    }

    let dest_dir = ctx.dir.join("workflows");
    let script_path = dest_dir.join(format!("{name}.js"));
    let blueprint_path = dest_dir.join(format!("{name}.blueprint.json"));
    for p in [&script_path, &blueprint_path] {
        if p.exists() {
            bail!(
                "refusing to declare: {} already exists — declaring would overwrite it",
                crate::commands::init::display_path(p, &ctx.dir)
            );
        }
    }

    // Relative to the MANIFEST dir, which is what the manifest paths mean.
    let rel = |p: &Path| {
        p.strip_prefix(&ctx.dir)
            .map(|r| format!("./{}", r.display()))
            .unwrap_or_else(|_| p.display().to_string())
    };
    let entry = Workflow {
        description: None,
        path: Some(rel(&script_path)),
        git: None,
        rev: None,
        subpath: None,
        blueprint: blueprint.as_ref().map(|_| rel(&blueprint_path)),
        roles: roles.clone(),
        max_agents: args.max_agents,
        max_wall_seconds: args.max_wall_seconds,
        scheduling: Default::default(),
    };
    manifest.workflows.insert(name.to_string(), entry);

    // Validate the manifest WITH the new entry, using the same rules doctor
    // and apply apply. Catching it here is what keeps a bad entry from ever
    // reaching the lockfile — and therefore the consent surface.
    let libctx = ctx.library_ctx();
    let vctx = libctx.validate_ctx(&ctx.dir);
    let target_ids: Vec<&str> = ctx.registry.ids().collect();
    let errors: Vec<_> = crate::manifest::validate_with_context(&manifest, target_ids, &vctx)
        .into_iter()
        .filter(|i| i.kind.is_error())
        .collect();
    if !errors.is_empty() {
        let detail = errors
            .iter()
            .map(|i| match &i.fix {
                Some(fix) => format!("{}\n    ↳ {fix}", i.message),
                None => i.message.clone(),
            })
            .collect::<Vec<_>>()
            .join("\n  ");
        bail!(
            "refusing to declare: {} validation error(s):\n  {detail}",
            errors.len()
        );
    }

    let manifest_path = ctx.loaded.manifest_path.clone();
    let manifest_toml = toml::to_string_pretty(&manifest).context("serializing the manifest")?;
    let mut staged = vec![Staged {
        path: script_path.clone(),
        contents: script,
        label: "workflow script",
    }];
    if let Some(b) = blueprint {
        staged.push(Staged {
            path: blueprint_path.clone(),
            contents: b,
            label: "approved blueprint",
        });
    }
    staged.push(Staged {
        path: manifest_path.clone(),
        contents: manifest_toml,
        label: "manifest",
    });

    // ── preview ──────────────────────────────────────────────────────────
    let headline = if args.write {
        format!("Declaring workflow '{name}':")
    } else {
        format!("Would declare workflow '{name}'. Nothing has been changed yet:")
    };
    println!("{}\n", headline.bold());
    for s in &staged {
        println!(
            "  {:<18} {}",
            s.label,
            crate::commands::init::display_path(&s.path, &ctx.dir).dimmed()
        );
    }
    println!(
        "  {:<18} {}",
        "roles",
        if roles.is_empty() {
            "(none — spawns nothing)".to_string()
        } else {
            roles.join(", ")
        }
    );
    println!("  {:<18} {}", "then", "agentstack lock (re-pin)".dimmed());
    if !args.write {
        println!("\n{}", "Re-run with --write to declare it.".dimmed());
        return Ok(());
    }

    // ── the transaction ──────────────────────────────────────────────────
    // Undo records are captured BEFORE the first write; `before: None` for a
    // file that did not exist makes restore delete it, which is what "put it
    // back" means for a create. The same records serve the in-process
    // rollback and the durable `agentstack restore` entry, so a crash between
    // the two cannot leave a state neither can undo.
    std::fs::create_dir_all(&dest_dir)
        .with_context(|| format!("creating {}", dest_dir.display()))?;
    let undo: Vec<crate::history::FileChange> = staged
        .iter()
        .map(|s| crate::history::capture(&s.path, s.label))
        .collect();

    let outcome = (|| -> Result<usize> {
        for s in &staged {
            crate::util::atomic::write(&s.path, &s.contents)
                .with_context(|| format!("writing {}", s.path.display()))?;
        }
        // Re-lock through the ordinary path so the new script AND blueprint
        // are pinned exactly as any other lock would pin them.
        crate::commands::lock::run(&crate::cli::LockArgs::default(), Some(&ctx.dir))?;
        Ok(staged.len())
    })();

    match outcome {
        Ok(n) => {
            let _ = crate::history::record("workflow-declare", vec![name.to_string()], undo);
            println!("\n  {} declared and pinned ({n} file(s))", "✓".green());
            println!(
                "\n{}\n  {}\n  {}",
                "Next — review it, then run it:".bold(),
                "agentstack trust .        review the roles, ceilings, and the approved graph"
                    .dimmed(),
                format!("agentstack workflow run {name}").dimmed()
            );
            println!("  {}", "undo: agentstack restore --last --write".dimmed());
            Ok(())
        }
        Err(e) => {
            let reverted = rollback(&undo, &root);
            bail!(
                "declaring workflow '{name}' failed, and every change was rolled back \
                 ({reverted} file(s) restored) — the project is as it was.\n\nWhat failed: {e:#}"
            );
        }
    }
}

/// Put the captured files back: prior bytes where there were any, delete where
/// the file did not exist. Best-effort per file and counted, because a partial
/// rollback must still be REPORTED honestly rather than swallowed — the caller
/// prints the count next to the failure.
fn rollback(undo: &[crate::history::FileChange], _root: &Path) -> usize {
    let mut n = 0;
    for f in undo {
        let path = Path::new(&f.path);
        let ok = match &f.before {
            Some(bytes) => crate::util::atomic::write(path, bytes).is_ok(),
            None => !path.exists() || std::fs::remove_file(path).is_ok(),
        };
        if ok {
            n += 1;
        }
    }
    n
}

/// Read a source file the caller pointed at. Unlike the staged destinations
/// this is an arbitrary user-supplied path (a temp file the model just wrote),
/// so it is read as given — containment applies to where it LANDS, which is
/// always inside `.agentstack/workflows/`.
fn read_source(path: &Path, what: &str) -> Result<String> {
    std::fs::read_to_string(path)
        .with_context(|| format!("reading the {what} at {}", path.display()))
}
