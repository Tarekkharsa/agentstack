// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Which project did the human mean?
//!
//! Two surfaces answer that question, and both used to answer it from where a
//! process happened to start rather than from what somebody said.
//!
//! * `agentstack trust` read only its positional path or the cwd, so the
//!   global `--manifest-dir` every other command obeys did nothing. A
//!   companion that spawns the CLI from `/` and names a project by absolute
//!   path could not preview or grant trust for it at all: a human's yes was
//!   blocked by a launch directory.
//! * `agentstack mcp --auto-project` ranked the process cwd ABOVE
//!   `$AGENTSTACK_MANIFEST_DIR`, so a bridge launched inside an unrelated repo
//!   served that repo instead of the project it was configured for.
//!
//! Each test below is one of those witnesses. The controls matter as much as
//! the witnesses: naming nothing must still resolve nothing, because the fix
//! is about honouring a stated target, never about widening what can be
//! trusted.

#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// Distinctive directory names, so assertions match on a component that no
/// symlinked spelling of a temp dir can blur (macOS hands out `/var/folders/…`
/// for `/private/var/folders/…`).
const PROJ: &str = "proj-XRAY";
const OTHER: &str = "proj-YANKEE";

/// A minimal project: a manifest, and a lock so nothing refuses on "unpinned"
/// instead of on the thing under test.
fn project(tmp: &Path, name: &str, toolset: &str) -> PathBuf {
    let proj = tmp.join(name);
    fs::create_dir_all(proj.join(".agentstack")).unwrap();
    fs::write(
        proj.join(".agentstack/agentstack.toml"),
        format!(
            "version = 1\n[servers]\n[skills]\n\
             [toolsets]\n{toolset} = {{ servers = [] }}\n\
             [instructions]\n[targets]\ndefault = [\"claude-code\"]\n"
        ),
    )
    .unwrap();
    proj
}

/// A directory with no manifest at or above it. The walk-up fence stops at
/// `$HOME` and at every ancestor of it, and `home` lives directly under `tmp`,
/// so a walk from `tmp/nowhere` dies at `tmp` without escaping into the real
/// machine's tree.
fn nowhere(tmp: &Path) -> PathBuf {
    let dir = tmp.join("nowhere");
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn home(tmp: &Path) -> PathBuf {
    let home = tmp.join("home");
    fs::create_dir_all(&home).unwrap();
    home
}

/// Run the real binary in an isolated environment, from `cwd`.
fn cli(tmp: &Path, cwd: &Path, args: &[&str]) -> (String, bool) {
    let home = home(tmp);
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("HOME", &home)
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

fn surface_digest(preview: &str) -> String {
    let v: Value = serde_json::from_str(preview)
        .unwrap_or_else(|e| panic!("preview was not JSON ({e}): {preview}"));
    // The preview is enveloped; the digest is the one field a grant must echo.
    fn find(v: &Value) -> Option<String> {
        match v {
            Value::Object(map) => {
                if let Some(Value::String(s)) = map.get("surface_digest") {
                    return Some(s.clone());
                }
                map.values().find_map(find)
            }
            Value::Array(items) => items.iter().find_map(find),
            _ => None,
        }
    }
    find(&v).unwrap_or_else(|| panic!("no surface_digest in preview: {preview}"))
}

// ---------------------------------------------------------------------------
// Witness A — `agentstack trust` obeys `--manifest-dir`.
// ---------------------------------------------------------------------------

/// The defect, stated as the thing that must now work: previewing a project
/// named by `--manifest-dir`, from a directory that has no manifest of its own.
///
/// Before the fix this failed with
/// `no agentstack manifest at or above <nowhere> — run `agentstack init` first`.
#[test]
fn preview_honours_manifest_dir_from_a_manifestless_cwd() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = project(tmp.path(), PROJ, "alpha");
    let away = nowhere(tmp.path());

    let (out, ok) = cli(
        tmp.path(),
        &away,
        &[
            "trust",
            "--preview",
            "--manifest-dir",
            &proj.to_string_lossy(),
        ],
    );
    assert!(
        ok,
        "preview should succeed with an explicit --manifest-dir: {out}"
    );

    // The digest must be the project's REAL surface — the same value the
    // positional form reports for the same project. A preview that resolved
    // some other base would still be JSON; only this comparison catches it.
    let (positional, ok2) = cli(
        tmp.path(),
        &away,
        &["trust", "--preview", &proj.to_string_lossy()],
    );
    assert!(ok2, "positional preview should succeed: {positional}");
    assert_eq!(
        surface_digest(&out),
        surface_digest(&positional),
        "--manifest-dir must resolve the same project the positional path does"
    );
}

/// The grant that follows the preview really lands on the named project.
#[test]
fn grant_honours_manifest_dir_from_a_manifestless_cwd() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = project(tmp.path(), PROJ, "alpha");
    let away = nowhere(tmp.path());
    let dir = proj.to_string_lossy().to_string();

    let (preview, ok) = cli(
        tmp.path(),
        &away,
        &["trust", "--preview", "--manifest-dir", &dir],
    );
    assert!(ok, "preview: {preview}");
    let digest = surface_digest(&preview);

    let (granted, ok) = cli(
        tmp.path(),
        &away,
        &[
            "trust",
            "--yes",
            "--consented-digest",
            &digest,
            "--manifest-dir",
            &dir,
        ],
    );
    assert!(
        ok,
        "grant should succeed with an explicit --manifest-dir: {granted}"
    );

    let (list, ok) = cli(tmp.path(), &away, &["trust", "--list"]);
    assert!(ok, "list: {list}");
    assert!(
        list.contains(PROJ),
        "the named project should now be trusted; `trust --list` said: {list}"
    );
    assert!(
        list.contains("current"),
        "the grant should match the bytes previewed: {list}"
    );
}

