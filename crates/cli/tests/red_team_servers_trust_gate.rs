// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! RED TEAM — a repository ships a `[servers.*]` entry, and nobody has said yes.
//!
//! A stdio server entry *is* a command line. Once it is in a harness's native
//! MCP config, the harness spawns that process itself, at startup, in the user's
//! own session — outside agentstack, so no gateway brokers it, no egress fence
//! sees it and no sandbox holds it (`docs/ARCHITECTURE.md`, Layer 1). The
//! launch-time gates agentstack does own (`session start`, the protected `run`,
//! the MCP auto-project gate) never run, because agentstack was never the one
//! launching. Writing that file is therefore delivery of executable content, and
//! STRATEGY.md gives executable content the full consent ceremony.
//!
//! Two states must deliver ZERO server bytes:
//!
//!   * **untrusted** — the project was never reviewed on this machine;
//!   * **changed** — it WAS reviewed, and the manifest changed since. The
//!     sharper of the two: consent is real but stale, so a server appended
//!     after the yes (a rogue commit, a merge, a `--depth 1` update) would
//!     otherwise ride in on the earlier review.
//!
//! Both are checked at BOTH destinations, for the reason the hooks witness
//! gives: `apply --scope global --write` puts a repository's server command
//! line into `~/.claude.json`, where the harness launches it in every project
//! the user opens, not just the hostile one.
//!
//! Each refusal has its own negative control immediately after it — the same
//! project, the same command, after a real grant. Without those, every
//! assertion here would also pass against a renderer that simply never writes
//! servers, which is a broken feature rather than a gate.
//!
//! The last two tests guard the other direction: pruning what agentstack
//! already owns is the inert direction and must keep working untrusted, and the
//! machine's OWN manifest (`$AGENTSTACK_HOME/agentstack.toml`) is the personal
//! layer, deliberately undiscoverable as a project and therefore untrustable —
//! gating it would make machine-level servers permanently unrenderable.

use std::fs;
use std::path::{Path, PathBuf};

/// The server command line. Its presence anywhere in a native MCP config is the
/// proof that untrusted content was delivered.
const EVIL_ARG: &str = "curl-evil-dot-example-slash-rootkit";
/// A second command line, appended AFTER a grant, for the changed-trust case.
const APPENDED_ARG: &str = "curl-evil-dot-example-slash-second-stage";

fn run(args: &[&str], home: &Path, cwd: &Path) -> (String, bool) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("HOME", home)
        .env("AGENTSTACK_HOME", home.join(".agentstack"))
        .env("PATH", "/usr/bin:/bin")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("spawn agentstack");
    (
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
        out.status.success(),
    )
}

fn json(args: &[&str], home: &Path, proj: &Path) -> serde_json::Value {
    let (text, _ok) = run(args, home, proj);
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{args:?} is not JSON ({e}):\n{text}"))
}

/// `render_locally` is load-bearing, not decoration: under the default routing
/// an MCP-capable harness's servers travel the live lane and `apply` writes no
/// config at all, which would make every assertion below pass for the wrong
/// reason. This fixture is the configuration in which files really are written.
fn manifest_with(servers: &str) -> String {
    format!(
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [delivery]\nrender_locally = true\n{servers}"
    )
}

fn server_block(name: &str, arg: &str) -> String {
    format!(
        "\n[servers.{name}]\ntype = \"stdio\"\ncommand = \"/bin/sh\"\nargs = [\"-c\", \"{arg}\"]\n"
    )
}

/// A hostile checkout: a manifest declaring one stdio server, pinned exactly as
/// an attacker would commit it. Pinning is not consent — shipping the lockfile
/// is what keeps the refusals below firing on "untrusted" rather than
/// "unpinned", which would prove nothing about the trust gate.
fn hostile_project(tmp: &Path) -> (PathBuf, PathBuf) {
    let home = tmp.join("home");
    let proj = tmp.join("proj");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(proj.join(".agentstack")).unwrap();
    fs::write(
        proj.join(".agentstack/agentstack.toml"),
        manifest_with(&server_block("evil", EVIL_ARG)),
    )
    .unwrap();
    let (text, ok) = run(&["lock", "--write"], &home, &proj);
    assert!(ok, "lock failed:\n{text}");
    (home, proj)
}

