// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Consolidation gathers real skill directories into the central library and
//! symlinks the originals back. It indexes each skill in `library.toml` (so it is
//! referenced by name), keeps content reachable through the links, backs up moved
//! dirs, and never writes `[skills.<name>]` path entries into the project
//! manifest. This lives in its own file so the `HOME`/`AGENTSTACK_HOME` overrides
//! run serialized (no env race with other integration tests).

use std::fs;
use std::path::{Path, PathBuf};

use agentstack::adapter::Registry;
use agentstack::commands::lib::{add_skill, LibSource};
use agentstack::consolidate::consolidate;
use agentstack::library::Library;

static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Set up a home with Codex's `figma` skill (a real dir) and return the roots.
fn setup(tmp: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let home = tmp.join("home");
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    let codex_skills = home.join(".agents/skills");
    fs::create_dir_all(codex_skills.join("figma")).unwrap();
    fs::write(codex_skills.join("figma/SKILL.md"), "# figma\n").unwrap();
    fs::write(codex_skills.join("figma/helper.py"), "print(1)\n").unwrap();

    let proj = tmp.join("proj");
    fs::create_dir_all(&proj).unwrap();
    let manifest = proj.join("agentstack.toml");
    fs::write(
        &manifest,
        "version = 1\n[targets]\ndefault = [\"claude-code\", \"codex\"]\n",
    )
    .unwrap();
    (home, proj, manifest)
}

#[test]
fn consolidate_indexes_into_library_and_symlinks_back() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let (home, proj, manifest) = setup(tmp.path());

    // A shared skill symlinked into Claude from an external source.
    let claude_skills = home.join(".claude/skills");
    fs::create_dir_all(&claude_skills).unwrap();
    let external = home.join("external-skills/shared");
    fs::create_dir_all(&external).unwrap();
    fs::write(external.join("SKILL.md"), "# shared\n").unwrap();
    std::os::unix::fs::symlink(&external, claude_skills.join("shared")).unwrap();

    let registry = Registry::load().unwrap();
    let report = consolidate(&registry, &manifest, &proj, None, false, true).unwrap();
    let names: Vec<&str> = report.skills.iter().map(|c| c.name.as_str()).collect();
    assert!(
        names.contains(&"figma") && names.contains(&"shared"),
        "{names:?}"
    );

    // Files now live in the central library.
    let lib_skills = home.join(".agentstack/lib/skills");
    assert!(lib_skills.join("figma/helper.py").is_file());
    assert!(lib_skills.join("shared/SKILL.md").is_file());

    // The library index records them (referenced by name, with a checksum).
    let library = Library::load(&home.join(".agentstack/lib")).unwrap();
    let figma = library.get("figma").expect("figma indexed");
    assert_eq!(figma.source, "path");
    assert!(figma.checksum.as_ref().unwrap().hex().len() == 64);
    assert!(library.get("shared").is_some());

    // Originals are symlinks resolving to the library copy (old behavior works).
    let figma_link = home.join(".agents/skills/figma");
    assert!(fs::symlink_metadata(&figma_link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(
        fs::canonicalize(&figma_link).unwrap(),
        fs::canonicalize(lib_skills.join("figma")).unwrap()
    );
    assert_eq!(
        fs::read_to_string(figma_link.join("helper.py")).unwrap(),
        "print(1)\n"
    );

    // A backup of the moved real dir was kept.
    assert!(home
        .join(".agentstack/backups/skills/figma/SKILL.md")
        .is_file());

    // The manifest is NOT given a `[skills.<name>]` path entry — the skill is
    // referenced by name from the library.
    let m = fs::read_to_string(&manifest).unwrap();
    assert!(!m.contains("[skills.figma]"), "no path entry written: {m}");

    // Re-running is idempotent: already-in-library skills don't error.
    let again = consolidate(&registry, &manifest, &proj, None, false, true).unwrap();
    assert!(again
        .skills
        .iter()
        .any(|c| c.name == "figma" && c.already_home));
    assert!(lib_skills.join("figma/helper.py").is_file());
}

#[test]
fn consolidate_dry_run_writes_nothing() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let (home, proj, manifest) = setup(tmp.path());

    let registry = Registry::load().unwrap();
    let report = consolidate(&registry, &manifest, &proj, None, false, false).unwrap();
    assert!(report.skills.iter().any(|c| c.name == "figma"));

    // No files, no library index, no symlink change.
    assert!(!home.join(".agentstack/lib/skills/figma").exists());
    assert!(Library::load(&home.join(".agentstack/lib"))
        .unwrap()
        .skills
        .is_empty());
    let figma = home.join(".agents/skills/figma");
    assert!(
        figma.is_dir()
            && !fs::symlink_metadata(&figma)
                .unwrap()
                .file_type()
                .is_symlink()
    );
}

