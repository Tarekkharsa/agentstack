//! `agentstack install` / `update` — resolve skill sources into the store and
//! maintain `agentstack.lock` (PLAN §9d). `install` is reproducible (prefers the
//! locked rev); `update` re-resolves git skills to their latest.

use agentstack_core::digest::Sha256Hex;
use std::path::Path;

use anyhow::Result;
use owo_colors::OwoColorize;

use crate::cli::{InstallArgs, UpdateArgs};
use crate::lock::{Lock, LockedSkill, SkillLockSource};
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

    // Skill names any profile references. In a clean-at-rest manifest these
    // resolve from the central library with no inline [skills.*] entry, and
    // their lock entries (written by `use --write` / `lock`) must survive the
    // reconcile pass below.
    let profile_names = profile_skill_names(manifest);

    if manifest.skills.is_empty() && profile_names.is_empty() {
        println!("Manifest defines no skills — nothing to install.");
        return Ok(());
    }
    if manifest.skills.is_empty() {
        println!(
            "Manifest defines no inline skills; {} profile-referenced skill(s) resolve \
             from the central library — pin them with `agentstack lock`.",
            profile_names.len()
        );
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

        // Relock = the update posture: ignore the pin, require the fetch, and
        // re-track rev-less skills to the remote head (resolve_refresh).
        let resolved = match if relock {
            store.resolve_refresh(skill, &ctx.dir)
        } else {
            store.resolve(skill, &ctx.dir, pinned.as_deref())
        } {
            Ok(r) => r,
            Err(e) => {
                println!(
                    "  {} {name}: {}",
                    "✗".red(),
                    classify_resolve_err(&e, skill)
                );
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
            // Unchanged content reads as cached even under a relock — an
            // update that fetched and found nothing new must not claim
            // "updated" (previously every path skill did, on every update).
            Some(prev) if prev == &entry => {
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

    // Skip-with-reason: profile-referenced (library-backed) names are not
    // re-resolved by this pass — under an update they'd otherwise vanish from
    // the output entirely, reading as "checked" when they weren't.
    for name in &profile_names {
        if manifest.skills.contains_key(name) {
            continue;
        }
        if relock_all || only == Some(name.as_str()) {
            println!(
                "  {} {name} skipped (library-referenced — `agentstack lock` refreshes it)",
                "·".dimmed()
            );
        }
    }

    // Drop locked skills no longer in the manifest. "In the manifest" covers
    // inline [skills.*] entries AND profile-referenced names — a library-backed
    // profile skill has no inline entry, yet its lock pin (from `use --write` /
    // `lock`) must not be silently deleted here.
    let mut keep: Vec<String> = manifest.skills.keys().cloned().collect();
    keep.extend(profile_names);
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

/// Turn a resolve failure into a line that names the likely upstream cause —
/// a deleted repo or a vanished subpath should not read like a generic git
/// hiccup, while reporting skip and deletion reasons.
fn classify_resolve_err(e: &anyhow::Error, skill: &crate::manifest::Skill) -> String {
    let chain = crate::text::sanitize_line(&format!("{e:#}"));
    let lower = chain.to_lowercase();
    if skill.git.is_some() {
        if lower.contains("subpath '") {
            return format!("vanished upstream — {chain} (keep the pin, or remove the skill)");
        }
        // Truly-gone signals get the removal remedy; transient transport
        // failures must NOT suggest removing a skill over a DNS blip.
        let gone = ["not found", "does not exist", "no such device"]
            .iter()
            .any(|s| lower.contains(s));
        let transient = [
            "could not resolve host",
            "could not read from remote",
            "unable to access",
        ]
        .iter()
        .any(|s| lower.contains(s));
        if gone {
            return format!(
                "upstream deleted or moved — {chain} (the lock pin still guards the \
                 cached content; remove the skill to drop it)"
            );
        }
        if transient {
            return format!(
                "upstream not reachable right now — {chain} (check the network and retry; \
                 the lock pin still guards the cached content)"
            );
        }
    }
    chain
}

/// Every skill name any profile references, deduplicated (wildcards expand
/// inline-only, exactly as activation does). Used to keep the lockfile's
/// reconcile pass from pruning library-backed profile skills.
fn profile_skill_names(manifest: &crate::manifest::Manifest) -> Vec<String> {
    let mut out = std::collections::BTreeSet::new();
    for pname in manifest.profiles.keys() {
        out.extend(crate::resolve::active_skill_names(manifest, pname));
    }
    out.into_iter().collect()
}

pub(crate) fn locked_entry(
    name: &str,
    skill: &crate::manifest::Skill,
    resolved: &crate::store::Resolved,
) -> Result<LockedSkill> {
    let (source, path, git, rev) = match skill.source()? {
        SkillSource::Path(p) => (SkillLockSource::Path, Some(p), None, None),
        SkillSource::Git { url, .. } => {
            (SkillLockSource::Git, None, Some(url), resolved.rev.clone())
        }
    };
    Ok(LockedSkill {
        name: name.to_string(),
        source,
        path,
        git,
        rev,
        checksum: Sha256Hex::parse(&resolved.checksum)?,
    })
}
