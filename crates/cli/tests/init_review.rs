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
        text.contains("Found 2 coding tools and their native configs"),
        "{text}"
    );
    assert!(text.contains("Claude Code"), "{text}");
    assert!(text.contains("~/.claude.json"), "{text}");
    assert!(text.contains("Codex CLI"), "{text}");
    assert!(text.contains("~/.codex/config.toml"), "{text}");

    // (2) Servers by name, with what each runs.
    assert!(text.contains("Importing 2 MCP servers"), "{text}");
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
    // The imported servers travel the LIVE lane, so `apply` will never write a
    // `.mcp.json` or a `.codex/config.toml` for them — and this block no longer
    // names those files. Promising a file that nothing writes is the
    // double-delivery defect stated as a plan.
    assert!(!text.contains("Claude Code \u{b7} MCP servers"), "{text}");
    assert!(!text.contains("Codex CLI \u{b7} MCP servers"), "{text}");
    // No bridge is registered in this scripted run, so the routing states the
    // live lane as a PLAN (invariant 8). The claim under test is that the row
    // names the live lane at all, not that it promises delivery.
    assert!(
        text.contains("MCP servers planned live (not connected)"),
        "{text}"
    );

    // The review preceded a real write.
    assert!(proj.join(".agentstack/agentstack.toml").exists());
    // ...and the import itself wrote no native config, as the block promised.
    assert!(!proj.join(".mcp.json").exists());
    assert!(!proj.join(".codex/config.toml").exists());
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

    // destinations[]: what the RENDERED lane will manage. The imported servers
    // route live on both of these CLIs, so no destination promises an MCP
    // server file — `apply` honours the delivery planner and would never write
    // one. A destination here is a promise, and this is the promise that used
    // to be broken.
    let dests = v["destinations"].as_array().unwrap();
    for d in dests {
        let writes: Vec<&str> = d["writes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w.as_str().unwrap())
            .collect();
        assert!(
            !writes.contains(&"MCP servers"),
            "no destination may promise MCP servers under live routing: {d}"
        );
    }

    // Planning wrote nothing.
    assert!(!proj.join(".agentstack").exists());
    assert!(!proj.join("agentstack.toml").exists());
}

/// A global config holding one server the user chose and two the ChatGPT and
/// Codex desktop applications installed and keep updated. The two commands are
/// copied from real entries on a machine with both apps present.
fn seed_tool_managed_fixture(home: &Path) {
    fs::write(
        home.join(".claude.json"),
        r#"{"mcpServers":{
            "github":{"command":"npx","args":["-y","github-mcp"]},
            "node_repl":{"command":"/Applications/ChatGPT.app/Contents/Resources/cua_node/bin/node_repl"},
            "computer-use":{"command":"sh","args":["-c","cd '.' && exec './Codex Computer Use.app/Contents/SharedSupport/SkyComputerUseClient.app/Contents/MacOS/SkyComputerUseClient' 'mcp'"]}
        }}"#,
    )
    .unwrap();
}

/// The default leaves another application's servers out of the import — and
/// says so. The absence is the easy half; the claim under test is that it is
/// NEVER silent. "Left alone" and "not found" are different statements, and a
/// run that printed neither would let a user read the first as the second.
#[test]
fn servers_another_app_owns_are_left_out_of_the_import_and_named() {
    let bin = env!("CARGO_BIN_EXE_agentstack");
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    seed_tool_managed_fixture(&home);

    let stub_bin = tmp.path().join("bin");
    fs::create_dir_all(&stub_bin).unwrap();
    write_stub(&stub_bin, "claude");

    let proj = tmp.path().join("proj");
    fs::create_dir_all(proj.join(".git")).unwrap();

    let (text, ok) = run(
        bin,
        &["init", "--dry-run", "--secrets", "skip"],
        &home,
        &proj,
        &stub_bin,
    );
    assert!(ok, "init --dry-run failed:\n{text}");

    // Only the user's own server is imported.
    assert!(text.contains("Importing 1 MCP server"), "{text}");
    assert!(text.contains("runs npx -y github-mcp"), "{text}");

    // And the two that were left out are NAMED, with who appears to own them,
    // the path that evidences it, and the flag that overrides the default.
    assert!(
        text.contains(
            "2 servers are managed by the apps that installed them and were left alone: \
             node_repl, computer-use"
        ),
        "the exclusion must be stated, not inferred:\n{text}"
    );
    assert!(text.contains("ChatGPT"), "{text}");
    assert!(text.contains("Codex Computer Use"), "{text}");
    assert!(
        text.contains("/Applications/ChatGPT.app/Contents/Resources/cua_node/bin/node_repl"),
        "the evidence behind the reading is shown:\n{text}"
    );
    assert!(text.contains("nothing was deleted"), "{text}");
    assert!(text.contains("--include-tool-managed"), "{text}");

    // `--dry-run` wrote nothing, as always.
    assert!(!proj.join(".agentstack/agentstack.toml").exists());
}

