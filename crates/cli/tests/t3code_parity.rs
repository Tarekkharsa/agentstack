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
        // profiles-edit-v1: the closed set of fixed panel verbs. A new panel
        // capability is a new arm here, never an MCP tool wired into the browser.
        Command::AddSkillToProfile(a) => commands::panel_edit::add_skill(&a, dir),
        Command::AddServerToProfile(a) => commands::panel_edit::add_server(&a, dir),
        Command::CreateProfile(a) => commands::panel_edit::create_profile(&a, dir),
        Command::UseProfile(a) => commands::panel_edit::use_profile(&a, dir),
        Command::LibraryIndex => commands::panel_edit::library_index(dir),
        _ => panic!("parity dispatch: unexpected subcommand in {argv:?}"),
    }
}

/// Parse a fixed argv into its `Command` without dispatching. The panel reads
/// (like `init_args_of`) need the typed args to call a preview primitive for its
/// consent digest — the same computation the apply binds to.
fn command_of(argv: &[&str]) -> Command {
    let matches = agentstack::cli::runtime_command()
        .try_get_matches_from(argv)
        .unwrap();
    Cli::from_arg_matches(&matches)
        .unwrap()
        .command
        .expect("argv names a subcommand")
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

/// The consent digest a panel preview returns for the `--name web --skill demo`
/// create-profile request against `argv`.
fn create_profile_digest(argv: &[&str], proj: &Path) -> String {
    match command_of(argv) {
        Command::CreateProfile(a) => commands::panel_edit::create_profile_preview(&a, Some(proj))
            .expect("preview must succeed")["consent_digest"]
            .as_str()
            .expect("preview carries a consent_digest")
            .to_string(),
        _ => panic!("argv names create-profile: {argv:?}"),
    }
}

/// Witness (profiles-edit-v1): the panel's `create-profile` apply runs the house
/// pipeline through the ONE activation path, so it both RE-LOCKS (pins
/// `agentstack.lock`) and RE-RENDERS (materializes the toolset's skills into the
/// target). Real, not mocked: a declared claude-code target and an inline skill
/// on disk, driven through the exact fixed argv the panel bridge emits — no MCP.
#[test]
fn panel_create_profile_relocks_and_rerenders() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    let proj = tmp.path().join("proj");
    fs::create_dir_all(proj.join("skills/demo")).unwrap();
    fs::write(proj.join("skills/demo/SKILL.md"), "# demo\n").unwrap();
    // A claude-code target (declared, so it activates without detection) and one
    // inline skill to enroll. No profiles yet — the panel creates one.
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [skills.demo]\npath = \"./skills/demo\"\n",
    )
    .unwrap();
    let proj_root = proj.to_str().unwrap();

    // Nothing rendered or locked before the apply.
    assert!(!proj.join("agentstack.lock").exists());
    assert!(!proj.join(".claude/skills/demo").exists());

    // Preview (read) → the digest the panel reviews, from the same computation
    // the apply binds to; then apply through the pinned dispatch arm.
    let preview_argv = [
        "agentstack",
        "--manifest-dir",
        proj_root,
        "create-profile",
        "--name",
        "web",
        "--skill",
        "demo",
    ];
    let digest = create_profile_digest(&preview_argv, &proj);
    dispatch(&[
        "agentstack",
        "--manifest-dir",
        proj_root,
        "create-profile",
        "--name",
        "web",
        "--skill",
        "demo",
        "--yes",
        "--consented",
        &digest,
    ])
    .unwrap();

    // Re-lock: the toolset's skill is now pinned in agentstack.lock.
    let lock = fs::read_to_string(proj.join("agentstack.lock"))
        .expect("create-profile activation must pin the lockfile");
    assert!(lock.contains("demo"), "the skill is pinned: {lock}");

    // Re-render: the skill materialized into the claude-code project skills dir.
    assert!(
        proj.join(".claude/skills/demo/SKILL.md").exists(),
        "create-profile activation must materialize the skill into the target"
    );

    // The library-index read arm routes too — a fresh read against the same
    // project (pins the LibraryIndex dispatch arm end-to-end).
    dispatch(&["agentstack", "--manifest-dir", proj_root, "library-index"]).unwrap();

    std::env::remove_var("HOME");
    std::env::remove_var("AGENTSTACK_HOME");
}

