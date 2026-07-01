//! Skill name resolution — the single seam that maps a `skills = ["name"]`
//! reference to a concrete on-disk source (see `plan/central-store.md`).
//!
//! Resolution order (first hit wins):
//!
//! 1. **Inline** — a `[skills.<name>]` entry in the project manifest. An inline
//!    definition always wins; this is also the collision rule for now (a project
//!    that wants to override a central skill defines it inline).
//! 2. **Central library** — a `[[skill]]` entry in `<lib_home>/library.toml`,
//!    whose body lives under `<lib_home>/skills/`.
//!
//! Catalog fallback (fetch-then-reference) is intentionally out of scope for this
//! step. An unresolved name is a hard, structured error ([`ResolveError`]).
//!
//! The resolver returns enough metadata (source kind, rev, checksum, provenance)
//! for later steps to write project `agentstack.lock` entries and to flag digest
//! drift; it does not itself write locks or verify against them yet.

use std::path::{Path, PathBuf};

use crate::library::Library;
use crate::lock::Lock;
use crate::manifest::{Manifest, Skill};
use crate::store::Store;

/// Where a resolved skill came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillOrigin {
    /// Defined inline in the project manifest (`[skills.<name>]`).
    Inline,
    /// Resolved from the central library (`library.toml`).
    Library,
}

/// A skill name resolved to a concrete source, with the metadata needed to
/// materialize it and to record a reproducible lock entry.
#[derive(Debug, Clone)]
pub struct ResolvedSkill {
    /// The name the project referenced.
    pub name: String,
    /// Which source satisfied the reference.
    pub origin: SkillOrigin,
    /// Local directory holding the skill body.
    pub path: PathBuf,
    /// `"path"` or `"git"`.
    pub source_kind: &'static str,
    /// Resolved git revision (git sources only).
    pub rev: Option<String>,
    /// SHA-256 of the content. Empty only if a path source does not exist on
    /// disk yet.
    pub checksum: String,
    /// Provenance recorded in the library index (library origin only).
    pub provenance: Option<String>,
}

/// A structured resolution failure. `Unresolved` is the hard error for a name
/// that matches neither an inline manifest skill nor a library entry; `Source`
/// wraps an underlying fetch/IO failure while resolving a matched entry.
#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("skill '{name}' is not defined in the project manifest or the central library")]
    Unresolved { name: String },
    #[error(transparent)]
    Source(#[from] anyhow::Error),
}

/// Resolve a single skill name through the resolution order above.
///
/// - `manifest` / `manifest_dir`: the project manifest and the directory its
///   relative skill paths are resolved against.
/// - `library` / `lib_home`: the loaded central index and its home directory
///   (skill bodies live under `<lib_home>/skills/`).
/// - `store`: reused to resolve both origins to a local path + checksum (and to
///   fetch git sources, exactly as normal skill materialization does).
pub fn resolve_skill(
    manifest: &Manifest,
    manifest_dir: &Path,
    library: &Library,
    lib_home: &Path,
    store: &Store,
    name: &str,
) -> Result<ResolvedSkill, ResolveError> {
    // 1. Inline manifest skill wins.
    if let Some(skill) = manifest.skills.get(name) {
        let resolved = store.resolve(skill, manifest_dir, None)?;
        return Ok(ResolvedSkill {
            name: name.to_string(),
            origin: SkillOrigin::Inline,
            path: resolved.path,
            source_kind: resolved.source_kind,
            rev: resolved.rev,
            checksum: resolved.checksum,
            provenance: None,
        });
    }

    // 2. Central library.
    if let Some(entry) = library.get(name) {
        let skill = Skill {
            path: entry.path.clone(),
            git: entry.git.clone(),
            rev: entry.rev.clone(),
        };
        // Library path sources are relative to `<lib_home>/skills/`.
        let base = lib_home.join("skills");
        let resolved = store.resolve(&skill, &base, entry.rev.as_deref())?;
        return Ok(ResolvedSkill {
            name: name.to_string(),
            origin: SkillOrigin::Library,
            path: resolved.path,
            source_kind: resolved.source_kind,
            rev: resolved.rev,
            checksum: resolved.checksum,
            provenance: entry.provenance.clone(),
        });
    }

    Err(ResolveError::Unresolved {
        name: name.to_string(),
    })
}

