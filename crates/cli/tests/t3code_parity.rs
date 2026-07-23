// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Parity witness (UI control-plane §"Acceptance criteria"): the t3code
//! journey and the direct terminal journey are two views of ONE CLI-owned
//! flow — same plan, same write path, same resulting files.
//!
//! The t3code server maps its closed action enum to fixed argv; this test
//! drives those exact argv strings through the real clap parser and command
//! dispatch (no frontend in the loop), runs the direct scripted journey in a
//! second identical project, and asserts both produce byte-identical
//! managed files. If either side's flags or behavior drift, this fails
//! before the panel ships the drift.
//!
//! The argv here must stay in sync with t3code's `AgentstackCli.actionArgv`
//! (apps/server/src/agentstack/AgentstackCli.ts). `--secrets skip` stands in
//! for the panel's store choice so the witness never touches the OS keychain;
//! parity is independent of which store both sides name.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Result;
use clap::FromArgMatches;

use agentstack::cli::{Cli, Command};
use agentstack::commands;

// These tests mutate the process-global HOME/AGENTSTACK_HOME; serialize them
// (also against other test binaries via the same env-var convention).
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Parse and run one fixed argv exactly as `main` would: clap parse, decode,
/// dispatch on the subcommand. Only the verbs the panel's closed enum maps to
/// are dispatchable here — a new action means extending this match.
fn dispatch(argv: &[&str]) -> Result<()> {
    let matches = agentstack::cli::runtime_command().try_get_matches_from(argv)?;
    let cli = Cli::from_arg_matches(&matches)?;
    let dir = cli.manifest_dir.as_deref();
    match cli.command.expect("argv names a subcommand") {
        Command::Init(args) => commands::init::run(&args, dir),
        Command::Restore(args) => commands::restore::run(&args, dir),
        Command::Trust(args) => commands::trust::run(&args),
        _ => panic!("parity dispatch: unexpected subcommand in {argv:?}"),
    }
}

/// Extract `InitArgs` from a fixed argv (for the plan read, which the panel
/// consumes as JSON — the test needs the digest out of the same computation).
fn init_args_of(argv: &[&str]) -> agentstack::cli::InitArgs {
    let matches = agentstack::cli::runtime_command()
        .try_get_matches_from(argv)
        .unwrap();
    let cli = Cli::from_arg_matches(&matches).unwrap();
    match cli.command.unwrap() {
        Command::Init(args) => args,
        _ => panic!("not an init argv"),
    }
}

/// Every file under `root`, as sorted relative paths.
fn file_tree(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, root, out);
            } else {
                out.push(path.strip_prefix(root).unwrap().to_path_buf());
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

#[test]
fn panel_argv_and_direct_cli_produce_identical_setup() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    // One detected CLI with an importable server and an inline token the
    // import lifts to a `${REF}` (never a value in the manifest).
    fs::write(
        home.join(".claude.json"),
        r#"{"mcpServers":{"search":{"command":"npx","args":["search-mcp"],"env":{"SEARCH_TOKEN":"sk-live-parity"}}}}"#,
    )
    .unwrap();

    // Same leaf directory name on both sides so any name-derived manifest
    // content is identical; only the (unmanaged) tmp prefix differs.
    let panel_proj = tmp.path().join("panel/proj");
    let direct_proj = tmp.path().join("direct/proj");
    fs::create_dir_all(&panel_proj).unwrap();
    fs::create_dir_all(&direct_proj).unwrap();
    let panel_root = panel_proj.to_str().unwrap();
    let direct_root = direct_proj.to_str().unwrap();

    // ── t3code journey: plan (read) → apply bound to the reviewed digest.
    let plan_args = init_args_of(&[
        "agentstack",
        "--manifest-dir",
        panel_root,
        "init",
        "--plan",
        "--secrets",
        "skip",
    ]);
    let plan = commands::init::plan_json(&plan_args, Some(&panel_proj)).unwrap();
    let digest = plan["plan_digest"].as_str().unwrap().to_string();

    dispatch(&[
        "agentstack",
        "--manifest-dir",
        panel_root,
        "init",
        "--yes",
        "--secrets",
        "skip",
        "--consented-plan",
        &digest,
    ])
    .unwrap();

    // ── Direct terminal journey: the documented scriptable import.
    dispatch(&[
        "agentstack",
        "--manifest-dir",
        direct_root,
        "init",
        "--yes",
        "--secrets",
        "skip",
    ])
    .unwrap();

    // Same files, same bytes.
    let panel_tree = file_tree(&panel_proj);
    assert_eq!(panel_tree, file_tree(&direct_proj), "same file set");
    assert!(
        panel_tree.contains(&PathBuf::from(".agentstack/agentstack.toml")),
        "setup wrote a manifest: {panel_tree:?}"
    );
    for rel in &panel_tree {
        let a = fs::read_to_string(panel_proj.join(rel)).unwrap();
        let b = fs::read_to_string(direct_proj.join(rel)).unwrap();
        assert_eq!(
            a,
            b,
            "managed file {} must be byte-identical",
            rel.display()
        );
        assert!(
            !a.contains("sk-live-parity"),
            "no secret value may enter a written file ({})",
            rel.display()
        );
    }

    // Same status through the same read contract.
    let panel_doctor = commands::doctor::collect(Some(&panel_proj)).unwrap();
    let direct_doctor = commands::doctor::collect(Some(&direct_proj)).unwrap();
    assert_eq!(panel_doctor["state"], direct_doctor["state"]);
    assert_eq!(
        panel_doctor["errors"], direct_doctor["errors"],
        "same error count through both journeys"
    );

    // ── Undo. The ledger is machine-global, so the panel's Undo must NOT be
    // a blind `--last` (here the machine-wide newest entry is the DIRECT
    // project's init). The panel reads the inventory, picks the newest entry
    // touching its own project, and undoes it by id — exactly what its fixed
    // action does.
    let registry = agentstack::adapter::Registry::load().unwrap();
    let inventory = commands::restore::list_json_value(&registry, &panel_proj);
    let entry = inventory["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["touches_project"] == true && e["undone"] == false)
        .expect("an undoable entry for the panel project");
    let id = entry["id"].as_str().unwrap().to_string();

    dispatch(&[
        "agentstack",
        "--manifest-dir",
        panel_root,
        "restore",
        &id,
        "--write",
        "--json",
    ])
    .unwrap();
    assert!(
        !panel_proj.join(".agentstack/agentstack.toml").exists(),
        "undo removes the manifest the setup wrote"
    );
    assert!(
        direct_proj.join(".agentstack/agentstack.toml").exists(),
        "undoing the panel project must not touch the other project"
    );

    std::env::remove_var("HOME");
    std::env::remove_var("AGENTSTACK_HOME");
}
