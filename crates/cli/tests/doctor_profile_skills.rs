//! `doctor`'s Skills section must consider the same name set a trust review
//! covers — inline `[skills.*]` PLUS profile-referenced (library) names — not
//! just inline entries. Regression for the "no skills defined" contradiction:
//! the Reproducibility section listed a pinned library skill the Skills section
//! claimed didn't exist.

use std::fs;

use agentstack::commands::doctor;
use agentstack::commands::lib::{add_skill, LibSource};

static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Pull the flattened `msg` strings of the doctor JSON section titled `title`.
fn section_lines(report: &serde_json::Value, title: &str) -> Vec<String> {
    report["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["title"] == title)
        .map(|s| {
            s["lines"]
                .as_array()
                .unwrap()
                .iter()
                .map(|l| l["msg"].as_str().unwrap().to_string())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn skills_section_counts_profile_referenced_library_skills() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    // Seed a skill into the central library only.
    let src = tmp.path().join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(
        src.join("SKILL.md"),
        "---\ndescription: SQL review\n---\n# body\n",
    )
    .unwrap();
    let lib_home = home.join(".agentstack/lib");
    add_skill(
        &lib_home,
        "sql-review",
        LibSource::Path(&src),
        false,
        true,
        false,
    )
    .unwrap();

    // A project that references the library skill through a profile — NO inline
    // `[skills.*]` entry, which is exactly the case that used to read as empty.
    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[profiles.dev]\nskills = [\"sql-review\"]\n",
    )
    .unwrap();

    let report = doctor::collect(Some(&proj)).unwrap();
    let skills = section_lines(&report, "Skills");
    // The profile-referenced library skill is now checked and present…
    assert!(
        skills.iter().any(|l| l.contains("sql-review")),
        "Skills section must list the library skill: {skills:?}"
    );
    // …instead of claiming there are no skills at all.
    assert!(
        !skills.iter().any(|l| l == "no skills defined"),
        "Skills section must not report empty: {skills:?}"
    );

    std::env::remove_var("AGENTSTACK_HOME");
    std::env::remove_var("HOME");
}
