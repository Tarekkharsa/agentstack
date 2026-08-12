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
            project_servers: false,
            include_tool_managed: false,
            yes: true,
            consented_plan: None,
            connect: false,
            verbose: false,
        },
        Some(&proj),
    )
    .unwrap();

    // The complete onboarding write set exists: manifest, .env with the
    // lifted value, and the .gitignore rule protecting it.
    let manifest = proj.join(".agentstack/agentstack.toml");
    assert!(manifest.exists());
    // Library-first import: the manifest references the server by name and the
    // `${REF}` lives in the library definition the import wrote.
    assert!(fs::read_to_string(&manifest).unwrap().contains("search"));
    let lib_def = tmp.path().join("home/.agentstack/lib/servers/search.toml");
    assert!(
        fs::read_to_string(&lib_def)
            .unwrap()
            .contains("${SEARCH_TOKEN}"),
        "the lifted reference landed in the library definition"
    );
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
            list: false,
            scope: None,
            write: true,
            json: false,
        },
        Some(&proj),
    )
    .unwrap();

    assert!(!manifest.exists(), "restore removed the imported manifest");
    assert!(
        !lib_def.exists(),
        "restore also reversed the library definitions the same init wrote"
    );
    assert!(!env_path.exists(), "restore removed the secrets .env");
    assert_eq!(
        fs::read_to_string(proj.join(".gitignore")).unwrap(),
        prior_gitignore,
        "restore returned .gitignore to its exact prior bytes"
    );

    std::env::remove_var("HOME");
    std::env::remove_var("AGENTSTACK_HOME");
}

// ─────────────────────────────────────────────────────────────────────────────
// Review finding 2: the reviewed surface an import records.
//
// The defect: `grant_trust_for_import` built its surface from
// `manifest.servers`, which library-first import leaves EMPTY — the definitions
// go to a linked library source and the default toolset references them by
// name. So the trust entry recorded zero server items while the consent digest
// blessed every one of them through the lock, and a later re-gate diffed
// against `[]`: every server could only read `+ added`, never `~ changed`.
// ─────────────────────────────────────────────────────────────────────────────

/// The recorded surface names every imported server, with the identity a human
/// was shown — the command line for stdio, the URL for http.
///
/// Reverting the fix (building the surface from `manifest.servers` again) fails
/// this on the first assertion: the recorded surface holds no server at all.
#[test]
fn a_library_first_import_records_every_imported_server_in_its_reviewed_surface() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    // Machine-global config only: a project-scope source withholds the grant
    // entirely (the `project_sourced` fence), which would make this vacuous.
    fs::write(
        home.join(".claude.json"),
        r#"{"mcpServers":{
             "search":{"command":"npx","args":["-y","search-mcp","--fast"]},
             "docs":{"type":"http","url":"https://docs.example/mcp"}
           }}"#,
    )
    .unwrap();

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    init::run(&library_first_args(), Some(&proj)).unwrap();

    // The manifest really is the library-first shape — otherwise this test
    // would pass for the wrong reason on a future default change.
    let loaded = agentstack::manifest::load_from_dir(&proj.join(".agentstack")).unwrap();
    assert!(
        loaded.manifest.servers.is_empty(),
        "library-first import leaves no inline [servers.*] — that is the whole \
         reason the old surface was empty"
    );

    let items = match agentstack::trust::prior_surface(&proj) {
        agentstack::trust::PriorSurface::Recorded(items) => items,
        other => panic!("the import must record a reviewed surface, got {other:?}"),
    };
    let identity = |name: &str| -> String {
        items
            .iter()
            .find(|i| i.kind == "server" && i.name == name)
            .unwrap_or_else(|| panic!("'{name}' is in the recorded surface: {items:?}"))
            .identity
            .clone()
    };
    assert_eq!(identity("search"), "npx -y search-mcp --fast");
    assert_eq!(identity("docs"), "https://docs.example/mcp");
}

/// The point of recording the surface: a changed command line reads as
/// `changed`, not `added`. Asserted through `trust::preview_value` — the real
/// re-gate card `agentstack trust` renders — so this is the classifier's own
/// verdict end to end, not a reimplementation of it.
///
/// Reverting the fix fails this on the `"changed"` assertion: with an empty
/// recorded baseline the card can only ever answer `"added"`.
#[test]
fn changing_a_library_servers_command_re_gates_as_changed_not_added() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));
    fs::write(
        home.join(".claude.json"),
        r#"{"mcpServers":{"search":{"command":"npx","args":["-y","search-mcp"]}}}"#,
    )
    .unwrap();

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    init::run(&library_first_args(), Some(&proj)).unwrap();

    // Someone edits the library definition's command line — the case the
    // re-gate diff exists to catch.
    let def = home.join(".agentstack/lib/servers/search.toml");
    let edited = fs::read_to_string(&def)
        .unwrap()
        .replace("search-mcp", "search-mcp-EVIL");
    fs::write(&def, &edited).unwrap();

    let change_for = |proj: &std::path::Path, name: &str| -> String {
        let card = agentstack::commands::trust::preview_value(proj).unwrap();
        card["review"]["items"]
            .as_array()
            .expect("the card carries reviewed items")
            .iter()
            .find(|i| i["kind"] == "server" && i["name"] == name)
            .unwrap_or_else(|| panic!("'{name}' is in the re-gate card: {card}"))["change"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };
    assert_eq!(
        change_for(&proj, "search"),
        "changed",
        "a command line that moved must read as CHANGED — reading `added` is \
         what an empty recorded surface produced"
    );

    // And putting the command back reads as unchanged, so the verdict above is
    // a real classification rather than everything reading `changed`.
    fs::write(&def, edited.replace("search-mcp-EVIL", "search-mcp")).unwrap();
    assert_eq!(change_for(&proj, "search"), "unchanged");
}

