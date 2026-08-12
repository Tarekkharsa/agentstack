// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! G11 WITNESS: **a keep-pinned item whose approved copy cannot be produced is
//! never quietly replaced by current bytes, and never silently omitted.**
//!
//! Standing decisions plus the content snapshot store are DELIVERY inputs, not
//! only card-render inputs. Five paths consume them, and each one is exercised
//! here in all three states of the snapshot the decision points at:
//!
//! | path                          | healthy       | missing | corrupted |
//! |-------------------------------|---------------|---------|-----------|
//! | 1 `use` skill materialization | approved copy | excluded, named | excluded, named |
//! | 2 MCP loadable catalog        | approved desc | listed, `loadable: false` | listed, `loadable: false` |
//! | 3 `agentstack_load`           | approved body | refuses | refuses |
//! | 4 instruction compilation     | approved text | excluded, named | excluded, named |
//! | 5 protected (`--locked`) run  | refuses       | refuses | refuses |
//!
//! Path 5 is the odd one and is asserted as such: it never reads the snapshot,
//! so the snapshot's state cannot change its answer. It refuses because a
//! keep-pinned item leaves the lock on the APPROVED bytes while the project
//! copy holds the declined ones, and a protected run delivers the project copy.
//! Its refusal must therefore name the standing decision — the generic drift
//! refusal it used to give sent the user to `agentstack lock --write`, which by
//! design will not move a decided pin.
//!
//! The "healthy" case is the control: it proves every assertion below is about
//! the FAILURE path, and that a working keep-pinned delivery is untouched.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

use agentstack::commands::trust::{Answer, ReGateProbe};
use serde_json::Value;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn isolate_home(home: &Path) {
    std::env::set_var("AGENTSTACK_HOME", home.join("agentstack-home"));
    std::env::set_var("HOME", home);
}

fn skill(proj: &Path, name: &str, body: &str) {
    let dir = proj.join(".agentstack/skills").join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("SKILL.md"), body).unwrap();
}

/// Two path skills and one instruction fragment, locked and trusted — the state
/// a re-gate starts from. `alpha` is the untouched control that must keep being
/// delivered in every mode; `beta` and `house` are the ones put on a decision.
fn trusted_project(root: &Path) -> PathBuf {
    let proj = root.join("proj");
    fs::create_dir_all(proj.join(".agentstack/instructions")).unwrap();
    skill(&proj, "alpha", "---\ndescription: a\n---\n# Alpha\nfirst\n");
    skill(&proj, "beta", "---\ndescription: b\n---\n# Beta\nfirst\n");
    fs::write(
        proj.join(".agentstack/instructions/house.md"),
        "House rule one.\n",
    )
    .unwrap();
    fs::write(
        proj.join(".agentstack/agentstack.toml"),
        "version = 1\n\n\
         [delivery]\nrender_locally = true\n\n\
         [targets]\ndefault = [\"claude-code\"]\n\n\
         [skills.alpha]\npath = \"./skills/alpha\"\n\n\
         [skills.beta]\npath = \"./skills/beta\"\n\n\
         [instructions.house]\npath = \"./instructions/house.md\"\n",
    )
    .unwrap();
    agentstack::commands::lock::run(&Default::default(), Some(&proj)).unwrap();
    let digest = agentstack::trust::digest_for(&proj).unwrap();
    agentstack::commands::trust::grant_with_answers(&proj, true, Some(&digest), false, None)
        .unwrap();
    proj
}

fn cli(proj: &Path, args: &[&str]) -> (String, bool) {
    let out = Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(args)
        .current_dir(proj)
        .env_clear()
        .env("HOME", std::env::var("HOME").unwrap())
        .env("AGENTSTACK_HOME", std::env::var("AGENTSTACK_HOME").unwrap())
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null())
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

/// One real MCP stdio process: the ambient index that `initialize` embeds, the
/// `agentstack_list_loadable` catalog, and an `agentstack_load` of `name`.
/// Three surfaces, one connection — the order an agent actually meets them in.
struct McpProbe {
    /// The `instructions` string from `initialize` (the ambient skill index).
    index: String,
    /// The parsed `agentstack_list_loadable` payload.
    catalog: Value,
    /// The `agentstack_load` response text, and whether it was an error.
    load: String,
    load_is_error: bool,
}

