// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! `doctor --json` describes the project it was NAMED, never the directory it
//! was launched from.
//!
//! The panel renders doctor faithfully, so a panel and a terminal that
//! disagree about the same project are two doctor answers, not two renderers.
//! t3code's server spawns from `/`; a human runs the same command from inside
//! the repo. With the same absolute `--manifest-dir` those two must be the
//! same report — otherwise only one of them can be right and the reader has no
//! way to tell which.
//!
//! The assertion is whole-report equality rather than a field pair. Every
//! reading doctor makes (scope, state keys, disk probes, trust, delivery mode)
//! is derived from the manifest dir, so ANY of them drifting toward the
//! process cwd is the same defect; naming only `next_action` and the warning
//! count would leave the next one to be found by running the product again.
//!
//! Both fixtures are deliberately past the point where doctor has real work to
//! do: a plain rendered project, and the abandoned-render shape — a config an
//! earlier `apply` wrote that is still on disk after the servers moved to the
//! live lane. The second is the branch whose two sides are "nothing to report"
//! and "`agentstack more unrender --write`", i.e. the widest gap two answers about
//! one project can have.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Fixture {
    _tmp: assert_fs::TempDir,
    home: PathBuf,
    proj: PathBuf,
}

impl Fixture {
    fn new(manifest: &str) -> Self {
        let tmp = assert_fs::TempDir::new().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(home.join(".agentstack")).unwrap();
        let proj = tmp.path().join("proj");
        fs::create_dir_all(proj.join(".git")).unwrap();
        fs::create_dir_all(proj.join(".agentstack")).unwrap();
        let fx = Fixture {
            _tmp: tmp,
            home,
            proj,
        };
        fx.write_manifest(manifest);
        fx
    }

    fn write_manifest(&self, body: &str) {
        fs::write(self.proj.join(".agentstack/agentstack.toml"), body).unwrap();
    }

    fn run_in(&self, cwd: &Path, args: &[&str]) -> (bool, String) {
        let out = Command::new(env!("CARGO_BIN_EXE_agentstack"))
            .args(args)
            .current_dir(cwd)
            .env("HOME", &self.home)
            .env("AGENTSTACK_HOME", self.home.join(".agentstack"))
            .output()
            .unwrap();
        let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&out.stderr));
        (out.status.success(), text)
    }

    fn run(&self, args: &[&str]) -> (bool, String) {
        let proj = self.proj.clone();
        self.run_in(&proj, args)
    }

    /// Consent bound to the exact bytes, through the real ceremony. Re-run
    /// after any manifest edit: trust pins the digest, and a stale pin would
    /// change what doctor reports for reasons that have nothing to do with the
    /// cwd.
    fn trust(&self) {
        let (ok, preview) = self.run(&["trust", "--preview"]);
        assert!(ok, "trust --preview failed: {preview}");
        let surface: serde_json::Value = serde_json::from_str(&preview).unwrap();
        let digest = surface["surface_digest"].as_str().unwrap().to_string();
        let (ok, out) = self.run(&["trust", ".", "--yes", "--consented", &digest]);
        assert!(ok, "trust failed: {out}");
    }

    /// `doctor --json` for THIS project, launched from `cwd`. `--all` keeps
    /// every section in the comparison, including the ones relevance would
    /// hide — a section that appears from one cwd and not the other is exactly
    /// the divergence under test.
    fn doctor_from(&self, cwd: &Path) -> serde_json::Value {
        let dir = self.proj.display().to_string();
        let (_, out) = self.run_in(cwd, &["doctor", "--json", "--all", "--manifest-dir", &dir]);
        serde_json::from_str(&out).unwrap_or_else(|e| {
            panic!(
                "doctor --json is not JSON ({e}) from {}: {out}",
                cwd.display()
            )
        })
    }

    /// The property, over every cwd worth naming: the project's own directory,
    /// the filesystem root (what a spawned server gets), and the machine home
    /// (a directory that carries a manifest of its own, so a cwd-derived read
    /// would find something plausible and wrong).
    fn assert_same_report_from_every_cwd(&self, case: &str) {
        let proj = self.proj.clone();
        let baseline = self.doctor_from(&proj);
        for cwd in [Path::new("/"), self.home.as_path()] {
            let other = self.doctor_from(cwd);
            assert_eq!(
                baseline["next_action"], other["next_action"],
                "[{case}] doctor names a different next action from {} than from the project itself",
                cwd.display()
            );
            assert_eq!(
                baseline["warnings"],
                other["warnings"],
                "[{case}] doctor counts different warnings from {} than from the project itself",
                cwd.display()
            );
            assert_eq!(
                baseline,
                other,
                "[{case}] the whole report differs when launched from {}",
                cwd.display()
            );
        }
    }
}

/// A rendered project: configs on disk, pinned, trusted — the ordinary state a
/// panel polls.
#[test]
fn a_rendered_project_reads_the_same_from_anywhere() {
    let fx = Fixture::new(
        "version = 1\n\
         \n[delivery]\nrender_locally = true\n\
         \n[targets]\ndefault = [\"claude-code\"]\n\
         \n[servers.demo]\ntype = \"http\"\nurl = \"https://demo.example/mcp\"\n",
    );
    fx.trust();
    let (ok, out) = fx.run(&["apply", "--write"]);
    assert!(ok, "apply --write failed: {out}");
    let (ok, out) = fx.run(&["lock", "--write"]);
    assert!(ok, "lock --write failed: {out}");

    fx.assert_same_report_from_every_cwd("rendered project");
}

/// The abandoned render: `apply` wrote a server config, then the servers moved
/// to the live lane and nothing new is written there — but the harness still
/// reads the file. This is the finding whose one next action is
/// `agentstack more unrender --write`, and the state in which the panel and the
/// terminal were seen describing one project two ways.
#[test]
fn an_abandoned_render_reads_the_same_from_anywhere() {
    let fx = Fixture::new(
        "version = 1\n\
         \n[delivery]\nrender_locally = true\n\
         \n[targets]\ndefault = [\"claude-code\"]\n\
         \n[servers.demo]\ntype = \"http\"\nurl = \"https://demo.example/mcp\"\n",
    );
    fx.trust();
    let (ok, out) = fx.run(&["apply", "--write"]);
    assert!(ok, "apply --write failed: {out}");
    let (ok, out) = fx.run(&["lock", "--write"]);
    assert!(ok, "lock --write failed: {out}");

    // Drop `render_locally`: the servers route live from here on, and the file
    // the write above left behind becomes the finding.
    fx.write_manifest(
        "version = 1\n\
         \n[targets]\ndefault = [\"claude-code\"]\n\
         \n[servers.demo]\ntype = \"http\"\nurl = \"https://demo.example/mcp\"\n",
    );
    fx.trust();

    // The fixture is only worth comparing if it actually reached the finding.
    let proj = fx.proj.clone();
    let report = fx.doctor_from(&proj);
    assert!(
        report["warnings"].as_u64().unwrap_or(0) >= 1,
        "the fixture must reach the abandoned-render finding: {report}"
    );

    fx.assert_same_report_from_every_cwd("abandoned render");
}
