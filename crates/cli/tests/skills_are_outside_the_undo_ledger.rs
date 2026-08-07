// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! **G31 — the undo ledger does not cover skill materialization, and now says so.**
//!
//! `use --write` reports "wrote skills to 3 locations"; `undo` then reported
//! "nothing recorded to undo for this project" and `x restore --list` reported
//! "No recorded changes yet", with nothing joining the two. Two honest repairs
//! existed: record materialized skills, or narrow the promise. This file is the
//! evidence for choosing the second, and the witness that the promise is now
//! narrow in the places a user meets it.
//!
//! The deciding question was whether an undo of a materialized skill can tell a
//! file WE wrote from a file the user edited afterwards. Through this ledger it
//! cannot, and the three mechanism tests below prove why — each paired with a
//! negative control, so none of them can pass for an accidental reason:
//!
//! 1. [`the_ledger_has_no_bytes_for_a_delivered_skill`] — a delivered skill is
//!    a directory, so `capture` records `before: None`. Control: an ordinary
//!    config FILE captures its bytes.
//! 2. [`rollback_cannot_take_back_a_delivered_skill_directory`] — `rollback` on
//!    that capture FAILS. Control: the same rollback restores a file perfectly.
//! 3. [`rollback_deletes_by_path_with_no_ownership_test`] — the reason a wider
//!    ledger would be unsafe. Control: `render::skills`, which DOES carry an
//!    ownership test, leaves the identical user directory alone.
//!
//! Then [`the_undo_surfaces_name_the_skills_they_cannot_reach`] and its control
//! pin the narrowed promise itself.

use std::fs;
use std::path::Path;
use std::sync::Mutex;

use agentstack::adapter::descriptor::SkillStrategy;
use agentstack::commands::restore;
use agentstack::history;
use agentstack::render::{skills, PriorTrust};
use agentstack::state::{State, TargetState};

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn setup_home(home: &Path) {
    fs::create_dir_all(home).unwrap();
    std::env::set_var("HOME", home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));
}

/// A project whose skills are trusted, so materialization exercises MECHANICS
/// rather than failing at the trust gate (which has its own witnesses in
/// `red_team_skills_trust_gate.rs`).
fn trusted_project(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    fs::write(dir.join("agentstack.toml"), "version = 1\n").unwrap();
    agentstack::trust::trust_unreviewed(dir).unwrap();
}

/// A skill source on disk: what `use --write` links to or copies from.
fn skill_source(root: &Path, name: &str) -> std::path::PathBuf {
    let dir = root.join("lib").join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("SKILL.md"), "# skill\n").unwrap();
    dir
}

/// Materialize `name` into `skills_dir` exactly as `use --write` does.
fn materialize(skills_dir: &Path, name: &str, source: &Path, strategy: SkillStrategy, proj: &Path) {
    let plan = skills::plan(
        skills_dir.to_path_buf(),
        strategy,
        vec![(name.to_string(), source.to_path_buf())],
        &[],
        proj,
        PriorTrust::STRICT,
    )
    .unwrap();
    skills::materialize(&plan).unwrap();
}

// ---------------------------------------------------------------------------
// 1. The ledger stores bytes. A delivered skill has none to store.
// ---------------------------------------------------------------------------

