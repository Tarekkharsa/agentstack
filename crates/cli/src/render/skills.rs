//! Skill materialization: make exactly the active set of skills present in a
//! target's skills directory, and prune only the ones agentstack owns.
//!
//! Strategy is adapter-declared (PLAN §9b, D9): `symlink` (default, no
//! duplication, trivially reversible) or `copy` (Windows/sandbox fallback). We
//! never clobber a skill directory the user created by hand.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::adapter::descriptor::SkillStrategy;

/// A marker dropped inside copied skill dirs so pruning can tell "ours" from a
/// user's hand-made directory.
const MARKER: &str = ".agentstack-managed";

/// What materialization would do for one target's skills dir.
pub struct SkillPlan {
    pub skills_dir: PathBuf,
    pub strategy: SkillStrategy,
    /// Active skills: (name, absolute source dir).
    pub active: Vec<(String, PathBuf)>,
    /// Previously-managed skills no longer active → to be removed.
    pub to_remove: Vec<String>,
    /// Active names where a non-managed real dir already exists (won't clobber).
    pub conflicts: Vec<String>,
    /// Names that must be COPIED even when `strategy` is `Symlink`.
    ///
    /// This is what makes a re-gate `keep-pinned` answer mean what it says. The
    /// approved bytes have to be what agents actually load, and a symlink into
    /// the project tree would track the very drift the user just declined —
    /// delivering the new content under the old pin's name. Their `active`
    /// source is the content-store snapshot, and copying detaches the delivered
    /// artifact from the live file.
    pub pinned_copies: Vec<String>,
}

impl SkillPlan {
    pub fn managed_names(&self) -> Vec<String> {
        self.active
            .iter()
            .filter(|(n, _)| !self.conflicts.contains(n))
            .map(|(n, _)| n.clone())
            .collect()
    }

    pub fn has_work(&self) -> bool {
        !self.active.is_empty() || !self.to_remove.is_empty()
    }
}

/// Compute the plan without touching the filesystem.
///
/// Fallible since the name contract (design §C.3): every active name is
/// validated before it can reach the `skills_dir.join(name)` below and in
/// `materialize` — a bad name in a hand-edited manifest fails the whole plan
/// here, at the last gate before filesystem writes, instead of traversing.
pub fn plan(
    skills_dir: PathBuf,
    strategy: SkillStrategy,
    active: Vec<(String, PathBuf)>,
    previously_managed: &[String],
) -> Result<SkillPlan> {
    plan_with_pinned(skills_dir, strategy, active, previously_managed, Vec::new())
}

/// [`plan`], plus the names a re-gate `keep-pinned` answer forces to be copied
/// from the content store rather than linked to the live project file.
pub fn plan_with_pinned(
    skills_dir: PathBuf,
    strategy: SkillStrategy,
    active: Vec<(String, PathBuf)>,
    previously_managed: &[String],
    pinned_copies: Vec<String>,
) -> Result<SkillPlan> {
    for (name, _) in &active {
        crate::text::validate_name(name)
            .with_context(|| format!("refusing to materialize skill '{}'", name.escape_debug()))?;
    }
    let active_names: Vec<&String> = active.iter().map(|(n, _)| n).collect();
    let to_remove: Vec<String> = previously_managed
        .iter()
        .filter(|n| !active_names.contains(n))
        .cloned()
        .collect();

    let mut conflicts = Vec::new();
    for (name, _) in &active {
        let dest = skills_dir.join(name);
        if is_unmanaged_dir(&dest) {
            conflicts.push(name.clone());
        }
    }

    Ok(SkillPlan {
        skills_dir,
        strategy,
        active,
        to_remove,
        conflicts,
        pinned_copies,
    })
}

