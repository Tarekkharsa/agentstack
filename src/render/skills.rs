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
pub fn plan(
    skills_dir: PathBuf,
    strategy: SkillStrategy,
    active: Vec<(String, PathBuf)>,
    previously_managed: &[String],
) -> SkillPlan {
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

    SkillPlan {
        skills_dir,
        strategy,
        active,
        to_remove,
        conflicts,
    }
}

/// Perform the plan: remove pruned managed skills, then materialize the active
/// set. Conflicting (user-owned) names are skipped.
pub fn materialize(plan: &SkillPlan) -> Result<()> {
    fs::create_dir_all(&plan.skills_dir)
        .with_context(|| format!("creating {}", plan.skills_dir.display()))?;

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
        match plan.strategy {
            SkillStrategy::Symlink => symlink_dir(source, &dest)
                .with_context(|| format!("symlinking skill '{name}' → {}", dest.display()))?,
            SkillStrategy::Copy => {
                copy_dir(source, &dest)
                    .with_context(|| format!("copying skill '{name}' → {}", dest.display()))?;
                fs::write(dest.join(MARKER), b"agentstack\n").ok();
            }
        }
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

fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
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
        );
        materialize(&p1).unwrap();
        assert!(skills_dir.join("a").join("SKILL.md").exists());
        assert!(skills_dir.join("b").join("SKILL.md").exists());

        // Round 2: only a active; b was previously managed → pruned.
        let p2 = plan(
            skills_dir.clone(),
            SkillStrategy::Symlink,
            vec![("a".into(), a.clone())],
            &["a".to_string(), "b".to_string()],
        );
        assert_eq!(p2.to_remove, vec!["b".to_string()]);
        materialize(&p2).unwrap();
        assert!(skills_dir.join("a").exists());
        assert!(!skills_dir.join("b").exists());
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
        );
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
        );
        materialize(&p1).unwrap();
        assert!(skills_dir.join("a").join("SKILL.md").exists());
        assert!(skills_dir.join("a").join(MARKER).exists());

        let p2 = plan(
            skills_dir.clone(),
            SkillStrategy::Copy,
            vec![],
            &["a".to_string()],
        );
        materialize(&p2).unwrap();
        assert!(!skills_dir.join("a").exists());
    }
}