/// `history::capture` is `fs::read_to_string`, so it can only describe a FILE.
/// A delivered skill is a symlink to a directory (the default strategy) or a
/// copied directory tree; either way `before` comes out `None` — the ledger
/// holds no record of what was there, which is the whole substance of an undo.
#[test]
fn the_ledger_has_no_bytes_for_a_delivered_skill() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    setup_home(&tmp.path().join("home"));
    let proj = tmp.path().join("proj");
    trusted_project(&proj);
    let source = skill_source(tmp.path(), "greet");

    for (strategy, dirname) in [
        (SkillStrategy::Symlink, "linked"),
        (SkillStrategy::Copy, "copied"),
    ] {
        let skills_dir = proj.join(dirname);
        materialize(&skills_dir, "greet", &source, strategy, &proj);
        let delivered = skills_dir.join("greet");
        assert!(
            delivered.symlink_metadata().is_ok(),
            "{dirname}: the skill was delivered"
        );

        let captured = history::capture(&delivered, "Claude Code · skills");
        assert!(
            captured.before.is_none(),
            "{dirname}: the ledger claimed to hold bytes for a delivered skill — it cannot, \
             and an undo built on that claim would be reverting to a state it never saw"
        );
    }

    // NEGATIVE CONTROL. The same capture on the artifact the ledger is FOR — an
    // ordinary config file — does hold its bytes. So the assertions above fail
    // for the shape of a skill, not for a broken capture.
    let config = proj.join(".mcp.json");
    fs::write(&config, "{\"mcpServers\":{}}").unwrap();
    let captured = history::capture(&config, "Claude Code · servers");
    assert_eq!(
        captured.before.as_deref(),
        Some("{\"mcpServers\":{}}"),
        "control: a config file's pre-write bytes ARE captured"
    );

    std::env::remove_var("AGENTSTACK_HOME");
}

// ---------------------------------------------------------------------------
// 2. And the undo half cannot act on one either.
// ---------------------------------------------------------------------------

/// With `before: None`, `history::rollback` deletes — via `fs::remove_file`,
/// which cannot remove a directory. So a copied skill recorded in the ledger
/// would produce an undo that ERRORS, leaving the user mid-revert. This is the
/// conclusion `x unrender` reached first: its skills leg is `capture: false`.
#[test]
fn rollback_cannot_take_back_a_delivered_skill_directory() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    setup_home(&tmp.path().join("home"));
    let proj = tmp.path().join("proj");
    trusted_project(&proj);
    let source = skill_source(tmp.path(), "greet");

    let skills_dir = proj.join("copied");
    materialize(&skills_dir, "greet", &source, SkillStrategy::Copy, &proj);
    let delivered = skills_dir.join("greet");

    let captured = history::capture(&delivered, "Claude Code · skills");
    assert!(
        history::rollback(std::slice::from_ref(&captured)).is_err(),
        "rollback appeared to take back a copied skill directory — if this ever passes, \
         re-open G31: the seam may have grown a way to carry skills"
    );
    assert!(
        delivered.join("SKILL.md").exists(),
        "the failed rollback left the delivery in place"
    );

    // NEGATIVE CONTROL. The identical call on a file the ledger DOES cover
    // restores it byte for byte — the seam works, skills just do not fit it.
    let config = proj.join(".mcp.json");
    fs::write(&config, "original").unwrap();
    let captured = history::capture(&config, "Claude Code · servers");
    fs::write(&config, "overwritten").unwrap();
    history::rollback(std::slice::from_ref(&captured)).unwrap();
    assert_eq!(
        fs::read_to_string(&config).unwrap(),
        "original",
        "control: rollback restores a captured file exactly"
    );

    std::env::remove_var("AGENTSTACK_HOME");
}

// ---------------------------------------------------------------------------
// 3. The deciding question, answered: no.
// ---------------------------------------------------------------------------

