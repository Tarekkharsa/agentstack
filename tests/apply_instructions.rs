//! Instructions in the mainstream lifecycle: `apply --write` compiles
//! `[instructions.*]` into each CLI's instruction file, a manifest WITHOUT
//! instructions never touches a region another layer owns, and `doctor`
//! reports stale regions / missing fragment sources.

use std::fs;
use std::sync::Mutex;

use agentstack::cli::{ApplyArgs, DoctorArgs};
use agentstack::commands::{apply, doctor};
use agentstack::scope::Scope;

// All tests mutate the process-global HOME; serialize them.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn set_home(home: &std::path::Path) {
    fs::create_dir_all(home).unwrap();
    std::env::set_var("HOME", home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));
}

fn args(write: bool) -> ApplyArgs {
    ApplyArgs {
        targets: vec!["claude-code".into()],
        profile: None,
        dry_run: !write,
        write,
        scope: Some(Scope::Global),
        allow_unresolved: false,
        no_gitignore: true,
    }
}

#[test]
fn apply_write_compiles_instructions_into_the_global_file() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    set_home(&home);

    let proj = tmp.path().join("proj");
    fs::create_dir_all(proj.join("instructions")).unwrap();
    fs::write(proj.join("instructions/house.md"), "House rule one.\n").unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [instructions.house]\npath = \"./instructions/house.md\"\n",
    )
    .unwrap();

    // Dry-run: nothing written.
    apply::run(&args(false), Some(&proj)).unwrap();
    assert!(!home.join(".claude/CLAUDE.md").exists());

    // --write: the fragment lands in the managed region.
    apply::run(&args(true), Some(&proj)).unwrap();
    let compiled = fs::read_to_string(home.join(".claude/CLAUDE.md")).unwrap();
    assert!(compiled.contains("<!-- agentstack:start -->"));
    assert!(compiled.contains("House rule one."));

    // In sync now — doctor --ci passes.
    doctor::run(
        &DoctorArgs {
            ci: true,
            live: false,
            fix: false,
        },
        Some(&proj),
    )
    .unwrap();

    // Editing the fragment makes the region stale → doctor warns (not an
    // error: drift-class), pointing at `instructions --write`.
    fs::write(proj.join("instructions/house.md"), "House rule two.\n").unwrap();
    let report = doctor::collect(Some(&proj)).unwrap();
    let instr = report["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["title"] == "Instructions")
        .expect("Instructions section");
    let stale = instr["lines"].as_array().unwrap().iter().any(|l| {
        l["level"] == "warn"
            && l["msg"]
                .as_str()
                .unwrap()
                .contains("agentstack instructions --write")
    });
    assert!(stale, "stale managed region should warn: {report}");

    std::env::remove_var("AGENTSTACK_HOME");
    std::env::remove_var("HOME");
}

#[test]
fn apply_without_instructions_leaves_a_foreign_region_alone() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    set_home(&home);

    // Another layer (e.g. the machine manifest) owns a region already.
    fs::create_dir_all(home.join(".claude")).unwrap();
    let existing = "# Mine\n\n<!-- agentstack:start -->\nMachine rules.\n<!-- agentstack:end -->\n";
    fs::write(home.join(".claude/CLAUDE.md"), existing).unwrap();

    // A project manifest with servers but NO [instructions.*].
    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [servers.demo]\ntype = \"http\"\nurl = \"https://demo/mcp\"\n",
    )
    .unwrap();

    apply::run(&args(true), Some(&proj)).unwrap();
    let after = fs::read_to_string(home.join(".claude/CLAUDE.md")).unwrap();
    assert_eq!(
        after, existing,
        "region owned by another layer must survive"
    );

    std::env::remove_var("AGENTSTACK_HOME");
    std::env::remove_var("HOME");
}

#[test]
fn doctor_ci_fails_on_a_missing_fragment_source() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    set_home(&tmp.path().join("home"));

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [instructions.ghost]\npath = \"./instructions/ghost.md\"\n",
    )
    .unwrap();

    let err = doctor::run(
        &DoctorArgs {
            ci: true,
            live: false,
            fix: false,
        },
        Some(&proj),
    )
    .unwrap_err();
    assert!(err.to_string().contains("error"), "got: {err:#}");

    std::env::remove_var("AGENTSTACK_HOME");
    std::env::remove_var("HOME");
}