/// Expand a profile's skill refs to active skill names, applying the same
/// wildcard rule as activation (`use_profile`): `"*"` means the manifest's inline
/// skills only — it does not pull in central-library skills.
pub fn active_skill_names(manifest: &Manifest, profile_name: &str) -> Vec<String> {
    match manifest.profiles.get(profile_name) {
        None => Vec::new(),
        Some(p) if p.loads_all_skills() => manifest.skills.keys().cloned().collect(),
        Some(p) => p.skills.clone(),
    }
}

/// Whether a skill ref resolves to a git source (inline or library). Read
/// commands use this to skip git-backed refs, whose resolution would fetch —
/// keeping offline checks offline until a non-fetching resolver mode lands
/// (tracked in `plan/central-store.md` Phase 3).
pub fn skill_ref_is_git(name: &str, manifest: &Manifest, library: &Library) -> bool {
    if let Some(s) = manifest.skills.get(name) {
        return s.git.is_some();
    }
    if let Some(e) = library.get(name) {
        return e.git.is_some();
    }
    false
}

/// How an active skill's currently-resolved content compares to its
/// `agentstack.lock` pin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillLockStatus {
    /// Resolved content matches the locked checksum (and rev, when applicable).
    Matches,
    /// The skill resolved but has no entry in the lockfile yet.
    MissingLockEntry,
    /// Resolved checksum differs from the locked checksum.
    ChecksumDrift { locked: String, current: String },
    /// Git rev differs from the locked rev (both sides carry one).
    RevDrift { locked: String, current: String },
    /// The reference could not be resolved (broken/missing source).
    ResolveFailed { error: String },
}

/// A neutral, render-agnostic lock/drift status for one skill. `doctor` maps it
/// to warning/error severity; `explain` renders it as provenance/detail.
#[derive(Debug, Clone)]
pub struct SkillLockReport {
    pub name: String,
    /// `None` when resolution failed.
    pub origin: Option<SkillOrigin>,
    /// Library provenance, when the skill resolved from the central library.
    pub provenance: Option<String>,
    pub status: SkillLockStatus,
}

