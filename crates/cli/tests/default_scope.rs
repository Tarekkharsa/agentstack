// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The context-derived default scope (docs/design/default-scope.md): with no
//! `--scope`, a repo manifest writes PROJECT artifacts (the README quickstart
//! promise — repo-local config plus the managed .gitignore block, nothing in
//! the machine-global configs), while the machine manifest (~/.agentstack)
//! keeps its GLOBAL default. Serialized-by-design: mutates process-global
//! HOME / env vars.

use std::fs;
use std::sync::Mutex;

use agentstack::cli::ApplyArgs;
use agentstack::commands::apply;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn apply_write() -> ApplyArgs {
    ApplyArgs {
        targets: vec![],
        profile: None,
        dry_run: false,
        write: true,
        scope: None, // the point of the test: no explicit --scope
        allow_unresolved: false,
        prune_foreign: false,
        no_gitignore: false,
    }
}

const MANIFEST: &str = "version = 1\n\
    [servers.demo]\ntype = \"http\"\nurl = \"https://demo/mcp\"\n\
    [targets]\ndefault = [\"claude-code\"]\n";

#[test]
fn quickstart_in_a_repo_defaults_to_project_artifacts() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    // The quickstart layout: a git repo with .agentstack/agentstack.toml
    // (the managed .gitignore block only ever appears in a git repo).
    let proj = tmp.path().join("proj");
    fs::create_dir_all(proj.join(".git")).unwrap();
    fs::create_dir_all(proj.join(".agentstack")).unwrap();
    fs::write(proj.join(".agentstack/agentstack.toml"), MANIFEST).unwrap();

    apply::run(&apply_write(), Some(&proj)).unwrap();

    // Repo-local artifacts…
    let mcp = fs::read_to_string(proj.join(".mcp.json")).unwrap();
    assert!(mcp.contains("demo"), "server lands in .mcp.json: {mcp}");
    // …behind the managed .gitignore block…
    let ignore = fs::read_to_string(proj.join(".gitignore")).unwrap();
    assert!(
        ignore.contains(">>> agentstack") && ignore.contains("/.mcp.json"),
        "managed block covers the rendered config: {ignore}"
    );
    // …and nothing leaked into the machine-global config.
    assert!(
        !home.join(".claude.json").exists(),
        "a repo apply must not touch ~/.claude.json by default"
    );

    // The machine manifest keeps the global default: same apply, no --scope,
    // against ~/.agentstack writes the machine-global config.
    let machine_home = home.join(".agentstack");
    fs::create_dir_all(&machine_home).unwrap();
    fs::write(machine_home.join("agentstack.toml"), MANIFEST).unwrap();
    apply::run(&apply_write(), Some(&machine_home)).unwrap();
    let config = fs::read_to_string(home.join(".claude.json")).unwrap();
    assert!(
        config.contains("demo"),
        "the machine manifest still defaults to global: {config}"
    );

    std::env::remove_var("AGENTSTACK_HOME");
    std::env::remove_var("HOME");
}
