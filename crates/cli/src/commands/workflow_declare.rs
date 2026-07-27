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
//! **What "every write" means** (review finding F21). The undo set is the
//! script, the blueprint, the manifest, *and* `agentstack.lock` — the lockfile
//! is rewritten by the `lock` call this command ends with, so a capture list
//! that named only the files declare authors left the workflow's pin and
//! blueprint checksum behind on undo. And the durable entry is recorded BEFORE
//! the first write rather than after the last, because the ledger is the only
//! thing that survives a kill in the middle; a clean failure that rolls itself
//! back discards the entry again so the ledger head stays the user's real last
//! change.
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
    // back" means for a create.
    //
    // The LOCKFILE is captured with them. `lock::run` below rewrites it, and a
    // capture list that omitted it made `restore --last --write` a liar: it
    // removed the three staged files and left the workflow's pin and blueprint
    // checksum behind in `agentstack.lock` (review finding F21). The undo set
    // is every file this transaction can touch, not every file it authors.
    //
    // The entry is recorded BEFORE the first write, not after the last. The
    // durable ledger is the only thing that survives a kill between two
    // writes, so recording on success made "one restore entry" true for every
    // outcome except the one it was for. Recording early is safe because the
    // records are declarative — replaying them over an already-clean project
    // is a no-op — so the only cost is an entry to discard when the in-process
    // rollback succeeds and makes it redundant.
    let lock_path = crate::lock::Lock::path(&ctx.dir);
    let mut undo: Vec<crate::history::FileChange> = staged
        .iter()
        .map(|s| crate::history::capture(&s.path, s.label))
        .collect();
    undo.push(crate::history::capture(&lock_path, "lockfile"));

    // Recording is a PRECONDITION, not a courtesy. History is best-effort
    // almost everywhere in this codebase — an unwritable ledger must never
    // fail an otherwise-good apply — but this command's contract is that the
    // declaration is undoable in one step. Proceeding without a durable entry
    // would deliver the writes and silently drop the guarantee that justifies
    // them. So it is written first, and its failure stops the transaction
    // while the project is still untouched.
    //
    // Note the ordering: this happens before `create_dir_all`, so a refusal
    // here does not even leave an empty `workflows/` behind.
    let recorded = crate::history::record("workflow-declare", vec![name.to_string()], undo.clone())
        .and_then(|id| {
            id.context("the undo ledger reported nothing recorded, but files were staged")
        })
        .with_context(|| {
            format!(
                "refusing to declare '{name}': the undo entry could not be written to {}, and \
                 declaring without one would leave no way back. Fix the permissions on that \
                 directory (or AGENTSTACK_HOME) and re-run — nothing has been changed.",
                crate::history::dir().display()
            )
        })?;

    let created_dest_dir = !dest_dir.exists();
    std::fs::create_dir_all(&dest_dir)
        .with_context(|| format!("creating {}", dest_dir.display()))?;

    let outcome = (|| -> Result<usize> {
        for s in &staged {
            fail_point(s.label)?;
            crate::util::atomic::write(&s.path, &s.contents)
                .with_context(|| format!("writing {}", s.path.display()))?;
        }
        fail_point("lock")?;
        // Re-lock through the ordinary path so the new script AND blueprint
        // are pinned exactly as any other lock would pin them.
        crate::commands::lock::run(&crate::cli::LockArgs::default(), Some(&ctx.dir))?;
        // A failure point AFTER the lock has actually rewritten the lockfile:
        // the only one that exercises rolling back a MODIFIED lockfile rather
        // than deleting a created one.
        fail_point("after-lock")?;
        Ok(staged.len())
    })();

    match outcome {
        Ok(n) => {
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
            if created_dest_dir {
                // Only if we made it and nothing else moved in: "byte-identical
                // to before" should not leave an empty directory behind.
                let _ = std::fs::remove_dir(&dest_dir);
            }
            if reverted == undo.len() {
                // Fully reverted, so the pre-recorded entry now describes a
                // state that already holds. Drop it rather than leave a no-op
                // shadowing the user's real last change in `restore --last`.
                crate::history::discard(&recorded);
                bail!(
                    "declaring workflow '{name}' failed, and every change was rolled back \
                     ({reverted} file(s) restored) — the project is as it was.\n\nWhat failed: {e:#}"
                );
            }
            // A partial rollback is the case the durable entry exists for: say
            // so, and point at the ledger rather than claiming it is undone.
            bail!(
                "declaring workflow '{name}' failed, and rolling it back was INCOMPLETE \
                 ({reverted} of {} file(s) restored) — the project is NOT as it was.\n\n\
                 Finish the undo with:  agentstack restore --last --write\n\nWhat failed: {e:#}",
                undo.len()
            );
        }
    }
}

