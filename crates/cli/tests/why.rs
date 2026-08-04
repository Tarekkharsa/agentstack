// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! `agentstack why <name>` — the provenance answer that has no substitute once
//! delivery is dynamic and nothing is written to disk.
//!
//! The binary is spawned rather than called in-process, with a cleared
//! environment and an isolated `HOME`. That is not ceremony: `why` reads the
//! trust store and the usage counters out of `AGENTSTACK_HOME`, and a
//! developer's real store would make every assertion here conditional on the
//! machine it ran on.

use std::fs;
use std::path::Path;
use std::process::Command;

use clap::{CommandFactory, Parser};

struct Out {
    text: String,
    ok: bool,
}

fn run(args: &[&str], home: &Path, proj: &Path) -> Out {
    let out = Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(args)
        .current_dir(proj)
        .env_clear()
        .env("HOME", home)
        .env("AGENTSTACK_HOME", home.join(".agentstack"))
        .env("PATH", "/usr/bin:/bin")
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("spawn agentstack");
    Out {
        text: format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
        ok: out.status.success(),
    }
}

/// A project declaring one HTTP server (a host + a secret) and one local skill.
fn fixture() -> (assert_fs::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let proj = tmp.path().join("proj");
    fs::create_dir_all(home.join(".agentstack")).unwrap();
    fs::create_dir_all(proj.join(".agentstack/skills/sql-review")).unwrap();
    fs::write(
        proj.join(".agentstack/agentstack.toml"),
        "version = 1\n\
         [targets]\ndefault = [\"claude-code\", \"codex\"]\n\
         [servers.github]\ntype = \"http\"\n\
         url = \"https://api.githubcopilot.com/mcp/\"\n\
         headers = { Authorization = \"Bearer ${GITHUB_TOKEN}\" }\n\
         [skills.sql-review]\npath = \"./skills/sql-review\"\n",
    )
    .unwrap();
    fs::write(
        proj.join(".agentstack/skills/sql-review/SKILL.md"),
        "---\ndescription: Review SQL migrations.\n---\n\nBody.\n",
    )
    .unwrap();
    (tmp, home, proj)
}

/// A project with one secret-free server, so `apply --write` really renders.
/// `claude-code` is made detectable so the bridge can be registered for it —
/// `bridge_registered` (the shared reading every surface uses) is false for an
/// undetected harness, which is exactly the un-connected state test 1 wants.
fn plain_fixture(
    detect_claude: bool,
) -> (assert_fs::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let proj = tmp.path().join("proj");
    fs::create_dir_all(home.join(".agentstack")).unwrap();
    fs::create_dir_all(proj.join(".agentstack")).unwrap();
    if detect_claude {
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::write(home.join(".claude.json"), "{}\n").unwrap();
    }
    fs::write(
        proj.join(".agentstack/agentstack.toml"),
        "version = 1\n\
         [targets]\ndefault = [\"claude-code\"]\n\
         [servers.demo]\ntype = \"http\"\n\
         url = \"https://demo.example.com/mcp/\"\n",
    )
    .unwrap();
    (tmp, home, proj)
}

/// Invariant 8, the exact breach this row was carrying: the plan routing a
/// server live says nothing about whether a harness can receive it. With no
/// bridge, `why` must not print a bare "live" — and it must say what `status`
/// says about the same harness in the same second.
#[test]
fn routed_live_with_no_bridge_is_never_a_bare_live() {
    let (_tmp, home, proj) = plain_fixture(false);
    let why = run(&["why", "demo"], &home, &proj).text;
    assert!(
        why.contains("planned live (not connected)"),
        "no bridge: the live row must be qualified, not asserted: {why}"
    );
    let status = run(&["status"], &home, &proj).text;
    assert!(
        status.contains("planned live (not connected)"),
        "the reference surface must agree: {status}"
    );
}

/// The other direction: with the bridge registered, the qualifier must go —
/// a connected harness really is served live, and hedging there would be just
/// as wrong as claiming delivery without a bridge.
#[test]
fn routed_live_with_the_bridge_registered_says_live_plainly() {
    let (_tmp, home, proj) = plain_fixture(true);
    let connect = run(
        &["x", "gateway", "connect", "--all", "--write"],
        &home,
        &proj,
    );
    assert!(connect.ok, "{}", connect.text);
    let why = run(&["why", "demo"], &home, &proj).text;
    assert!(
        !why.contains("not connected"),
        "the bridge is registered; the hedge must be gone: {why}"
    );
    assert!(why.contains("live      Claude Code"), "{why}");
}

