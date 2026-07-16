// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Doctor must surface dead skill symlinks. A broken link in a CLI's skills
//! dir loads nothing and `consolidate` skips it, so without this check the
//! skill just silently stops existing (the real-world case: two pi skills
//! linked into an empty ~/.agents/skills/). Lives in its own file so the
//! `HOME`/`AGENTSTACK_HOME` overrides run serialized.

use std::fs;
use std::sync::Mutex;

use agentstack::commands::doctor;

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// The (level, msg) lines of one titled section from `doctor::collect`.
fn section_lines(report: &serde_json::Value, title: &str) -> Vec<(String, String)> {
    report["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["title"] == title)
        .unwrap_or_else(|| panic!("no '{title}' section in {report}"))["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            (
                l["level"].as_str().unwrap().to_string(),
                l["msg"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

/// A dead symlink in a detected CLI's skills dir warns under "Skills", naming
/// the missing target and the exact fix (remove the link / reinstall).
#[test]
fn broken_skill_symlink_warns_with_fix() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    // Claude Code counts as detected via its config file.
    fs::write(home.join(".claude.json"), "{}\n").unwrap();
    // Its global skills dir holds one healthy skill and one dead link.
    let skills = home.join(".claude/skills");
    fs::create_dir_all(skills.join("good")).unwrap();
    fs::write(skills.join("good/SKILL.md"), "# good\n").unwrap();
    let gone = home.join(".agents/skills/find-skills");
    std::os::unix::fs::symlink(&gone, skills.join("find-skills")).unwrap();

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n",
    )
    .unwrap();

    let report = doctor::collect(Some(&proj)).unwrap();
    let lines = section_lines(&report, "Skills");
    let broken: Vec<_> = lines
        .iter()
        .filter(|(_, msg)| msg.contains("broken skill link"))
        .collect();
    assert_eq!(broken.len(), 1, "one dead link, got: {lines:?}");
    let (level, msg) = broken[0];
    assert_eq!(level, "warn");
    assert!(msg.contains("'find-skills'"), "names the link: {msg}");
    assert!(
        msg.contains(&gone.display().to_string()) && msg.contains("target missing"),
        "names the missing target: {msg}"
    );
    assert!(
        msg.contains(&format!("rm {}", skills.join("find-skills").display()))
            && msg.contains("reinstall"),
        "carries the fix: {msg}"
    );
    // The healthy skill must not be flagged.
    assert!(
        !lines.iter().any(|(_, m)| m.contains("'good'")),
        "{lines:?}"
    );

    std::env::remove_var("AGENTSTACK_HOME");
    std::env::remove_var("HOME");
}
