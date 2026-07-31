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

use agentstack::cli::{ApplyArgs, Cli, Command, WorkflowCmd};
use agentstack::commands;
use agentstack::scope::Scope;

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
        Command::SetGitignore(a) => commands::panel_edit::set_gitignore(&a, dir),
        Command::UseProfile(a) => commands::panel_edit::use_profile(&a, dir),
        Command::LibraryIndex => commands::panel_edit::library_index(dir),
        Command::RemoveFromLibrary(a) => commands::panel_edit::remove_from_library(&a, dir),
        Command::RemoveCapability(a) => commands::panel_edit::remove_capability(&a, dir),
        // workflow-observe-v1: the two read-only observation verbs t3code emits
        // as fixed argv. They print the enveloped body; the JSON-shape witness
        // asserts on `list_value`/`runs_value` directly (below), while these
        // arms keep the real clap → dispatch path exercised.
        Command::Workflow(WorkflowCmd::List(args)) => commands::workflow::list(dir, &args),
        Command::Workflow(WorkflowCmd::Runs(args)) => commands::workflow::runs(&args),
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

/// Witness (toolset-create-v2): the panel's `create-profile` apply RE-LOCKS
/// (pins `agentstack.lock`) and RENDERS NOTHING — naming a toolset is not
/// switching to it (review finding H3). Activation stays a separate verb
/// (`use-profile`, `session start`), which is why the capability got a new name
/// rather than a wider reading of `profiles-edit-v1`. Real, not mocked: a
/// declared claude-code target and an inline skill on disk, driven through the
/// exact fixed argv the panel bridge emits — no MCP.
#[test]
fn panel_create_profile_relocks_and_does_not_render() {
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
        .expect("create-profile must pin the lockfile");
    assert!(lock.contains("demo"), "the skill is pinned: {lock}");

    // The manifest entry exists — the create half really happened.
    let manifest = fs::read_to_string(proj.join("agentstack.toml")).unwrap();
    assert!(
        manifest.contains("[profiles.web]"),
        "create-profile writes the manifest entry: {manifest}"
    );

    // No render: the skill did NOT materialize into the claude-code target, and
    // the target's own skills dir was never created. This is the H3 half — if
    // it regresses, creating a toolset silently switches the user's CLIs.
    assert!(
        !proj.join(".claude/skills/demo").exists(),
        "create-profile must not materialize the skill into the target"
    );
    assert!(
        !proj.join(".claude").exists(),
        "create-profile must not create the target's config dir at all"
    );

    // The library-index read arm routes too — a fresh read against the same
    // project (pins the LibraryIndex dispatch arm end-to-end).
    dispatch(&["agentstack", "--manifest-dir", proj_root, "library-index"]).unwrap();

    std::env::remove_var("HOME");
    std::env::remove_var("AGENTSTACK_HOME");
}

#[test]
fn panel_remove_capability_updates_manifest_and_rendered_config() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n\
         [targets]\ndefault = [\"claude-code\"]\n\
         [servers.keep]\ntype = \"http\"\nurl = \"https://keep.example/mcp\"\n\
         [servers.remove-me]\ntype = \"http\"\nurl = \"https://remove.example/mcp\"\n",
    )
    .unwrap();
    let proj_root = proj.to_str().unwrap();

    commands::apply::run(
        &ApplyArgs {
            targets: vec![],
            profile: None,
            dry_run: false,
            write: true,
            scope: Some(Scope::Project),
            allow_unresolved: false,
            prune_foreign: false,
            no_gitignore: true,
            verbose: false,
        },
        Some(&proj),
    )
    .unwrap();
    let rendered_before = fs::read_to_string(proj.join(".mcp.json")).unwrap();
    assert!(rendered_before.contains("remove-me"));

    let preview_argv = [
        "agentstack",
        "--manifest-dir",
        proj_root,
        "remove-capability",
        "--kind",
        "server",
        "--name",
        "remove-me",
        "--preview",
    ];
    let digest = match command_of(&preview_argv) {
        Command::RemoveCapability(args) => {
            commands::panel_edit::remove_capability_preview(&args, Some(&proj)).unwrap()
                ["consent_digest"]
                .as_str()
                .unwrap()
                .to_string()
        }
        _ => panic!("argv names remove-capability"),
    };
    dispatch(&[
        "agentstack",
        "--manifest-dir",
        proj_root,
        "remove-capability",
        "--kind",
        "server",
        "--name",
        "remove-me",
        "--yes",
        "--consented",
        &digest,
    ])
    .unwrap();

    let manifest = fs::read_to_string(proj.join("agentstack.toml")).unwrap();
    assert!(!manifest.contains("[servers.remove-me]"));
    let rendered_after = fs::read_to_string(proj.join(".mcp.json")).unwrap();
    assert!(!rendered_after.contains("remove-me"));
    assert!(rendered_after.contains("keep"));

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