/// The args every library-first witness above shares: no `--project-servers`,
/// so the import takes the default path under test.
fn library_first_args() -> InitArgs {
    InitArgs {
        global: false,
        force: false,
        dry_run: false,
        plan: false,
        secrets: Some(SecretStore::Env),
        no_keychain: true,
        project_servers: false,
        include_tool_managed: false,
        yes: true,
        consented_plan: None,
        connect: false,
        verbose: false,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Review finding 3: the library is shared state.
//
// The defect: `init` called `lib::add_server_def` with `replace = true`, while
// every other caller makes the user pass `--replace`. A second project's import
// could rewrite `<lib>/servers/<name>.toml` and break a first project that had
// pinned the old digest — with the undo record living only in the second
// project.
// ─────────────────────────────────────────────────────────────────────────────

/// A differing definition is left ALONE without an explicit yes, and the
/// collision is named in the review.
///
/// Reverting the fix (calling `add_server_def` unconditionally again) fails
/// this on the "the library's own definition survives" assertion.
#[test]
fn init_leaves_a_differing_library_definition_untouched_without_a_yes() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    // A first project already put `search` in the library and pinned it.
    let lib_home = home.join(".agentstack/lib");
    let existing: agentstack::manifest::Server =
        toml::from_str("type = \"stdio\"\ncommand = \"npx\"\nargs = [\"-y\", \"search-mcp\"]\n")
            .unwrap();
    agentstack::commands::lib::add_server_def(
        &lib_home,
        "search",
        &existing,
        "test".into(),
        false,
        true,
    )
    .unwrap();
    let before = fs::read_to_string(lib_home.join("servers/search.toml")).unwrap();

    // A second project's machine config has the SAME NAME with a different
    // command line.
    fs::write(
        home.join(".claude.json"),
        r#"{"mcpServers":{"search":{"command":"npx","args":["-y","search-mcp-FORK"]}}}"#,
    )
    .unwrap();
    let proj = tmp.path().join("proj2");
    fs::create_dir_all(&proj).unwrap();
    init::run(&library_first_args(), Some(&proj)).unwrap();

    assert_eq!(
        fs::read_to_string(lib_home.join("servers/search.toml")).unwrap(),
        before,
        "the library's own definition survives an import that did not get a yes"
    );

    // And the project uses what the review said it would: the library's
    // version, not the imported one. The recorded reviewed surface is the
    // honest record of that (finding 2's fix makes it so automatically).
    let items = match agentstack::trust::prior_surface(&proj) {
        agentstack::trust::PriorSurface::Recorded(items) => items,
        other => panic!("expected a recorded surface, got {other:?}"),
    };
    let search = items
        .iter()
        .find(|i| i.kind == "server" && i.name == "search")
        .expect("the project still references 'search'");
    assert_eq!(
        search.identity, "npx -y search-mcp",
        "the surface names the definition that will actually run — the \
         library's, not the one this import wanted to write"
    );
}

/// Identical content is not a collision: re-importing the same machine config
/// into a second project asks nothing and changes nothing.
#[test]
fn an_identical_library_definition_is_not_a_collision() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));
    fs::write(
        home.join(".claude.json"),
        r#"{"mcpServers":{"search":{"command":"npx","args":["-y","search-mcp"]}}}"#,
    )
    .unwrap();

    let first = tmp.path().join("proj1");
    fs::create_dir_all(&first).unwrap();
    init::run(&library_first_args(), Some(&first)).unwrap();
    let lib_def = home.join(".agentstack/lib/servers/search.toml");
    let after_first = fs::read_to_string(&lib_def).unwrap();

    // The same config, a second project: no question, no change, and the
    // project still gets the server.
    let second = tmp.path().join("proj2");
    fs::create_dir_all(&second).unwrap();
    init::run(&library_first_args(), Some(&second)).unwrap();

    assert_eq!(fs::read_to_string(&lib_def).unwrap(), after_first);
    let items = match agentstack::trust::prior_surface(&second) {
        agentstack::trust::PriorSurface::Recorded(items) => items,
        other => panic!("expected a recorded surface, got {other:?}"),
    };
    assert!(
        items
            .iter()
            .any(|i| i.kind == "server" && i.name == "search"),
        "an identical definition is still imported, silently: {items:?}"
    );
}
