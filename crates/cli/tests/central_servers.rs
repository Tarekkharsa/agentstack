// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Phase 1b: `apply` (and the shared render path used by `diff`/`session`)
//! resolves profile server refs from the central library, renders them into a
//! provider config, and resolves `${REF}` per-machine at render time. Inline
//! `[servers.<name>]` still wins over the library. Serialized because these
//! mutate the process-global `HOME`/`AGENTSTACK_HOME` and secret env vars.

use std::fs;
use std::sync::Mutex;

use agentstack::cli::ApplyArgs;
use agentstack::commands::{apply, explain::explain_text};
use agentstack::library::{Library, LibraryServer};
use agentstack::scope::Scope;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn apply_args(profile: &str) -> ApplyArgs {
    ApplyArgs {
        targets: vec!["claude-code".into()],
        profile: Some(profile.into()),
        dry_run: false,
        write: true,
        scope: Some(Scope::Global),
        allow_unresolved: false,
        prune_foreign: false,
        no_gitignore: true,
    }
}

/// Install a central-library server `kibana` pointing at `url`, with a `${REF}`
/// header, under `<home>/.agentstack/lib`.
fn install_library_server(home: &std::path::Path, url: &str) {
    let lib_home = home.join(".agentstack/lib");
    fs::create_dir_all(lib_home.join("servers")).unwrap();
    fs::write(
        lib_home.join("servers/kibana.toml"),
        format!("type = \"http\"\nurl = \"{url}\"\n\n[headers]\nAuthorization = \"Bearer ${{KIBANA_TOKEN}}\"\n"),
    )
    .unwrap();
    let mut lib = Library::default();
    lib.upsert_server(LibraryServer {
        name: "kibana".into(),
        checksum: None,
        version: None,
        provenance: Some("consolidated:codex".into()),
    });
    lib.save(&lib_home).unwrap();
}

#[test]
fn apply_renders_library_server_ref() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));
    std::env::set_var("KIBANA_TOKEN", "secret-value");

    install_library_server(&home, "https://central-kibana/mcp");

    // Project references the server only by name — no inline `[servers.*]`.
    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [profiles.design]\nservers = [\"kibana\"]\n",
    )
    .unwrap();

    apply::run(&apply_args("design"), Some(&proj)).unwrap();

    let cfg = fs::read_to_string(home.join(".claude.json")).unwrap();
    assert!(cfg.contains("kibana"), "library server rendered: {cfg}");
    assert!(cfg.contains("central-kibana"), "library definition used");
    // ${REF} resolved at render time, not stored anywhere earlier.
    assert!(
        cfg.contains("secret-value"),
        "secret resolved during render"
    );
    assert!(
        !cfg.contains("${KIBANA_TOKEN}"),
        "no unresolved ref written"
    );

    std::env::remove_var("KIBANA_TOKEN");
    std::env::remove_var("AGENTSTACK_HOME");
    std::env::remove_var("HOME");
}

#[test]
fn inline_server_overrides_library_in_apply() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));
    std::env::set_var("KIBANA_TOKEN", "secret-value");

    install_library_server(&home, "https://central-kibana/mcp");

    // Same name defined inline — the inline definition must win.
    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [servers.kibana]\ntype = \"http\"\nurl = \"https://inline-kibana/mcp\"\n\
         headers = { Authorization = \"Bearer ${KIBANA_TOKEN}\" }\n\
         [profiles.design]\nservers = [\"kibana\"]\n",
    )
    .unwrap();

    apply::run(&apply_args("design"), Some(&proj)).unwrap();

    let cfg = fs::read_to_string(home.join(".claude.json")).unwrap();
    assert!(
        cfg.contains("inline-kibana"),
        "inline definition wins: {cfg}"
    );
    assert!(
        !cfg.contains("central-kibana"),
        "library copy shadowed by inline"
    );

    std::env::remove_var("KIBANA_TOKEN");
    std::env::remove_var("AGENTSTACK_HOME");
    std::env::remove_var("HOME");
}

#[test]
fn explain_reports_library_server_origin_and_lock() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    install_library_server(&home, "https://central-kibana/mcp");

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("agentstack.toml"), "version = 1\n").unwrap();

    // A library-only server explains: origin, provenance, and (unlocked) status.
    let out = explain_text("kibana", Some(&proj)).unwrap();
    assert!(out.contains("MCP server"));
    assert!(out.contains("central library"), "names its origin: {out}");
    assert!(out.contains("consolidated:codex"), "shows provenance");
    assert!(out.contains("not locked"), "shows lockfile status");
    // The ${REF} stays a placeholder in explain output — never a resolved value.
    assert!(out.contains("${KIBANA_TOKEN}"));

    std::env::remove_var("AGENTSTACK_HOME");
    std::env::remove_var("HOME");
}