/// Witness (profiles-edit-v1, library curation): `remove-from-library` is the
/// one panel mutation that edits MACHINE state instead of the project, so this
/// pins the three properties that make it safe to expose in a UI:
///
///  1. It is recoverable — the body and the index row move to `lib/.trash`,
///     and `lib trash --restore` puts both back.
///  2. It never touches the project — manifest and lockfile are byte-identical
///     after the removal (no silent re-lock, no re-render).
///  3. Consent binds the library index, not the manifest: a library edit
///     between preview and apply moves the digest, so stale consent is refused.
#[test]
fn panel_remove_from_library_is_recoverable_and_leaves_the_project_alone() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));
    let lib_home = home.join(".agentstack").join("lib");

    // A library skill, added through the one insertion path.
    let src = tmp.path().join("src-pdf");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("SKILL.md"), "---\ndescription: pdf\n---\n# pdf\n").unwrap();
    commands::lib::add_skill(
        &lib_home,
        "pdf",
        commands::lib::LibSource::Path(&src),
        false,
        true,
        false,
    )
    .unwrap();

    // A project that references it by name — the case where the panel must warn.
    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[profiles.web]\nskills = [\"pdf\"]\n",
    )
    .unwrap();
    let proj_root = proj.to_str().unwrap();
    let manifest_before = fs::read(proj.join("agentstack.toml")).unwrap();

    let argv = [
        "agentstack",
        "--manifest-dir",
        proj_root,
        "remove-from-library",
        "--kind",
        "skill",
        "--name",
        "pdf",
    ];
    let args = match command_of(&argv) {
        Command::RemoveFromLibrary(a) => a,
        _ => panic!("not a remove-from-library argv"),
    };

    // The preview names the scope, the dependency, and the way back.
    let preview = commands::panel_edit::remove_from_library_preview(&args, Some(&proj)).unwrap();
    let digest = preview["consent_digest"].as_str().unwrap().to_string();
    assert!(digest.starts_with("sha256:"), "{digest}");
    let removal = &preview["removal"];
    assert_eq!(removal["scope"], "machine");
    assert_eq!(removal["used_by_this_project"], true);
    assert_eq!(removal["profiles"][0], "web");
    assert!(removal["restore_command"]
        .as_str()
        .unwrap()
        .contains("lib trash --restore"));

    // A library edit between preview and apply re-keys the digest: the panel's
    // reviewed consent cannot be replayed against different library state.
    commands::lib::add_skill(
        &lib_home,
        "other",
        commands::lib::LibSource::Path(&src),
        false,
        true,
        false,
    )
    .unwrap();
    let after_drift =
        commands::panel_edit::remove_from_library_preview(&args, Some(&proj)).unwrap();
    assert_ne!(
        digest,
        after_drift["consent_digest"].as_str().unwrap(),
        "a library edit must move the digest"
    );
    let digest = after_drift["consent_digest"].as_str().unwrap().to_string();

    // Apply through the real clap → dispatch path with the reviewed digest.
    let apply = [
        "agentstack",
        "--manifest-dir",
        proj_root,
        "remove-from-library",
        "--kind",
        "skill",
        "--name",
        "pdf",
        "--yes",
        "--consented",
        &digest,
    ];
    dispatch(&apply).unwrap();

    // (1) gone from the library, (2) the project untouched, (3) recoverable.
    let library = agentstack::library::Library::load(&lib_home).unwrap();
    assert!(library.get("pdf").is_none(), "removed from the library");
    assert!(
        !lib_home.join("skills/pdf").exists(),
        "body left lib/skills"
    );
    assert_eq!(
        manifest_before,
        fs::read(proj.join("agentstack.toml")).unwrap(),
        "the project manifest is not touched by a library removal"
    );
    assert!(
        !proj.join("agentstack.lock").exists(),
        "a library removal never re-locks the project"
    );

    let trashed = commands::lib_trash::list(&lib_home).unwrap();
    assert_eq!(trashed.len(), 1, "the removal is in the trash");
    commands::lib_trash::restore(&lib_home, &trashed[0].id, false, true).unwrap();
    assert!(
        agentstack::library::Library::load(&lib_home)
            .unwrap()
            .get("pdf")
            .is_some(),
        "restore puts the entry back"
    );
    assert!(
        lib_home.join("skills/pdf/SKILL.md").exists(),
        "restore puts the body back"
    );

    // Stale consent is refused before anything moves.
    let stale = [
        "agentstack",
        "--manifest-dir",
        proj_root,
        "remove-from-library",
        "--kind",
        "skill",
        "--name",
        "pdf",
        "--yes",
        "--consented",
        "sha256:0000000000000000000000000000000000000000000000000000000000000000",
    ];
    let err = dispatch(&stale).unwrap_err().to_string();
    assert!(err.contains("consent digest mismatch"), "{err}");
    assert!(
        agentstack::library::Library::load(&lib_home)
            .unwrap()
            .get("pdf")
            .is_some(),
        "refusal changed nothing"
    );

    std::env::remove_var("HOME");
    std::env::remove_var("AGENTSTACK_HOME");
}

