// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! v0.17.1 — the project-scope-only fixture, and the false-ready it used to
//! produce.
//!
//! The shape: a repo whose entire agent setup lives in project-scope native
//! configs (`.mcp.json`, `.codex/config.toml`) and nothing in the user's home.
//! The activation-study pilot (docs/design/activation-study.md §8.1, Run B)
//! found that this shape dead-ended silently — `status` reported "none detected
//! on this machine", `init` wrote an empty starter manifest, and `doctor` then
//! reported `0 error(s), 0 warning(s)` over it. `adopt` could read the files the
//! whole time; no surface ever said they existed.
//!
//! Each test below is one of that finding's witnesses.

use std::fs;
use std::path::Path;

use agentstack::cli::InitArgs;
use agentstack::commands::{doctor, init};

// These tests set HOME/AGENTSTACK_HOME and the process cwd, all of which are
// process-global; serialize the binary against itself.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn init_args() -> InitArgs {
    InitArgs {
        global: false,
        force: false,
        dry_run: false,
        plan: false,
        secrets: None,
        no_keychain: true,
        project_servers: false,
        include_tool_managed: false,
        yes: true,
        consented: None,
        connect: false,
        verbose: false,
    }
}

/// Library-first `init` puts the imported definitions in the first linked
/// library source and leaves the project referencing them by name. "Imported"
/// therefore means both halves: the manifest's default toolset names it, and
/// the library holds its definition.
fn imported(loaded: &agentstack::manifest::LoadedManifest, tmp: &Path, name: &str) -> bool {
    let referenced = loaded
        .manifest
        .profiles
        .values()
        .any(|p| p.servers.iter().any(|s| s == name));
    let defined = tmp
        .join("home/.agentstack/lib/servers")
        .join(format!("{name}.toml"))
        .exists();
    referenced && defined
}

/// A repo with servers ONLY in project-scope native configs, and an isolated,
/// empty HOME so nothing from the real machine can be mistaken for discovery.
fn project_scope_only_fixture(tmp: &Path) -> std::path::PathBuf {
    let home = tmp.join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    let proj = tmp.join("proj");
    fs::create_dir_all(proj.join(".codex")).unwrap();
    fs::write(
        proj.join(".mcp.json"),
        r#"{"mcpServers":{"filesystem":{"command":"npx","args":["-y","@modelcontextprotocol/server-filesystem","./"]}}}"#,
    )
    .unwrap();
    fs::write(
        proj.join(".codex/config.toml"),
        "[mcp_servers.sqlite]\ncommand = \"uvx\"\nargs = [\"mcp-server-sqlite\"]\n",
    )
    .unwrap();
    proj
}

/// Witness (a): the orientation reading NAMES what is configured here.
/// Before: `clis_detected` was empty — a machine-scope answer to a question
/// about this directory.
#[test]
fn status_sees_project_scope_configs() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project_scope_only_fixture(tmp.path());

    let registry = agentstack::adapter::Registry::load().unwrap();
    let detected: Vec<&str> = registry
        .iter()
        .filter(|d| d.detected_in(&proj))
        .map(|d| d.id.as_str())
        .collect();
    assert!(
        detected.contains(&"claude-code") && detected.contains(&"codex"),
        "both project-scope configs are detected for this directory: {detected:?}"
    );

    // And the servers behind them are named, with the manifest-coverage
    // question answered — this is what `status` prints and what routes it to
    // `adopt` instead of a dead end.
    let native = agentstack::discover::native_configs(&registry, &proj, &Default::default(), false);
    let found: Vec<&str> = native
        .iter()
        .flat_map(|n| n.unimported.iter().map(String::as_str))
        .collect();
    assert!(
        found.contains(&"filesystem") && found.contains(&"sqlite"),
        "both servers are reported as not-yet-covered: {found:?}"
    );
}

