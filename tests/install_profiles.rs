//! `install` in profile-driven projects: the lockfile reconcile pass must not
//! prune library-backed profile skills. Clean-at-rest repos reference library
//! skills by name (no inline `[skills.*]`) and pin them via `use --write` /
//! `lock` — running `install` afterwards must leave those pins alone.
//! Serialized because these mutate the process-global `HOME`/`AGENTSTACK_HOME`.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use agentstack::cli::InstallArgs;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn install(proj: &Path) {
    agentstack::commands::install::run(
        &InstallArgs {
            locked: false,
            allow_flagged: false,
        },
        Some(proj),
    )
    .unwrap();
}

/// A project dir with the given manifest and a pre-seeded lock entry for the
/// library-backed profile skill `sql-review` (as `use --write`/`lock` records).
fn setup_project(tmp: &Path, manifest: &str) -> PathBuf {
    let proj = tmp.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join("agentstack.toml"), manifest).unwrap();
    std::fs::write(
        proj.join("agentstack.lock"),
        "version = 1\n\n[[skill]]\nname = \"sql-review\"\nsource = \"path\"\n\
         path = \"sql-review\"\nchecksum = \"cafe\"\n",
    )
    .unwrap();
    proj
}

#[test]
fn install_keeps_lock_entries_for_profile_referenced_library_skills() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    // One inline skill (so the reconcile pass runs) + one library-backed
    // profile ref with no inline entry.
    let proj = setup_project(
        tmp.path(),
        "version = 1\n[skills.local-notes]\npath = \"./skills/local-notes\"\n\
         [profiles.default]\nskills = [\"local-notes\", \"sql-review\"]\n",
    );
    std::fs::create_dir_all(proj.join("skills/local-notes")).unwrap();
    std::fs::write(proj.join("skills/local-notes/SKILL.md"), "# local\n").unwrap();

    install(&proj);

    let lock = std::fs::read_to_string(proj.join("agentstack.lock")).unwrap();
    assert!(lock.contains("local-notes"), "inline skill locked: {lock}");
    assert!(
        lock.contains("sql-review"),
        "library-profile pin survives the reconcile pass: {lock}"
    );

    std::env::remove_var("AGENTSTACK_HOME");
    std::env::remove_var("HOME");
}

#[test]
fn clean_at_rest_install_preserves_profile_pins() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    // No inline [skills.*] at all — the clean-at-rest shape.
    let proj = setup_project(
        tmp.path(),
        "version = 1\n[profiles.default]\nskills = [\"sql-review\"]\n",
    );

    install(&proj);

    let lock = std::fs::read_to_string(proj.join("agentstack.lock")).unwrap();
    assert!(
        lock.contains("sql-review"),
        "profile pin untouched by a no-inline-skills install: {lock}"
    );

    std::env::remove_var("AGENTSTACK_HOME");
    std::env::remove_var("HOME");
}