fn mcp_probe(proj: &Path, name: &str) -> McpProbe {
    let mut child = Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(["mcp", "--manifest-dir"])
        .arg(proj)
        .env_clear()
        .env("HOME", std::env::var("HOME").unwrap())
        .env("AGENTSTACK_HOME", std::env::var("AGENTSTACK_HOME").unwrap())
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for request in [
        serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-11-25", "capabilities": {},
            "clientInfo": { "name": "agentstack-test", "version": "1" }
        } }),
        serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "agentstack_list_loadable", "arguments": {} } }),
        serde_json::json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "agentstack_load", "arguments": { "name": name, "reason": "witness" } } }),
    ] {
        writeln!(stdin, "{request}").unwrap();
    }
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    let responses: Vec<Value> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .collect();
    let by_id = |id: u64| -> Value {
        responses
            .iter()
            .find(|r| r["id"] == id)
            .cloned()
            .unwrap_or(Value::Null)
    };
    let tool_text = |v: &Value| -> String {
        v["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .to_string()
    };
    let init = by_id(1);
    let list = by_id(2);
    let load = by_id(3);
    McpProbe {
        index: init["result"]["instructions"]
            .as_str()
            .unwrap_or("")
            .to_string(),
        catalog: serde_json::from_str(&tool_text(&list)).unwrap_or(Value::Null),
        load: tool_text(&load),
        load_is_error: load["result"]["isError"] == Value::Bool(true),
    }
}

/// One catalog entry by name, or `None` when the catalog does not list it.
fn entry<'a>(catalog: &'a Value, name: &str) -> Option<&'a Value> {
    catalog["loadable"]
        .as_array()?
        .iter()
        .find(|e| e["name"] == name)
}

/// A protected run against a stub harness binary. The stub exits 0, so if a
/// gate ever let the run through, the assertions below fail loudly rather than
/// the process merely not being found.
fn locked_run(tmp: &Path, proj: &Path) -> (String, bool) {
    let stub_bin = tmp.join("stubbin");
    fs::create_dir_all(&stub_bin).unwrap();
    fs::write(stub_bin.join("claude"), "#!/bin/sh\nexit 0\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(stub_bin.join("claude"), fs::Permissions::from_mode(0o755)).unwrap();
    }
    let out = Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(["run", "claude-code", "--locked"])
        .current_dir(proj)
        .env_clear()
        .env("HOME", std::env::var("HOME").unwrap())
        .env("AGENTSTACK_HOME", std::env::var("AGENTSTACK_HOME").unwrap())
        .env("PATH", format!("{}:/usr/bin:/bin", stub_bin.display()))
        .stdin(Stdio::null())
        .output()
        .unwrap();
    (
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
        out.status.success(),
    )
}

fn snapshot_of(proj: &Path, kind: &str, name: &str) -> PathBuf {
    let Some(agentstack::trust::Decision::KeepPinned { pin }) =
        agentstack::trust::decision_for(proj, kind, name)
    else {
        panic!("fixture: no keep-pinned decision for {kind} '{name}'");
    };
    let hex = pin.rsplit(':').next().unwrap().to_string();
    PathBuf::from(std::env::var("AGENTSTACK_HOME").unwrap())
        .join("store/content")
        .join(hex)
}

fn compile_instructions(proj: &Path) -> String {
    agentstack::commands::instructions::run(
        &agentstack::cli::InstructionsArgs {
            toolset: None,
            targets: vec!["claude-code".into()],
            scope: Some(agentstack::scope::Scope::Project),
            write: true,
        },
        Some(proj),
    )
    .unwrap();
    fs::read_to_string(proj.join("CLAUDE.md")).unwrap_or_default()
}

fn delivered_dir(proj: &Path) -> Option<PathBuf> {
    [".claude/skills", ".agents/skills", ".pi/skills"]
        .iter()
        .map(|d| proj.join(d))
        .find(|d| d.is_dir())
}

/// Every file under `root`, as (relpath, bytes) — for proving nothing planted
/// in the store reached the project.
fn tree(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            let Ok(meta) = p.symlink_metadata() else {
                continue;
            };
            if meta.is_dir() {
                stack.push(p);
            } else if let Ok(bytes) = fs::read(&p) {
                out.push((
                    p.strip_prefix(root).unwrap().to_string_lossy().to_string(),
                    bytes,
                ));
            }
        }
    }
    out
}

