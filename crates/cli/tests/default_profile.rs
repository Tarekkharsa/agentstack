// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The implicit default profile: a manifest with no `[profiles.*]` is fully
//! usable — `use --write` activates every inline skill and server, and `lock`
//! pins the same set. Profiles are opt-in selectivity, not a prerequisite.

use std::fs;
use std::sync::Mutex;

use agentstack::cli::{LockArgs, UseArgs};
use agentstack::commands::{lock as lock_cmd, use_profile};
use agentstack::scope::Scope;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn use_args(profile: Option<&str>) -> UseArgs {
    UseArgs {
        profile: profile.map(str::to_string),
        targets: vec!["claude-code".into()],
        scope: Some(Scope::Global),
        write: true,
        allow_unresolved: false,
        prune_foreign: false,
        no_gitignore: true,
    }
}

fn set_home(home: &std::path::Path) {
    fs::create_dir_all(home).unwrap();
    std::env::set_var("HOME", home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));
}

#[test]
fn profile_less_manifest_activates_and_locks_the_full_inline_set() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    set_home(&home);

    let proj = tmp.path().join("proj");
    fs::create_dir_all(proj.join("skills/helper")).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [skills.helper]\npath = \"./skills/helper\"\n",
    )
    .unwrap();
    fs::write(proj.join("skills/helper/SKILL.md"), "# helper\n").unwrap();

    // No profiles declared: `use --write` with no name activates everything.
    use_profile::run(&use_args(None), Some(&proj)).unwrap();
    assert!(
        home.join(".claude/skills/helper").exists(),
        "inline skill must materialize without any profile declared"
    );
    let lock = fs::read_to_string(proj.join("agentstack.lock")).unwrap();
    assert!(lock.contains("helper"), "activation pinned the skill");

    // `lock` pins the same implicit set (idempotent here).
    lock_cmd::run(&LockArgs::default(), Some(&proj)).unwrap();
    let lock2 = fs::read_to_string(proj.join("agentstack.lock")).unwrap();
    assert!(lock2.contains("helper"));
}

#[test]
fn several_profiles_without_a_name_is_an_error_naming_them() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    set_home(&tmp.path().join("home"));

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [profiles.a]\nskills = []\n[profiles.b]\nskills = []\n",
    )
    .unwrap();

    let err = use_profile::run(&use_args(None), Some(&proj))
        .unwrap_err()
        .to_string();
    assert!(err.contains("several profiles"), "{err}");
    assert!(
        err.contains('a') && err.contains('b'),
        "names listed: {err}"
    );
}

#[test]
fn a_single_declared_profile_is_selected_automatically() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    set_home(&home);

    let proj = tmp.path().join("proj");
    fs::create_dir_all(proj.join("skills/solo")).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [skills.solo]\npath = \"./skills/solo\"\n\
         [profiles.only]\nskills = [\"solo\"]\n",
    )
    .unwrap();
    fs::write(proj.join("skills/solo/SKILL.md"), "# solo\n").unwrap();

    use_profile::run(&use_args(None), Some(&proj)).unwrap();
    assert!(
        home.join(".claude/skills/solo").exists(),
        "the only declared profile is unambiguous — no name needed"
    );
}
