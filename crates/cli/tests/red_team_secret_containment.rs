// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! RED TEAM — hunt one resolved secret across every byte the tool writes.
//!
//! Invariant 5 says manifests and configuration carry `${REF}`, never values,
//! and that unresolved values fail closed. The interesting question is not
//! "does the manifest writer redact?" — it is whether the value leaks into any
//! of the *other* things a run produces: the lockfile, the store under
//! `~/.agentstack`, an event log, or one of the JSON surfaces a driver polls
//! and a bug reporter pastes into an issue.
//!
//! So this test does not check a list of known fields. It plants the same
//! sentinel in three shapes an attacker would use (an HTTP header, a stdio
//! argv element, a child `env` entry), runs the whole lock → trust → apply
//! lane with the value really resolvable, and then walks EVERY file under the
//! project and under HOME looking for it. Exactly one file may contain it: the
//! native config, which is the entire point of the render. A new file that
//! leaks the value fails this test without anyone remembering to update it.

use std::fs;
use std::path::{Path, PathBuf};

/// Distinctive enough that a substring hit cannot be a coincidence.
const SECRET: &str = "sk-live-9Q3rDEADBEEFcafef00d-supersecret";
const REF_NAME: &str = "RT_RED_TEAM_TOKEN";

const MANIFEST: &str = "version = 1\n\
[delivery]\nrender_locally = true\n\
[targets]\ndefault = [\"claude-code\"]\n\
[servers.api]\ntype = \"http\"\nurl = \"https://api.example/mcp\"\n\
headers = { Authorization = \"Bearer ${RT_RED_TEAM_TOKEN}\" }\n\
[servers.local]\ntype = \"stdio\"\ncommand = \"/bin/echo\"\n\
args = [\"--key\", \"${RT_RED_TEAM_TOKEN}\"]\n\
env = { API_KEY = \"${RT_RED_TEAM_TOKEN}\" }\n";

/// `with_secret = false` is the fail-closed half: the reference is declared but
/// nothing can resolve it.
fn run(args: &[&str], home: &Path, cwd: &Path, with_secret: bool) -> (String, bool) {
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_agentstack"));
    cmd.args(args)
        .current_dir(cwd)
        .env_clear()
        .env("HOME", home)
        .env("AGENTSTACK_HOME", home.join(".agentstack"))
        .env("PATH", "/usr/bin:/bin")
        .stdin(std::process::Stdio::null());
    if with_secret {
        cmd.env(REF_NAME, SECRET);
    }
    let out = cmd.output().expect("spawn agentstack");
    (
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
        out.status.success(),
    )
}

fn fixture(tmp: &Path) -> (PathBuf, PathBuf) {
    let home = tmp.join("home");
    let proj = tmp.join("proj");
    fs::create_dir_all(&home).unwrap();
    // A `.git` directory is what makes the managed ignore block apply; the
    // render is machine-local and must not become a commit.
    fs::create_dir_all(proj.join(".git")).unwrap();
    fs::write(proj.join("agentstack.toml"), MANIFEST).unwrap();
    (home, proj)
}

