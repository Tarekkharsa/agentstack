//! `agentstack init --global` seeds the machine-level manifest — the personal
//! instructions layer at `~/.agentstack/` (honoring `AGENTSTACK_HOME`).

use std::fs;
use std::sync::Mutex;

use agentstack::cli::InitArgs;
use agentstack::commands::init;

// init --global reads the process-global AGENTSTACK_HOME; serialize the tests
// in this binary so they don't race on it.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn args(force: bool) -> InitArgs {
    InitArgs {
        global: true,
        force,
        dry_run: false,
        no_keychain: false,
    }
}

#[test]
fn init_global_seeds_home_manifest_and_instructions_dir() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join(".agentstack");
    std::env::set_var("AGENTSTACK_HOME", &home);

    init::run(&args(false), None).unwrap();

    let manifest_path = home.join("agentstack.toml");
    assert!(manifest_path.exists(), "manifest seeded");
    assert!(home.join("instructions").is_dir(), "instructions/ created");

    // The seed is a valid, loadable manifest with an (empty) instructions map.
    let m: agentstack::manifest::Manifest =
        toml::from_str(&fs::read_to_string(&manifest_path).unwrap()).unwrap();
    assert!(m.instructions.is_empty());
    assert!(m.servers.is_empty(), "--global imports nothing");
    let loaded = agentstack::manifest::load_from_dir(&home).unwrap();
    assert!(loaded.manifest.instructions.is_empty());

    // A second run refuses without --force, succeeds with it.
    assert!(init::run(&args(false), None).is_err());
    init::run(&args(true), None).unwrap();

    std::env::remove_var("AGENTSTACK_HOME");
}

#[test]
fn init_global_dry_run_writes_nothing() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join(".agentstack");
    std::env::set_var("AGENTSTACK_HOME", &home);

    let mut preview = args(false);
    preview.dry_run = true;

    // Dry-run on a clean machine: no manifest, no instructions dir, no home.
    init::run(&preview, None).unwrap();
    assert!(!home.exists(), "--dry-run must not create anything");

    // Dry-run over an existing manifest previews without --force and without
    // overwriting.
    init::run(&args(false), None).unwrap();
    let before = fs::read_to_string(home.join("agentstack.toml")).unwrap();
    init::run(&preview, None).unwrap();
    assert_eq!(
        fs::read_to_string(home.join("agentstack.toml")).unwrap(),
        before,
        "--dry-run must leave an existing manifest untouched"
    );

    std::env::remove_var("AGENTSTACK_HOME");
}

#[test]
fn house_rules_seed_is_idempotent_and_compiles() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join(".agentstack");
    std::env::set_var("HOME", tmp.path());
    std::env::set_var("AGENTSTACK_HOME", &home);

    let dir = init::ensure_global_manifest().unwrap();
    assert_eq!(dir, home);
    assert!(init::seed_house_rules(&home).unwrap());
    assert!(
        !init::seed_house_rules(&home).unwrap(),
        "second seed is a no-op"
    );

    // The manifest declares the fragment and the bundled body landed on disk.
    let loaded = agentstack::manifest::load_from_dir(&home).unwrap();
    let instr = loaded
        .manifest
        .instructions
        .get(init::HOUSE_RULES_NAME)
        .expect("[instructions.agentstack] declared");
    assert_eq!(instr.path, "./instructions/agentstack.md");
    let body = fs::read_to_string(home.join("instructions/agentstack.md")).unwrap();
    assert!(body.contains("agentstack house rules"));
    assert!(body.contains("do not create one")); // clean-at-rest lesson

    // And it compiles into a managed region for a harness.
    let reg = agentstack::adapter::Registry::load().unwrap();
    let desc = reg.get("claude-code").unwrap();
    let plan = agentstack::render::instructions::plan_instructions(
        &loaded.manifest,
        desc,
        agentstack::scope::Scope::Global,
        &home,
    )
    .unwrap();
    assert_eq!(plan.fragments, vec![init::HOUSE_RULES_NAME.to_string()]);
    assert!(plan.proposed.contains("agentstack house rules"));
    assert!(plan.proposed.contains("<!-- agentstack:start -->"));

    std::env::remove_var("AGENTSTACK_HOME");
    std::env::remove_var("HOME");
}