/// **Why widening the ledger would be unsafe, not merely awkward.**
///
/// `history::rollback` acts on a path and carries no ownership test at all: a
/// `before: None` entry deletes whatever is at that path NOW. Every capture of
/// a delivered skill is a `before: None` entry (test 1), so a skills-aware
/// ledger would delete a hand-made skill directory the user put there after the
/// activation — bytes we did not write and cannot prove we wrote.
///
/// This is trust-adjacent code, so that alone settles it: narrow the promise.
#[test]
fn rollback_deletes_by_path_with_no_ownership_test() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    setup_home(&tmp.path().join("home"));
    let proj = tmp.path().join("proj");
    trusted_project(&proj);

    // The path a skill would have been recorded at, captured while absent —
    // exactly the `before: None` every skill capture produces.
    let path = proj.join("skills-note.md");
    let captured = history::capture(&path, "Claude Code · skills");
    assert!(captured.before.is_none());

    // The user then writes their OWN file at that path.
    fs::write(&path, "mine, written by hand\n").unwrap();
    history::rollback(std::slice::from_ref(&captured)).unwrap();
    assert!(
        !path.exists(),
        "the ledger is expected to delete by path; if it ever learns ownership, G31 can be \
         revisited"
    );

    // NEGATIVE CONTROL. `render::skills` — the module that DOES know what we
    // own — leaves the user's identical directory alone under the same prune it
    // uses for our own deliveries. The ownership test exists; it just lives
    // where the ledger cannot reach it without new recording machinery.
    let skills_dir = proj.join("claude-skills");
    fs::create_dir_all(skills_dir.join("greet")).unwrap();
    fs::write(skills_dir.join("greet/SKILL.md"), "user's own\n").unwrap();
    let plan = skills::plan(
        skills_dir.clone(),
        SkillStrategy::Symlink,
        Vec::new(),
        &["greet".to_string()],
        &proj,
        PriorTrust::STRICT,
    )
    .unwrap();
    skills::materialize(&plan).unwrap();
    assert_eq!(
        fs::read_to_string(skills_dir.join("greet/SKILL.md")).unwrap(),
        "user's own\n",
        "control: the skills prune refuses to remove a directory it cannot prove is ours"
    );

    std::env::remove_var("AGENTSTACK_HOME");
}

// ---------------------------------------------------------------------------
// 4. The narrowed promise, where the user meets it.
// ---------------------------------------------------------------------------

/// The undo inventory names the materialized skills it cannot reach, so a
/// person — and a panel drawing an Undo affordance from this exact value —
/// learns the boundary from the surface that has it, not by discovering it
/// afterwards while recovering.
#[test]
fn the_undo_surfaces_name_the_skills_they_cannot_reach() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    setup_home(&tmp.path().join("home"));
    let proj = tmp.path().join("proj");
    trusted_project(&proj);
    let other = tmp.path().join("other");
    trusted_project(&other);

    // What `use --write` leaves behind: skill ownership recorded per target in
    // the state ledger, and NOTHING in the history ledger.
    let mut state = State::load().unwrap();
    for id in ["claude-code", "codex"] {
        state.targets.insert(
            agentstack::state::target_key(id, agentstack::scope::Scope::Project, &proj),
            TargetState {
                managed_skills: vec!["greet".to_string()],
                ..Default::default()
            },
        );
    }
    // A DIFFERENT project's delivery, to prove the note is project-scoped: the
    // state ledger is machine-global, and naming another project's skills here
    // would be a new lie in place of the old one.
    state.targets.insert(
        agentstack::state::target_key("claude-code", agentstack::scope::Scope::Project, &other),
        TargetState {
            managed_skills: vec!["not-ours".to_string()],
            ..Default::default()
        },
    );
    state.save().unwrap();
    assert!(
        history::list().is_empty(),
        "the premise: materializing skills records nothing in the history ledger"
    );

    let registry = agentstack::adapter::Registry::load().unwrap();
    let inventory = restore::list_json_value(&registry, &proj);
    let named: Vec<&str> = inventory["skills_not_recorded"]
        .as_array()
        .expect("the inventory declares its own boundary")
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(
        named,
        vec!["greet"],
        "the inventory names this project's unreachable skill, once, and not another \
         project's"
    );

    // NEGATIVE CONTROL. A project that materialized no skills gets no note —
    // the boundary is a fact about the project, not a permanent disclaimer
    // that would train users to ignore it.
    let clean = tmp.path().join("clean");
    trusted_project(&clean);
    let inventory = restore::list_json_value(&registry, &clean);
    assert!(
        inventory["skills_not_recorded"]
            .as_array()
            .unwrap()
            .is_empty(),
        "control: no materialized skills, no note"
    );

    std::env::remove_var("AGENTSTACK_HOME");
}