/// The same two facts as data, for the panel (`init-tool-managed-v1`): the
/// excluded servers are absent from `servers[]` AND present in
/// `tool_managed[]` with the owner, the evidence and the reason — so a UI can
/// render "left alone" instead of a gap or a duplicate. The opt-in flag flips
/// both halves.
#[test]
fn plan_json_names_the_servers_another_app_owns() {
    let bin = env!("CARGO_BIN_EXE_agentstack");
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    seed_tool_managed_fixture(&home);

    let stub_bin = tmp.path().join("bin");
    fs::create_dir_all(&stub_bin).unwrap();
    write_stub(&stub_bin, "claude");

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();

    let names = |v: &serde_json::Value, key: &str| -> Vec<String> {
        v[key]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["name"].as_str().unwrap().to_string())
            .collect()
    };

    let (text, ok) = run(bin, &["init", "--plan"], &home, &proj, &stub_bin);
    assert!(ok, "init --plan failed:\n{text}");
    let v: serde_json::Value = serde_json::from_str(&text).expect("plan is JSON");
    assert!(
        v["features"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f == "init-tool-managed-v1"),
        "the contract is advertised: {}",
        v["features"]
    );
    assert_eq!(names(&v, "servers"), vec!["github".to_string()]);

    let managed = v["tool_managed"].as_array().unwrap();
    assert_eq!(
        managed.len(),
        2,
        "one row per name, deduplicated: {managed:?}"
    );
    let node_repl = managed
        .iter()
        .find(|t| t["name"] == "node_repl")
        .expect("node_repl is named as left alone");
    assert_eq!(node_repl["application"], "ChatGPT");
    assert_eq!(
        node_repl["path"],
        "/Applications/ChatGPT.app/Contents/Resources/cua_node/bin/node_repl"
    );
    assert!(node_repl["reason"]
        .as_str()
        .unwrap()
        .contains("another application's bundle"));
    assert_eq!(node_repl["imported"], false);
    let computer_use = managed
        .iter()
        .find(|t| t["name"] == "computer-use")
        .expect("the shell-wrapped one is named too");
    assert_eq!(computer_use["application"], "Codex Computer Use");

    // The opt-in: a user who genuinely wants one gets it, and the row says so.
    let (text, ok) = run(
        bin,
        &["init", "--plan", "--include-tool-managed"],
        &home,
        &proj,
        &stub_bin,
    );
    assert!(ok, "init --plan --include-tool-managed failed:\n{text}");
    let v: serde_json::Value = serde_json::from_str(&text).expect("plan is JSON");
    let imported = names(&v, "servers");
    assert!(imported.contains(&"node_repl".to_string()), "{imported:?}");
    assert!(
        imported.contains(&"computer-use".to_string()),
        "{imported:?}"
    );
    assert!(
        v["tool_managed"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["imported"] == true),
        "opting in is stated too, never left to be inferred: {}",
        v["tool_managed"]
    );

    assert!(!proj.join(".agentstack").exists());
}

#[test]
fn init_imports_a_namespaced_server_name_into_the_library_unchanged() {
    let bin = env!("CARGO_BIN_EXE_agentstack");
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    fs::write(
        home.join(".claude.json"),
        r#"{"mcpServers":{
            "upstash/context7":{"type":"http","url":"https://mcp.context7.com/mcp"},
            "filesystem":{"command":"npx","args":["-y","@modelcontextprotocol/server-filesystem","."]}
        }}"#,
    )
    .unwrap();

    let stub_bin = tmp.path().join("bin");
    fs::create_dir_all(&stub_bin).unwrap();
    write_stub(&stub_bin, "claude");

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();

    let (text, ok) = run(bin, &["init", "--yes"], &home, &proj, &stub_bin);
    assert!(ok, "init failed after its pre-write review:\n{text}");

    let manifest_path = proj.join(".agentstack/agentstack.toml");
    let manifest_text = fs::read_to_string(&manifest_path).unwrap();
    let manifest: toml::Value = toml::from_str(&manifest_text).unwrap();
    // Both take the library-first path now: the namespaced name is encoded
    // into a file name rather than being kept inline, so the manifest holds
    // names only — the shape a first project should have.
    assert!(
        manifest
            .get("servers")
            .and_then(|s| s.get("upstash/context7"))
            .is_none(),
        "a namespaced server no longer needs an inline body:\n{manifest_text}"
    );
    assert!(
        manifest
            .get("servers")
            .and_then(|s| s.get("filesystem"))
            .is_none(),
        "the filename-safe server should use the normal library-first path:\n{manifest_text}"
    );

    let default_servers = manifest["toolsets"]["default"]["servers"]
        .as_array()
        .unwrap();
    assert!(
        default_servers
            .iter()
            .any(|name| name.as_str() == Some("upstash/context7")),
        "the default toolset activates it under its exact native name"
    );
    assert!(
        default_servers
            .iter()
            .any(|name| name.as_str() == Some("filesystem")),
        "the default toolset still activates the library server"
    );

    assert!(
        home.join(".agentstack/lib/servers/filesystem.toml")
            .exists(),
        "the safe server definition was imported to the library"
    );
    assert!(
        home.join(".agentstack/lib/servers/upstash%2Fcontext7.toml")
            .exists(),
        "the namespaced definition is stored under an encoded file name"
    );
    assert!(
        !home.join(".agentstack/lib/servers/upstash").exists(),
        "a namespaced identifier must never become a nested library path"
    );
}
