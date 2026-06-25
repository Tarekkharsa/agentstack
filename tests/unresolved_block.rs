//! Finding 1 (project review): an unresolved `${REF}` must never reach a live
//! config write. `apply --write` blocks the target by default and only writes
//! when `--allow-unresolved` is passed.

use std::fs;

use agentstack::cli::ApplyArgs;
use agentstack::commands::apply;
use agentstack::scope::Scope;

fn args(write: bool, allow_unresolved: bool) -> ApplyArgs {
    ApplyArgs {
        targets: vec!["claude-code".into()],
        profile: None,
        dry_run: false,
        write,
        scope: Some(Scope::Global),
        allow_unresolved,
    }
}

#[test]
fn unresolved_secret_blocks_write_unless_allowed() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    // A server whose header needs a secret that does NOT resolve on this machine.
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [servers.kibana]\ntype = \"http\"\nurl = \"https://k/mcp\"\n\
         headers = { Authorization = \"Bearer ${NOPE_TOKEN}\" }\n",
    )
    .unwrap();

    let claude_cfg = home.join(".claude.json");

    // Default: write is blocked → the live config is never created.
    apply::run(&args(true, false), Some(&proj)).unwrap();
    assert!(
        !claude_cfg.exists(),
        "unresolved secret must block the write — but {} was written",
        claude_cfg.display()
    );

    // Escape hatch: --allow-unresolved writes (leaving the ${REF} in place).
    apply::run(&args(true, true), Some(&proj)).unwrap();
    let written = fs::read_to_string(&claude_cfg).unwrap();
    assert!(written.contains("kibana"));
    assert!(
        written.contains("${NOPE_TOKEN}"),
        "the ref is left verbatim, never blanked"
    );
}
