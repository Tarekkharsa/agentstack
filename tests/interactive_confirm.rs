//! Plan D safety contract: in non-interactive shells, `apply` without `--write`
//! must never write and never block. Only `--write` — the
//! scripting escape hatch — applies in a non-interactive shell. Under `cargo`
//! test, stdin/stdout are not terminals, so this exercises exactly the
//! non-interactive branch a CI runner takes.

use std::fs;
use std::sync::Mutex;

use agentstack::cli::ApplyArgs;
use agentstack::commands::apply;
use agentstack::scope::Scope;

// Mutates the process-global HOME; serialize with the other HOME-touching tests.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn write_manifest(proj: &std::path::Path) {
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [servers.demo]\ntype = \"http\"\nurl = \"https://demo/mcp\"\n",
    )
    .unwrap();
}

fn args(write: bool) -> ApplyArgs {
    ApplyArgs {
        targets: vec!["claude-code".into()],
        profile: None,
        dry_run: false,
        write,
        scope: Some(Scope::Global),
        allow_unresolved: false,
        no_gitignore: true,
    }
}

#[test]
fn default_apply_is_dry_run_when_not_a_terminal() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    write_manifest(&proj);

    let claude_cfg = home.join(".claude.json");

    // No --write, no terminal → dry-run. It must return promptly (never block on
    // stdin) and write nothing.
    apply::run(&args(false), Some(&proj)).unwrap();
    assert!(
        !claude_cfg.exists(),
        "default apply in a non-interactive shell must not write — but {} appeared",
        claude_cfg.display()
    );

    // --write is the escape hatch: it applies even with no terminal.
    apply::run(&args(true), Some(&proj)).unwrap();
    let written = fs::read_to_string(&claude_cfg).unwrap();
    assert!(
        written.contains("demo"),
        "--write must apply the server even in a non-interactive shell"
    );
}
