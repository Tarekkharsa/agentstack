//! SKILL.md discovery inside a fetched repo — the ecosystem's conventional
//! locations with OUR policies (design:
//! `docs/design/add-skill-source-grammar.md` §2).
//!
//! The location list and depth discipline are the de-facto interop spec
//! established by vercel-labs/skills (MIT): root-as-skill, `skills/` and its
//! dot-variants, the agent-convention project dirs, one level deep per
//! container (two for `skills/<category>/<skill>` catalogs), never
//! descending past a found SKILL.md. Our policies replace theirs where they
//! differ: duplicate names fail loudly naming both paths (never
//! first-wins), the recursive fallback is announced and its hits are never
//! auto-selected, and `metadata.internal` is not read at all.

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

/// Where discovery may look before falling back to a recursive walk.
/// `skills/` variants first, then the agent-convention dirs.
const PRIORITY_CONTAINERS: &[&str] = &[
    "skills",
    "skills/.curated",
    "skills/.experimental",
    "skills/.system",
    ".agents/skills",
    ".claude/skills",
    ".cline/skills",
    ".codebuddy/skills",
    ".codex/skills",
    ".commandcode/skills",
    ".continue/skills",
    ".github/skills",
    ".goose/skills",
    ".iflow/skills",
    ".junie/skills",
    ".kilocode/skills",
    ".kiro/skills",
    ".mux/skills",
    ".neovate/skills",
    ".opencode/skills",
    ".openhands/skills",
    ".pi/skills",
    ".qoder/skills",
    ".roo/skills",
    ".trae/skills",
    ".windsurf/skills",
    ".zcode/skills",
    ".zencoder/skills",
];

/// Never entered, in any walk.
const SKIP_DIRS: &[&str] = &["node_modules", ".git", "dist", "build", "__pycache__"];

/// Fallback walk depth cap (directories below `root`).
const FALLBACK_MAX_DEPTH: usize = 5;

#[derive(Debug, Clone)]
pub struct DiscoveredSkill {
    /// Default manifest key: the skill dir's basename (identity is external
    /// — frontmatter `name:` is never read anywhere in this codebase). For
    /// the root-as-skill case this is the caller's `root_name` hint (the
    /// repo name), since a staged clone's dir basename is meaningless.
    pub name: String,
    /// Location inside the repo, `/`-separated — the future manifest
    /// `subpath` (empty for the root-as-skill case).
    pub rel_path: String,
    /// One-line frontmatter description — already sanitized by
    /// `parse_frontmatter_description`. `None` = missing (warn, don't hide).
    pub description: Option<String>,
    /// Found by the recursive fallback, not a priority location: announced
    /// in output and never auto-selected.
    pub via_fallback: bool,
    /// Whether `name` passes the skill-name contract; invalid names are
    /// listed (sanitized) but unselectable without `--name`.
    pub name_valid: bool,
}

