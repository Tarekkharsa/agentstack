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

    let codex_skills = home.join(".codex/skills");
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
    let external = home.join(".agents/skills/shared");
    fs::create_dir_all(&external).unwrap();
    fs::write(external.join("SKILL.md"), "# shared\n").unwrap();
    std::os::unix::fs::symlink(&external, claude_skills.join("shared")).unwrap();

    let registry = Registry::load().unwrap();
    let report = consolidate(&registry, &manifest, &proj, None, false, true).unwrap();
    let names: Vec<&str> = report.iter().map(|c| c.name.as_str()).collect();
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
    assert!(figma.checksum.as_deref().unwrap().len() == 64);
    assert!(library.get("shared").is_some());

    // Originals are symlinks resolving to the library copy (old behavior works).
    let figma_link = home.join(".codex/skills/figma");
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
    assert!(again.iter().any(|c| c.name == "figma" && c.already_home));
    assert!(lib_skills.join("figma/helper.py").is_file());
}

#[test]
fn consolidate_dry_run_writes_nothing() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let (home, proj, manifest) = setup(tmp.path());

    let registry = Registry::load().unwrap();
    let report = consolidate(&registry, &manifest, &proj, None, false, false).unwrap();
    assert!(report.iter().any(|c| c.name == "figma"));

    // No files, no library index, no symlink change.
    assert!(!home.join(".agentstack/lib/skills/figma").exists());
    assert!(Library::load(&home.join(".agentstack/lib"))
        .unwrap()
        .skills
        .is_empty());
    let figma = home.join(".codex/skills/figma");
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
    add_skill(&lib_home, "figma", LibSource::Path(&other), false, true).unwrap();

    let registry = Registry::load().unwrap();
    // Discovered figma differs from the library's → hard error without --replace.
    let err = consolidate(&registry, &manifest, &proj, None, false, true).unwrap_err();
    assert!(err.to_string().contains("--replace"), "{err}");

    // With --replace it overwrites the library copy.
    consolidate(&registry, &manifest, &proj, None, true, true).unwrap();
    let body = fs::read_to_string(lib_home.join("skills/figma/SKILL.md")).unwrap();
    assert_eq!(body, "# figma\n");
}
