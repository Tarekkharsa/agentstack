// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! `doctor`'s Hooks check must judge the scope `apply` actually writes at.
//!
//! The Settings check one section above already derives its scope from the
//! manifest's home (`Scope::default_for`). The Hooks check did not: it asked
//! `plan_hooks` about the GLOBAL file for every project. On a repo manifest —
//! the ordinary case — `apply --write` renders the hook into the repo's own
//! config and `doctor` then compared the machine-wide file, which of course
//! still holds nothing. The report said "hooks stale ↳ agentstack apply
//! --write" over a project that had just applied, and the named command
//! never touches the file the check judged: run it, re-run doctor, same
//! answer, forever.
//!
//! The second half is the machine guard hook. `apply` passes it to
//! `plan_hooks` only at global scope, so machine protection never lands in a
//! repo's committed config; `doctor` passed it unconditionally, so at project
//! scope it diffed a hook `apply` would never write there. A machine with the
//! guard wired therefore reported stale hooks for every repo on it.
//!
//! Both are driven through the real binary — `trust`, then `apply --write`,
//! then `doctor --json` — because the defect is about what the shipped
//! command sequence reports, not about what `plan_hooks` returns in isolation.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// A project with one hook, in the `.agentstack/` layout a repo uses.
const MANIFEST: &str = "version = 1\n\
    \n[targets]\ndefault = [\"claude-code\"]\n\
    \n[hooks.fmt]\nevent = \"PostToolUse\"\nmatcher = \"Edit\"\ncommand = \"echo formatted\"\n";

struct Fixture {
    _tmp: assert_fs::TempDir,
    home: PathBuf,
    proj: PathBuf,
}

impl Fixture {
    /// A sandboxed HOME plus a trusted, hook-declaring project.
    fn new(machine_manifest: Option<&str>) -> Self {
        let tmp = assert_fs::TempDir::new().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(home.join(".agentstack")).unwrap();
        if let Some(body) = machine_manifest {
            fs::write(home.join(".agentstack/agentstack.toml"), body).unwrap();
        }
        // A git dir makes this the ordinary repo shape the defect lived in.
        let proj = tmp.path().join("proj");
        fs::create_dir_all(proj.join(".git")).unwrap();
        fs::create_dir_all(proj.join(".agentstack")).unwrap();
        fs::write(proj.join(".agentstack/agentstack.toml"), MANIFEST).unwrap();

        let fx = Fixture {
            _tmp: tmp,
            home,
            proj,
        };
        fx.trust();
        fx
    }

    fn run(&self, args: &[&str]) -> (bool, String) {
        let out = Command::new(env!("CARGO_BIN_EXE_agentstack"))
            .args(args)
            // Every command runs from the project: `trust` resolves its base
            // from the cwd, not from `--manifest-dir`.
            .current_dir(&self.proj)
            .env("HOME", &self.home)
            .env("AGENTSTACK_HOME", self.home.join(".agentstack"))
            .output()
            .unwrap();
        let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&out.stderr));
        (out.status.success(), text)
    }

    /// Rendering hooks needs consent bound to the exact bytes, so the fixture
    /// walks the real ceremony: preview for the digest, then grant with it.
    fn trust(&self) {
        let (ok, preview) = self.run(&["trust", "--preview"]);
        assert!(ok, "trust --preview failed: {preview}");
        let surface: serde_json::Value = serde_json::from_str(&preview).unwrap();
        let digest = surface["surface_digest"].as_str().unwrap().to_string();
        let (ok, out) = self.run(&["trust", ".", "--yes", "--consented-digest", &digest]);
        assert!(ok, "trust failed: {out}");
    }

    fn doctor(&self) -> serde_json::Value {
        let (_, out) = self.run(&["doctor", "--json", "--all"]);
        serde_json::from_str(&out)
            .unwrap_or_else(|e| panic!("doctor --json is not JSON ({e}): {out}"))
    }
}

/// Every line of the named section, as `(level, msg)`.
fn section(report: &serde_json::Value, title: &str) -> Vec<(String, String)> {
    report["sections"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["title"] == title)
        .flat_map(|s| s["lines"].as_array().unwrap())
        .map(|l| {
            (
                l["level"].as_str().unwrap_or_default().to_string(),
                l["msg"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

fn assert_hooks_in_sync(report: &serde_json::Value, case: &str) {
    let lines = section(report, "Hooks");
    assert!(
        !lines.is_empty(),
        "[{case}] the Hooks section must report something: {report}"
    );
    for (level, msg) in &lines {
        assert!(
            !msg.contains("hooks stale"),
            "[{case}] a hook just written by `apply --write` is reported stale: {msg:?}"
        );
        assert_eq!(
            level, "ok",
            "[{case}] every Hooks line must be green after a clean apply: {msg:?}"
        );
    }
}

/// The reproduction: apply a PROJECT-scope hook, then ask doctor about it.
#[test]
fn a_project_scope_hook_is_in_sync_after_apply() {
    let fx = Fixture::new(None);
    let (ok, out) = fx.run(&["apply", "--write"]);
    assert!(ok, "apply --write failed: {out}");

    assert_hooks_in_sync(&fx.doctor(), "no machine guard");
}

/// The same project on a machine whose guard is wired. `apply` keeps the guard
/// hook out of a repo's config, so `doctor` must not diff it there either.
#[test]
fn a_wired_machine_guard_does_not_make_every_repo_report_stale_hooks() {
    let fx = Fixture::new(Some("version = 1\n\n[guard]\nenabled = true\n"));
    let (ok, out) = fx.run(&["apply", "--write"]);
    assert!(ok, "apply --write failed: {out}");

    assert_hooks_in_sync(&fx.doctor(), "machine guard wired");
}