/// CONTROL — naming NOTHING must still resolve nothing. The fix honours a
/// stated target; it must not turn "nobody said" into "trust whatever".
#[test]
fn no_path_and_no_manifest_dir_still_fails_from_a_manifestless_cwd() {
    let tmp = tempfile::tempdir().unwrap();
    let _proj = project(tmp.path(), PROJ, "alpha");
    let away = nowhere(tmp.path());

    let (out, ok) = cli(tmp.path(), &away, &["trust", "--preview"]);
    assert!(!ok, "an unnamed target must not resolve a project: {out}");
    assert!(
        out.contains("no agentstack manifest at or above"),
        "the failure should still be the discovery failure: {out}"
    );
}

/// CONTROL — the positional path still outranks `--manifest-dir`. The most
/// specific thing the user typed wins; this pins that order so a later edit
/// cannot quietly invert it.
#[test]
fn positional_path_outranks_manifest_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let wanted = project(tmp.path(), PROJ, "alpha");
    let decoy = project(tmp.path(), OTHER, "beta");
    let away = nowhere(tmp.path());

    let (out, ok) = cli(
        tmp.path(),
        &away,
        &[
            "trust",
            "--preview",
            &wanted.to_string_lossy(),
            "--manifest-dir",
            &decoy.to_string_lossy(),
        ],
    );
    assert!(ok, "preview: {out}");
    let (only_wanted, _) = cli(
        tmp.path(),
        &away,
        &["trust", "--preview", &wanted.to_string_lossy()],
    );
    assert_eq!(
        surface_digest(&out),
        surface_digest(&only_wanted),
        "the typed path is the most specific target and must win"
    );
}

/// CONTROL — `--manifest-dir` still goes through discovery, so it cannot
/// assert a project into existence. An undiscoverable directory fails closed
/// rather than becoming trustable.
#[test]
fn manifest_dir_pointing_at_no_project_still_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let _proj = project(tmp.path(), PROJ, "alpha");
    let away = nowhere(tmp.path());

    let (out, ok) = cli(
        tmp.path(),
        &away,
        &[
            "trust",
            "--preview",
            "--manifest-dir",
            &away.to_string_lossy(),
        ],
    );
    assert!(
        !ok,
        "an undiscoverable --manifest-dir must not resolve: {out}"
    );
    assert!(
        out.contains("no agentstack manifest at or above"),
        "it should fail the same discovery way: {out}"
    );
}

/// The control that matters most: `--manifest-dir` must not make the MACHINE
/// home trustable.
///
/// `$AGENTSTACK_HOME/agentstack.toml` is a real, readable manifest, so the only
/// thing keeping it out of the trust store is that `discover_project_base`
/// refuses to see it as a project. `resolve_base` therefore feeds
/// `--manifest-dir` into that same walk instead of taking it verbatim the way
/// `commands::project_base` does — a grant MINTS consent, so its target has to
/// earn discovery. Take the flag at face value and the machine's own personal
/// layer becomes trustable through a command-line argument, which would make
/// `$HOME` a project and hand every future gate the wrong base.
#[test]
fn manifest_dir_cannot_make_the_machine_home_trustable() {
    let tmp = tempfile::tempdir().unwrap();
    let _proj = project(tmp.path(), PROJ, "alpha");
    let machine = home(tmp.path()).join(".agentstack");
    fs::create_dir_all(&machine).unwrap();
    // A machine manifest that is valid in every way except being a project.
    fs::write(
        machine.join("agentstack.toml"),
        "version = 1\n[servers]\n[skills]\n[instructions]\n",
    )
    .unwrap();
    let away = nowhere(tmp.path());

    for args in [
        vec![
            "trust".to_string(),
            "--preview".to_string(),
            "--manifest-dir".to_string(),
            machine.to_string_lossy().into_owned(),
        ],
        vec![
            "trust".to_string(),
            "--yes".to_string(),
            "--manifest-dir".to_string(),
            machine.to_string_lossy().into_owned(),
        ],
    ] {
        let argv: Vec<&str> = args.iter().map(String::as_str).collect();
        let (out, ok) = cli(tmp.path(), &away, &argv);
        assert!(
            !ok,
            "the machine home became trustable through --manifest-dir ({argv:?}): {out}"
        );
    }

    // And nothing was recorded, whatever the exit code said.
    let store = machine.join("trust.json");
    let recorded = fs::read_to_string(&store).unwrap_or_default();
    assert!(
        !recorded.contains(".agentstack"),
        "a trust entry was written for the machine layer: {recorded}"
    );
}

