//! Every shipped machine-policy preset in `examples/policies/` must parse with
//! the REAL loader — the same `machine_policy_health()` the gateway consults —
//! not a hand-rolled parse. A preset that documents a syntax the loader rejects
//! would be worse than no example, so this guards them.

use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

// machine_policy_health() reads the process-global AGENTSTACK_HOME; serialize
// the tests that point it at a temp dir.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// The `examples/policies/` dir, resolved from this crate's manifest dir
/// (`crates/cli`) up to the workspace root.
fn presets_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("examples")
        .join("policies")
}

const PRESETS: &[&str] = &["compatible", "developer", "locked-down", "ci"];

#[test]
fn every_preset_parses_with_the_real_loader() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = presets_dir();
    for name in PRESETS {
        let src = dir.join(format!("{name}.toml"));
        let body = fs::read_to_string(&src)
            .unwrap_or_else(|e| panic!("reading preset {}: {e}", src.display()));

        // Point AGENTSTACK_HOME at a fresh temp dir and drop the preset in as
        // the machine manifest, exactly where the loader looks for it.
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", home.path());
        fs::write(home.path().join("agentstack.toml"), &body).unwrap();

        let health = agentstack::manifest::machine_policy_health();
        std::env::remove_var("AGENTSTACK_HOME");

        match health {
            Some(Ok(source)) => assert!(
                !source.policy.is_empty(),
                "preset {name} parsed but produced an empty [policy] — it should set some rules"
            ),
            Some(Err(e)) => panic!("preset {name}.toml failed the real loader: {e:#}"),
            None => panic!("preset {name}.toml was not seen as a machine manifest"),
        }
    }
}

#[test]
fn readme_lists_every_preset() {
    // Keep the README table honest: a file added here without a row (or vice
    // versa) fails, so the docs can't silently drift from the shipped presets.
    let dir = presets_dir();
    let readme = fs::read_to_string(dir.join("README.md")).unwrap();
    for name in PRESETS {
        assert!(
            readme.contains(&format!("{name}.toml")),
            "README.md is missing a row for {name}.toml"
        );
    }
}