/// CONSENT WITNESS (Phase 2, keep-pinned). A `keep-pinned` answer means the
/// APPROVED BYTES ARE WHAT AGENTS LOAD — not merely that the trust digest stays
/// put. On an adapter that normally symlinks, the delivered artifact must
/// therefore become a COPY of the content-store snapshot: a link into the
/// project tree would silently deliver the exact change the human just
/// declined, under the old pin's name. NEVER weaken this to "the strategy is
/// respected" — the property is about what is on disk at the delivery point.
#[cfg(test)]
mod keep_pinned_tests {
    use super::*;
    use assert_fs::prelude::*;

    #[test]
    fn a_keep_pinned_skill_is_copied_from_the_snapshot_even_when_the_adapter_symlinks() {
        let tmp = assert_fs::TempDir::new().unwrap();
        // The bytes consent covered, as they live in the content store.
        let snapshot = tmp.child("store/content/abc");
        snapshot.child("SKILL.md").write_str("approved\n").unwrap();
        // A normal skill, delivered the adapter's usual way.
        let plain = tmp.child("proj/skills/plain");
        plain.child("SKILL.md").write_str("ordinary\n").unwrap();

        let skills_dir = tmp.child("out").to_path_buf();
        let plan = plan_with_pinned(
            skills_dir.clone(),
            SkillStrategy::Symlink,
            vec![
                ("pinned".to_string(), snapshot.to_path_buf()),
                ("plain".to_string(), plain.to_path_buf()),
            ],
            &[],
            vec!["pinned".to_string()],
        )
        .unwrap();
        materialize(&plan).unwrap();

        let delivered = skills_dir.join("pinned");
        // 1. It is NOT a link — nothing about it can track the live file.
        assert!(
            !is_symlink(&delivered),
            "a keep-pinned skill was symlinked; it would follow the declined change"
        );
        // 2. The delivered content is the APPROVED content.
        assert_eq!(
            std::fs::read_to_string(delivered.join("SKILL.md")).unwrap(),
            "approved\n"
        );
        // 3. The adapter's usual strategy still applies to everything else —
        //    keep-pinned changes delivery for the item it answers, not globally.
        assert!(
            is_symlink(&skills_dir.join("plain")),
            "keep-pinned must not turn every skill into a copy"
        );
    }

    /// The reason copying matters, stated as a test: editing the live project
    /// file after delivery must NOT change what was delivered.
    #[test]
    fn editing_the_live_file_cannot_reach_a_keep_pinned_delivery() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let snapshot = tmp.child("store/content/abc");
        snapshot.child("SKILL.md").write_str("approved\n").unwrap();
        let skills_dir = tmp.child("out").to_path_buf();
        let plan = plan_with_pinned(
            skills_dir.clone(),
            SkillStrategy::Symlink,
            vec![("pinned".to_string(), snapshot.to_path_buf())],
            &[],
            vec!["pinned".to_string()],
        )
        .unwrap();
        materialize(&plan).unwrap();

        // The user edits the project copy again after deciding to keep the pin.
        tmp.child("proj/skills/pinned/SKILL.md")
            .write_str("sneaky\n")
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(skills_dir.join("pinned").join("SKILL.md")).unwrap(),
            "approved\n",
            "the delivered artifact tracked a later edit — it is not detached"
        );
    }
}

