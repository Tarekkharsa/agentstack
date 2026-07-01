//! `agentstack lib` — manage the central capability library
//! (`~/.agentstack/lib/`) that projects reference by name instead of copying
//! files (see `plan/central-store.md`).
//!
//! This module owns the **library write contract**: [`add_skill`] is the single
//! insertion path — how an item enters `library.toml`, how its files land under
//! `lib/skills/`, and how its checksum + provenance are recorded. Future
//! surface (`lib migrate`, `consolidate` emitting name refs) reuses it rather
//! than inventing its own file/index logic.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use owo_colors::OwoColorize;

use crate::cli::{LibAddArgs, LibArgs, LibKind, LibMigrateArgs, LibRemoveArgs};
use crate::library::{Library, LibrarySkill};
use crate::manifest::Skill;
use crate::store::{dir_digest, Store};
use crate::util::paths;

pub fn run(args: &LibArgs, _manifest_dir: Option<&Path>) -> Result<()> {
    match &args.kind {
        LibKind::Add(a) => add(a),
        LibKind::List => list(),
        LibKind::Remove(a) => remove(a),
        LibKind::Migrate(a) => migrate(a),
    }
}

/// Where a library skill's content is being added from.
pub enum LibSource<'a> {
    /// A local skill directory (copied into `lib/skills/<name>`).
    Path(&'a Path),
    /// A git source (resolved via the store; referenced, not copied).
    Git { url: &'a str, rev: Option<&'a str> },
}

/// The result of a library insertion (or a dry-run preview of one).
#[derive(Debug)]
pub struct AddOutcome {
    pub name: String,
    /// `"path"` or `"git"`.
    pub source_kind: &'static str,
    /// SHA-256 of the resolved content.
    pub checksum: String,
    /// The `lib/skills/<name>` directory for path sources; `None` for git.
    pub dest: Option<PathBuf>,
    /// False on a dry run (nothing was written).
    pub written: bool,
    /// True when an existing entry of the same name was overwritten.
    pub replaced: bool,
}

/// Insert a skill into the central library at `lib_home`. The single library
/// write path, reused by the CLI and (later) migrate/consolidate.
///
/// - `Path`: validated to contain `SKILL.md`, copied into `lib/skills/<name>`,
///   digested there, recorded as `path = "<name>"`.
/// - `Git`: resolved through the store (records `git`, resolved `rev`, and
///   checksum); the body stays in the store, referenced by the entry.
///
/// A same-named entry is a hard error unless `replace` is set. When `write` is
/// false, nothing is mutated and the returned outcome is a preview.
pub fn add_skill(
    lib_home: &Path,
    name: &str,
    source: LibSource,
    replace: bool,
    write: bool,
) -> Result<AddOutcome> {
    if !valid_lib_name(name) {
        bail!("invalid library skill name '{name}' — must be non-empty and contain no path separators");
    }

    let mut library = Library::load(lib_home)?;
    let replacing = library.get(name).is_some();
    if replacing && !replace {
        bail!("'{name}' is already in the central library — pass --replace to overwrite");
    }

    let (entry, dest, checksum, source_kind) = match source {
        LibSource::Path(src) => {
            let src = absolutize(src)?;
            require_skill_md(&src)?;
            let dest = lib_home.join("skills").join(name);
            if same_dir(&src, &dest) {
                bail!(
                    "source {} is already the library location — nothing to add",
                    src.display()
                );
            }
            // Digest the source now so the preview reflects what would land; a
            // write copies first and re-digests the destination.
            let checksum = if write {
                if dest.exists() {
                    std::fs::remove_dir_all(&dest)
                        .with_context(|| format!("removing {}", dest.display()))?;
                }
                crate::consolidate::copy_dir(&src, &dest)?;
                dir_digest(&dest)?
            } else {
                dir_digest(&src)?
            };
            let entry = LibrarySkill {
                name: name.to_string(),
                source: "path".into(),
                path: Some(name.to_string()),
                git: None,
                rev: None,
                checksum: Some(checksum.clone()),
                version: None,
                provenance: Some(format!("path:{}", src.display())),
            };
            (entry, Some(dest), checksum, "path")
        }
        LibSource::Git { url, rev } => {
            // Resolving fetches into the store (needed to learn rev + checksum and
            // to validate SKILL.md) — this touches the network even on a dry run.
            let store = Store::default_store();
            let skill = Skill {
                path: None,
                git: Some(url.to_string()),
                rev: rev.map(str::to_string),
            };
            let resolved = store
                .resolve(&skill, lib_home, rev)
                .with_context(|| format!("resolving git source {url}"))?;
            require_skill_md(&resolved.path)?;
            let entry = LibrarySkill {
                name: name.to_string(),
                source: "git".into(),
                path: None,
                git: Some(url.to_string()),
                rev: resolved.rev.clone(),
                checksum: Some(resolved.checksum.clone()),
                version: None,
                provenance: Some(format!("git:{url}")),
            };
            (entry, None, resolved.checksum, "git")
        }
    };

    if write {
        library.upsert(entry);
        library.save(lib_home)?;
    }

    Ok(AddOutcome {
        name: name.to_string(),
        source_kind,
        checksum,
        dest,
        written: write,
        replaced: replacing,
    })
}