/// Witness (b): `init` does not silently write an empty manifest over a setup
/// that is sitting right there. It imports it.
#[test]
fn init_imports_project_scope_configs_instead_of_an_empty_manifest() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project_scope_only_fixture(tmp.path());

    init::run(&init_args(), Some(&proj)).unwrap();

    let loaded = agentstack::manifest::load_from_dir(&proj.join(".agentstack")).unwrap();
    assert!(
        imported(&loaded, tmp.path(), "filesystem"),
        "the .mcp.json server was imported into the library and referenced here"
    );
    assert!(
        imported(&loaded, tmp.path(), "sqlite"),
        "the .codex/config.toml server was imported into the library and referenced here"
    );
}

/// Witness (c): a manifest that does not cover what is configured here is NOT
/// reported as healthy. The finding names the file, the servers, and `adopt`.
///
/// This is the Status pillar: a clean doctor has to MEAN ready.
#[test]
fn doctor_refuses_to_call_an_uncovered_setup_clean() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project_scope_only_fixture(tmp.path());
    // A manifest that knows nothing about either native config — exactly the
    // empty starter the old `init` wrote here.
    fs::create_dir_all(proj.join(".agentstack")).unwrap();
    fs::write(proj.join(".agentstack/agentstack.toml"), "version = 1\n").unwrap();

    let report = doctor::collect(Some(&proj)).unwrap();

    assert!(
        report["warnings"].as_u64().unwrap() > 0,
        "an uncovered setup is not 0 warnings: {report}"
    );
    let section = report["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["title"] == "Unmanaged setup")
        .expect("the Unmanaged setup section is reported");
    assert_eq!(section["relevant"], true);
    let text = section["lines"].to_string();
    assert!(text.contains("filesystem"), "names the server: {text}");
    assert!(text.contains("sqlite"), "names the server: {text}");
    assert!(
        text.contains("agentstack adopt --write"),
        "names the one next action: {text}"
    );

    // One next action, and it is `adopt` — not the review.
    //
    // This assertion used to read `agentstack trust .`, on the general rule
    // that consent outranks warning-level repairs. That rule is right, and it
    // is still what both ladders do; it is just not the rule that governs THIS
    // state. The manifest here declares nothing, so the review has nothing to
    // review and `adopt` is the rung that sits above it — see the comment on
    // the `unimported_native` arm of `overview::next_step`, and the matching
    // `declares_anything` guard on `doctor`'s own consent rung.
    //
    // Measured in this exact fixture, which is why the old expectation was
    // wrong rather than merely different:
    //
    // - `agentstack trust .` prints "nothing — this project declares no
    //   capabilities yet" and "(no servers)". The grant buys nothing.
    // - Granting it anyway leaves doctor's next action on the adopt rung
    //   regardless, so the review is a detour, not a step.
    // - `agentstack adopt --write` then rewrites the manifest a grant binds
    //   itself to, and trust drops to `drifted` — a SECOND review, which is the
    //   precise cost the rung order exists to avoid.
    // - Taken in the order named here the ladder converges and never repeats:
    //   `adopt` → `adopt --write` → `lock --write` → `trust .`.
    //
    // What the test's NAME promises is unchanged and asserted above: an
    // uncovered setup is not 0 warnings, the section is relevant, and it names
    // both servers and the command that absorbs them.
    assert_eq!(report["trust"], "untrusted", "{report}");
    // …and the WRITING form of that rung (G35). This assertion read
    // `agentstack adopt` until the rung was corrected, which encoded the very
    // defect the correction removed: `adopt` PREVIEWS by default. Measured in
    // this exact fixture, with the built binary:
    //
    // - `agentstack adopt` exits 0, prints the manifest diff it WOULD apply,
    //   and ends with "Dry run. Re-run with --write". Every file in the fixture
    //   — manifest and HOME included — is byte-identical afterwards.
    // - `doctor` therefore reports the identical 4 warnings and hands back the
    //   identical rung on the next poll. Nothing in the output says it failed,
    //   which is what makes the exit-0 shape the worse one: a driver that runs
    //   the field verbatim loops forever. This is the same fault the fence
    //   first caught on `agentstack search`.
    // - `agentstack adopt --write` declares both servers in the manifest
    //   ("✓ adopted 2 servers"), the Unmanaged setup section goes irrelevant,
    //   warnings drop 4 → 2, and the rung advances to `agentstack lock
    //   --write`. That is progress, so that is what a machine field may name.
    //
    // The rung is one shared constant, `overview::ADOPT_RUNG_FIX`, so `status`
    // and `doctor` cannot disagree about it.
    assert_eq!(
        report["next_action"], "agentstack adopt --write",
        "{report}"
    );
}