/// The shared arrangement: drift `beta` and `house`, answer keep-pinned to
/// both, and hand back the project plus the two snapshots the decisions point
/// at. Each test below then damages those snapshots differently.
fn keep_pinned_project(tmp: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let proj = trusted_project(tmp);
    skill(&proj, "beta", "---\ndescription: b\n---\n# Beta\nCHANGED\n");
    fs::write(
        proj.join(".agentstack/instructions/house.md"),
        "House rule CHANGED.\n",
    )
    .unwrap();
    agentstack::commands::trust::grant_with_answers(
        &proj,
        false,
        None,
        true,
        Some(&ReGateProbe {
            answers: vec![
                ("beta".to_string(), Answer::KeepPinned),
                ("house".to_string(), Answer::KeepPinned),
            ],
            confirm: true,
        }),
    )
    .unwrap();
    let skill_snap = snapshot_of(&proj, "skill", "beta");
    let instr_snap = snapshot_of(&proj, "instruction", "house");
    assert!(skill_snap.is_dir(), "fixture: the skill snapshot exists");
    assert!(
        instr_snap.is_dir(),
        "fixture: the instruction snapshot exists"
    );
    (proj, skill_snap, instr_snap)
}

// ---------------------------------------------------------------------------
// CONTROL — a healthy keep-pinned delivery is unchanged by any of this.
// ---------------------------------------------------------------------------

#[test]
fn healthy_keep_pinned_still_delivers_the_approved_bytes_everywhere() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    isolate_home(tmp.path());
    let (proj, _skill_snap, _instr_snap) = keep_pinned_project(tmp.path());

    // 1 — `use` materializes the APPROVED bytes, as a real copy.
    let (out, ok) = cli(&proj, &["use", "--write"]);
    assert!(ok, "use --write failed:\n{out}");
    let delivered = delivered_dir(&proj).expect("something was materialized");
    let body = fs::read_to_string(delivered.join("beta/SKILL.md"))
        .expect("the keep-pinned skill was delivered");
    assert!(
        body.contains("first") && !body.contains("CHANGED"),
        "the control delivered the declined content: {body}"
    );
    assert!(
        !delivered
            .join("beta")
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink(),
        "keep-pinned was symlinked; it would follow the declined change"
    );
    assert!(
        delivered.join("alpha").exists(),
        "the control skill is gone"
    );

    // 4 — the instruction compiler emits the APPROVED text.
    let compiled = compile_instructions(&proj);
    assert!(
        compiled.contains("House rule one.") && !compiled.contains("CHANGED"),
        "the control compiled the declined content:\n{compiled}"
    );

    // 2 + 3 — the catalog carries the APPROVED description and still offers the
    // skill, and the loader serves the APPROVED body.
    let mcp = mcp_probe(&proj, "beta");
    let beta = entry(&mcp.catalog, "beta").expect("beta is catalogued");
    assert_eq!(beta["description"], "b", "the control lost the description");
    assert_eq!(beta["origin"], "approved-copy");
    assert!(
        beta.get("loadable").is_none(),
        "the control marked a healthy entry unloadable: {beta}"
    );
    assert!(
        !mcp.index.contains("NOT LOADABLE"),
        "the control's ambient index disowned a healthy skill:\n{}",
        mcp.index
    );
    assert!(
        !mcp.load_is_error,
        "the control refused a load:\n{}",
        mcp.load
    );
    assert!(
        mcp.load.contains("first") && !mcp.load.contains("CHANGED"),
        "the control loaded the declined content:\n{}",
        mcp.load
    );

    // 5 — the protected run refuses even with a perfect snapshot: it delivers
    // the PROJECT copy, which is the declined content. The refusal must name
    // the decision and the review that can clear it, not send the user to
    // `lock --write`, which by design will not move a decided pin.
    let (locked, locked_ok) = locked_run(tmp.path(), &proj);
    assert!(
        !locked_ok,
        "a protected run delivered a project copy the human declined:\n{locked}"
    );
    assert!(
        locked.contains("keep the approved version") && locked.contains("agentstack trust"),
        "the protected refusal does not name the standing decision:\n{locked}"
    );
}