/// Perform the plan: remove pruned managed skills, then materialize the active
/// set. Conflicting (user-owned) names are skipped. A plan that leaves nothing
/// managed also clears the skills dir itself if pruning emptied it — so
/// deactivation leaves no stray `.claude/skills/` husk behind.
pub fn materialize(plan: &SkillPlan) -> Result<()> {
    // Prune-only plans (deactivation) must not create the very dir they are
    // about to empty.
    if !plan.active.is_empty() {
        fs::create_dir_all(&plan.skills_dir)
            .with_context(|| format!("creating {}", plan.skills_dir.display()))?;
    }

    for name in &plan.to_remove {
        remove_managed(&plan.skills_dir.join(name))?;
    }

    for (name, source) in &plan.active {
        if plan.conflicts.contains(name) {
            continue;
        }
        let dest = plan.skills_dir.join(name);
        // Replace an existing managed link/dir so re-runs are idempotent.
        if dest.exists() || is_symlink(&dest) {
            remove_managed(&dest)?;
        }
        // A keep-pinned item is copied whatever the adapter's usual strategy
        // is: linking would re-attach it to the file whose change was declined.
        let strategy = if plan.pinned_copies.contains(name) {
            SkillStrategy::Copy
        } else {
            plan.strategy
        };
        match strategy {
            SkillStrategy::Symlink => symlink_dir(source, &dest)
                .with_context(|| format!("symlinking skill '{name}' → {}", dest.display()))?,
            SkillStrategy::Copy => {
                crate::util::fsx::copy_dir_all(source, &dest)
                    .with_context(|| format!("copying skill '{name}' → {}", dest.display()))?;
                fs::write(dest.join(MARKER), b"agentstack\n").ok();
            }
        }
    }

    // Nothing managed here any more: best-effort rmdir of the emptied dir.
    // `remove_dir` refuses non-empty dirs, so user content is inherently safe.
    if plan.managed_names().is_empty() {
        let _ = fs::remove_dir(&plan.skills_dir);
    }
    Ok(())
}

/// True if `path` is a directory we did NOT create (real dir, no marker, not a
/// symlink) — those are never removed or overwritten.
fn is_unmanaged_dir(path: &Path) -> bool {
    match path.symlink_metadata() {
        Ok(meta) if meta.file_type().is_symlink() => false,
        Ok(meta) if meta.is_dir() => !path.join(MARKER).exists(),
        _ => false,
    }
}

fn is_symlink(path: &Path) -> bool {
    path.symlink_metadata()
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

/// Remove only something we own: a symlink, or a directory bearing our marker.
fn remove_managed(path: &Path) -> Result<()> {
    if is_symlink(path) {
        fs::remove_file(path).with_context(|| format!("removing link {}", path.display()))?;
    } else if path.is_dir() && path.join(MARKER).exists() {
        fs::remove_dir_all(path).with_context(|| format!("removing {}", path.display()))?;
    }
    Ok(())
}

#[cfg(unix)]
fn symlink_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(src, dst)
}