fn add(args: &LibAddArgs) -> Result<()> {
    let lib_home = paths::lib_home();
    let source = match (&args.path, &args.git) {
        (Some(p), None) => LibSource::Path(Path::new(p)),
        (None, Some(url)) => LibSource::Git {
            url,
            rev: args.rev.as_deref(),
        },
        (None, None) => bail!("specify a source: --path <dir> or --git <url>"),
        (Some(_), Some(_)) => bail!("--path and --git are mutually exclusive"),
    };

    let outcome = add_skill(&lib_home, &args.name, source, args.replace, args.write)?;

    let verb = if outcome.replaced { "replace" } else { "add" };
    if outcome.written {
        println!(
            "{} {verb}d '{}' ({}) in the central library",
            "✓".green(),
            outcome.name,
            outcome.source_kind
        );
        if let Some(dest) = &outcome.dest {
            println!("  files → {}", dest.display());
        }
        println!("  checksum {}", short(&outcome.checksum));
    } else {
        println!(
            "Would {verb} '{}' ({}) into the central library:",
            outcome.name.bold(),
            outcome.source_kind
        );
        if let Some(dest) = &outcome.dest {
            println!("  {} files → {}", "→".cyan(), dest.display());
        }
        println!("  {} checksum {}", "→".cyan(), short(&outcome.checksum));
        println!("\nDry run. Re-run with {} to apply.", "--write".bold());
    }
    Ok(())
}

/// `lib list` — a plain read of the index. No resolver, no store, no filesystem
/// validation: it reports what `library.toml` records, nothing more.
fn list() -> Result<()> {
    let lib_home = paths::lib_home();
    let library = Library::load(&lib_home)?;
    print!("{}", render_list(&library));
    Ok(())
}

/// Render the library index as a plain table (shared with tests). Rows are
/// sorted by name for stable output regardless of on-disk order.
fn render_list(library: &Library) -> String {
    if library.skills.is_empty() {
        return "No skills installed in the central library.\n".to_string();
    }
    let mut skills = library.skills.clone();
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    let mut o = String::new();
    o.push_str(&format!(
        "{:<20} {:<6} {:<16} {}\n",
        "NAME", "SOURCE", "REV/CHECKSUM", "PROVENANCE"
    ));
    for s in &skills {
        o.push_str(&format!(
            "{:<20} {:<6} {:<16} {}\n",
            s.name,
            s.source,
            locator(s),
            s.provenance.as_deref().unwrap_or("-")
        ));
    }
    o
}

