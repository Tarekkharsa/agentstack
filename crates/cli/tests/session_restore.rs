// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! `session end` must restore the filesystem exactly: it removes the skills
//! dir it emptied only when the session itself created that dir — a dir the
//! user pre-created (even empty) survives. Serialized because these mutate
//! the process-global `HOME`/`AGENTSTACK_HOME`.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use agentstack::scope::Scope;
use agentstack::session;

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// A project with one inline skill wired into `[profiles.p]`.
fn setup_project(tmp: &Path) -> PathBuf {
    let proj = tmp.join("proj");
    fs::create_dir_all(proj.join("skills/local-notes")).unwrap();
    fs::write(proj.join("skills/local-notes/SKILL.md"), "# local\n").unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [skills.local-notes]\npath = \"./skills/local-notes\"\n\
         [profiles.p]\nskills = [\"local-notes\"]\n",
    )
    .unwrap();
    proj
}

fn set_home(home: &Path) {
    fs::create_dir_all(home).unwrap();
    std::env::set_var("HOME", home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));
}

fn unset_home() {
    std::env::remove_var("AGENTSTACK_HOME");
    std::env::remove_var("HOME");
}

#[test]
fn end_keeps_a_preexisting_empty_skills_dir() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    set_home(&tmp.path().join("home"));
    let proj = setup_project(tmp.path());

    // The user created the (empty) skills dir before any session existed.
    let skills_dir = proj.join(".claude/skills");
    fs::create_dir_all(&skills_dir).unwrap();

    session::start(Some(&proj), "p", Scope::Project).unwrap();
    assert!(
        skills_dir.join("local-notes").exists(),
        "session materialized the skill"
    );
    session::end(Some(&proj)).unwrap();

    assert!(
        !skills_dir.join("local-notes").exists(),
        "session skill reverted"
    );
    assert!(
        skills_dir.exists(),
        "a dir the user pre-created must survive session end — exact restore"
    );

    unset_home();
}

#[test]
fn end_removes_the_skills_dir_it_created() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    set_home(&tmp.path().join("home"));
    let proj = setup_project(tmp.path());

    let skills_dir = proj.join(".claude/skills");
    assert!(!skills_dir.exists(), "precondition: no skills dir at rest");

    session::start(Some(&proj), "p", Scope::Project).unwrap();
    assert!(skills_dir.join("local-notes").exists());
    session::end(Some(&proj)).unwrap();

    assert!(
        !skills_dir.exists(),
        "a dir the session created is cleaned up on end"
    );

    unset_home();
}

#[test]
fn old_session_records_default_to_preexisting() {
    // sessions.json written by an older binary has no `dir_preexisted` field —
    // it must load as true so `end` conservatively never removes the dir.
    let sa: session::SkillAdd =
        serde_json::from_str(r#"{ "dir": "/x/.claude/skills", "names": ["a"] }"#).unwrap();
    assert!(sa.dir_preexisted, "missing field defaults to pre-existing");
}