#[cfg(windows)]
fn symlink_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(src, dst)
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::prelude::*;

    fn lib_skill(tmp: &assert_fs::TempDir, name: &str) -> PathBuf {
        let dir = tmp.child(format!("lib/{name}"));
        dir.create_dir_all().unwrap();
        dir.child("SKILL.md").write_str("# skill\n").unwrap();
        dir.path().to_path_buf()
    }

    #[test]
    fn symlinks_active_and_prunes_removed() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let a = lib_skill(&tmp, "a");
        let b = lib_skill(&tmp, "b");
        let skills_dir = tmp.child("skills").path().to_path_buf();

        // Round 1: activate a + b.
        let p1 = plan(
            skills_dir.clone(),
            SkillStrategy::Symlink,
            vec![("a".into(), a.clone()), ("b".into(), b.clone())],
            &[],
        )
        .unwrap();
        materialize(&p1).unwrap();
        assert!(skills_dir.join("a").join("SKILL.md").exists());
        assert!(skills_dir.join("b").join("SKILL.md").exists());

        // Round 2: only a active; b was previously managed → pruned.
        let p2 = plan(
            skills_dir.clone(),
            SkillStrategy::Symlink,
            vec![("a".into(), a.clone())],
            &["a".to_string(), "b".to_string()],
        )
        .unwrap();
        assert_eq!(p2.to_remove, vec!["b".to_string()]);
        materialize(&p2).unwrap();
        assert!(skills_dir.join("a").exists());
        assert!(!skills_dir.join("b").exists());
    }

    #[test]
    fn prune_to_zero_removes_the_emptied_skills_dir() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let a = lib_skill(&tmp, "a");
        let skills_dir = tmp.child("skills").path().to_path_buf();

        let p1 = plan(
            skills_dir.clone(),
            SkillStrategy::Symlink,
            vec![("a".into(), a)],
            &[],
        )
        .unwrap();
        materialize(&p1).unwrap();
        assert!(skills_dir.exists());

        // Deactivation: pruning the last managed skill removes the dir itself.
        let p2 = plan(
            skills_dir.clone(),
            SkillStrategy::Symlink,
            vec![],
            &["a".to_string()],
        )
        .unwrap();
        materialize(&p2).unwrap();
        assert!(!skills_dir.exists(), "emptied managed dir removed");
    }

    #[test]
    fn prune_only_plans_keep_user_content_and_create_nothing() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let skills_dir = tmp.child("skills");
        skills_dir
            .child("mine/SKILL.md")
            .write_str("user's own\n")
            .unwrap();

        // User content keeps the dir alive through a full prune.
        let p = plan(
            skills_dir.path().to_path_buf(),
            SkillStrategy::Symlink,
            vec![],
            &["gone".to_string()],
        )
        .unwrap();
        materialize(&p).unwrap();
        assert!(skills_dir.child("mine/SKILL.md").path().exists());
        assert!(skills_dir.path().exists());

        // And a prune-only plan never creates a missing dir.
        let missing = tmp.child("other-skills").path().to_path_buf();
        let p2 = plan(
            missing.clone(),
            SkillStrategy::Symlink,
            vec![],
            &["gone".to_string()],
        )
        .unwrap();
        materialize(&p2).unwrap();
        assert!(
            !missing.exists(),
            "prune-only plans must not create the dir"
        );
    }

    #[test]
    fn never_clobbers_a_user_skill_dir() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let a = lib_skill(&tmp, "a");
        let skills_dir = tmp.child("skills");
        // User already has a real "a" skill dir (no marker, not a symlink).
        skills_dir
            .child("a/SKILL.md")
            .write_str("user's own\n")
            .unwrap();

        let p = plan(
            skills_dir.path().to_path_buf(),
            SkillStrategy::Symlink,
            vec![("a".into(), a)],
            &[],
        )
        .unwrap();
        assert_eq!(p.conflicts, vec!["a".to_string()]);
        materialize(&p).unwrap();
        // Untouched.
        assert_eq!(
            fs::read_to_string(skills_dir.child("a/SKILL.md").path()).unwrap(),
            "user's own\n"
        );
    }

    #[test]
    fn copy_strategy_materializes_and_prunes_with_marker() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let a = lib_skill(&tmp, "a");
        let skills_dir = tmp.child("skills").path().to_path_buf();

        let p1 = plan(
            skills_dir.clone(),
            SkillStrategy::Copy,
            vec![("a".into(), a)],
            &[],
        )
        .unwrap();
        materialize(&p1).unwrap();
        assert!(skills_dir.join("a").join("SKILL.md").exists());
        assert!(skills_dir.join("a").join(MARKER).exists());

        let p2 = plan(
            skills_dir.clone(),
            SkillStrategy::Copy,
            vec![],
            &["a".to_string()],
        )
        .unwrap();
        materialize(&p2).unwrap();
        assert!(!skills_dir.join("a").exists());
    }

    #[test]
    fn copy_strategy_never_carries_the_sources_git_dir() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let a = lib_skill(&tmp, "a");
        tmp.child("lib/a/.git/HEAD")
            .write_str("ref: refs/heads/main\n")
            .unwrap();
        let skills_dir = tmp.child("skills").path().to_path_buf();

        let p = plan(
            skills_dir.clone(),
            SkillStrategy::Copy,
            vec![("a".into(), a)],
            &[],
        )
        .unwrap();
        materialize(&p).unwrap();
        assert!(skills_dir.join("a").join("SKILL.md").exists());
        assert!(!skills_dir.join("a").join(".git").exists());
    }
}