/// The two-step a panel drives: review the surface, then bind the yes to the
/// digest of exactly those bytes.
fn grant(home: &Path, proj: &Path) {
    let (text, ok) = run(&["lock", "--write"], home, proj);
    assert!(ok, "lock failed:\n{text}");
    let digest = json(&["trust", "--preview"], home, proj)["surface_digest"]
        .as_str()
        .expect("preview must carry a surface digest")
        .to_string();
    let (text, ok) = run(&["trust", "--yes", "--consented", &digest], home, proj);
    assert!(ok, "grant failed:\n{text}");
}

/// Every byte a harness would read at this scope. Absent file = empty string:
/// "we never created it" and "we created it without the server" are both
/// passes, and only the command line's presence is a failure.
fn config_at(home: &Path, proj: &Path, scope: &str) -> String {
    let path = match scope {
        "global" => home.join(".claude.json"),
        _ => proj.join(".mcp.json"),
    };
    fs::read_to_string(path).unwrap_or_default()
}

/// The refusal must be legible and actionable, not a silent skip.
fn assert_refused(text: &str, ok: bool, scope: &str) {
    assert!(
        !ok,
        "apply --write --scope {scope} exited 0 on a gated project — a script \
         cannot tell this from success:\n{text}"
    );
    let lower = text.to_lowercase();
    assert!(
        lower.contains("refusing to render mcp servers"),
        "the refusal must name what it refused ({scope}):\n{text}"
    );
    assert!(
        text.contains("agentstack trust"),
        "the refusal must name the command that answers it ({scope}):\n{text}"
    );
}

// ---------------------------------------------------------------- untrusted

#[test]
fn an_untrusted_project_renders_no_server_bytes_at_either_scope() {
    for scope in ["project", "global"] {
        let tmp = tempfile::tempdir().unwrap();
        let (home, proj) = hostile_project(tmp.path());

        let (text, ok) = run(&["apply", "--write", "--scope", scope], &home, &proj);
        assert_refused(&text, ok, scope);
        assert!(
            !config_at(&home, &proj, scope).contains(EVIL_ARG),
            "an untrusted project's server command was written at {scope} scope:\n{}",
            config_at(&home, &proj, scope)
        );
    }
}

/// `--allow-unresolved` forgives a missing secret. It is not a consent
/// override, and a flag that quietly became one would be the whole gate.
#[test]
fn allow_unresolved_does_not_forgive_a_missing_consent() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = hostile_project(tmp.path());

    let (text, ok) = run(&["apply", "--write", "--allow-unresolved"], &home, &proj);
    assert_refused(&text, ok, "project");
    assert!(
        !config_at(&home, &proj, "project").contains(EVIL_ARG),
        "--allow-unresolved delivered an unconsented server:\n{}",
        config_at(&home, &proj, "project")
    );
}

/// The control for the cases above: the same project, the same command, after a
/// real grant. If the server does not land here, the refusals prove nothing.
#[test]
fn the_same_server_lands_once_the_project_is_trusted() {
    for scope in ["project", "global"] {
        let tmp = tempfile::tempdir().unwrap();
        let (home, proj) = hostile_project(tmp.path());
        grant(&home, &proj);

        let (text, ok) = run(&["apply", "--write", "--scope", scope], &home, &proj);
        assert!(ok, "trusted apply failed at {scope} scope:\n{text}");
        assert!(
            config_at(&home, &proj, scope).contains(EVIL_ARG),
            "a trusted project's server never rendered at {scope} scope — the \
             witness above would pass against a renderer that does nothing:\n{}",
            config_at(&home, &proj, scope)
        );
    }
}

// ------------------------------------------------------------------ changed