/// The complement, so the check cannot become permanent noise: once the
/// manifest covers what is on disk, the section is quiet and tagged
/// irrelevant (hidden from the default report).
#[test]
fn doctor_is_quiet_once_the_manifest_covers_the_configs() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project_scope_only_fixture(tmp.path());

    init::run(&init_args(), Some(&proj)).unwrap();
    let report = doctor::collect(Some(&proj)).unwrap();

    let section = report["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["title"] == "Unmanaged setup")
        .expect("the section still exists — checks run, nothing is skipped");
    assert_eq!(
        section["relevant"], false,
        "nothing uncovered → hidden from the default report: {section}"
    );
}

/// F7 witness (FINDINGS.md, rc.1 review): `init --yes` over a repo-supplied
/// project config imports it but never self-trusts it. The tampered field is
/// the one that moved — the GRANT, not the import: the `.mcp.json` stdio
/// command lines arrived with the clone, and a documented promptless command
/// must not bless them without the ordinary review. The import still works
/// (witness (b) above); the project simply meets `agentstack trust .`.
#[test]
fn init_over_repo_supplied_config_imports_but_never_self_trusts() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project_scope_only_fixture(tmp.path());

    init::run(&init_args(), Some(&proj)).unwrap();

    // Imported — the convenience is intact…
    let loaded = agentstack::manifest::load_from_dir(&proj.join(".agentstack")).unwrap();
    assert!(imported(&loaded, tmp.path(), "filesystem"));
    // …but NOT granted: repo-supplied bytes take the gated review.
    assert_eq!(
        agentstack::trust::check(&proj),
        agentstack::trust::TrustState::Untrusted,
        "init --yes self-trusted a manifest holding repo-supplied server commands"
    );
}

/// The H1 boundary from the other side: an import built purely from the
/// user's own machine-global configs still records trust, so a newcomer's
/// first run does not end at the trust gate in their own repo. This is the
/// counter-witness that keeps the F7 fix from quietly widening into "init
/// never grants".
///
/// Since H5 the scripted route earns that grant with a REVIEWED PLAN
/// (`init --plan` → `--consented <plan_digest>`) rather than with `--yes`
/// alone; `tests/red_team_agent_self_consent.rs` holds the other half, that a
/// bare `--yes` imports and leaves the project untrusted.
#[test]
fn init_over_machine_global_config_still_grants_on_a_reviewed_plan() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));
    fs::write(
        home.join(".claude.json"),
        r#"{"mcpServers":{"filesystem":{"command":"npx","args":["-y","@modelcontextprotocol/server-filesystem","./"]}}}"#,
    )
    .unwrap();
    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();

    // The scripted route that grants: emit the plan, review it outside this
    // process, hand its digest back. Through the production `plan_json`, so a
    // digest this test computed differently could not paper over a drift.
    let mut args = init_args();
    let plan = init::plan_json(&args, Some(&proj)).unwrap();
    args.consented = Some(plan["plan_digest"].as_str().unwrap().to_string());
    init::run(&args, Some(&proj)).unwrap();

    let loaded = agentstack::manifest::load_from_dir(&proj.join(".agentstack")).unwrap();
    assert!(
        imported(&loaded, tmp.path(), "filesystem"),
        "the machine-global server was imported"
    );
    assert_eq!(
        agentstack::trust::check(&proj),
        agentstack::trust::TrustState::Trusted,
        "an import from the user's own machine configuration lost its H1 grant"
    );
}
