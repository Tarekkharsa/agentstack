//! Quarantine: where fetched bytes wait, inert, until someone says yes.
//!
//! Phase 4. One staging area shared by every intake source — a teammate's
//! signed bundle, an external registry, a URL — so that "intake never becomes
//! activation" is a property of one directory rather than a promise repeated at
//! each source.
//!
//! # What inert means here, concretely
//!
//! Staged content lives under `.agentstack/quarantine/<id>/`. That path is:
//!
//! - not `.agentstack/skills/` or `.agentstack/instructions/`, so the intake
//!   scanner does not offer it and no adopt path sees it;
//! - not referenced by any manifest entry, so nothing resolves it;
//! - not on any search path, in any agent's context, or reachable by a server.
//!
//! It is a directory of files nobody reads. That is the entire mechanism, and
//! its plainness is the point: there is no execution to disable, because
//! nothing was ever arranged to execute it.
//!
//! # Declining leaves nothing
//!
//! [`discard`] removes the staging directory. The property a witness asserts is
//! the Phase 1 one: after fetching and declining, the project is byte-identical
//! to before and there is nothing on disk to find later. Content that survives
//! a "no" is content the user has to remember to clean up, which is how a
//! decline quietly becomes a deferral.
//!
//! # Every path in a bundle is hostile input
//!
//! [`check_relative`] is the choke point (invariant 7). A path from someone
//! else's file is refused unless it is plainly relative and plainly
//! descending — no absolute paths, no `..`, no Windows drive letters, no NUL,
//! no leading separator. It is called before staging and again before adopting,
//! because the second call is what makes traversal a property of the module
//! rather than of remembering to check at each call site.

use anyhow::{bail, Context, Result};
use std::path::{Component, Path, PathBuf};

/// Refuse any path that could escape the directory it is staged into.
///
/// Allow-list shaped rather than deny-list shaped: a component is accepted only
/// when it is a plain name. `..`, absolute roots, and prefixes are rejected by
/// not being on the list, so a spelling nobody thought of fails closed instead
/// of slipping through a missing rule.
pub fn check_relative(path: &str) -> Result<()> {
    if path.is_empty() {
        bail!("a bundle entry has an empty path");
    }
    if path.contains('\0') {
        bail!("a bundle entry path contains a NUL byte");
    }
    // Reject before `Path` normalizes anything away.
    if path.starts_with('/') || path.starts_with('\\') || path.contains(':') {
        bail!("'{path}' is not a relative path — refusing to stage it");
    }
    for component in Path::new(path).components() {
        match component {
            Component::Normal(part) => {
                if part.to_str().is_none() {
                    bail!("'{path}' contains a non-UTF-8 name");
                }
            }
            Component::CurDir => {}
            Component::ParentDir => {
                bail!("'{path}' walks upward with '..' — refusing to stage it")
            }
            Component::RootDir | Component::Prefix(_) => {
                bail!("'{path}' is absolute — refusing to stage it")
            }
        }
    }
    Ok(())
}

/// The only two kinds `adopt` reads — and therefore the only two a bundle may
/// declare (F1). `kind` becomes a PATH SEGMENT in [`stage`], so an
/// unvalidated value was the same traversal hole `check_relative` closes for
/// `path`, one field over: `kind: "../../.."` plus `path: ".zshrc"` landed
/// bytes in the user's home before any card printed. Allow-list, not a path
/// check: there is no legitimate third value, so anything else is refused
/// outright rather than merely contained.
pub fn check_kind(kind: &str) -> Result<()> {
    if kind == "skill" || kind == "instruction" {
        return Ok(());
    }
    bail!(
        "a bundle entry declares kind '{}' — only 'skill' and 'instruction' exist, refusing",
        crate::text::sanitize_line(kind)
    )
}

/// Where a project's staged intake lives.
pub fn root(dir: &Path) -> PathBuf {
    dir.join("quarantine")
}