// ---------------------------------------------------------------------------
// The approved copy is GONE.
// ---------------------------------------------------------------------------

#[test]
fn a_missing_approved_copy_is_refused_on_every_path() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    isolate_home(tmp.path());
    let (proj, skill_snap, instr_snap) = keep_pinned_project(tmp.path());
    fs::remove_dir_all(&skill_snap).unwrap();
    fs::remove_dir_all(&instr_snap).unwrap();

    // 1 — excluded, and SAID, with the action that can clear it.
    let (out, ok) = cli(&proj, &["use", "--write"]);
    assert!(ok, "use --write failed outright:\n{out}");
    let delivered = delivered_dir(&proj).expect("something was materialized");
    assert!(
        !delivered.join("beta").exists(),
        "a keep-pinned skill with no approved copy was delivered anyway:\n{out}"
    );
    assert!(
        out.contains("missing or failed verification") && out.contains("agentstack trust"),
        "the exclusion does not say what failed or what to do:\n{out}"
    );
    assert!(
        delivered.join("alpha").exists(),
        "the failure took an unrelated skill with it"
    );

    // 4 — excluded, rather than compiled from the live file.
    let compiled = compile_instructions(&proj);
    assert!(
        !compiled.contains("House rule"),
        "the compiler fell back to live bytes:\n{compiled}"
    );

    // 2 — the CATALOG still lists the name (it is an inventory, and hiding the
    // name would be the silent omission this gap is about) but it is no longer
    // an OFFER: machine-readably unloadable, with the reason and the action.
    let mcp = mcp_probe(&proj, "beta");
    let beta = entry(&mcp.catalog, "beta").expect("the catalog hid the item entirely");
    assert_eq!(
        beta["loadable"],
        Value::Bool(false),
        "the catalog still advertises it as loadable: {beta}"
    );
    assert!(
        beta["unavailable"]
            .as_str()
            .unwrap_or("")
            .contains("agentstack trust"),
        "the entry does not say what to do: {beta}"
    );
    assert_eq!(beta["action"], "agentstack trust");
    assert_ne!(
        beta["description"], "b",
        "the catalog served a description it could not verify"
    );
    assert!(
        mcp.catalog["note"]
            .as_str()
            .unwrap_or("")
            .contains("NOT loadable"),
        "the answer's prose hides the unloadable entry: {}",
        mcp.catalog["note"]
    );
    // …and the ambient index an agent reads at initialize says the same.
    assert!(
        mcp.index.contains("beta — NOT LOADABLE"),
        "the ambient index still offers it:\n{}",
        mcp.index
    );

    // 3 — and the loader, the actual boundary, refuses.
    assert!(
        mcp.load_is_error,
        "the loader served a skill with no approved copy:\n{}",
        mcp.load
    );
    assert!(
        mcp.load.contains("missing or failed verification")
            && mcp.load.contains("agentstack trust"),
        "the load refusal does not say what failed or what to do:\n{}",
        mcp.load
    );

    // 5 — the protected run refuses, naming the decision.
    let (locked, locked_ok) = locked_run(tmp.path(), &proj);
    assert!(!locked_ok, "a protected run proceeded:\n{locked}");
    assert!(
        locked.contains("keep the approved version") && locked.contains("agentstack trust"),
        "the protected refusal does not name the standing decision:\n{locked}"
    );
}

// ---------------------------------------------------------------------------
// The approved copy is PRESENT but no longer hashes to the approved digest.
// The store directory is writable, so this is the case a bare `is_dir()` would
// have delivered under the approved name.
// ---------------------------------------------------------------------------