/// A file really on disk from a previous render. The row must come from disk,
/// so the reference surfaces and `why` describe the same `.mcp.json`.
#[test]
fn a_rendered_file_on_disk_is_what_written_reports() {
    let (_tmp, home, proj) = plain_fixture(true);
    assert!(
        run(&["delivery", "render-locally", "--write"], &home, &proj).ok,
        "render locally must be settable"
    );
    let applied = run(&["apply", "--write"], &home, &proj);
    assert!(applied.ok, "{}", applied.text);
    assert!(proj.join(".mcp.json").exists(), "the render must land");
    let why = run(&["why", "demo"], &home, &proj).text;
    assert!(why.contains("written   Claude Code"), "{why}");
    assert!(why.contains("live      —"), "nothing is live here: {why}");
}

/// Both at once — the state the delivery change creates and the plan alone can
/// never describe: routed live now, with the earlier render still on disk.
#[test]
fn a_capability_routed_live_still_names_its_abandoned_file() {
    let (_tmp, home, proj) = plain_fixture(true);
    assert!(run(&["delivery", "render-locally", "--write"], &home, &proj).ok);
    assert!(run(&["apply", "--write"], &home, &proj).ok);
    // Back to automatic: servers go live, and the file stays where it is.
    assert!(
        run(
            &["delivery", "render-locally", "--off", "--write"],
            &home,
            &proj
        )
        .ok
    );
    assert!(proj.join(".mcp.json").exists(), "the file is still there");

    let why = run(&["why", "demo"], &home, &proj).text;
    assert!(
        why.contains("left over from an earlier render"),
        "an abandoned file is the most useful thing this row can say: {why}"
    );
    let out = run(&["why", "demo", "--json"], &home, &proj);
    let v: serde_json::Value = serde_json::from_str(&out.text).expect("valid JSON");
    assert_eq!(
        v["abandoned"].as_array().map(Vec::len),
        Some(1),
        "the panel gets the same fact: {v}"
    );
}

/// THE CLONE CASE. A teammate clones a repo with `.mcp.json` committed. This
/// machine has no state ledger entry for it, and AgentStack never wrote it —
/// but the harness reads it and spawns those servers all the same. A `why`
/// that answered from the ledger printed `written: —` beside that live file,
/// which is invariant 8. The `written` row is a DISK reading, so it names it.
#[test]
fn a_config_no_ledger_entry_knows_about_is_still_named_as_written() {
    let (_tmp, home, proj) = plain_fixture(true);
    // Committed by somebody else, on disk before AgentStack ever ran here.
    fs::write(
        proj.join(".mcp.json"),
        "{\n  \"mcpServers\": {\n    \"demo\": {\n      \"command\": \"demo-server\"\n    }\n  }\n}\n",
    )
    .unwrap();
    let why = run(&["why", "demo"], &home, &proj).text;
    assert!(
        !why.contains("written   —"),
        "a file the harness is reading cannot be reported as nothing written: {why}"
    );
    assert!(why.contains("written   Claude Code"), "{why}");
    let out = run(&["why", "demo", "--json"], &home, &proj);
    let v: serde_json::Value = serde_json::from_str(&out.text).expect("valid JSON");
    assert_eq!(
        v["abandoned"].as_array().map(Vec::len),
        Some(1),
        "servers are routed live here, so the file is abandoned: {v}"
    );
}

/// The other half of the disk-first rule: a project that never rendered must
/// name nothing. An over-reporting warning is as useless as a missing one.
#[test]
fn a_project_that_never_rendered_names_no_file() {
    let (_tmp, home, proj) = plain_fixture(true);
    assert!(!proj.join(".mcp.json").exists());
    let why = run(&["why", "demo"], &home, &proj).text;
    assert!(why.contains("written   —"), "nothing is on disk: {why}");
    let out = run(&["why", "demo", "--json"], &home, &proj);
    let v: serde_json::Value = serde_json::from_str(&out.text).expect("valid JSON");
    assert_eq!(v["abandoned"].as_array().map(Vec::len), Some(0), "{v}");
}

/// The seven rows, on a server, in the order the command promises them.
#[test]
fn why_answers_origin_pin_consent_and_lane_for_a_server() {
    let (_tmp, home, proj) = fixture();
    let out = run(&["why", "github"], &home, &proj);
    assert!(
        out.ok,
        "why must succeed on a declared server: {}",
        out.text
    );
    for row in [
        "from", "pinned", "approved", "live", "written", "scope", "used",
    ] {
        assert!(out.text.contains(row), "row '{row}' missing: {}", out.text);
    }
    assert!(out.text.contains("(MCP server)"), "{}", out.text);
    // The lane rows come from the delivery planner, not from a guess: with two
    // MCP-capable harnesses targeted and no override, servers are served live
    // and nothing is written.
    assert!(out.text.contains("Claude Code"), "{}", out.text);
    // Scope is read off the declaration — the host it contacts and the secret
    // it reads, never invented.
    assert!(out.text.contains("api.githubcopilot.com"), "{}", out.text);
    assert!(out.text.contains("GITHUB_TOKEN"), "{}", out.text);
}