/// Write `entries` into a fresh staging directory and return its path.
///
/// The id is derived from the content, so staging the same bundle twice is
/// idempotent rather than accumulating directories a user has to clean up.
pub fn stage(dir: &Path, entries: &[crate::commands::share::Entry]) -> Result<PathBuf> {
    let id = stage_id(entries);
    let staged = root(dir).join(&id);
    if staged.exists() {
        std::fs::remove_dir_all(&staged).ok();
    }
    std::fs::create_dir_all(&staged).with_context(|| format!("creating {}", staged.display()))?;
    // Canonical root for the physical backstop below: `Path::starts_with` is
    // component-wise and does not resolve `..` or links, so the lexical check
    // alone verifies spelling, not destination.
    let staged_real = staged
        .canonicalize()
        .with_context(|| format!("resolving {}", staged.display()))?;
    for entry in entries {
        // Checked again here, deliberately. The caller checks too; this is the
        // call that makes "staged content cannot escape" true of the module
        // rather than true of one caller's diligence.
        check_kind(&entry.kind)?;
        check_relative(&entry.path)?;
        let dest = staged.join(&entry.kind).join(&entry.path);
        // Belt and braces: after joining, the result must still be inside the
        // staging directory. Catches anything a future path spelling gets past
        // the component check.
        if !dest.starts_with(&staged) {
            bail!("'{}' would land outside quarantine — refusing", entry.path);
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
            // The physical backstop: after creation, the parent must RESOLVE
            // inside the staging root, not merely spell like it.
            let parent_real = parent
                .canonicalize()
                .with_context(|| format!("resolving {}", parent.display()))?;
            if !parent_real.starts_with(&staged_real) {
                bail!(
                    "'{}' resolves outside quarantine — refusing",
                    crate::text::sanitize_line(&entry.path)
                );
            }
        }
        std::fs::write(&dest, &entry.body)
            .with_context(|| format!("staging {}", dest.display()))?;
    }
    Ok(staged)
}

/// Remove a staging directory. Called on decline, and on any failure after
/// staging — the invariant is that nothing survives a path that did not end in
/// a yes.
pub fn discard(staged: &Path) -> Result<()> {
    if staged.exists() {
        std::fs::remove_dir_all(staged)
            .with_context(|| format!("removing {}", staged.display()))?;
    }
    // Tidy the parent when it is empty, so a declined intake leaves no trace at
    // all — not even an empty `quarantine/` a user has to wonder about.
    if let Some(parent) = staged.parent() {
        if parent.file_name().is_some_and(|n| n == "quarantine")
            && std::fs::read_dir(parent).is_ok_and(|mut d| d.next().is_none())
        {
            std::fs::remove_dir(parent).ok();
        }
    }
    Ok(())
}

/// Move staged content into the project's own intake directories, where the
/// ordinary declare/lock/trust funnel can see it.
///
/// This is the ONLY function here that puts anything where the product will
/// look at it, and it is reached only after a yes. It deliberately lands
/// content in `.agentstack/skills/` and `.agentstack/instructions/` — the same
/// place a user dropping a file by hand lands it — so that from this point on
/// received content and authored content are the same thing, reviewed by the
/// same card and pinned by the same lock.
pub fn adopt(staged: &Path, dir: &Path) -> Result<usize> {
    let mut copied: Vec<PathBuf> = Vec::new();
    let out = adopt_inner(staged, dir, &mut copied);
    match out {
        Ok(moved) => {
            discard(staged)?;
            Ok(moved)
        }
        Err(e) => {
            // A partial adopt is worse than a refused one: files already
            // copied would sit in the intake directories as though they had
            // been accepted, while the error reads as "nothing happened".
            // Undo the copies, then discard the staging dir — the module's
            // stated invariant is that nothing survives a path that did not
            // end in a completed yes.
            for p in copied.iter().rev() {
                std::fs::remove_file(p).ok();
            }
            discard(staged).ok();
            Err(e.context("nothing was adopted — the partial copies were removed"))
        }
    }
}

