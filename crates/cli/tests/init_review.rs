// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Stage 1.2 witnesses: the import states its evidence BEFORE writing.
//!
//! 1. The first screen names which CLIs were found AND which native config
//!    files back that claim.
//! 2. The servers being imported are listed by name (secret references are
//!    covered by the lifted-token block, witnessed elsewhere).
//! 3. The destination files are stated in user terms — which CLI, which
//!    file, which scope — without adapter vocabulary.
//! 4. `init --plan` carries the same facts as data (`detected[].configs`,
//!    `destinations[]`) so t3code renders the identical review.
//!
//! Spawns the real binary in a sandboxed HOME (like the first-value demo), so
//! the claims are about what the terminal actually prints and the JSON the
//! panel actually decodes.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

fn write_stub(bin_dir: &Path, name: &str) {
    fs::write(bin_dir.join(name), "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(bin_dir.join(name), fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn run(bin: &str, args: &[&str], home: &Path, cwd: &Path, stub_bin: &Path) -> (String, bool) {
    let out = Command::new(bin)
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("HOME", home)
        .env("AGENTSTACK_HOME", home.join(".agentstack"))
        .env("PATH", format!("{}:/usr/bin:/bin", stub_bin.display()))
        .stdin(Stdio::null())
        .output()
        .expect("spawn agentstack");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (text, out.status.success())
}

/// Two real native configs (the first-value fixture shape): Claude Code knows
/// `github` (with an inline token), Codex knows `tldraw`.
fn seed_fixtures(home: &Path) {
    fs::write(
        home.join(".claude.json"),
        r#"{"mcpServers":{"github":{"command":"/usr/bin/env","args":["npx","-y","github-mcp"],"env":{"GITHUB_TOKEN":"ghp-fake-0000"}}}}"#,
    )
    .unwrap();
    fs::create_dir_all(home.join(".codex")).unwrap();
    fs::write(
        home.join(".codex/config.toml"),
        "[mcp_servers.tldraw]\ncommand = \"/usr/bin/env\"\nargs = [\"npx\", \"-y\", \"tldraw-mcp\"]\n",
    )
    .unwrap();
}

#[test]
fn scripted_init_states_clis_configs_servers_and_destinations_before_writing() {
    let bin = env!("CARGO_BIN_EXE_agentstack");
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    seed_fixtures(&home);

    let stub_bin = tmp.path().join("bin");
    fs::create_dir_all(&stub_bin).unwrap();
    write_stub(&stub_bin, "claude");
    write_stub(&stub_bin, "codex");

    let proj = tmp.path().join("proj");
    fs::create_dir_all(proj.join(".git")).unwrap();

    let (text, ok) = run(
        bin,
        &["init", "--yes", "--secrets", "skip"],
        &home,
        &proj,
        &stub_bin,
    );
    assert!(ok, "init failed:\n{text}");

    // (1) First screen: the CLIs found, each with the exact config files that
    // back the detection — displayed ~-compacted since they live under HOME.
    assert!(
        text.contains("Found 2 coding tool(s) and their native configs"),
        "{text}"
    );
    assert!(text.contains("Claude Code"), "{text}");
    assert!(text.contains("~/.claude.json"), "{text}");
    assert!(text.contains("Codex CLI"), "{text}");
    assert!(text.contains("~/.codex/config.toml"), "{text}");

    // (2) Servers by name, with what each runs.
    assert!(text.contains("Importing 2 MCP server(s)"), "{text}");
    assert!(text.contains("github"), "{text}");
    assert!(text.contains("tldraw"), "{text}");
    assert!(
        text.contains("runs /usr/bin/env npx -y github-mcp"),
        "{text}"
    );
    // The lifted secret reference is named (never its value).
    assert!(text.contains("${GITHUB_TOKEN}"), "{text}");
    assert!(
        !text.contains("ghp-fake-0000"),
        "the token value must never print:\n{text}"
    );

    // (3) Destinations in user terms: the manifest plus each CLI's native
    // file, scope spelled out — before the write happened.
    assert!(text.contains("Files agentstack will manage"), "{text}");
    assert!(text.contains(".agentstack/agentstack.toml"), "{text}");
    assert!(
        text.contains("the manifest — written by this import"),
        "{text}"
    );
    assert!(text.contains(".mcp.json"), "{text}");
    assert!(
        text.contains("Claude Code · MCP servers (this project)"),
        "{text}"
    );
    assert!(text.contains(".codex/config.toml"), "{text}");
    assert!(
        text.contains("Codex CLI · MCP servers (this project)"),
        "{text}"
    );

    // The review preceded a real write.
    assert!(proj.join(".agentstack/agentstack.toml").exists());
}

#[test]
fn plan_json_carries_configs_found_and_destinations() {
    let bin = env!("CARGO_BIN_EXE_agentstack");
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    seed_fixtures(&home);

    let stub_bin = tmp.path().join("bin");
    fs::create_dir_all(&stub_bin).unwrap();
    write_stub(&stub_bin, "claude");
    write_stub(&stub_bin, "codex");

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();

    let (text, ok) = run(bin, &["init", "--plan"], &home, &proj, &stub_bin);
    assert!(ok, "init --plan failed:\n{text}");
    let v: serde_json::Value = serde_json::from_str(&text).expect("plan is JSON");

    // detected[]: id/display plus the evidence — binary and config files.
    let detected = v["detected"].as_array().unwrap();
    let claude = detected
        .iter()
        .find(|d| d["id"] == "claude-code")
        .expect("claude-code detected");
    assert_eq!(claude["bin_on_path"], true);
    let configs: Vec<&str> = claude["configs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap())
        .collect();
    assert!(
        configs.iter().any(|c| c.ends_with(".claude.json")),
        "configs name the file detection read: {configs:?}"
    );

    // destinations[]: full path, plain scope, and what renders there.
    let dests = v["destinations"].as_array().unwrap();
    let claude_dest = dests
        .iter()
        .find(|d| d["id"] == "claude-code")
        .expect("claude-code destination");
    assert_eq!(claude_dest["scope"], "project");
    assert!(claude_dest["path"].as_str().unwrap().ends_with(".mcp.json"));
    assert_eq!(claude_dest["writes"][0], "MCP servers");
    let codex_dest = dests
        .iter()
        .find(|d| d["id"] == "codex")
        .expect("codex destination");
    assert!(codex_dest["path"]
        .as_str()
        .unwrap()
        .ends_with(".codex/config.toml"));

    // Planning wrote nothing.
    assert!(!proj.join(".agentstack").exists());
    assert!(!proj.join("agentstack.toml").exists());
}
