// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Commands run from a subdirectory act on the ROOT project: manifest
//! resolution walks up to the nearest ancestor carrying a manifest (the same
//! anchor rule as the guard), and a bare `init` from inside an initialized
//! project refuses to silently nest a second manifest.

use std::fs;
use std::path::Path;
use std::sync::Mutex;

use agentstack::cli::InitArgs;
use agentstack::commands;

// `commands::load` and `init` read the process-global cwd; serialize the
// tests in this binary so a chdir can't leak into a parallel test.
static CWD_LOCK: Mutex<()> = Mutex::new(());

/// Run `f` with the process cwd at `dir`, restoring the previous cwd after.
fn with_cwd<T>(dir: &Path, f: impl FnOnce() -> T) -> T {
    let prev = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir).unwrap();
    let out = f();
    std::env::set_current_dir(prev).unwrap();
    out
}

#[test]
fn subdir_commands_resolve_the_root_manifest() {
    let _g = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    // TempDirs live under a symlinked root on macOS (/var → /private/var);
    // canonicalize so comparisons against current_dir()-derived paths hold.
    let root = tmp.path().canonicalize().unwrap().join("proj");
    fs::create_dir_all(root.join(".agentstack")).unwrap();
    fs::write(root.join(".agentstack/agentstack.toml"), "version = 1\n").unwrap();
    let deep = root.join("src/deep");
    fs::create_dir_all(&deep).unwrap();

    // The shared base resolver walks up to the root (cwd-independent seam)…
    assert_eq!(commands::project_base_from(&deep), root);
    // …and the real command path (doctor/lock/apply/use/run all funnel
    // through commands::load) resolves the ROOT manifest from `src/deep`.
    let ctx = with_cwd(&deep, || commands::load(None)).unwrap();
    assert_eq!(ctx.dir, root.join(".agentstack"));

    // No project anywhere above → falls back to the start dir itself, so
    // "no manifest" errors keep pointing at the cwd.
    let bare = tmp.path().canonicalize().unwrap().join("loose/dir");
    fs::create_dir_all(&bare).unwrap();
    assert_eq!(commands::project_base_from(&bare), bare);
}

#[test]
fn init_from_a_subdir_refuses_to_silently_nest() {
    let _g = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let root = tmp.path().canonicalize().unwrap().join("proj");
    fs::create_dir_all(root.join(".agentstack")).unwrap();
    fs::write(root.join(".agentstack/agentstack.toml"), "version = 1\n").unwrap();
    let deep = root.join("src/deep");
    fs::create_dir_all(&deep).unwrap();

    let args = InitArgs {
        global: false,
        force: false,
        dry_run: false,
        secrets: None,
        no_keychain: false,
    };
    let err = with_cwd(&deep, || commands::init::run(&args, None)).unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("already initialized"), "{msg}");
    assert!(
        msg.contains(
            &root
                .join(".agentstack/agentstack.toml")
                .display()
                .to_string()
        ),
        "the refusal names the ROOT manifest: {msg}"
    );
    assert!(
        !deep.join(".agentstack").exists() && !deep.join("agentstack.toml").exists(),
        "nothing nested was written"
    );
}