#[test]
fn why_answers_for_a_skill_too() {
    let (_tmp, home, proj) = fixture();
    let out = run(&["why", "sql-review"], &home, &proj);
    assert!(out.ok, "{}", out.text);
    assert!(out.text.contains("(skill)"), "{}", out.text);
}

/// An untrusted project must read as untrusted HERE too. This is the row most
/// easily got wrong: the trust grant is keyed on the project BASE, while the
/// manifest lives one directory below it, so a `why` that consulted the
/// manifest directory would report "never trusted" for every ordinary project
/// and contradict `doctor`, `status` and `trust --preview` on the same facts.
#[test]
fn the_consent_row_agrees_with_the_trust_gate() {
    let (_tmp, home, proj) = fixture();
    let why = run(&["why", "github"], &home, &proj).text;
    let preview = run(&["trust", "--preview"], &home, &proj).text;
    let untrusted = preview.contains("not trusted") || preview.contains("never trusted");
    if untrusted {
        assert!(
            why.contains("not yet"),
            "trust --preview says untrusted; why must not say otherwise: {why}"
        );
    }
    // Whatever it says, the row may only offer a command that really exists.
    assert!(
        !why.contains("approved  yes") || !why.contains("agentstack trust"),
        "a granted project must not also be told to grant: {why}"
    );
}

/// The house error voice: name what was searched, say plainly what `why` takes,
/// and point at the command that lists what exists — which must itself parse
/// and be findable from `agentstack --help`.
#[test]
fn an_unknown_name_names_a_runnable_discoverable_command() {
    let (_tmp, home, proj) = fixture();
    let out = run(&["why", "create_issue"], &home, &proj);
    assert!(!out.ok, "an unknown name is an error: {}", out.text);
    assert!(out.text.contains("create_issue"), "{}", out.text);
    // No fabricated tool→server mapping, and it says so.
    assert!(
        out.text.contains("not a tool name"),
        "the refusal must say what it does NOT accept: {}",
        out.text
    );

    let visible: Vec<String> = agentstack::cli::Cli::command()
        .get_subcommands()
        .filter(|c| !c.is_hide_set())
        .map(|c| c.get_name().to_string())
        .collect();
    let listing = agentstack::cli::namespace_listing();
    for suggested in ["agentstack search create_issue", "agentstack lib list"] {
        assert!(
            out.text.contains(suggested),
            "the refusal must name `{suggested}`: {}",
            out.text
        );
        let argv: Vec<&str> = suggested.split_whitespace().collect();
        agentstack::cli::Cli::try_parse_from(&argv)
            .unwrap_or_else(|e| panic!("`{suggested}` must parse: {e}"));
        let verb = argv[1].to_string();
        assert!(
            visible.contains(&verb) || listing.contains(&verb),
            "`{verb}` is named by guidance but is neither visible on \
             `agentstack --help` nor listed by `agentstack x`"
        );
    }
}

/// `--json` carries the same facts as the text, inside the crate's standard
/// envelope — one collection, two renderings, so they cannot drift apart.
#[test]
fn json_carries_the_same_facts_in_the_standard_envelope() {
    let (_tmp, home, proj) = fixture();
    let out = run(&["why", "github", "--json"], &home, &proj);
    assert!(out.ok, "{}", out.text);
    let v: serde_json::Value = serde_json::from_str(&out.text).expect("valid JSON");
    assert!(v["schema_version"].is_number(), "envelope applied: {v}");
    assert_eq!(v["name"], "github");
    assert_eq!(v["kind"], "servers");
    for key in ["from", "pinned", "approved"] {
        assert!(v[key].is_string(), "{key} is a string: {v}");
    }
    for key in ["live", "written", "scope"] {
        assert!(v[key].is_array(), "{key} is an array: {v}");
    }
    assert!(v["activations"].is_number(), "{v}");
    let text = run(&["why", "github"], &home, &proj).text;
    assert!(
        text.contains(v["pinned"].as_str().unwrap()),
        "the two renderings must agree on the pin: {text} / {v}"
    );
}
