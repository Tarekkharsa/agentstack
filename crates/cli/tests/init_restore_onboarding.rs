// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Stage 1.2 witness: `agentstack restore` undoes the COMPLETE onboarding
//! write set — not just the manifest, but the `.env` holding lifted secret
//! values and the `.gitignore` line that keeps it out of git — returning the
//! project byte-for-byte to its pre-init state.

use std::fs;
use std::sync::Mutex;

use agentstack::cli::{InitArgs, RestoreArgs, SecretStore};
use agentstack::commands::{init, restore};

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn restore_undoes_manifest_env_and_gitignore_from_one_init() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    // One detected CLI with a server whose inline token init lifts into a
    // project `.env` (the `--secrets env` store).
    fs::write(
        home.join(".claude.json"),
        r#"{"mcpServers":{"search":{"command":"npx","args":["search-mcp"],"env":{"SEARCH_TOKEN":"sk-live-undo"}}}}"#,
    )
    .unwrap();

    // A git project with a pre-existing .gitignore whose exact bytes must
    // survive the round trip.
    let proj = tmp.path().join("proj");
    fs::create_dir_all(proj.join(".git")).unwrap();
    let prior_gitignore = "target/\n*.log\n";
    fs::write(proj.join(".gitignore"), prior_gitignore).unwrap();

    init::run(
        &InitArgs {
            global: false,
            force: false,
            dry_run: false,
            plan: false,
            secrets: Some(SecretStore::Env),
            no_keychain: false,
            yes: true,
            consented_plan: None,
        },
        Some(&proj),
    )
    .unwrap();

    // The complete onboarding write set exists: manifest, .env with the
    // lifted value, and the .gitignore rule protecting it.
    let manifest = proj.join(".agentstack/agentstack.toml");
    assert!(manifest.exists());
    assert!(fs::read_to_string(&manifest)
        .unwrap()
        .contains("${SEARCH_TOKEN}"));
    let env_file = proj.join(".agentstack/.env");
    let env_path = if env_file.exists() {
        env_file
    } else {
        proj.join(".env")
    };
    assert!(
        fs::read_to_string(&env_path)
            .unwrap()
            .contains("sk-live-undo"),
        "the lifted value landed in the project .env"
    );
    let ignored = fs::read_to_string(proj.join(".gitignore")).unwrap();
    assert!(ignored.contains(prior_gitignore), "prior rules kept");
    assert_ne!(ignored, prior_gitignore, "an ignore rule was added");

    // One undo reverses all of it, byte-for-byte.
    restore::run(
        &RestoreArgs {
            adapter: None,
            last: true,
            scope: None,
            write: true,
            json: false,
        },
        Some(&proj),
    )
    .unwrap();

    assert!(!manifest.exists(), "restore removed the imported manifest");
    assert!(!env_path.exists(), "restore removed the secrets .env");
    assert_eq!(
        fs::read_to_string(proj.join(".gitignore")).unwrap(),
        prior_gitignore,
        "restore returned .gitignore to its exact prior bytes"
    );

    std::env::remove_var("HOME");
    std::env::remove_var("AGENTSTACK_HOME");
}