fn adopt_inner(staged: &Path, dir: &Path, copied: &mut Vec<PathBuf>) -> Result<usize> {
    let mut moved = 0;
    for (kind, sub) in [("skill", "skills"), ("instruction", "instructions")] {
        let from = staged.join(kind);
        if !from.exists() {
            continue;
        }
        let base = dir.join(sub);
        // F16: never adopt THROUGH a link. A repo shipping
        // `.agentstack/skills` as a symlink at `~/.claude/` would otherwise
        // receive every "adopted" byte at the link's target — the lexical
        // `starts_with` below cannot see that, because it checks how the
        // path is spelled, not where it resolves.
        if base
            .symlink_metadata()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            bail!(
                "{} is a symlink — refusing to adopt through it",
                base.display()
            );
        }
        std::fs::create_dir_all(&base).with_context(|| format!("creating {}", base.display()))?;
        let base_real = base
            .canonicalize()
            .with_context(|| format!("resolving {}", base.display()))?;
        for path in files_under(&from) {
            let rel = path.strip_prefix(&from).unwrap_or(&path);
            let rel_str = rel.to_string_lossy().to_string();
            check_relative(&rel_str)?;
            let dest = base.join(rel);
            if !dest.starts_with(&base) {
                bail!("'{rel_str}' would land outside the project — refusing");
            }
            // `symlink_metadata`, not `exists()`: the latter FOLLOWS a link,
            // so a dangling symlink at the destination read as "nothing
            // there" and the copy wrote through it.
            if dest.symlink_metadata().is_ok() {
                // Never overwrite. A received file that silently replaced an
                // authored one would be the collision case Phase 2 built the
                // diff card for, arriving through a door that has no card.
                bail!(
                    "'{}' already exists in this project — nothing was moved. Rename or \
                     remove it first, then re-run.",
                    dest.display()
                );
            }
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
                // Physical containment, same backstop staging applies: the
                // created parent must RESOLVE under the destination root.
                let parent_real = parent
                    .canonicalize()
                    .with_context(|| format!("resolving {}", parent.display()))?;
                if !parent_real.starts_with(&base_real) {
                    bail!("'{rel_str}' resolves outside the project — refusing");
                }
            }
            std::fs::copy(&path, &dest).with_context(|| format!("adopting {}", dest.display()))?;
            copied.push(dest);
            // F3: what adopt lands is RECEIVED content — record its digest in
            // the machine-local ledger so the intake scanner can never label
            // these exact bytes "your own work". Best-effort: a failed ledger
            // append must not fail an adopt, it only costs the label its
            // extra precision (the content still takes the full review).
            if let Ok(bytes) = std::fs::read(&path) {
                crate::intake::record_received(&bytes);
            }
            moved += 1;
        }
    }
    Ok(moved)
}

fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        if e.file_type().map(|t| t.is_symlink()).unwrap_or(false) {
            continue;
        }
        if p.is_dir() {
            out.extend(files_under(&p));
        } else {
            out.push(p);
        }
    }
    out
}

fn stage_id(entries: &[crate::commands::share::Entry]) -> String {
    let mut hasher = <sha2::Sha256 as sha2::Digest>::new();
    for e in entries {
        sha2::Digest::update(&mut hasher, e.path.as_bytes());
        sha2::Digest::update(&mut hasher, b"\0");
        sha2::Digest::update(&mut hasher, e.body.as_bytes());
        sha2::Digest::update(&mut hasher, b"\0");
    }
    let digest = sha2::Digest::finalize(hasher);
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The choke point, over every spelling of "escape" worth naming. If a new
    /// one is ever found, it belongs here rather than in a call site.
    #[test]
    fn a_path_that_could_escape_is_refused() {
        for hostile in [
            "../outside",
            "a/../../outside",
            "/etc/passwd",
            "\\windows\\system32",
            "C:/Windows",
            "",
            "ok/../../..",
        ] {
            assert!(
                check_relative(hostile).is_err(),
                "{hostile:?} must be refused"
            );
        }
        for fine in ["skill.md", "a/b/c.md", "./nested/file.txt"] {
            assert!(check_relative(fine).is_ok(), "{fine:?} must be allowed");
        }
    }

    /// A NUL byte truncates paths in C APIs; refuse it before anything else
    /// gets a chance to interpret it.
    #[test]
    fn a_nul_byte_is_refused() {
        assert!(check_relative("ok.md\0../../etc/passwd").is_err());
    }

    /// F1 choke point: `kind` is an allow-list, because it becomes a path
    /// segment in `stage`. The two real kinds pass; everything else — a
    /// traversal, an unknown word — is refused.
    #[test]
    fn only_the_two_real_kinds_are_accepted() {
        assert!(check_kind("skill").is_ok());
        assert!(check_kind("instruction").is_ok());
        for hostile in ["../../../..", "server", "..", "skills", "", "skill/.."] {
            assert!(check_kind(hostile).is_err(), "{hostile:?} must be refused");
        }
    }

    /// F1 end to end: a hostile `kind` cannot walk out of the staging
    /// directory even with an innocuous `path`. `stage` refuses, and nothing
    /// lands at the escape target.
    #[test]
    fn a_traversing_kind_cannot_escape_staging() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let dir = tmp.path().join("proj/.agentstack");
        std::fs::create_dir_all(&dir).unwrap();
        let entries = vec![crate::commands::share::Entry {
            name: "x".into(),
            kind: "../../../../..".into(),
            path: "pwned.md".into(),
            body: "x".into(),
            license: None,
            origin: None,
            notice: None,
        }];
        assert!(stage(&dir, &entries).is_err(), "a traversing kind staged");
        assert!(
            !tmp.path().join("pwned.md").exists(),
            "a byte escaped staging via kind"
        );
    }
}