#[test]
fn consolidate_preserves_inline_manifest_definition() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let (_home, proj, manifest) = setup(tmp.path());

    // The project pins figma inline to its own path.
    fs::write(
        &manifest,
        "version = 1\n[targets]\ndefault = [\"codex\"]\n\
         [skills.figma]\npath = \"./skills/figma\"\n",
    )
    .unwrap();

    let registry = Registry::load().unwrap();
    let report = consolidate(&registry, &manifest, &proj, None, false, true).unwrap();

    // The inline definition is left exactly as-is...
    let m = fs::read_to_string(&manifest).unwrap();
    assert!(m.contains("[skills.figma]"));
    assert!(m.contains("./skills/figma"));
    // ...and the override is reported.
    assert!(report
        .skills
        .iter()
        .any(|c| c.name == "figma" && c.inline_override));
}

#[test]
fn consolidate_collision_fails_without_replace() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let (home, proj, manifest) = setup(tmp.path());
    let lib_home = home.join(".agentstack/lib");

    // Seed a DIFFERENT-content figma into the library first.
    let other = tmp.path().join("other");
    fs::create_dir_all(&other).unwrap();
    fs::write(other.join("SKILL.md"), "# a different figma\n").unwrap();
    add_skill(
        &lib_home,
        "figma",
        LibSource::Path(&other),
        false,
        true,
        false,
    )
    .unwrap();

    let registry = Registry::load().unwrap();
    // Discovered figma differs from the library's → hard error without --replace.
    let err = consolidate(&registry, &manifest, &proj, None, false, true).unwrap_err();
    assert!(err.to_string().contains("--replace"), "{err}");

    // With --replace it overwrites the library copy.
    consolidate(&registry, &manifest, &proj, None, true, true).unwrap();
    let body = fs::read_to_string(lib_home.join("skills/figma/SKILL.md")).unwrap();
    assert_eq!(body, "# figma\n");
}

/// A dead symlink and a dir without SKILL.md are skipped, but the report must
/// SAY so — silently dropping them reads as "my skills weren't migrated".
#[test]
fn consolidate_reports_skipped_broken_links_and_non_skills() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let (home, proj, manifest) = setup(tmp.path());

    // A dead symlink in Claude's skills dir (its target was never created)…
    let claude_skills = home.join(".claude/skills");
    fs::create_dir_all(&claude_skills).unwrap();
    let gone = home.join("external-skills/find-skills");
    std::os::unix::fs::symlink(&gone, claude_skills.join("find-skills")).unwrap();
    // …and a real directory without a SKILL.md in Codex's.
    fs::create_dir_all(home.join(".agents/skills/notes")).unwrap();

    let registry = Registry::load().unwrap();
    let report = consolidate(&registry, &manifest, &proj, None, false, false).unwrap();

    // The valid skill still consolidates.
    assert!(report.skills.iter().any(|c| c.name == "figma"));

    // The dead link is reported with its target and origin CLI.
    let broken = report
        .skipped
        .iter()
        .find(|s| s.name == "find-skills")
        .expect("dead link reported");
    assert!(broken.broken);
    assert_eq!(broken.cli, "claude-code");
    assert_eq!(broken.target.as_deref(), Some(gone.as_path()));

    // The SKILL.md-less dir is reported as skipped, not broken.
    let notes = report
        .skipped
        .iter()
        .find(|s| s.name == "notes")
        .expect("non-skill dir reported");
    assert!(!notes.broken);
    assert_eq!(notes.cli, "codex");
}

/// When EVERY discovered entry is a dead link, consolidate must not claim
/// "no skills found" — it returns the skipped entries so the command can
/// explain what it saw and why nothing moved (the real-world pi case).
#[test]
fn consolidate_only_broken_links_returns_skipped_not_error() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    // Two dead links, nothing else, in Pi's skills dir.
    let pi_skills = home.join(".pi/agent/skills");
    fs::create_dir_all(&pi_skills).unwrap();
    for name in ["find-skills", "playwright-cli"] {
        std::os::unix::fs::symlink(
            home.join("external-skills").join(name),
            pi_skills.join(name),
        )
        .unwrap();
    }

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    let manifest = proj.join("agentstack.toml");
    fs::write(&manifest, "version = 1\n[targets]\ndefault = [\"pi\"]\n").unwrap();

    let registry = Registry::load().unwrap();
    let report = consolidate(&registry, &manifest, &proj, None, false, false).unwrap();
    assert!(report.skills.is_empty());
    let skipped: Vec<(&str, &str)> = report
        .skipped
        .iter()
        .map(|s| (s.cli.as_str(), s.name.as_str()))
        .collect();
    assert_eq!(
        skipped,
        vec![("pi", "find-skills"), ("pi", "playwright-cli")]
    );
    assert!(report.skipped.iter().all(|s| s.broken));
}
