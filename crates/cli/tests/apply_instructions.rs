// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

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
        prune_foreign: false,
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
            deep: false,
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
fn apply_write_blocks_on_a_missing_fragment_source() {
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
    apply::run(&args(true), Some(&proj)).unwrap();
    let before = fs::read_to_string(home.join(".claude/CLAUDE.md")).unwrap();
    assert!(before.contains("House rule one."));

    // The fragment source disappears (deleted, bad checkout, typoed path).
    fs::remove_file(proj.join("instructions/house.md")).unwrap();

    // A missing source must BLOCK the write — a compile that silently dropped
    // the fragment would delete the previously compiled region.
    apply::run(&args(true), Some(&proj)).unwrap();
    let after = fs::read_to_string(home.join(".claude/CLAUDE.md")).unwrap();
    assert_eq!(
        after, before,
        "missing fragment source must not clobber the compiled region"
    );

    std::env::remove_var("AGENTSTACK_HOME");
    std::env::remove_var("HOME");
}

#[test]
fn doctor_warns_when_a_fragment_targets_a_cli_without_instructions() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    set_home(&home);

    // A fragment explicitly targets Cursor, which has no instruction file
    // agentstack manages — so it reaches Cursor nowhere.
    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("house.md"), "House rule.\n").unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [instructions.house]\npath = \"./house.md\"\ntargets = [\"cursor\"]\n",
    )
    .unwrap();

    let report = doctor::collect(Some(&proj)).unwrap();
    let instr = report["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["title"] == "Instructions")
        .expect("Instructions section");
    let warned = instr["lines"].as_array().unwrap().iter().any(|l| {
        l["level"] == "warn"
            && l["msg"].as_str().unwrap().contains("house")
            && l["msg"].as_str().unwrap().contains("no instructions file")
    });
    assert!(
        warned,
        "a fragment targeting an instructions-less CLI should warn: {report}"
    );

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
fn project_scope_apply_never_empties_a_region_over_inherited_fragments() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    set_home(&home);

    // The machine layer declares a fragment → merge_user_layer makes every
    // project load's `instructions` non-empty.
    let as_home = home.join(".agentstack");
    fs::create_dir_all(as_home.join("instructions")).unwrap();
    fs::write(
        as_home.join("agentstack.toml"),
        "version = 1\n[instructions.style]\npath = \"./instructions/style.md\"\n",
    )
    .unwrap();
    fs::write(as_home.join("instructions/style.md"), "Machine style.\n").unwrap();

    // A project with NO instructions of its own, whose committed CLAUDE.md
    // already carries a managed region (e.g. compiled and checked in).
    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [servers.demo]\ntype = \"http\"\nurl = \"https://demo/mcp\"\n",
    )
    .unwrap();
    let existing =
        "# Repo\n\n<!-- agentstack:start -->\nCommitted rules.\n<!-- agentstack:end -->\n";
    fs::write(proj.join("CLAUDE.md"), existing).unwrap();

    // Project scope filters out every inherited fragment — the compile is
    // empty, and an empty compile must never remove the committed region.
    let mut a = args(true);
    a.scope = Some(Scope::Project);
    apply::run(&a, Some(&proj)).unwrap();
    assert_eq!(
        fs::read_to_string(proj.join("CLAUDE.md")).unwrap(),
        existing,
        "an all-filtered (empty) compile must not touch the repo's managed region"
    );

    std::env::remove_var("AGENTSTACK_HOME");
    std::env::remove_var("HOME");
}

#[test]
fn doctor_accepts_a_project_compiled_at_project_scope() {
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

    // The project compiles at PROJECT scope — it never writes the global file.
    let mut a = args(true);
    a.scope = Some(Scope::Project);
    apply::run(&a, Some(&proj)).unwrap();
    assert!(fs::read_to_string(proj.join("CLAUDE.md"))
        .unwrap()
        .contains("House rule one."));
    assert!(!home.join(".claude/CLAUDE.md").exists());

    // Doctor must not warn forever against the global file it never writes.
    let report = doctor::collect(Some(&proj)).unwrap();
    let instr = report["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["title"] == "Instructions")
        .expect("Instructions section");
    let stale = instr["lines"]
        .as_array()
        .unwrap()
        .iter()
        .any(|l| l["level"] == "warn" && l["msg"].as_str().unwrap().contains("stale"));
    assert!(
        !stale,
        "in-sync project-scope compile must not warn stale: {report}"
    );

    // Editing the fragment makes the project-scope region genuinely stale.
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
    assert!(
        stale,
        "a genuinely stale region should still warn: {report}"
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
            deep: false,
        },
        Some(&proj),
    )
    .unwrap_err();
    assert!(err.to_string().contains("error"), "got: {err:#}");

    std::env::remove_var("AGENTSTACK_HOME");
    std::env::remove_var("HOME");
}

