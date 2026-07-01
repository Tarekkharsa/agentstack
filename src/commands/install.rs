//! `agentstack install` / `update` — resolve skill sources into the store and
//! maintain `agentstack.lock` (PLAN §9d). `install` is reproducible (prefers the
//! locked rev); `update` re-resolves git skills to their latest.

use std::path::Path;

use anyhow::Result;
use owo_colors::OwoColorize;

use crate::cli::{InstallArgs, UpdateArgs};
use crate::lock::{Lock, LockedSkill};
use crate::manifest::SkillSource;
use crate::scan::Severity;
use crate::store::Store;

pub fn run(args: &InstallArgs, manifest_dir: Option<&Path>) -> Result<()> {
    sync(manifest_dir, false, None, args.locked, args.allow_flagged)
}

pub fn run_update(args: &UpdateArgs, manifest_dir: Option<&Path>) -> Result<()> {
    sync(
        manifest_dir,
        args.name.is_none(),
        args.name.as_deref(),
        false,
        false,
    )
}

/// Resolve every skill, fetching sources as needed and reconciling the lockfile.
/// `relock_all` ignores pinned revs for all skills; `only` does so for one.
fn sync(
    manifest_dir: Option<&Path>,
    relock_all: bool,
    only: Option<&str>,
    locked: bool,
    allow_flagged: bool,
) -> Result<()> {
    let ctx = super::load(manifest_dir)?;
    let manifest = &ctx.loaded.manifest;
    let store = Store::default_store();
    let mut lock = Lock::load(&ctx.dir)?;

    if manifest.skills.is_empty() {
        println!("Manifest defines no skills — nothing to install.");
        return Ok(());
    }

    let mut changed = false;
    let mut errors = 0;

    for (name, skill) in &manifest.skills {
        let relock = relock_all || only == Some(name.as_str());
        let pinned = if relock {
            None
        } else {
            lock.get(name).and_then(|l| l.rev.clone())
        };

        let resolved = match store.resolve(skill, &ctx.dir, pinned.as_deref()) {
            Ok(r) => r,
            Err(e) => {
                println!("  {} {name}: {e}", "✗".red());
                errors += 1;
                continue;
            }
        };

        // Supply-chain gate: scan the resolved content before locking it in.
        // High findings (hidden Unicode) block this skill — same philosophy as
        // unresolved secrets blocking writes; Warn findings advise, never block.
        if resolved.path.exists() {
            let findings = match crate::scan::scan_tree(&resolved.path) {
                Ok(f) => f,
                Err(e) => {
                    println!("  {} {name}: content scan failed: {e}", "✗".red());
                    errors += 1;
                    continue;
                }
            };
            let high = findings
                .iter()
                .filter(|f| f.severity == Severity::High)
                .count();
            for f in &findings {
                let mark = if f.severity == Severity::High && !allow_flagged {
                    "✗".red().to_string()
                } else {
                    "⚠".yellow().to_string()
                };
                println!("  {mark} {name}: {}", f.describe());
            }
            if high > 0 && !allow_flagged {
                println!(
                    "  {} {name}: {high} high-severity finding(s) — install blocked \
                     (pass --allow-flagged to install anyway)",
                    "✗".red()
                );
                errors += 1;
                continue;
            }
        }

        let entry = locked_entry(name, skill, &resolved)?;
        let fetched_note = if resolved.fetched { " (fetched)" } else { "" };

        match lock.get(name) {
            Some(prev) if prev == &entry && !relock => {
                println!("  {} {name} cached ({})", "✓".green(), resolved.source_kind);
            }
            Some(_) => {
                if locked {
                    println!("  {} {name}: lockfile out of date", "✗".red());
                    errors += 1;
                } else {
                    lock.upsert(entry);
                    changed = true;
                    println!("  {} {name} updated{fetched_note}", "↑".cyan());
                }
            }
            None => {
                if locked {
                    println!("  {} {name}: missing from lockfile", "✗".red());
                    errors += 1;
                } else {
                    lock.upsert(entry);
                    changed = true;
                    println!("  {} {name} locked{fetched_note}", "+".green());
                }
            }
        }
    }

    // Drop locked skills no longer in the manifest.
    let keep: Vec<String> = manifest.skills.keys().cloned().collect();
    let before = lock.skills.len();
    lock.retain_names(&keep);
    if lock.skills.len() != before {
        changed = true;
    }

    if locked && (changed || errors > 0) {
        anyhow::bail!("lockfile is out of date — run `agentstack install` to refresh it");
    }
    if !locked && changed {
        lock.save(&ctx.dir)?;
        println!("\n{} wrote {}", "✓".green(), Lock::path(&ctx.dir).display());
    } else if errors == 0 {
        println!("\n{} lockfile up to date.", "✓".green());
    }
    if errors > 0 {
        anyhow::bail!("{errors} skill(s) failed to resolve");
    }
    Ok(())
}

pub(crate) fn locked_entry(
    name: &str,
    skill: &crate::manifest::Skill,
    resolved: &crate::store::Resolved,
) -> Result<LockedSkill> {
    let (source, path, git, rev) = match skill.source()? {
        SkillSource::Path(p) => ("path", Some(p), None, None),
        SkillSource::Git { url, .. } => ("git", None, Some(url), resolved.rev.clone()),
    };
    Ok(LockedSkill {
        name: name.to_string(),
        source: source.to_string(),
        path,
        git,
        rev,
        checksum: resolved.checksum.clone(),
    })
}