/// Every regular file under `root`, recursively, skipping `.git` (test fixture
/// noise, never written by us).
fn walk(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            if path.file_name().is_some_and(|n| n == ".git") {
                continue;
            }
            walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

fn files_containing(root: &Path, needle: &str) -> Vec<PathBuf> {
    let mut all = Vec::new();
    walk(root, &mut all);
    all.into_iter()
        .filter(|p| {
            fs::read(p)
                .map(|b| String::from_utf8_lossy(&b).contains(needle))
                .unwrap_or(false)
        })
        .collect()
}

#[test]
fn a_resolved_secret_reaches_the_native_config_and_nothing_else() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = fixture(tmp.path());

    let (text, ok) = run(&["lock", "--write"], &home, &proj, true);
    assert!(ok, "lock failed:\n{text}");
    let (text, _) = run(&["trust", "--preview"], &home, &proj, true);
    let digest = serde_json::from_str::<serde_json::Value>(&text).unwrap()["surface_digest"]
        .as_str()
        .unwrap()
        .to_string();
    let (text, ok) = run(
        &["trust", "--yes", "--consented", &digest],
        &home,
        &proj,
        true,
    );
    assert!(ok, "grant failed:\n{text}");
    let (text, ok) = run(&["apply", "--write"], &home, &proj, true);
    assert!(ok, "apply failed:\n{text}");

    // The render really did resolve — otherwise the sweep below proves nothing.
    let native = proj.join(".mcp.json");
    let rendered = fs::read_to_string(&native).unwrap();
    assert!(
        rendered.contains(SECRET),
        "fixture invalid: nothing resolved the secret, so no leak was possible:\n{rendered}"
    );

    // The sweep: the native config, and only the native config.
    let mut leaks = files_containing(&proj, SECRET);
    leaks.extend(files_containing(&home, SECRET));
    assert_eq!(
        leaks,
        vec![native.clone()],
        "the resolved secret escaped the native config"
    );

    // The manifest and lockfile carry the reference, unchanged.
    assert_eq!(
        fs::read_to_string(proj.join("agentstack.toml")).unwrap(),
        MANIFEST
    );
    let lock = fs::read_to_string(proj.join("agentstack.lock")).unwrap();
    assert!(
        !lock.contains(SECRET),
        "lockfile leaked the secret:\n{lock}"
    );

    // And the one file that does hold it is kept out of git.
    let ignore = fs::read_to_string(proj.join(".gitignore")).unwrap();
    assert!(
        ignore.contains(".mcp.json"),
        "the only file holding a live secret is not ignored:\n{ignore}"
    );
}

/// The surfaces a driver polls and a human pastes into a bug report. Checked
/// separately because they are computed, not stored — a redaction bug here
/// would leave no trace on disk for the sweep above to find.
#[test]
fn no_json_surface_ever_prints_the_resolved_value() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = fixture(tmp.path());
    let (text, ok) = run(&["lock", "--write"], &home, &proj, true);
    assert!(ok, "lock failed:\n{text}");

    for args in [
        &["status", "--json"][..],
        &["trust", "--preview"][..],
        &["diff", "--json"][..],
        &["explain", "--json"][..],
        &["list", "--json"][..],
    ] {
        let (text, _ok) = run(args, &home, &proj, true);
        assert!(
            !text.contains(SECRET),
            "{args:?} printed the resolved secret:\n{text}"
        );
        assert!(
            !text.is_empty(),
            "{args:?} printed nothing — a surface that says nothing proves nothing"
        );
    }
}

/// Fail closed, not open: with the reference unresolvable, the write is
/// refused outright rather than emitting a config with an empty credential.
#[test]
fn an_unresolvable_secret_blocks_the_write_instead_of_emitting_a_blank() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = fixture(tmp.path());
    // The subject is the SECRET gate, so the consent gate must not be what
    // stops the write: pin and grant first (the same two-step the resolved
    // case above performs), leaving the unresolvable reference as the only
    // reason a write can be refused.
    let (text, ok) = run(&["lock", "--write"], &home, &proj, false);
    assert!(ok, "lock failed:\n{text}");
    let (text, _) = run(&["trust", "--preview"], &home, &proj, false);
    let digest = serde_json::from_str::<serde_json::Value>(&text).unwrap()["surface_digest"]
        .as_str()
        .unwrap()
        .to_string();
    let (text, ok) = run(
        &["trust", "--yes", "--consented", &digest],
        &home,
        &proj,
        false,
    );
    assert!(ok, "grant failed:\n{text}");

    let (text, ok) = run(&["apply", "--write"], &home, &proj, false);
    assert!(!ok, "apply succeeded with an unresolvable secret:\n{text}");
    assert!(
        !proj.join(".mcp.json").exists(),
        "a blocked write left a native config behind:\n{text}"
    );

    // `--allow-unresolved` is the explicit escape hatch, and it must write the
    // REFERENCE — never an empty string, which would read as "no auth needed".
    let (text, ok) = run(
        &["apply", "--write", "--allow-unresolved"],
        &home,
        &proj,
        false,
    );
    assert!(ok, "--allow-unresolved failed:\n{text}");
    let rendered = fs::read_to_string(proj.join(".mcp.json")).unwrap();
    assert!(
        rendered.contains("${RT_RED_TEAM_TOKEN}"),
        "the unresolved render must keep the reference verbatim:\n{rendered}"
    );
    assert!(
        !rendered.contains("\"\""),
        "the unresolved render emitted an empty credential:\n{rendered}"
    );
}