/// Witness (profiles-edit-v1): a panel mutation whose server carries an
/// unresolved `${REF}` FAILS CLOSED — the render is blocked (nonzero exit), no
/// native config is written, and the manifest keeps the `${REF}` verbatim (never
/// a value, never blanked). This is the feature, not a bug. Driven through the
/// fixed argv.
#[test]
fn panel_add_server_fails_closed_on_unresolved_ref() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    // An existing toolset (add-*-to-profile requires one) and a claude-code
    // target to render into.
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [profiles.web]\nservers = []\nskills = []\n",
    )
    .unwrap();
    let proj_root = proj.to_str().unwrap();

    // A new HTTP server whose auth header needs a secret that will not resolve.
    let define = [
        "agentstack",
        "--manifest-dir",
        proj_root,
        "add-server-to-profile",
        "--profile",
        "web",
        "--name",
        "kibana",
        "--type",
        "http",
        "--url",
        "https://k/mcp",
        "--header",
        "Authorization=Bearer ${NOPE_TOKEN}",
    ];
    let digest = match command_of(&define) {
        Command::AddServerToProfile(a) => commands::panel_edit::add_server_preview(&a, Some(&proj))
            .unwrap()["consent_digest"]
            .as_str()
            .unwrap()
            .to_string(),
        _ => panic!("argv names add-server-to-profile"),
    };

    let mut apply: Vec<&str> = define.to_vec();
    apply.extend_from_slice(&["--yes", "--consented", &digest]);
    let err = dispatch(&apply).expect_err("an unresolved ${REF} must block the apply");
    assert!(
        err.to_string().contains("blocked"),
        "the error names the blockage: {err}"
    );

    // The manifest kept the server AND the ${REF} verbatim — never a value.
    let manifest = fs::read_to_string(proj.join("agentstack.toml")).unwrap();
    assert!(
        manifest.contains("kibana"),
        "the server was written to the manifest: {manifest}"
    );
    assert!(
        manifest.contains("${NOPE_TOKEN}"),
        "the ref is preserved, never blanked or resolved: {manifest}"
    );

    // Fail closed: the blocked render wrote no native config for the target.
    assert!(
        !proj.join(".mcp.json").exists(),
        "a blocked render must not write the target's server config"
    );
    // A fully-blocked activation is a no-op on disk — no phantom lockfile either.
    assert!(
        !proj.join("agentstack.lock").exists(),
        "a fully-blocked apply must not leave a lockfile behind"
    );

    std::env::remove_var("HOME");
    std::env::remove_var("AGENTSTACK_HOME");
}

/// Witness (profiles-edit-v1): the consent digest a preview returns is
/// DETERMINISTIC for identical input (two previews of the same request against
/// the same manifest match) and MOVES when the manifest drifts — which is what
/// lets an apply refuse a digest reviewed against different state.
#[test]
fn panel_consent_digest_is_stable_and_binds_manifest() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    let proj = tmp.path().join("proj");
    fs::create_dir_all(proj.join("skills/demo")).unwrap();
    fs::write(proj.join("skills/demo/SKILL.md"), "# demo\n").unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[skills.demo]\npath = \"./skills/demo\"\n",
    )
    .unwrap();
    let proj_root = proj.to_str().unwrap();

    let argv = [
        "agentstack",
        "--manifest-dir",
        proj_root,
        "create-profile",
        "--name",
        "web",
        "--skill",
        "demo",
    ];

    let first = create_profile_digest(&argv, &proj);
    let second = create_profile_digest(&argv, &proj);
    assert_eq!(first, second, "identical input → identical digest");
    assert!(first.starts_with("sha256:"), "digest is a sha256: {first}");

    // Bind the manifest bytes: an edit re-keys the digest, so an apply carrying
    // the pre-edit digest would be refused by the consent gate.
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[skills.demo]\npath = \"./skills/demo\"\n# drift\n",
    )
    .unwrap();
    assert_ne!(
        first,
        create_profile_digest(&argv, &proj),
        "a manifest edit must move the digest"
    );

    std::env::remove_var("HOME");
    std::env::remove_var("AGENTSTACK_HOME");
}