/// A short, glanceable locator for a row: the git rev if present, else the
/// content checksum, both truncated.
fn locator(s: &LibrarySkill) -> String {
    if let Some(rev) = &s.rev {
        return format!("rev {}", short(rev));
    }
    match &s.checksum {
        Some(c) => short(c).to_string(),
        None => "-".to_string(),
    }
}

/// The result of a library removal (or a dry-run preview of one).
#[derive(Debug)]
pub struct RemoveOutcome {
    pub name: String,
    /// `"path"` or `"git"`, from the removed index entry.
    pub source_kind: String,
    /// The contained `lib/skills/<name>` dir that would be / was deleted
    /// (path skills only; `None` for git-backed or uncontained entries).
    pub removed_dir: Option<PathBuf>,
    /// False on a dry run (nothing was mutated).
    pub written: bool,
}

/// Remove a skill from the central library at `lib_home`. The inverse of
/// [`add_skill`]: drops the `library.toml` entry and, for a path skill, deletes
/// its contained `lib/skills/<name>` directory. Git-backed entries leave the
/// shared store cache untouched. Does not touch project manifests or lockfiles.
///
/// A missing name is a hard error. When `write` is false, nothing is mutated.
pub fn remove_skill(lib_home: &Path, name: &str, write: bool) -> Result<RemoveOutcome> {
    let mut library = Library::load(lib_home)?;
    let Some(entry) = library.get(name).cloned() else {
        bail!("'{name}' is not in the central library");
    };

    // Only path skills own files to delete, and only within lib/skills. A git
    // entry references the shared store cache — never delete that here.
    let removed_dir = if entry.source == "path" {
        entry
            .path
            .as_deref()
            .and_then(|p| contained_lib_skill_dir(lib_home, p))
    } else {
        None
    };

    if write {
        if let Some(dir) = &removed_dir {
            if dir.exists() {
                std::fs::remove_dir_all(dir)
                    .with_context(|| format!("removing {}", dir.display()))?;
            }
        }
        library.remove(name);
        library.save(lib_home)?;
    }

    Ok(RemoveOutcome {
        name: name.to_string(),
        source_kind: entry.source,
        removed_dir,
        written: write,
    })
}

fn remove(args: &LibRemoveArgs) -> Result<()> {
    let lib_home = paths::lib_home();
    let outcome = remove_skill(&lib_home, &args.name, args.write)?;

    if outcome.written {
        println!(
            "{} removed '{}' ({}) from the central library",
            "✓".green(),
            outcome.name,
            outcome.source_kind
        );
        if let Some(dir) = &outcome.removed_dir {
            println!("  deleted {}", dir.display());
        }
    } else {
        println!(
            "Would remove '{}' ({}) from the central library:",
            outcome.name.bold(),
            outcome.source_kind
        );
        match &outcome.removed_dir {
            Some(dir) => println!("  {} delete {}", "−".yellow(), dir.display()),
            None if outcome.source_kind == "git" => {
                println!(
                    "  {} index entry only (store cache left in place)",
                    "−".yellow()
                )
            }
            None => println!("  {} index entry only", "−".yellow()),
        }
        println!("\nDry run. Re-run with {} to apply.", "--write".bold());
    }
    Ok(())
}

/// A migration plan/result: which skills were (or would be) migrated and which
/// on-disk entries were skipped and why.
#[derive(Debug)]
pub struct MigrateReport {
    pub migrated: Vec<String>,
    /// `(entry name, reason)` for directories that were not valid skills.
    pub skipped: Vec<(String, String)>,
    pub written: bool,
}