/// Discover every skill under `root`, sorted by `rel_path` (deterministic).
///
/// `root_name` names the root-as-skill case (pass the repo name for git
/// sources, the dir basename for local ones). Duplicate skill names across
/// locations are a hard error naming every path involved.
pub fn discover_skills(root: &Path, root_name: Option<&str>) -> Result<Vec<DiscoveredSkill>> {
    // Root-as-skill: the repo IS one skill; nothing else is scanned.
    if root.join("SKILL.md").is_file() {
        let name = root_name
            .map(str::to_string)
            .or_else(|| root.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_default();
        return Ok(vec![make(root, name, String::new(), false)]);
    }

    let mut found: Vec<DiscoveredSkill> = Vec::new();
    for container in PRIORITY_CONTAINERS {
        let dir = root.join(container);
        if !dir.is_dir() {
            continue;
        }
        for child in read_dirs_sorted(&dir)? {
            // Dot-named children of a container are either separately
            // enumerated containers (`skills/.curated` under `skills/`) or
            // hidden dirs — never skills themselves. Without this skip the
            // overlapping containers double-discover the same path.
            if child.name.starts_with('.') {
                continue;
            }
            let rel = format!("{container}/{}", child.name);
            if child.path.join("SKILL.md").is_file() {
                found.push(make(&child.path, child.name, rel, false));
                continue; // never descend past a found SKILL.md
            }
            // The one extra level is the `skills/<category>/<skill>` catalog
            // layout ONLY — the agent-convention dirs (.claude/skills, …)
            // hold flat skills, so descending a level there would discover
            // more than the stated policy. Grandchildren only under skills/.
            if *container == "skills" || container.starts_with("skills/") {
                for grand in read_dirs_sorted(&child.path)? {
                    if grand.path.join("SKILL.md").is_file() {
                        found.push(make(
                            &grand.path,
                            grand.name.clone(),
                            format!("{rel}/{}", grand.name),
                            false,
                        ));
                    }
                }
            }
        }
    }

    // Recursive fallback ONLY when the conventional locations are empty —
    // and every hit is marked so the caller announces its origin.
    if found.is_empty() {
        walk_fallback(root, root, 0, &mut found)?;
    }

    found.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    reject_duplicates(&found)?;
    Ok(found)
}

fn make(dir: &Path, name: String, rel_path: String, via_fallback: bool) -> DiscoveredSkill {
    let description = std::fs::read_to_string(dir.join("SKILL.md"))
        .ok()
        .and_then(|md| crate::library::parse_frontmatter_description(&md))
        .filter(|d| !d.is_empty());
    let name_valid = crate::text::validate_name(&name).is_ok();
    DiscoveredSkill {
        name,
        rel_path,
        description,
        via_fallback,
        name_valid,
    }
}

struct DirEntryLite {
    name: String,
    path: PathBuf,
}

/// Immediate child directories, sorted by name (deterministic order),
/// skipping the never-entered set. Symlinked dirs are not followed — a
/// hostile repo must not route discovery outside the checkout.
fn read_dirs_sorted(dir: &Path) -> Result<Vec<DirEntryLite>> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(out);
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if SKIP_DIRS.contains(&name.as_str()) {
            continue;
        }
        let path = entry.path();
        let is_real_dir = entry
            .file_type()
            .map(|t| t.is_dir() && !t.is_symlink())
            .unwrap_or(false);
        if is_real_dir {
            out.push(DirEntryLite { name, path });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

fn walk_fallback(
    root: &Path,
    dir: &Path,
    depth: usize,
    found: &mut Vec<DiscoveredSkill>,
) -> Result<()> {
    // A call at `depth` inspects children at level `depth + 1`, so returning
    // at `depth == FALLBACK_MAX_DEPTH` caps discovered skills at exactly that
    // depth (a `>` guard admitted one level deeper than documented).
    if depth >= FALLBACK_MAX_DEPTH {
        return Ok(());
    }
    for child in read_dirs_sorted(dir)? {
        if child.path.join("SKILL.md").is_file() {
            let rel = child
                .path
                .strip_prefix(root)
                .unwrap_or(&child.path)
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/");
            found.push(make(&child.path, child.name, rel, true));
            continue; // never descend past a found SKILL.md
        }
        walk_fallback(root, &child.path, depth + 1, found)?;
    }
    Ok(())
}

/// Duplicate names across locations are the caller's ambiguity to resolve,
/// not ours to guess at — never first-wins (design §2).
fn reject_duplicates(found: &[DiscoveredSkill]) -> Result<()> {
    use std::collections::BTreeMap;
    let mut by_name: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for s in found {
        by_name.entry(&s.name).or_default().push(&s.rel_path);
    }
    let dups: Vec<String> = by_name
        .iter()
        .filter(|(_, paths)| paths.len() > 1)
        .map(|(name, paths)| {
            format!(
                "'{}' at {}",
                crate::text::sanitize_line(name),
                paths
                    .iter()
                    .map(|p| crate::text::sanitize_line(p))
                    .collect::<Vec<_>>()
                    .join(" and ")
            )
        })
        .collect();
    if dups.is_empty() {
        Ok(())
    } else {
        bail!(
            "duplicate skill names across locations — {} — a name must be unambiguous; \
             use --subpath to scope the source to one location",
            dups.join("; ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::prelude::*;

    fn skill(tmp: &assert_fs::TempDir, rel: &str, desc: &str) {
        let d = tmp.child(rel);
        d.create_dir_all().unwrap();
        d.child("SKILL.md")
            .write_str(&format!("---\nname: x\ndescription: {desc}\n---\n# s\n"))
            .unwrap();
    }

    #[test]
    fn priority_locations_and_depth_rules() {
        let tmp = assert_fs::TempDir::new().unwrap();
        skill(&tmp, "skills/alpha", "first");
        skill(&tmp, "skills/.curated/beta", "curated");
        skill(&tmp, ".claude/skills/gamma", "agent dir");
        // Catalog layout: one extra level, but ONLY under skills/.
        skill(&tmp, "skills/writing/delta", "nested catalog");
        // A grandchild under an agent dir is NOT discovered — the extra
        // level is a skills/<category>/<skill> convention only.
        skill(&tmp, ".claude/skills/cat/hidden", "agent grandchild");
        // Below a found SKILL.md — must NOT be discovered.
        skill(&tmp, "skills/alpha/inner", "too deep");
        // Never-entered dirs.
        skill(&tmp, "node_modules/pkg", "skipped");
        // A skill in a random location — ignored because priority found hits.
        skill(&tmp, "random/deep/omega", "not scanned");

        let found = discover_skills(tmp.path(), None).unwrap();
        let names: Vec<(&str, &str, bool)> = found
            .iter()
            .map(|s| (s.name.as_str(), s.rel_path.as_str(), s.via_fallback))
            .collect();
        assert_eq!(
            names,
            vec![
                ("gamma", ".claude/skills/gamma", false),
                ("beta", "skills/.curated/beta", false),
                ("alpha", "skills/alpha", false),
                ("delta", "skills/writing/delta", false),
            ]
        );
        assert_eq!(found[2].description.as_deref(), Some("first"));
        assert!(found.iter().all(|s| s.name_valid));
    }

    #[test]
    fn root_skill_uses_hint_and_scans_nothing_else() {
        let tmp = assert_fs::TempDir::new().unwrap();
        tmp.child("SKILL.md")
            .write_str("---\ndescription: root skill\n---\n")
            .unwrap();
        skill(&tmp, "skills/other", "must not appear");
        let found = discover_skills(tmp.path(), Some("my-repo")).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "my-repo");
        assert_eq!(found[0].rel_path, "");
        assert_eq!(found[0].description.as_deref(), Some("root skill"));
    }

    #[test]
    fn fallback_is_marked_and_only_fires_when_priority_is_empty() {
        let tmp = assert_fs::TempDir::new().unwrap();
        skill(&tmp, "tools/helper", "found by fallback");
        let found = discover_skills(tmp.path(), None).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].rel_path, "tools/helper");
        assert!(found[0].via_fallback);

        // Depth cap is exactly 5: a skill 5 levels below root is found, 6 is
        // out of reach (the guard was off-by-one, admitting depth 6).
        let at5 = assert_fs::TempDir::new().unwrap();
        skill(&at5, "a/b/c/d/e", "at the cap");
        assert_eq!(discover_skills(at5.path(), None).unwrap().len(), 1);
        let at6 = assert_fs::TempDir::new().unwrap();
        skill(&at6, "a/b/c/d/e/f", "beyond the cap");
        assert!(discover_skills(at6.path(), None).unwrap().is_empty());
    }

    #[test]
    fn duplicate_names_error_naming_both_paths() {
        let tmp = assert_fs::TempDir::new().unwrap();
        skill(&tmp, "skills/pdf", "one");
        skill(&tmp, ".claude/skills/pdf", "two");
        let err = discover_skills(tmp.path(), None).unwrap_err().to_string();
        assert!(err.contains("skills/pdf"), "{err}");
        assert!(err.contains(".claude/skills/pdf"), "{err}");
    }

    #[test]
    fn invalid_basenames_are_listed_not_hidden() {
        let tmp = assert_fs::TempDir::new().unwrap();
        skill(&tmp, "skills/PDF", "uppercase violates the contract");
        let found = discover_skills(tmp.path(), None).unwrap();
        assert_eq!(found.len(), 1);
        assert!(!found[0].name_valid);
    }
}
