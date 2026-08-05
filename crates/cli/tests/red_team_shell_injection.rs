// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! RED TEAM — hostile strings in every manifest field that reaches a process.
//!
//! Invariant 7's operative half is "never interpolate repository content into
//! shell commands". A claim like that is only worth what its worst case
//! proves, so the fixture below is not a sanitised string: the server *name*,
//! its argv entries and its child `env` values all contain `$(…)`, backticks
//! and `;` sequences that create a file if anything ever hands them to a
//! shell. The file's existence is the assertion.
//!
//! Two independent paths are attacked, because they fail differently:
//!
//! - the **spawn** path, in-process, with a real child that reports the argv
//!   and env it actually received. Passing here means the strings arrived
//!   verbatim through `execve` — the positive proof that there was no shell,
//!   not merely the absence of a side effect;
//! - the **render/report** path, through the real binary across the whole
//!   lock → trust → apply lane plus every reporting verb, where the same
//!   strings must survive as inert data and no stray file may appear anywhere
//!   under the fixture root.

#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use agentstack::gateway::Gateway;
use serde_json::json;

// The in-process test mutates the process-global HOME; serialize.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// A minimal MCP stdio server in POSIX sh that reports, on `tools/call`, the
/// first argument and the `EVIL` env value it was actually given. If a shell
/// ever expanded them, the `$(touch …)` payloads run and `echo` reports the
/// *expanded* (empty) text instead — so this fixture detects interpolation
/// two ways at once.
const REPORTER: &str = r#"#!/bin/sh
arg1="$1"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-06-18","capabilities":{},"serverInfo":{"name":"probe","version":"0"}}}\n' "$id"
      ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"report","description":"Report received argv and env.","inputSchema":{"type":"object","properties":{}}}]}}\n' "$id"
      ;;
    *'"method":"tools/call"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"argv1=[%s] evil=[%s]"}]}}\n' "$id" "$arg1" "$EVIL"
      ;;
  esac
done
"#;

/// The payload, parameterised on a marker directory so each test can prove
/// "nothing ran" by looking for files that would only exist if it had.
fn payload(marker_dir: &Path, tag: &str) -> String {
    format!(
        "x$(touch {0}/{tag}_SUB)`touch {0}/{tag}_TICK`; touch {0}/{tag}_SEMI",
        marker_dir.display()
    )
}

/// Any file created by an executed payload — must always be empty.
fn detonations(marker_dir: &Path) -> Vec<String> {
    fs::read_dir(marker_dir)
        .map(|d| {
            d.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.contains("_SUB") || n.contains("_TICK") || n.contains("_SEMI"))
                .collect()
        })
        .unwrap_or_default()
}

/// The definitive one: the child process reports what it received.
#[test]
fn a_hostile_argv_entry_reaches_the_child_verbatim_and_never_a_shell() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    let marker = tmp.path().join("marker");
    fs::create_dir_all(&marker).unwrap();
    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    let script = proj.join("probe.sh");
    fs::write(&script, REPORTER).unwrap();

    let arg = payload(&marker, "ARG");
    let env = payload(&marker, "ENV");
    fs::write(
        proj.join("agentstack.toml"),
        format!(
            "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
             [servers.probe]\ntype = \"stdio\"\ncommand = \"/bin/sh\"\n\
             args = [\"{}\", \"{arg}\"]\nenv = {{ EVIL = \"{env}\" }}\n",
            script.display()
        ),
    )
    .unwrap();

    let gw = Gateway::from_manifest(Some(&proj));
    let res = gw
        .try_call("probe__report", &json!({}))
        .expect("routed")
        .expect("call ok");
    let text = res["content"][0]["text"].as_str().unwrap_or_default();

    assert_eq!(
        text,
        format!("argv1=[{arg}] evil=[{env}]"),
        "the child did not receive the manifest strings verbatim — something \
         expanded them on the way"
    );
    assert!(
        detonations(&marker).is_empty(),
        "a manifest string was executed: {:?}",
        detonations(&marker)
    );
    drop(gw);
}

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

/// The whole CLI lane over a manifest whose server *name* is itself a payload
/// — the field most likely to be pasted into a path, a message, or a command.
#[test]
fn no_verb_in_the_lane_executes_or_mangles_a_hostile_manifest_string() {
    let tmp = tempfile::tempdir().unwrap();
    let root: PathBuf = tmp.path().to_path_buf();
    let home = root.join("home");
    let marker = root.join("marker");
    let proj = root.join("proj");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&marker).unwrap();
    fs::create_dir_all(&proj).unwrap();

    let name = format!("evil; touch {}/NAME_SEMI", marker.display());
    let arg = payload(&marker, "ARG");
    fs::write(
        proj.join("agentstack.toml"),
        format!(
            "version = 1\n[delivery]\nrender_locally = true\n\
             [targets]\ndefault = [\"claude-code\"]\n\
             [servers.\"{name}\"]\ntype = \"stdio\"\ncommand = \"/bin/echo\"\n\
             args = [\"{arg}\"]\n"
        ),
    )
    .unwrap();

    let (text, ok) = run(&["lock", "--write"], &home, &proj);
    assert!(ok, "lock failed:\n{text}");
    let digest =
        serde_json::from_str::<serde_json::Value>(&run(&["trust", "--preview"], &home, &proj).0)
            .unwrap()["surface_digest"]
            .as_str()
            .unwrap()
            .to_string();
    let (text, ok) = run(
        &["trust", "--yes", "--consented-digest", &digest],
        &home,
        &proj,
    );
    assert!(ok, "grant failed:\n{text}");

    for args in [
        &["apply", "--write", "--no-gitignore"][..],
        &["use", "--write", "--no-gitignore"][..],
        &["status", "--json"][..],
        &["diff", "--json"][..],
        &["explain", "--json"][..],
        &["doctor"][..],
        &["list"][..],
    ] {
        let (text, _ok) = run(args, &home, &proj);
        assert!(
            detonations(&marker).is_empty() && !marker.join("NAME_SEMI").exists(),
            "{args:?} executed a manifest string ({:?}):\n{text}",
            detonations(&marker)
        );
    }

    // The payload survived as data: the native config holds the hostile name
    // and argument byte for byte, quoted as JSON — inert, not "cleaned up".
    let rendered = fs::read_to_string(proj.join(".mcp.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&rendered).unwrap();
    let server = &parsed["mcpServers"][&name];
    assert!(
        !server.is_null(),
        "the hostile server name was silently rewritten:\n{rendered}"
    );
    assert_eq!(server["args"][0].as_str().unwrap(), arg);

    // …and the hostile name never became a filesystem path: the only things
    // written are the expected files, not a directory carved out of the name.
    let stray: Vec<_> = fs::read_dir(&root)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains("evil") || n.contains("touch"))
        .collect();
    assert!(
        stray.is_empty(),
        "a manifest string became a path: {stray:?}"
    );
}