#[test]
fn a_corrupted_approved_copy_is_refused_on_every_path() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    isolate_home(tmp.path());
    let (proj, skill_snap, instr_snap) = keep_pinned_project(tmp.path());
    fs::write(
        skill_snap.join("SKILL.md"),
        "---\ndescription: PLANTED\n---\n# Beta\nEVIL PAYLOAD\n",
    )
    .unwrap();
    for e in fs::read_dir(&instr_snap).unwrap().flatten() {
        fs::write(e.path(), "EVIL PAYLOAD instruction.\n").unwrap();
    }

    // 1 — the planted bytes never reach a harness.
    let (out, ok) = cli(&proj, &["use", "--write"]);
    assert!(ok, "use --write failed outright:\n{out}");
    let delivered = delivered_dir(&proj).expect("something was materialized");
    assert!(
        !delivered.join("beta").exists(),
        "a tampered snapshot was delivered under the approved name:\n{out}"
    );
    assert!(
        out.contains("missing or failed verification") && out.contains("agentstack trust"),
        "the exclusion does not say what failed or what to do:\n{out}"
    );

    // 4 — nor a managed instruction region.
    let compiled = compile_instructions(&proj);
    assert!(
        !compiled.contains("EVIL PAYLOAD") && !compiled.contains("House rule"),
        "planted bytes reached the managed region:\n{compiled}"
    );

    // 2 — the catalog does not repeat the planted DESCRIPTION, and does not
    // offer the entry.
    let mcp = mcp_probe(&proj, "beta");
    let beta = entry(&mcp.catalog, "beta").expect("the catalog hid the item entirely");
    assert_eq!(
        beta["loadable"],
        Value::Bool(false),
        "still offered: {beta}"
    );
    assert!(
        !serde_json::to_string(beta).unwrap().contains("PLANTED"),
        "the catalog repeated a planted description: {beta}"
    );
    assert!(
        mcp.index.contains("beta — NOT LOADABLE"),
        "the ambient index still offers it:\n{}",
        mcp.index
    );

    // 3 — and no planted BODY is served.
    assert!(
        mcp.load_is_error,
        "the loader served a tampered snapshot:\n{}",
        mcp.load
    );
    assert!(
        !mcp.load.contains("EVIL PAYLOAD"),
        "planted bytes reached an MCP response:\n{}",
        mcp.load
    );

    // 5 — the protected run refuses, naming the decision.
    let (locked, locked_ok) = locked_run(tmp.path(), &proj);
    assert!(!locked_ok, "a protected run proceeded:\n{locked}");
    assert!(
        locked.contains("keep the approved version") && locked.contains("agentstack trust"),
        "the protected refusal does not name the standing decision:\n{locked}"
    );

    // Nothing planted in the store escaped into the project on ANY path.
    for (path, bytes) in tree(&proj) {
        assert!(
            !bytes.windows(12).any(|w| w == b"EVIL PAYLOAD"),
            "planted bytes escaped into the project at {path}"
        );
    }
}

// ---------------------------------------------------------------------------
// The store is writable, so the tamper that matters most is a SYMLINK planted
// under the approved digest name — the shape that turns "deliver the approved
// copy" into "copy whatever this points at".
// ---------------------------------------------------------------------------

#[test]
fn a_symlinked_approved_copy_never_exfiltrates() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    isolate_home(tmp.path());
    let (proj, skill_snap, _instr_snap) = keep_pinned_project(tmp.path());

    let secret = tmp.path().join("secret-key");
    fs::write(&secret, "PRIVATE KEY MATERIAL\n").unwrap();
    fs::remove_file(skill_snap.join("SKILL.md")).unwrap();
    std::os::unix::fs::symlink(&secret, skill_snap.join("SKILL.md")).unwrap();

    let (out, ok) = cli(&proj, &["use", "--write"]);
    assert!(ok, "use --write failed outright:\n{out}");
    let delivered = delivered_dir(&proj).expect("something was materialized");
    assert!(
        !delivered.join("beta").exists(),
        "a symlinked snapshot was delivered:\n{out}"
    );

    let mcp = mcp_probe(&proj, "beta");
    assert!(
        mcp.load_is_error,
        "the loader followed a symlink:\n{}",
        mcp.load
    );
    assert!(
        !mcp.load.contains("PRIVATE KEY MATERIAL"),
        "secret bytes reached an MCP response:\n{}",
        mcp.load
    );
    let beta = entry(&mcp.catalog, "beta").expect("the catalog hid the item entirely");
    assert_eq!(
        beta["loadable"],
        Value::Bool(false),
        "still offered: {beta}"
    );
    assert!(
        !serde_json::to_string(&mcp.catalog)
            .unwrap()
            .contains("PRIVATE KEY MATERIAL"),
        "secret bytes reached the catalog"
    );

    for (path, bytes) in tree(&proj) {
        assert!(
            !bytes.windows(20).any(|w| w == b"PRIVATE KEY MATERIAL"),
            "secret bytes escaped into the project at {path}"
        );
    }
}