/// Consent was given, then the manifest changed. The reviewed server is already
/// on disk; the APPENDED one must not join it, at either scope.
#[test]
fn a_server_appended_after_the_grant_renders_no_bytes_at_either_scope() {
    for scope in ["project", "global"] {
        let tmp = tempfile::tempdir().unwrap();
        let (home, proj) = hostile_project(tmp.path());
        grant(&home, &proj);
        let (text, ok) = run(&["apply", "--write", "--scope", scope], &home, &proj);
        assert!(ok, "the reviewed apply must succeed first:\n{text}");
        assert!(
            config_at(&home, &proj, scope).contains(EVIL_ARG),
            "fixture: the reviewed server must be delivered before drift is tested"
        );

        // The rogue edit. Trust is now Changed: real, but stale.
        let mut toml = manifest_with(&server_block("evil", EVIL_ARG));
        toml.push_str(&server_block("second-stage", APPENDED_ARG));
        fs::write(proj.join(".agentstack/agentstack.toml"), &toml).unwrap();

        let (text, ok) = run(&["apply", "--write", "--scope", scope], &home, &proj);
        assert_refused(&text, ok, scope);
        let config = config_at(&home, &proj, scope);
        assert!(
            !config.contains(APPENDED_ARG),
            "a server appended after the grant was delivered on stale consent at \
             {scope} scope:\n{config}"
        );
        // The refusal leaves the file alone rather than rewriting it, so what
        // the human DID review stays exactly as they approved it.
        assert!(
            config.contains(EVIL_ARG),
            "the refusal must not disturb the already-reviewed server:\n{config}"
        );
    }
}

/// The control for the case above: re-reviewing the edited manifest delivers
/// the appended server. "Changed" must be a pending review, not a dead end.
#[test]
fn the_appended_server_lands_once_the_change_is_re_reviewed() {
    for scope in ["project", "global"] {
        let tmp = tempfile::tempdir().unwrap();
        let (home, proj) = hostile_project(tmp.path());
        grant(&home, &proj);

        let mut toml = manifest_with(&server_block("evil", EVIL_ARG));
        toml.push_str(&server_block("second-stage", APPENDED_ARG));
        fs::write(proj.join(".agentstack/agentstack.toml"), &toml).unwrap();
        grant(&home, &proj); // the human reviews the change and says yes again

        let (text, ok) = run(&["apply", "--write", "--scope", scope], &home, &proj);
        assert!(ok, "re-trusted apply failed at {scope} scope:\n{text}");
        assert!(
            config_at(&home, &proj, scope).contains(APPENDED_ARG),
            "a re-reviewed server never rendered at {scope} scope:\n{}",
            config_at(&home, &proj, scope)
        );
    }
}

// ------------------------------------------------------- the other direction

/// Taking bytes we already own back OFF disk is the inert direction: it removes
/// capability rather than adding it. A project whose consent went stale must
/// still be able to un-render, or the gate would trap its own artifacts on disk
/// with no command that clears them.
#[test]
fn pruning_a_server_we_already_own_still_works_untrusted() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = hostile_project(tmp.path());
    grant(&home, &proj);
    let (text, ok) = run(&["apply", "--write"], &home, &proj);
    assert!(ok, "the reviewed apply must succeed first:\n{text}");
    assert!(
        config_at(&home, &proj, "project").contains(EVIL_ARG),
        "fixture: the server must be on disk before the prune is tested"
    );

    // The declaration is withdrawn. Trust is now Changed — and the prune must
    // still happen.
    fs::write(proj.join(".agentstack/agentstack.toml"), manifest_with("")).unwrap();
    let (text, _ok) = run(&["apply", "--write"], &home, &proj);
    assert!(
        !config_at(&home, &proj, "project").contains(EVIL_ARG),
        "an untrusted project could not remove what agentstack had already \
         written for it:\n{text}\n{}",
        config_at(&home, &proj, "project")
    );
}

/// The machine's own manifest is the personal layer, not a project: the
/// zero-files bridge refuses to discover it, so no `trust` command can ever
/// reach it. Gating it on trust would make machine-level servers permanently
/// undeliverable — a gate nobody can satisfy is a broken feature, not a
/// stronger one — so the exemption is deliberate and witnessed here.
#[test]
fn the_machine_manifests_own_servers_are_not_gated_on_a_project_it_has_no_way_to_trust() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let machine = home.join(".agentstack");
    fs::create_dir_all(&machine).unwrap();
    fs::write(
        machine.join("agentstack.toml"),
        manifest_with(&server_block("mine", EVIL_ARG)),
    )
    .unwrap();

    let dir = machine.display().to_string();
    let (text, ok) = run(
        &["--manifest-dir", &dir, "apply", "--write"],
        &home,
        tmp.path(),
    );
    assert!(ok, "the machine manifest's own apply failed:\n{text}");
    assert!(
        fs::read_to_string(home.join(".claude.json"))
            .unwrap_or_default()
            .contains(EVIL_ARG),
        "the machine layer's own server was gated on a project that cannot exist:\n{text}"
    );
}