#[test]
fn standalone_instructions_write_blocks_on_missing_fragment_sources() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    set_home(&home);

    let proj = tmp.path().join("proj");
    fs::create_dir_all(proj.join("instructions")).unwrap();
    fs::write(proj.join("instructions/house.md"), "House rule one.\n").unwrap();
    fs::write(proj.join("instructions/extra.md"), "Extra rule two.\n").unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [instructions.house]\npath = \"./instructions/house.md\"\n\
         [instructions.extra]\npath = \"./instructions/extra.md\"\n",
    )
    .unwrap();
    let iargs = agentstack::cli::InstructionsArgs {
        targets: vec!["claude-code".into()],
        scope: Some(Scope::Global),
        write: true,
    };

    // First compile lands both fragments.
    agentstack::commands::instructions::run(&iargs, Some(&proj)).unwrap();
    let file = home.join(".claude/CLAUDE.md");
    let before = fs::read_to_string(&file).unwrap();
    assert!(before.contains("House rule one."));
    assert!(before.contains("Extra rule two."));

    // A partially missing source must BLOCK the write — compiling without it
    // would silently drop its content from the managed region.
    fs::remove_file(proj.join("instructions/extra.md")).unwrap();
    let err = agentstack::commands::instructions::run(&iargs, Some(&proj)).unwrap_err();
    assert!(
        err.to_string().contains("missing fragment source"),
        "got: {err:#}"
    );
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        before,
        "managed region untouched when a source is missing"
    );

    std::env::remove_var("AGENTSTACK_HOME");
    std::env::remove_var("HOME");
}

#[test]
fn machine_layer_instructions_do_not_gitignore_a_project_instruction_file() {
    // A project that ran `init --global` inherits machine-level [instructions.*]
    // (compiled at GLOBAL scope only). At project scope agentstack writes no
    // CLAUDE.md there, so it must not gitignore one the user hand-wrote.
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    set_home(&home);
    let as_home = home.join(".agentstack");
    fs::create_dir_all(as_home.join("instructions")).unwrap();
    fs::write(
        as_home.join("agentstack.toml"),
        "version = 1\n[instructions.style]\npath = \"./instructions/style.md\"\n",
    )
    .unwrap();
    fs::write(as_home.join("instructions/style.md"), "Machine style.\n").unwrap();

    // A project with servers (so the block is written) but NO instructions.
    let proj = tmp.path().join("proj");
    fs::create_dir_all(proj.join(".git")).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [servers.demo]\ntype = \"http\"\nurl = \"https://x/mcp\"\n",
    )
    .unwrap();

    let mut a = args(true);
    a.scope = Some(Scope::Project);
    a.no_gitignore = false;
    apply::run(&a, Some(&proj)).unwrap();

    let ignore = fs::read_to_string(proj.join(".gitignore")).unwrap_or_default();
    assert!(
        ignore.contains("/.mcp.json"),
        "config still ignored: {ignore}"
    );
    assert!(
        !ignore.contains("/CLAUDE.md"),
        "must NOT gitignore a CLAUDE.md the tool never generates here: {ignore}"
    );

    std::env::remove_var("AGENTSTACK_HOME");
    std::env::remove_var("HOME");
}

#[test]
fn project_apply_gitignores_the_compiled_instruction_file() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    set_home(&home);

    let proj = tmp.path().join("proj");
    fs::create_dir_all(proj.join("instructions")).unwrap();
    // The managed .gitignore block is only written inside a git repo.
    fs::create_dir_all(proj.join(".git")).unwrap();
    fs::write(proj.join("instructions/house.md"), "House rule one.\n").unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [instructions.house]\npath = \"./instructions/house.md\"\n",
    )
    .unwrap();

    // Apply at project scope with gitignore management ON (the default).
    let mut a = args(true);
    a.scope = Some(Scope::Project);
    a.no_gitignore = false;
    apply::run(&a, Some(&proj)).unwrap();

    // The compiled instruction file lands in the repo...
    assert!(fs::read_to_string(proj.join("CLAUDE.md"))
        .unwrap()
        .contains("House rule one."));
    // ...and the managed block keeps it out of git — the repo tracks the
    // .agentstack source (instructions/house.md), not the generated output.
    let ignore = fs::read_to_string(proj.join(".gitignore")).unwrap();
    assert!(
        ignore.contains("/CLAUDE.md"),
        "compiled instruction file should be gitignored: {ignore}"
    );

    std::env::remove_var("AGENTSTACK_HOME");
    std::env::remove_var("HOME");
}
