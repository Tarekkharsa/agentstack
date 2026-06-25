//! Consolidation moves real skill directories — verify it gathers them into the
//! managed home, symlinks the originals back, keeps content reachable, and backs
//! up. This lives in its own test file so the `HOME`/`AGENTSTACK_HOME` overrides
//! run in an isolated process (no env race with other integration tests).

use std::fs;
use std::path::Path;

use agentstack::adapter::Registry;
use agentstack::consolidate::consolidate;

#[test]
fn consolidate_moves_skills_to_home_and_symlinks_back() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    // A real skill dir inside Codex's own skills folder, and a shared skill
    // symlinked into Claude from an external source.
    let codex_skills = home.join(".codex/skills");
    fs::create_dir_all(codex_skills.join("figma")).unwrap();
    fs::write(codex_skills.join("figma/SKILL.md"), "# figma\n").unwrap();
    fs::write(codex_skills.join("figma/helper.py"), "print(1)\n").unwrap();

    let claude_skills = home.join(".claude/skills");
    fs::create_dir_all(&claude_skills).unwrap();
    let external = home.join(".agents/skills/shared");
    fs::create_dir_all(&external).unwrap();
    fs::write(external.join("SKILL.md"), "# shared\n").unwrap();
    std::os::unix::fs::symlink(&external, claude_skills.join("shared")).unwrap();

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    let manifest = proj.join("agentstack.toml");
    fs::write(
        &manifest,
        "version = 1\n[targets]\ndefault = [\"claude-code\", \"codex\"]\n",
    )
    .unwrap();

    let registry = Registry::load().unwrap();
    let report = consolidate(&registry, &manifest, &proj, None).unwrap();
    let names: Vec<&str> = report.iter().map(|c| c.name.as_str()).collect();
    assert!(
        names.contains(&"figma") && names.contains(&"shared"),
        "{names:?}"
    );

    // Files now live in the managed home.
    let skills_home = home.join(".agentstack/skills");
    assert!(skills_home.join("figma/helper.py").is_file());
    assert!(skills_home.join("shared/SKILL.md").is_file());

    // Originals are now symlinks pointing at the managed home.
    let figma_link = codex_skills.join("figma");
    assert!(fs::symlink_metadata(&figma_link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(
        fs::canonicalize(&figma_link).unwrap(),
        fs::canonicalize(skills_home.join("figma")).unwrap()
    );

    // Content is still reachable through the link (the agent keeps working).
    assert_eq!(
        fs::read_to_string(figma_link.join("helper.py")).unwrap(),
        "print(1)\n"
    );

    // A backup of the moved real dir was kept.
    assert!(home
        .join(".agentstack/backups/skills/figma/SKILL.md")
        .is_file());

    // Manifest now declares them as path skills pointing at the home.
    let m = fs::read_to_string(&manifest).unwrap();
    assert!(m.contains("[skills.figma]"));
    assert!(m.contains(&format!("{}", skills_home.join("figma").display())));

    // Re-running is safe (idempotent): already-home skills don't error.
    let again = consolidate(&registry, &manifest, &proj, None).unwrap();
    assert!(again
        .iter()
        .all(|c| c.already_home || !c.linked_into.is_empty()));
    assert!(Path::new(&skills_home.join("figma/helper.py")).is_file());
}