/// Injected-failure hook for the transaction's witnesses.
///
/// Fault injection is the only way to test "a failure at step four leaves the
/// project byte-identical", and the failure has to happen BETWEEN two real
/// writes — which no public API can arrange from outside. Compiled only under
/// `cfg(test)`, so the release binary has no branch here at all.
#[cfg(test)]
fn fail_point(label: &str) -> Result<()> {
    // Snapshot the ledger every time the hook is reached, so a test can assert
    // what a process killed at exactly this point would have left behind. This
    // is the only way to witness "the entry is durable BEFORE the first write"
    // — after the call returns, the observation is indistinguishable from one
    // taken at the end.
    let entries = crate::history::list().len();
    LEDGER_AT.with(|l| l.borrow_mut().push((label.to_string(), entries)));
    FAIL_AT.with(|f| match f.borrow().as_deref() {
        Some(at) if at == label => bail!("injected failure at '{label}'"),
        _ => Ok(()),
    })
}

#[cfg(not(test))]
fn fail_point(_label: &str) -> Result<()> {
    Ok(())
}

#[cfg(test)]
thread_local! {
    static FAIL_AT: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
    /// `(step, history entries on disk)` at each fail point reached this run.
    static LEDGER_AT: std::cell::RefCell<Vec<(String, usize)>> =
        const { std::cell::RefCell::new(Vec::new()) };
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A project with one role defined, a script, and a blueprint on disk.
    /// Returns (tempdir, manifest dir, script path, blueprint path).
    fn project() -> (assert_fs::TempDir, PathBuf, PathBuf, PathBuf) {
        let tmp = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", tmp.path().join("home"));
        let dir = tmp.path().to_path_buf();
        fs::write(
            dir.join("agentstack.toml"),
            "version = 1\n\n[profiles.reviewer]\nskills = []\nservers = []\n",
        )
        .unwrap();
        let script = dir.join("src.js");
        fs::write(&script, "export const meta = {name:'x'}\n").unwrap();
        let blueprint = dir.join("src.blueprint.json");
        fs::write(&blueprint, r#"{"nodes":[]}"#).unwrap();
        (tmp, dir, script, blueprint)
    }

    fn args(name: &str, script: &Path, blueprint: &Path, write: bool) -> WorkflowDeclareArgs {
        WorkflowDeclareArgs {
            name: name.into(),
            script: script.to_path_buf(),
            blueprint: Some(blueprint.to_path_buf()),
            roles: vec!["reviewer".into()],
            max_agents: None,
            max_wall_seconds: None,
            preview: !write,
            write,
        }
    }

    /// F21 witness — the undo set covers the LOCKFILE, not just the files
    /// declare authors. Before the fix, `restore --last --write` deleted the
    /// script, blueprint, and manifest entry but left the workflow's pin and
    /// blueprint checksum behind in `agentstack.lock`.
    #[test]
    fn undoing_a_successful_declare_also_reverts_the_lockfile() {
        let _g = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let (_tmp, dir, script, blueprint) = project();
        let lock_path = crate::lock::Lock::path(&dir);
        let lock_before = fs::read_to_string(&lock_path).unwrap_or_default();
        let manifest_before = fs::read_to_string(dir.join("agentstack.toml")).unwrap();

        run(Some(&dir), &args("triage", &script, &blueprint, true)).unwrap();

        let lock_after = fs::read_to_string(&lock_path).unwrap_or_default();
        assert!(
            lock_after.contains("triage"),
            "the declare should have pinned the workflow: {lock_after}"
        );

        let entry = crate::history::list()
            .into_iter()
            .find(|e| e.scope == "workflow-declare")
            .expect("declare recorded one restore entry");
        assert!(
            entry
                .files
                .iter()
                .any(|f| f.path == lock_path.to_string_lossy()),
            "the lockfile must be in the undo set: {:?}",
            entry.files.iter().map(|f| &f.label).collect::<Vec<_>>()
        );

        crate::history::undo(&entry.id).unwrap();

        assert_eq!(
            fs::read_to_string(&lock_path).unwrap_or_default(),
            lock_before,
            "the lockfile must be back to its pre-declare bytes"
        );
        assert_eq!(
            fs::read_to_string(dir.join("agentstack.toml")).unwrap(),
            manifest_before
        );
        assert!(!dir.join("workflows/triage.js").exists());
        assert!(!dir.join("workflows/triage.blueprint.json").exists());
        std::env::remove_var("AGENTSTACK_HOME");
    }

    /// F21 witness — a failure injected at EACH write leaves the project
    /// byte-identical, and leaves no no-op entry at the head of the ledger.
    #[test]
    fn a_failure_at_any_step_rolls_back_completely() {
        let _g = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for step in [
            "workflow script",
            "approved blueprint",
            "manifest",
            "lock",
            "after-lock",
        ] {
            let (_tmp, dir, script, blueprint) = project();
            let manifest_before = fs::read_to_string(dir.join("agentstack.toml")).unwrap();
            let lock_path = crate::lock::Lock::path(&dir);
            let lock_before = fs::read_to_string(&lock_path).unwrap_or_default();

            FAIL_AT.with(|f| *f.borrow_mut() = Some(step.to_string()));
            let err = run(Some(&dir), &args("triage", &script, &blueprint, true)).unwrap_err();
            FAIL_AT.with(|f| *f.borrow_mut() = None);

            assert!(
                err.to_string().contains("the project is as it was"),
                "step '{step}' should report a complete rollback: {err:#}"
            );
            assert_eq!(
                fs::read_to_string(dir.join("agentstack.toml")).unwrap(),
                manifest_before,
                "manifest changed after failure at '{step}'"
            );
            assert_eq!(
                fs::read_to_string(&lock_path).unwrap_or_default(),
                lock_before,
                "lockfile changed after failure at '{step}'"
            );
            assert!(
                !dir.join("workflows/triage.js").exists(),
                "script survived failure at '{step}'"
            );
            assert!(
                !dir.join("workflows/triage.blueprint.json").exists(),
                "blueprint survived failure at '{step}'"
            );
            assert!(
                !crate::history::list()
                    .iter()
                    .any(|e| e.scope == "workflow-declare"),
                "a fully rolled-back declare must not leave an entry at the head \
                 of the ledger (step '{step}')"
            );
        }
        std::env::remove_var("AGENTSTACK_HOME");
    }

    /// F21 witness — the durable entry is on disk BEFORE the first write, so a
    /// process killed between two writes still has a way back. The fail-point
    /// hook snapshots the ledger at each step; a kill at that instant would
    /// leave exactly what the snapshot saw.
    #[test]
    fn the_undo_entry_is_durable_before_the_first_write() {
        let _g = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let (_tmp, dir, script, blueprint) = project();
        assert_eq!(crate::history::list().len(), 0, "the ledger starts empty");

        LEDGER_AT.with(|l| l.borrow_mut().clear());
        // Fail at the LAST step, so the hook is reached at every earlier one.
        FAIL_AT.with(|f| *f.borrow_mut() = Some("lock".to_string()));
        let _ = run(Some(&dir), &args("triage", &script, &blueprint, true));
        FAIL_AT.with(|f| *f.borrow_mut() = None);

        let observed = LEDGER_AT.with(|l| l.borrow().clone());
        assert_eq!(
            observed.len(),
            4,
            "every step should have been reached: {observed:?}"
        );
        for (step, entries) in &observed {
            assert_eq!(
                *entries, 1,
                "a kill at '{step}' must leave exactly one durable undo entry \
                 — recording after the writes would leave zero: {observed:?}"
            );
        }

        // And the entry that a kill would have left is complete: it covers the
        // lockfile too, so the crash recovery is not a partial one.
        LEDGER_AT.with(|l| l.borrow_mut().clear());
        run(Some(&dir), &args("triage", &script, &blueprint, true)).unwrap();
        let entry = crate::history::list()
            .into_iter()
            .find(|e| e.scope == "workflow-declare")
            .expect("one durable entry");
        assert_eq!(
            entry.files.len(),
            4,
            "script + blueprint + manifest + lockfile"
        );
        std::env::remove_var("AGENTSTACK_HOME");
    }

    /// F21 witness — recording is a PRECONDITION. With the ledger unwritable,
    /// declare must refuse BEFORE it writes anything, rather than deliver the
    /// files and silently drop the undo guarantee that justifies them.
    #[test]
    fn declare_refuses_when_the_undo_entry_cannot_be_written() {
        let _g = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let (tmp, dir, script, blueprint) = project();

        // Make the history directory unwritable by occupying its path with a
        // regular file — `create_dir_all` then fails, so `record` cannot land.
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();
        fs::write(home.join("history"), "not a directory").unwrap();

        let err = run(Some(&dir), &args("triage", &script, &blueprint, true)).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("refusing to declare"), "{msg}");
        assert!(msg.contains("no way back"), "{msg}");
        assert!(msg.contains("nothing has been changed"), "{msg}");

        // And nothing was: no files, no manifest entry, not even the directory.
        assert!(!dir.join("workflows").exists(), "no workflows/ dir created");
        let manifest = fs::read_to_string(dir.join("agentstack.toml")).unwrap();
        assert!(
            !manifest.contains("triage"),
            "manifest untouched: {manifest}"
        );
        std::env::remove_var("AGENTSTACK_HOME");
    }
}