/// Migrate skills from the legacy skills home (`old`) into the central library
/// at `lib_home`, reusing [`add_skill`] for each. **Copy-first and reversible**:
/// originals under `old` are never touched, so a failed or unwanted migration
/// leaves the source intact.
///
/// Only directories containing `SKILL.md` with a safe name are migrated; other
/// entries are recorded in `skipped`. Collisions with existing library entries
/// are a hard error (checked up front, before any write) unless `replace`.
pub fn migrate_skills(
    old: &Path,
    lib_home: &Path,
    replace: bool,
    write: bool,
) -> Result<MigrateReport> {
    let mut candidates: Vec<(String, PathBuf)> = Vec::new();
    let mut skipped: Vec<(String, String)> = Vec::new();

    if old.exists() {
        let mut entries: Vec<_> = std::fs::read_dir(old)
            .with_context(|| format!("reading {}", old.display()))?
            .collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let path = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if !path.is_dir() {
                continue; // ignore stray files at the skills-home root
            }
            if !path.join("SKILL.md").exists() {
                skipped.push((name, "no SKILL.md".into()));
                continue;
            }
            if !valid_lib_name(&name) {
                skipped.push((name, "unsafe name".into()));
                continue;
            }
            candidates.push((name, path));
        }
    }

    // Fail fast on collisions (matching `lib add`) before mutating anything.
    if !replace {
        let library = Library::load(lib_home)?;
        let collisions: Vec<String> = candidates
            .iter()
            .filter(|(n, _)| library.get(n).is_some())
            .map(|(n, _)| n.clone())
            .collect();
        if !collisions.is_empty() {
            bail!(
                "already in the central library: {} — pass --replace to overwrite",
                collisions.join(", ")
            );
        }
    }

    let mut migrated = Vec::new();
    for (name, path) in &candidates {
        add_skill(lib_home, name, LibSource::Path(path), replace, write)?;
        migrated.push(name.clone());
    }

    Ok(MigrateReport {
        migrated,
        skipped,
        written: write,
    })
}

fn migrate(args: &LibMigrateArgs) -> Result<()> {
    let old = paths::skills_home();
    let lib_home = paths::lib_home();
    let report = migrate_skills(&old, &lib_home, args.replace, args.write)?;

    if report.migrated.is_empty() && report.skipped.is_empty() {
        println!("Nothing to migrate — {} is empty or absent.", old.display());
        return Ok(());
    }

    let verb = if report.written {
        "Migrated"
    } else {
        "Would migrate"
    };
    println!(
        "{verb} {} skill(s) from {} → {}:",
        report.migrated.len(),
        old.display(),
        lib_home.join("skills").display()
    );
    for n in &report.migrated {
        let mark = if report.written {
            "✓".green().to_string()
        } else {
            "→".cyan().to_string()
        };
        println!("  {mark} {n}");
    }
    for (n, why) in &report.skipped {
        println!("  {} skipped {n} — {why}", "⚠".yellow());
    }

    if report.written {
        println!(
            "\nOriginals left in place at {} (migration is reversible).",
            old.display()
        );
    } else {
        println!("\nDry run. Re-run with {} to apply.", "--write".bold());
    }
    Ok(())
}

/// Resolve a library entry's `path` to the exact contained `lib/skills/<...>`
/// dir that is safe to `remove_dir_all`. Rejects any path with a `.`, `..`,
/// root, or drive-prefix component so a hand-edited index can never delete
/// outside the library. `None` → leave the filesystem untouched.
fn contained_lib_skill_dir(lib_home: &Path, path: &str) -> Option<PathBuf> {
    let rel = Path::new(path.trim_start_matches("./"));
    let mut comps = 0;
    for c in rel.components() {
        if !matches!(c, std::path::Component::Normal(_)) {
            return None;
        }
        comps += 1;
    }
    if comps == 0 {
        return None;
    }
    Some(lib_home.join("skills").join(rel))
}

/// A name safe to use as a `lib/skills/<name>` directory and index key.
fn valid_lib_name(name: &str) -> bool {
    !name.is_empty() && !name.contains('/') && !name.contains('\\') && name != "." && name != ".."
}