// ---------------------------------------------------------------------------
// Witness B — the configured environment outranks the process cwd.
// ---------------------------------------------------------------------------

/// One live `agentstack mcp --auto-project` process, driven request by
/// request. Copied rather than shared: test binaries cannot import each other.
struct McpSession {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    stdout: std::io::BufReader<std::process::ChildStdout>,
}

impl McpSession {
    /// Launch with the cwd and `$AGENTSTACK_MANIFEST_DIR` deliberately
    /// disagreeing — the whole point of the witness.
    fn open(tmp: &Path, cwd: &Path, manifest_dir: &Path) -> McpSession {
        let home = home(tmp);
        let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_agentstack"))
            .args(["mcp", "--auto-project"])
            .current_dir(cwd)
            .env_clear()
            .env("HOME", &home)
            .env("AGENTSTACK_HOME", home.join(".agentstack"))
            .env("PATH", "/usr/bin:/bin")
            .env("AGENTSTACK_MANIFEST_DIR", manifest_dir)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = std::io::BufReader::new(child.stdout.take().unwrap());
        let mut s = McpSession {
            child,
            stdin,
            stdout,
        };
        s.request(1, "initialize", json!({}));
        s
    }

    fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        use std::io::{BufRead, Write};
        let frame = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        writeln!(self.stdin, "{frame}").unwrap();
        self.stdin.flush().unwrap();
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        serde_json::from_str(&line)
            .unwrap_or_else(|e| panic!("not a JSON-RPC frame ({e}): {line:?}"))
    }

    fn call(&mut self, id: u64, name: &str, arguments: Value) -> Value {
        self.request(
            id,
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        )
    }

    fn close(mut self) {
        drop(self.stdin);
        let _ = self.child.wait();
    }
}

fn call_text(response: &Value) -> String {
    response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The witness: an explicitly configured project beats the directory the
/// process happened to start in.
///
/// Neither project is trusted, which is what makes the binding observable
/// without granting anything — the trust refusal names the project the session
/// actually bound to. Before the fix this named the cwd's project.
#[test]
fn configured_manifest_dir_outranks_the_process_cwd() {
    let tmp = tempfile::tempdir().unwrap();
    let configured = project(tmp.path(), PROJ, "alpha");
    let launched_in = project(tmp.path(), OTHER, "beta");

    let mut s = McpSession::open(tmp.path(), &launched_in, &configured);
    let text = call_text(&s.call(2, "agentstack_lease_open", json!({ "profile": "alpha" })));
    s.close();

    assert!(
        text.contains(PROJ),
        "the session should serve the CONFIGURED project ({PROJ}); it said: {text}"
    );
    assert!(
        !text.contains(OTHER),
        "the cwd's project ({OTHER}) must not outrank an explicitly configured one; it said: {text}"
    );
}

/// CONTROL — with nothing configured, the cwd walk still resolves the project.
/// The reorder demotes the cwd; it must not disable it.
#[test]
fn the_cwd_still_resolves_when_nothing_is_configured() {
    let tmp = tempfile::tempdir().unwrap();
    let launched_in = project(tmp.path(), OTHER, "beta");
    let home = home(tmp.path());

    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(["mcp", "--auto-project"])
        .current_dir(&launched_in)
        .env_clear()
        .env("HOME", &home)
        .env("AGENTSTACK_HOME", home.join(".agentstack"))
        .env("PATH", "/usr/bin:/bin")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let stdin = child.stdin.take().unwrap();
    let stdout = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut s = McpSession {
        child,
        stdin,
        stdout,
    };
    s.request(1, "initialize", json!({}));
    let text = call_text(&s.call(2, "agentstack_lease_open", json!({ "profile": "beta" })));
    s.close();

    assert!(
        text.contains(OTHER),
        "with nothing configured the cwd's project should still bind; it said: {text}"
    );
}