/// Resolve one skill by name and compare it to its lockfile pin, through the
/// same resolution seam as activation ([`resolve_skill`]). Checksum drift takes
/// precedence over rev drift.
pub fn skill_lock_status(
    name: &str,
    manifest: &Manifest,
    manifest_dir: &Path,
    library: &Library,
    lib_home: &Path,
    store: &Store,
    lock: &Lock,
) -> SkillLockReport {
    match resolve_skill(manifest, manifest_dir, library, lib_home, store, name) {
        Err(e) => SkillLockReport {
            name: name.to_string(),
            origin: None,
            provenance: None,
            status: SkillLockStatus::ResolveFailed {
                error: e.to_string(),
            },
        },
        Ok(resolved) => {
            let status = match lock.get(name) {
                None => SkillLockStatus::MissingLockEntry,
                Some(entry) if entry.checksum != resolved.checksum => {
                    SkillLockStatus::ChecksumDrift {
                        locked: entry.checksum.clone(),
                        current: resolved.checksum.clone(),
                    }
                }
                Some(entry) => match (&entry.rev, &resolved.rev) {
                    (Some(l), Some(c)) if l != c => SkillLockStatus::RevDrift {
                        locked: l.clone(),
                        current: c.clone(),
                    },
                    _ => SkillLockStatus::Matches,
                },
            };
            SkillLockReport {
                name: name.to_string(),
                origin: Some(resolved.origin),
                provenance: resolved.provenance.clone(),
                status,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::LibrarySkill;
    use assert_fs::prelude::*;

    /// A library home with one path-source skill body written under
    /// `lib/skills/<name>/`, plus an index entry pointing at it.
    fn library_with_skill(lib_home: &assert_fs::TempDir, name: &str, body: &str) -> Library {
        lib_home
            .child(format!("skills/{name}/SKILL.md"))
            .write_str(body)
            .unwrap();
        let mut lib = Library::default();
        lib.upsert(LibrarySkill {
            name: name.into(),
            source: "path".into(),
            path: Some(name.into()),
            git: None,
            rev: None,
            checksum: None,
            version: None,
            provenance: Some("consolidated".into()),
        });
        lib
    }

    fn empty_manifest() -> Manifest {
        toml::from_str("version = 1").unwrap()
    }

    fn manifest_with_inline_skill(dir: &assert_fs::TempDir, name: &str, body: &str) -> Manifest {
        dir.child(format!("skills/{name}/SKILL.md"))
            .write_str(body)
            .unwrap();
        let toml = format!("version = 1\n[skills.{name}]\npath = \"./skills/{name}\"\n");
        toml::from_str(&toml).unwrap()
    }

    #[test]
    fn inline_wins_over_library() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());

        // Same name defined in both places, with different content.
        let manifest = manifest_with_inline_skill(&proj, "review", "# inline\n");
        let library = library_with_skill(&lib_home, "review", "# library\n");

        let r = resolve_skill(
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            "review",
        )
        .unwrap();

        assert_eq!(r.origin, SkillOrigin::Inline);
        assert_eq!(r.provenance, None);
        let contents = std::fs::read_to_string(r.path.join("SKILL.md")).unwrap();
        assert_eq!(contents, "# inline\n");
    }

    #[test]
    fn resolves_from_library_when_not_inline() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());

        let manifest = empty_manifest();
        let library = library_with_skill(&lib_home, "sql-review", "# from library\n");

        let r = resolve_skill(
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            "sql-review",
        )
        .unwrap();

        assert_eq!(r.origin, SkillOrigin::Library);
        assert_eq!(r.source_kind, "path");
        assert_eq!(r.provenance.as_deref(), Some("consolidated"));
        assert!(r.path.join("SKILL.md").exists());
    }

    #[test]
    fn returns_checksum_for_resolved_skill() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());

        let manifest = empty_manifest();
        let library = library_with_skill(&lib_home, "x", "# x\n");

        let r = resolve_skill(
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            "x",
        )
        .unwrap();
        assert_eq!(r.checksum.len(), 64, "sha-256 hex digest expected");
    }

    #[test]
    fn unresolved_name_is_structured_error() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());

        let manifest = empty_manifest();
        let library = Library::default();

        let err = resolve_skill(
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            "nope",
        )
        .unwrap_err();

        match err {
            ResolveError::Unresolved { name } => assert_eq!(name, "nope"),
            other => panic!("expected Unresolved, got {other:?}"),
        }
    }

    // ---------- drift / lock-status helpers ----------

    use crate::lock::{Lock, LockedSkill};

    fn lock_with(entry: LockedSkill) -> Lock {
        let mut lock = Lock::default();
        lock.upsert(entry);
        lock
    }

    #[test]
    fn active_skill_names_wildcard_is_inline_only() {
        let proj = assert_fs::TempDir::new().unwrap();
        let manifest = manifest_with_inline_skill(&proj, "a", "# a\n");
        // Give the manifest a wildcard profile.
        let manifest: Manifest = {
            let mut m = manifest;
            let p: crate::manifest::Profile = toml::from_str("skills = [\"*\"]").unwrap();
            m.profiles.insert("p".into(), p);
            m
        };
        assert_eq!(active_skill_names(&manifest, "p"), vec!["a".to_string()]);
    }

    #[test]
    fn stable_digest_matches_lock() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());
        let manifest = empty_manifest();
        let library = library_with_skill(&lib_home, "sql-review", "# body\n");

        // Lock the current resolved digest.
        let resolved = resolve_skill(
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            "sql-review",
        )
        .unwrap();
        let lock = lock_with(LockedSkill {
            name: "sql-review".into(),
            source: "path".into(),
            path: Some("sql-review".into()),
            git: None,
            rev: None,
            checksum: resolved.checksum.clone(),
        });

        let report = skill_lock_status(
            "sql-review",
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            &lock,
        );
        assert_eq!(report.status, SkillLockStatus::Matches);
        assert_eq!(report.origin, Some(SkillOrigin::Library));
        assert_eq!(report.provenance.as_deref(), Some("consolidated"));
    }

    #[test]
    fn changed_central_skill_is_checksum_drift() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());
        let manifest = empty_manifest();
        let library = library_with_skill(&lib_home, "sql-review", "# original\n");

        // Lock a stale digest, then change the library content underneath it.
        let lock = lock_with(LockedSkill {
            name: "sql-review".into(),
            source: "path".into(),
            path: Some("sql-review".into()),
            git: None,
            rev: None,
            checksum: "staledigest".into(),
        });
        lib_home
            .child("skills/sql-review/SKILL.md")
            .write_str("# changed\n")
            .unwrap();

        let report = skill_lock_status(
            "sql-review",
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            &lock,
        );
        match report.status {
            SkillLockStatus::ChecksumDrift { locked, .. } => assert_eq!(locked, "staledigest"),
            other => panic!("expected ChecksumDrift, got {other:?}"),
        }
    }

    #[test]
    fn active_skill_without_lock_entry_reports_missing() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());
        let manifest = empty_manifest();
        let library = library_with_skill(&lib_home, "sql-review", "# body\n");

        let report = skill_lock_status(
            "sql-review",
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            &Lock::default(),
        );
        assert_eq!(report.status, SkillLockStatus::MissingLockEntry);
    }

    #[test]
    fn broken_library_ref_reports_resolve_failed() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());
        let manifest = empty_manifest();
        let mut library = Library::default();
        library.upsert(LibrarySkill {
            name: "sql-review".into(),
            source: "path".into(),
            path: None,
            git: None,
            rev: None,
            checksum: None,
            version: None,
            provenance: None,
        });

        let report = skill_lock_status(
            "sql-review",
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            &Lock::default(),
        );
        assert!(matches!(
            report.status,
            SkillLockStatus::ResolveFailed { .. }
        ));
        assert_eq!(report.origin, None);
    }

    #[test]
    fn inline_and_library_origins_are_distinguished() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());

        // Inline "review" and library-only "sql-review".
        let manifest = manifest_with_inline_skill(&proj, "review", "# inline\n");
        let library = library_with_skill(&lib_home, "sql-review", "# lib\n");

        let inline = skill_lock_status(
            "review",
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            &Lock::default(),
        );
        let lib = skill_lock_status(
            "sql-review",
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            &Lock::default(),
        );
        assert_eq!(inline.origin, Some(SkillOrigin::Inline));
        assert_eq!(inline.provenance, None);
        assert_eq!(lib.origin, Some(SkillOrigin::Library));
        assert_eq!(lib.provenance.as_deref(), Some("consolidated"));
    }

    #[test]
    fn git_rev_drift_is_reported() {
        // A local git repo used as a library git source.
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let repo = proj.child("repo");
        repo.create_dir_all().unwrap();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(repo.path())
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?} failed");
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@e.st"]);
        git(&["config", "user.name", "t"]);
        repo.child("SKILL.md").write_str("# git skill\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "init"]);

        let store = Store::with_root(proj.child("store").path().to_path_buf());
        let url = format!("file://{}", repo.path().display());
        let manifest = empty_manifest();
        let mut library = Library::default();
        library.upsert(LibrarySkill {
            name: "gitskill".into(),
            source: "git".into(),
            path: None,
            git: Some(url),
            rev: None,
            checksum: None,
            version: None,
            provenance: None,
        });

        // Resolve to learn the real checksum + HEAD rev, then lock the same
        // checksum but a different rev → rev drift (checksum still matches).
        let resolved = resolve_skill(
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            "gitskill",
        )
        .unwrap();
        let lock = lock_with(LockedSkill {
            name: "gitskill".into(),
            source: "git".into(),
            path: None,
            git: resolved.rev.clone().map(|_| "url".into()),
            rev: Some("0000000000000000000000000000000000000000".into()),
            checksum: resolved.checksum.clone(),
        });

        let report = skill_lock_status(
            "gitskill",
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            &lock,
        );
        match report.status {
            SkillLockStatus::RevDrift { locked, current } => {
                assert_eq!(locked, "0000000000000000000000000000000000000000");
                assert_eq!(Some(current), resolved.rev);
            }
            other => panic!("expected RevDrift, got {other:?}"),
        }
    }
}
