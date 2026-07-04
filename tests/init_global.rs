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
