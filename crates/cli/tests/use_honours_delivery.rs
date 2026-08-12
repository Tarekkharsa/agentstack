// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! `use`, `session` and `diff` read the SAME delivery plan `apply` reads.
//!
//! The defect these tests pin: `apply` was routed through
//! [`agentstack::delivery::Plan`] while the other server-writing commands were
//! not, so `agentstack use <toolset> --write` still wrote `.mcp.json` while
//! `status`, `doctor`, `diff` and `apply` all reported that the servers were
//! served live and that nothing was on disk. That put a server in the harness
//! direct and unbrokered while four reading surfaces said it was not there —
//! a silent switch to the writing lane (`docs/design/automatic-delivery.md`
//! §"Failure semantics" 3) and a breach of invariant 8 on every one of those
//! surfaces.

use std::fs;
use std::path::Path;
use std::sync::Mutex;

use agentstack::cli::{DiffArgs, UseArgs};
use agentstack::commands::{diff, use_profile};
use agentstack::scope::Scope;

// These commands read the process-global HOME; serialize them.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn set_home(home: &Path) {
    fs::create_dir_all(home).unwrap();
    std::env::set_var("HOME", home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));
}

/// A project targeting one MCP-capable harness with one server and one skill.
/// `render_locally` decides which lane those capabilities travel.
fn project(root: &Path, render_locally: bool) -> std::path::PathBuf {
    let proj = root.join("proj");
    fs::create_dir_all(proj.join(".git")).unwrap();
    fs::create_dir_all(proj.join("skill-sources/demo")).unwrap();
    fs::write(proj.join("skill-sources/demo/SKILL.md"), "# Demo\n").unwrap();
    let delivery = if render_locally {
        "[delivery]\nrender_locally = true\n"
    } else {
        ""
    };
    fs::write(
        proj.join("agentstack.toml"),
        format!(
            "version = 1\n{delivery}[targets]\ndefault = [\"claude-code\"]\n\
             [skills.demo]\npath = \"./skill-sources/demo\"\n\
             [servers.demo]\ntype = \"stdio\"\ncommand = \"/bin/echo\"\nargs = [\"hi\"]\n\
             [profiles.p]\nservers = [\"demo\"]\nskills = [\"demo\"]\n"
        ),
    )
    .unwrap();
    proj
}

fn use_args() -> UseArgs {
    UseArgs {
        profile: Some("p".into()),
        targets: vec![],
        scope: Some(Scope::Project),
        write: true,
        allow_unresolved: false,
        prune_foreign: false,
        no_gitignore: false,
        list: false,
        json: false,
        quiet: false,
    }
}

/// Default routing: an MCP-capable harness's servers travel the live lane, so
/// activation writes no server config. This is the reviewer's reproduction.
#[test]
fn use_write_does_not_write_servers_under_default_routing() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    set_home(&tmp.path().join("home"));
    let proj = project(tmp.path(), false);

    use_profile::run(&use_args(), Some(&proj)).unwrap();

    assert!(
        !proj.join(".mcp.json").exists(),
        "activation must not write a server config for a live-routed harness"
    );
    assert!(
        !proj.join(".claude/skills").exists(),
        "activation must not create a skills directory for a live-routed harness"
    );
    // …and it must not hide a phantom file in the managed .gitignore block
    // either: a block naming a file nobody wrote is the same lie in a second
    // place (it is what kept `git status` quiet about the write).
    let gitignore = fs::read_to_string(proj.join(".gitignore")).unwrap_or_default();
    assert!(
        !gitignore.contains("/.mcp.json"),
        "no config was written, so none may be ignored: {gitignore}"
    );
    assert!(
        !gitignore.contains("/.claude/skills"),
        "no skills were written, so no skills directory may be ignored: {gitignore}"
    );
}

/// The override still writes. Routing is not a removal of the rendered lane.
#[test]
fn render_locally_makes_use_write_servers_again() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    set_home(&tmp.path().join("home"));
    let proj = project(tmp.path(), true);
    // Consent is not this test's subject: grant so the rendered lane's trust
    // gate (`render::apply::trust_refusal`) is out of the way.
    agentstack::trust::trust_unreviewed(&proj).unwrap();

    use_profile::run(&use_args(), Some(&proj)).unwrap();

    let cfg = fs::read_to_string(proj.join(".mcp.json")).expect("render locally writes the config");
    assert!(
        cfg.contains("demo"),
        "the selected server is written: {cfg}"
    );
    assert!(
        proj.join(".claude/skills/demo/SKILL.md").exists(),
        "render locally must still materialize the selected skill"
    );
}