/// Witness (workflow-observe-v1): the two read-only observation verbs t3code
/// emits — `workflow list --json` and `workflow runs --json` — route through
/// the real clap → dispatch path AND carry the versioned UI-contract envelope.
///
/// The dispatch arms (which print) are exercised for parity with the panel's
/// fixed argv; the JSON shape is asserted on `list_value`/`runs_value` directly
/// (the Rust-callable primitives behind those verbs), in the panel_edit
/// envelope-assertion style. `runs` reads the machine-global runs dir, isolated
/// here via `AGENTSTACK_HOME` exactly as `workflow.rs`'s own run tests do.
#[test]
fn panel_workflow_observe_reads_carry_envelope() {
    use agentstack::calllog::RunEvent;

    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home").join(".agentstack");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", tmp.path().join("home"));
    std::env::set_var("AGENTSTACK_HOME", &home);

    // A project declaring one workflow: `list` surfaces every declared entry
    // regardless of admission, so the row appears without pinning or trust.
    let proj = tmp.path().join("proj");
    fs::create_dir_all(proj.join("workflows")).unwrap();
    fs::write(
        proj.join("workflows/main.js"),
        "export const meta = { roles: ['w'] };\nreturn 1;\n",
    )
    .unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[profiles.w]\n[workflows.demo]\npath = \"./workflows/main.js\"\nroles = [\"w\"]\n",
    )
    .unwrap();
    let proj_root = proj.to_str().unwrap();

    // The exact fixed argv t3code emits — routed through clap + dispatch (the
    // arms print; here we only need them to run without panicking).
    dispatch(&[
        "agentstack",
        "--manifest-dir",
        proj_root,
        "workflow",
        "list",
        "--json",
    ])
    .unwrap();
    dispatch(&[
        "agentstack",
        "--manifest-dir",
        proj_root,
        "workflow",
        "runs",
        "--json",
    ])
    .unwrap();

    // list_value: enveloped, and the `workflows` body key is intact with the
    // declared entry present.
    let list = commands::workflow::list_value(Some(&proj)).unwrap();
    assert_eq!(
        list["schema_version"],
        agentstack::ui_contract::SCHEMA_VERSION
    );
    assert!(
        list["features"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f == "workflow-observe-v1"),
        "features advertises the observe contract: {list}"
    );
    // workflow-serial-roles-v1: the field AND the name that lets a UI gate on
    // it. Pinned together on purpose — advertising the feature without the
    // field would over-promise, and shipping the field without the feature
    // would leave a panel sniffing for it, which is what these names replace.
    assert!(
        list["features"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f == "workflow-serial-roles-v1"),
        "features advertises the serial-roles contract: {list}"
    );
    let workflows = list["workflows"]
        .as_array()
        .expect("workflows body key intact");
    assert!(
        workflows.iter().any(|w| w["name"] == "demo"),
        "declared workflow surfaced: {list}"
    );
    let demo = workflows
        .iter()
        .find(|w| w["name"] == "demo")
        .expect("demo row");
    assert!(
        demo["serial_roles"].is_array(),
        "every row carries serial_roles (empty when no role is serial): {list}"
    );

    // Seed one recorded run under the isolated machine-global runs dir, then
    // assert runs_value reads it and rides the same envelope.
    let run_dir = home.join("runs").join("w-paritytest");
    fs::create_dir_all(&run_dir).unwrap();
    let events = [
        RunEvent::WorkflowStarted {
            ts: 100,
            workflow: "demo".into(),
            workflow_digest: "d".into(),
            grant_digest: "g".into(),
            args_digest: "a".into(),
            max_agents: 3,
            max_wall_seconds: 60,
        },
        RunEvent::WorkflowCompleted {
            ts: 130,
            outcome: "done".into(),
            exhausted: false,
            duration_ms: 30_000,
        },
    ];
    let jsonl: String = events
        .iter()
        .map(|e| serde_json::to_string(e).unwrap() + "\n")
        .collect();
    fs::write(run_dir.join("events.jsonl"), jsonl).unwrap();

    let runs = commands::workflow::runs_value(20).unwrap();
    assert_eq!(
        runs["schema_version"],
        agentstack::ui_contract::SCHEMA_VERSION
    );
    assert!(runs["features"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f == "workflow-observe-v1"));
    let run_rows = runs["runs"].as_array().expect("runs body key intact");
    let row = run_rows
        .iter()
        .find(|r| r["run"] == "w-paritytest")
        .expect("seeded run surfaced from the machine-global runs dir");
    assert_eq!(row["workflow"], "demo");
    assert_eq!(row["outcome"], "completed");

    std::env::remove_var("HOME");
    std::env::remove_var("AGENTSTACK_HOME");
}
