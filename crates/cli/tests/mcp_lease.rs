// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! MCP profile lease protocol smoke test: exercise one real stdio process so
//! request-to-request state and gateway replacement cannot regress unnoticed.

use std::io::Write;
use std::process::{Command, Stdio};

use assert_fs::prelude::*;
use serde_json::Value;

#[test]
fn lease_lives_for_one_stdio_process_and_writes_no_native_artifacts() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.child("home");
    home.create_dir_all().unwrap();
    let project = tmp.child("project");
    project
        .child(".agentstack/agentstack.toml")
        .write_str("version = 1\n[profiles.backend]\nservers = []\nskills = []\n")
        .unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(["mcp", "--manifest-dir"])
        .arg(project.path())
        .env("AGENTSTACK_HOME", home.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let mut stdin = child.stdin.take().unwrap();
    for request in [
        serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }),
        serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "agentstack_lease_open", "arguments": { "profile": "backend" } } }),
        serde_json::json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "agentstack_lease_status", "arguments": {} } }),
        serde_json::json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": { "name": "agentstack_lease_close", "arguments": {} } }),
    ] {
        writeln!(stdin, "{request}").unwrap();
    }
    drop(stdin); // EOF is the implicit final lease cleanup.

    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let responses: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(responses.len(), 4, "one response per request");
    assert_eq!(responses[0]["id"], 1);
    let opened = responses[1]["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(opened.contains("\"opened\": \"backend\""));
    assert!(opened.contains("\"native_files_written\": false"));
    let status = responses[2]["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(status.contains("\"active\": true"));
    assert!(status.contains("\"profile\": \"backend\""));
    let closed = responses[3]["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(closed.contains("\"closed\": \"backend\""));
    assert!(closed.contains("\"native_restore_needed\": false"));

    assert!(!home.child("sessions.json").path().exists());
    assert!(!project.child(".mcp.json").path().exists());
    assert!(!project.child(".claude/skills").path().exists());
}

/// `agentstack_list_loadable` honors an optional `query`: it filters the listing
/// by skill name + description (case-insensitive substring), and returns a
/// graceful "no match" note when nothing matches.
#[test]
fn list_loadable_filters_by_query() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.child("home");
    home.create_dir_all().unwrap();

    // A central library with one path skill whose SKILL.md description carries a
    // unique word.
    home.child("lib/library.toml")
        .write_str("version = 1\n\n[[skill]]\nname = \"quokka-lint\"\nsource = \"path\"\npath = \"quokka-lint\"\n")
        .unwrap();
    home.child("lib/skills/quokka-lint/SKILL.md")
        .write_str(
            "---\nname: quokka-lint\ndescription: Guards against zzuniqueword drift.\n---\nbody\n",
        )
        .unwrap();

    let project = tmp.child("project");
    project
        .child(".agentstack/agentstack.toml")
        .write_str("version = 1\n")
        .unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(["mcp", "--manifest-dir"])
        .arg(project.path())
        .env("AGENTSTACK_HOME", home.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let mut stdin = child.stdin.take().unwrap();
    for request in [
        serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }),
        // Match on the unique description word.
        serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "agentstack_list_loadable", "arguments": { "query": "zzuniqueword" } } }),
        // No match → graceful empty answer.
        serde_json::json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "agentstack_list_loadable", "arguments": { "query": "zzz-nomatch" } } }),
    ] {
        writeln!(stdin, "{request}").unwrap();
    }
    drop(stdin);

    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let responses: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(responses.len(), 3, "one response per request");

    // The matching query returns exactly the skill it describes.
    let hit: Value = serde_json::from_str(
        responses[1]["result"]["content"][0]["text"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let loadable = hit["loadable"].as_array().unwrap();
    assert_eq!(loadable.len(), 1, "only the matching skill: {hit}");
    assert_eq!(loadable[0]["name"], "quokka-lint");

    // The non-matching query returns an empty list and a graceful note.
    let miss: Value = serde_json::from_str(
        responses[2]["result"]["content"][0]["text"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert!(miss["loadable"].as_array().unwrap().is_empty());
    assert!(miss["note"]
        .as_str()
        .unwrap()
        .contains("No loadable skills match 'zzz-nomatch'"));
}