/// Resolve a possibly-relative, possibly-`~` path to an absolute one.
fn absolutize(p: &Path) -> Result<PathBuf> {
    let expanded = paths::expand_tilde(&p.to_string_lossy());
    if expanded.is_absolute() {
        Ok(expanded)
    } else {
        Ok(std::env::current_dir()?.join(expanded))
    }
}

fn require_skill_md(dir: &Path) -> Result<()> {
    if !dir.join("SKILL.md").exists() {
        bail!(
            "no SKILL.md in {} — not a valid skill directory",
            dir.display()
        );
    }
    Ok(())
}

/// Whether two paths point at the same directory (best-effort via canonicalize).
fn same_dir(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// First 12 chars of a checksum, for a glanceable pin.
fn short(sum: &str) -> &str {
    &sum[..sum.len().min(12)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::prelude::*;

    fn src_skill(dir: &assert_fs::TempDir, body: &str) -> PathBuf {
        dir.child("src/SKILL.md").write_str(body).unwrap();
        dir.child("src").path().to_path_buf()
    }

    #[test]
    fn add_path_copies_digests_and_records_provenance() {
        let lib = assert_fs::TempDir::new().unwrap();
        let work = assert_fs::TempDir::new().unwrap();
        let src = src_skill(&work, "# body\n");

        let out = add_skill(lib.path(), "sql-review", LibSource::Path(&src), false, true).unwrap();

        assert!(out.written);
        assert_eq!(out.source_kind, "path");
        assert_eq!(out.checksum.len(), 64);
        // Files landed under lib/skills/<name>.
        assert!(lib.child("skills/sql-review/SKILL.md").path().exists());
        // Index records the entry with checksum + provenance.
        let library = Library::load(lib.path()).unwrap();
        let entry = library.get("sql-review").unwrap();
        assert_eq!(entry.path.as_deref(), Some("sql-review"));
        assert_eq!(entry.checksum.as_deref(), Some(out.checksum.as_str()));
        assert!(entry.provenance.as_deref().unwrap().starts_with("path:"));
    }

    #[test]
    fn dry_run_writes_nothing() {
        let lib = assert_fs::TempDir::new().unwrap();
        let work = assert_fs::TempDir::new().unwrap();
        let src = src_skill(&work, "# body\n");

        let out = add_skill(lib.path(), "x", LibSource::Path(&src), false, false).unwrap();

        assert!(!out.written);
        assert_eq!(out.checksum.len(), 64, "preview still digests the source");
        assert!(!lib.child("skills/x").path().exists(), "no files written");
        assert!(Library::load(lib.path()).unwrap().get("x").is_none());
    }

    #[test]
    fn collision_without_replace_is_error() {
        let lib = assert_fs::TempDir::new().unwrap();
        let work = assert_fs::TempDir::new().unwrap();
        let src = src_skill(&work, "# body\n");
        add_skill(lib.path(), "x", LibSource::Path(&src), false, true).unwrap();

        let err = add_skill(lib.path(), "x", LibSource::Path(&src), false, true).unwrap_err();
        assert!(err.to_string().contains("--replace"));
    }

    #[test]
    fn replace_overwrites_content_and_digest() {
        let lib = assert_fs::TempDir::new().unwrap();
        let work = assert_fs::TempDir::new().unwrap();
        let src1 = src_skill(&work, "# original\n");
        let first = add_skill(lib.path(), "x", LibSource::Path(&src1), false, true).unwrap();

        // A different source body under the same name, with --replace.
        let work2 = assert_fs::TempDir::new().unwrap();
        let src2 = src_skill(&work2, "# changed\n");
        let second = add_skill(lib.path(), "x", LibSource::Path(&src2), true, true).unwrap();

        assert!(second.replaced);
        assert_ne!(first.checksum, second.checksum);
        let body = std::fs::read_to_string(lib.child("skills/x/SKILL.md").path()).unwrap();
        assert_eq!(body, "# changed\n");
    }

    #[test]
    fn missing_skill_md_is_error() {
        let lib = assert_fs::TempDir::new().unwrap();
        let work = assert_fs::TempDir::new().unwrap();
        work.child("src/notes.txt").write_str("x").unwrap();
        let src = work.child("src").path().to_path_buf();

        let err = add_skill(lib.path(), "x", LibSource::Path(&src), false, true).unwrap_err();
        assert!(err.to_string().contains("SKILL.md"));
    }

    #[test]
    fn invalid_name_is_error() {
        let lib = assert_fs::TempDir::new().unwrap();
        let work = assert_fs::TempDir::new().unwrap();
        let src = src_skill(&work, "# body\n");
        let err =
            add_skill(lib.path(), "../escape", LibSource::Path(&src), false, true).unwrap_err();
        assert!(err.to_string().contains("invalid library skill name"));
    }

    fn path_entry(name: &str, checksum: &str) -> LibrarySkill {
        LibrarySkill {
            name: name.into(),
            source: "path".into(),
            path: Some(name.into()),
            git: None,
            rev: None,
            checksum: Some(checksum.into()),
            version: None,
            provenance: Some(format!("path:/src/{name}")),
        }
    }

    #[test]
    fn list_empty_says_none() {
        let out = render_list(&Library::default());
        assert!(out.contains("No skills installed"));
    }

    #[test]
    fn list_path_row_shows_name_source_checksum_provenance() {
        let mut library = Library::default();
        library.upsert(path_entry("sql-review", "abcdef0123456789deadbeef"));
        let out = render_list(&library);
        assert!(out.contains("sql-review"));
        assert!(out.contains("path"));
        assert!(out.contains("abcdef012345"), "short checksum (12 chars)");
        assert!(out.contains("path:/src/sql-review"), "provenance");
    }

    #[test]
    fn list_git_row_shows_short_rev() {
        let mut library = Library::default();
        library.upsert(LibrarySkill {
            name: "gitskill".into(),
            source: "git".into(),
            path: None,
            git: Some("https://example.com/x.git".into()),
            rev: Some("0123456789abcdef0123456789abcdef01234567".into()),
            checksum: Some("feedface00001111".into()),
            version: None,
            provenance: Some("git:https://example.com/x.git".into()),
        });
        let out = render_list(&library);
        assert!(out.contains("git"));
        assert!(
            out.contains("rev 0123456789ab"),
            "short rev preferred for git"
        );
    }

    #[test]
    fn list_is_sorted_by_name() {
        let mut library = Library::default();
        // Insert out of order; render must sort.
        library.skills.push(path_entry("zebra", "1111"));
        library.skills.push(path_entry("alpha", "2222"));
        let out = render_list(&library);
        let a = out.find("alpha").unwrap();
        let z = out.find("zebra").unwrap();
        assert!(a < z, "rows sorted by name");
    }

    #[test]
    fn add_git_records_rev_and_checksum() {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", home.path());

        // A local git repo used as the source.
        let work = assert_fs::TempDir::new().unwrap();
        let repo = work.child("repo");
        repo.create_dir_all().unwrap();
        let git = |a: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(repo.path())
                .args(a)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {a:?} failed");
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@e.st"]);
        git(&["config", "user.name", "t"]);
        repo.child("SKILL.md").write_str("# git skill\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "init"]);

        let lib_home = home.child("lib");
        let url = format!("file://{}", repo.path().display());
        let out = add_skill(
            lib_home.path(),
            "gitskill",
            LibSource::Git {
                url: &url,
                rev: None,
            },
            false,
            true,
        )
        .unwrap();

        assert_eq!(out.source_kind, "git");
        assert_eq!(out.checksum.len(), 64);
        let library = Library::load(lib_home.path()).unwrap();
        let entry = library.get("gitskill").unwrap();
        assert_eq!(entry.git.as_deref(), Some(url.as_str()));
        assert!(entry.rev.is_some());
        assert!(entry.provenance.as_deref().unwrap().starts_with("git:"));

        std::env::remove_var("AGENTSTACK_HOME");
    }

    // ---------- remove ----------

    #[test]
    fn remove_dry_run_leaves_entry_and_files() {
        let lib = assert_fs::TempDir::new().unwrap();
        let work = assert_fs::TempDir::new().unwrap();
        let src = src_skill(&work, "# body\n");
        add_skill(lib.path(), "x", LibSource::Path(&src), false, true).unwrap();

        let out = remove_skill(lib.path(), "x", false).unwrap();

        assert!(!out.written);
        assert!(lib.child("skills/x/SKILL.md").path().exists(), "files kept");
        assert!(
            Library::load(lib.path()).unwrap().get("x").is_some(),
            "entry kept"
        );
    }

    #[test]
    fn remove_write_deletes_path_entry_and_files() {
        let lib = assert_fs::TempDir::new().unwrap();
        let work = assert_fs::TempDir::new().unwrap();
        let src = src_skill(&work, "# body\n");
        add_skill(lib.path(), "x", LibSource::Path(&src), false, true).unwrap();

        let out = remove_skill(lib.path(), "x", true).unwrap();

        assert!(out.written);
        assert_eq!(
            out.removed_dir.as_deref(),
            Some(lib.child("skills/x").path())
        );
        assert!(!lib.child("skills/x").path().exists(), "dir deleted");
        assert!(
            Library::load(lib.path()).unwrap().get("x").is_none(),
            "entry gone"
        );
    }

    #[test]
    fn remove_git_leaves_store_cache_alone() {
        let lib = assert_fs::TempDir::new().unwrap();
        // A git entry whose "cache" lives outside lib/skills — must be untouched.
        let cache = assert_fs::TempDir::new().unwrap();
        cache.child("SKILL.md").write_str("# cached\n").unwrap();
        let mut library = Library::default();
        library.upsert(LibrarySkill {
            name: "gitskill".into(),
            source: "git".into(),
            path: None,
            git: Some("https://example.com/x.git".into()),
            rev: Some("abc123".into()),
            checksum: Some("deadbeef".into()),
            version: None,
            provenance: Some("git:https://example.com/x.git".into()),
        });
        library.save(lib.path()).unwrap();

        let out = remove_skill(lib.path(), "gitskill", true).unwrap();

        assert!(out.written);
        assert_eq!(out.removed_dir, None, "git entries delete no files");
        assert!(
            cache.child("SKILL.md").path().exists(),
            "store cache untouched"
        );
        assert!(Library::load(lib.path()).unwrap().get("gitskill").is_none());
    }

    #[test]
    fn remove_missing_name_errors() {
        let lib = assert_fs::TempDir::new().unwrap();
        let err = remove_skill(lib.path(), "nope", true).unwrap_err();
        assert!(err.to_string().contains("not in the central library"));
    }

    #[test]
    fn remove_never_deletes_outside_the_library() {
        let lib = assert_fs::TempDir::new().unwrap();
        // A directory outside the library that a malicious index path targets.
        let outside = assert_fs::TempDir::new().unwrap();
        outside.child("keep.txt").write_str("important\n").unwrap();

        // Hand-crafted index entry with an escaping path.
        let mut library = Library::default();
        library.upsert(LibrarySkill {
            name: "evil".into(),
            source: "path".into(),
            path: Some("../../../../../../../../etc".into()),
            git: None,
            rev: None,
            checksum: Some("x".into()),
            version: None,
            provenance: None,
        });
        library.save(lib.path()).unwrap();

        let out = remove_skill(lib.path(), "evil", true).unwrap();

        // Uncontained path → no directory targeted for deletion...
        assert_eq!(out.removed_dir, None);
        // ...nothing outside is touched...
        assert!(outside.child("keep.txt").path().exists());
        // ...but the bogus index entry is still cleaned up.
        assert!(Library::load(lib.path()).unwrap().get("evil").is_none());
    }

    // ---------- migrate ----------

    /// Create `<old>/<name>/SKILL.md` in a legacy skills-home layout.
    fn old_skill(old: &Path, name: &str, body: &str) {
        let dir = old.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), body).unwrap();
    }

    #[test]
    fn migrate_empty_source_migrates_nothing() {
        let lib = assert_fs::TempDir::new().unwrap();
        let old = assert_fs::TempDir::new().unwrap();
        let report = migrate_skills(old.path(), lib.path(), false, true).unwrap();
        assert!(report.migrated.is_empty());
        assert!(report.skipped.is_empty());
    }

    #[test]
    fn migrate_dry_run_writes_nothing() {
        let lib = assert_fs::TempDir::new().unwrap();
        let old = assert_fs::TempDir::new().unwrap();
        old_skill(old.path(), "a", "# a\n");
        old_skill(old.path(), "b", "# b\n");

        let report = migrate_skills(old.path(), lib.path(), false, false).unwrap();

        assert_eq!(report.migrated, vec!["a".to_string(), "b".to_string()]);
        assert!(!lib.child("skills/a").path().exists(), "no files written");
        assert!(
            Library::load(lib.path()).unwrap().skills.is_empty(),
            "index untouched"
        );
    }

    #[test]
    fn migrate_write_copies_and_indexes_multiple() {
        let lib = assert_fs::TempDir::new().unwrap();
        let old = assert_fs::TempDir::new().unwrap();
        old_skill(old.path(), "a", "# a\n");
        old_skill(old.path(), "b", "# b\n");

        let report = migrate_skills(old.path(), lib.path(), false, true).unwrap();

        assert_eq!(report.migrated.len(), 2);
        assert!(lib.child("skills/a/SKILL.md").path().exists());
        assert!(lib.child("skills/b/SKILL.md").path().exists());
        let library = Library::load(lib.path()).unwrap();
        assert!(library.get("a").is_some() && library.get("b").is_some());
        // Copy-first: originals remain.
        assert!(
            old.child("a/SKILL.md").path().exists(),
            "source left in place"
        );
        assert!(old.child("b/SKILL.md").path().exists());
    }

    #[test]
    fn migrate_collision_fails_without_replace_and_succeeds_with() {
        let lib = assert_fs::TempDir::new().unwrap();
        let old = assert_fs::TempDir::new().unwrap();
        let work = assert_fs::TempDir::new().unwrap();
        let src = src_skill(&work, "# existing\n");
        add_skill(lib.path(), "a", LibSource::Path(&src), false, true).unwrap();
        old_skill(old.path(), "a", "# migrated\n");

        // Same name already in the library → hard error, nothing written.
        let err = migrate_skills(old.path(), lib.path(), false, true).unwrap_err();
        assert!(err.to_string().contains("--replace"));

        // With --replace it overwrites.
        let report = migrate_skills(old.path(), lib.path(), true, true).unwrap();
        assert_eq!(report.migrated, vec!["a".to_string()]);
        let body = std::fs::read_to_string(lib.child("skills/a/SKILL.md").path()).unwrap();
        assert_eq!(body, "# migrated\n");
    }

    #[test]
    fn migrate_reports_dirs_without_skill_md() {
        let lib = assert_fs::TempDir::new().unwrap();
        let old = assert_fs::TempDir::new().unwrap();
        old_skill(old.path(), "good", "# good\n");
        // A directory that is not a valid skill.
        std::fs::create_dir_all(old.path().join("notaskill")).unwrap();
        std::fs::write(old.path().join("notaskill/readme.txt"), "x").unwrap();

        let report = migrate_skills(old.path(), lib.path(), false, true).unwrap();

        assert_eq!(report.migrated, vec!["good".to_string()]);
        assert!(report
            .skipped
            .iter()
            .any(|(n, why)| n == "notaskill" && why.contains("SKILL.md")));
        assert!(Library::load(lib.path())
            .unwrap()
            .get("notaskill")
            .is_none());
    }
}