/// Moving from rendered delivery back to the default live lane removes only
/// AgentStack-managed skill artifacts; users do not need to clean stale
/// folders by hand.
#[test]
fn switching_to_live_delivery_prunes_previously_rendered_skills() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    set_home(&tmp.path().join("home"));
    let proj = project(tmp.path(), true);
    agentstack::trust::trust_unreviewed(&proj).unwrap();
    use_profile::run(&use_args(), Some(&proj)).unwrap();
    assert!(proj.join(".claude/skills/demo").exists());

    let manifest_path = proj.join("agentstack.toml");
    let manifest = fs::read_to_string(&manifest_path)
        .unwrap()
        .replace("[delivery]\nrender_locally = true\n", "");
    fs::write(&manifest_path, manifest).unwrap();

    use_profile::run(&use_args(), Some(&proj)).unwrap();

    assert!(
        !proj.join(".claude/skills").exists(),
        "the empty AgentStack-managed skills parent should disappear"
    );
    let gitignore = fs::read_to_string(proj.join(".gitignore")).unwrap_or_default();
    assert!(
        !gitignore.contains("/.claude/skills"),
        "the removed rendered lane must also leave .gitignore"
    );
}

/// `session start` activates through `use_profile::activate`, so it inherits
/// the routing — and its undo snapshot must read the same plan, or the history
/// ledger records a target the session never touched.
#[test]
fn session_start_does_not_write_servers_under_default_routing() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    set_home(&tmp.path().join("home"));
    let proj = project(tmp.path(), false);
    agentstack::commands::lock::run(&Default::default(), Some(&proj)).unwrap();
    agentstack::trust::trust_unreviewed(&proj).unwrap();

    agentstack::session::start(Some(&proj), "p", Scope::Project).unwrap();

    assert!(
        !proj.join(".mcp.json").exists(),
        "a session must not write a server config for a live-routed harness"
    );
    assert!(
        !proj.join(".claude/skills").exists(),
        "a session must not write skills for a live-routed harness"
    );
    agentstack::session::end(Some(&proj)).unwrap();
}

/// `diff` may not claim "in sync" about a lane it did not compare. A
/// live-routed target is reported as not compared and carries no
/// `TargetOutcome` — `changed: false` is the wire form of "in sync", and a
/// structured consumer must not read that about a comparison that never
/// happened.
#[test]
fn diff_does_not_claim_sync_for_a_lane_it_did_not_compare() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    set_home(&tmp.path().join("home"));
    let proj = project(tmp.path(), false);

    let outcome = diff::report(
        &DiffArgs {
            targets: vec![],
            profile: None,
            scope: Some(Scope::Project),
            json: false,
        },
        Some(&proj),
    )
    .unwrap();

    assert!(
        outcome.targets.is_empty(),
        "a live-routed target is not compared, so it reports no drift verdict: {:?}",
        outcome
            .targets
            .iter()
            .map(|t| t.id.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(outcome.drifted, 0);
    assert!(
        outcome
            .warnings
            .iter()
            // The lane, not the verb: with no bridge registered the honest
            // wording is "planned live (not connected)", and pinning "served
            // live" here would pin the invariant-8 breach this round removed.
            .any(|w| {
                (w.contains("served live") || w.contains("planned live (not connected)"))
                    && w.contains("nothing rendered to compare")
            }),
        "the omission is named, not silent: {:?}",
        outcome.warnings
    );
}

/// The rendered lane keeps its verdict: with the override set, `diff` compares
/// the file and reports drift exactly as before.
#[test]
fn diff_still_compares_the_rendered_lane() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    set_home(&tmp.path().join("home"));
    let proj = project(tmp.path(), true);

    let outcome = diff::report(
        &DiffArgs {
            targets: vec![],
            profile: None,
            scope: Some(Scope::Project),
            json: false,
        },
        Some(&proj),
    )
    .unwrap();

    assert_eq!(outcome.targets.len(), 1, "the target IS compared");
    assert!(outcome.targets[0].changed, "nothing rendered yet → drift");
}
