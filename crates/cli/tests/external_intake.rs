// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Phase 4 — governed intake from an external ecosystem.
//!
//! `add from <path-or-url>` consumes supply that was never designed for us and
//! puts it through the same funnel as everything else: fetch → bound →
//! quarantine → card → yes.
//!
//! The load-bearing property, witnessed the Phase 1 way, is that **intake never
//! becomes activation**: the accepted run and its declined twin are run against
//! identical inputs, and the declined one must leave the project byte-identical
//! with nothing staged anywhere.
//!
//! Everything else here is invariant 7 at the door — a package that traverses,
//! a connection shipping a live credential, a registry listing that must not
//! install anything by being read.

use std::fs;
use std::process::Command;

fn exe() -> &'static str {
    env!("CARGO_BIN_EXE_agentstack")
}

struct Fix {
    _tmp: assert_fs::TempDir,
    home: std::path::PathBuf,
    proj: std::path::PathBuf,
    supply: std::path::PathBuf,
}

fn fixture() -> Fix {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let proj = tmp.path().join("proj");
    let supply = tmp.path().join("supply");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(proj.join(".agentstack")).unwrap();
    fs::write(
        proj.join(".agentstack/agentstack.toml"),
        "version = 1\n[servers.notes]\ntype = \"stdio\"\ncommand = \"echo\"\n",
    )
    .unwrap();
    Fix {
        _tmp: tmp,
        home,
        proj,
        supply,
    }
}

impl Fix {
    fn run(&self, args: &[&str]) -> (String, bool) {
        let out = Command::new(exe())
            .args(args)
            .arg("--manifest-dir")
            .arg(&self.proj)
            .env("HOME", &self.home)
            .env("AGENTSTACK_HOME", self.home.join(".agentstack"))
            .env("NO_COLOR", "1")
            .output()
            .expect("the binary must run");
        let mut s = String::from_utf8_lossy(&out.stdout).to_string();
        s.push_str(&String::from_utf8_lossy(&out.stderr));
        (s, out.status.success())
    }

    /// An eve-format skill package: SKILL.md with a licence in frontmatter,
    /// plus the LICENSE text the obligation actually lives in.
    fn eve_package(&self) -> String {
        let pkg = self.supply.join("summarize-pro");
        fs::create_dir_all(&pkg).unwrap();
        fs::write(
            pkg.join("SKILL.md"),
            "---\nname: summarize-pro\nlicense: Apache-2.0\n\
             description: Summarize long documents.\n---\n\nSummarize it.\n",
        )
        .unwrap();
        fs::write(
            pkg.join("LICENSE"),
            "Apache License, Version 2.0\n\nCopyright 2026 Someone Else\n",
        )
        .unwrap();
        pkg.to_string_lossy().to_string()
    }

    /// Everything under `.agentstack/`, as (relative path, bytes).
    fn snapshot(&self) -> Vec<(String, Vec<u8>)> {
        let root = self.proj.join(".agentstack");
        let mut out = Vec::new();
        fn walk(dir: &std::path::Path, root: &std::path::Path, out: &mut Vec<(String, Vec<u8>)>) {
            let Ok(rd) = fs::read_dir(dir) else { return };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, root, out);
                } else {
                    let rel = p
                        .strip_prefix(root)
                        .unwrap_or(&p)
                        .to_string_lossy()
                        .to_string();
                    out.push((rel, fs::read(&p).unwrap_or_default()));
                }
            }
        }
        walk(&root, &root, &mut out);
        out.sort();
        out
    }
}

/// End to end: fetch → quarantine → card with attribution → yes → live.
#[test]
fn an_external_package_flows_through_the_funnel_to_live() {
    let _env = agentstack::util::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let fix = fixture();
    let pkg = fix.eve_package();

    // The card, shown by the real binary. Headless runs cannot accept (F15),
    // so this run is also the proof that `--write` alone no longer consents.
    let (out, ok) = fix.run(&["add", "from", &pkg, "--write"]);
    assert!(ok, "{out}");
    assert!(
        !fix.proj.join(".agentstack/skills").exists(),
        "--write alone must not be consent for external supply:\n{out}"
    );

    // The accept, through the injected-answer seam production prompts on.
    std::env::set_var("HOME", &fix.home);
    std::env::set_var("AGENTSTACK_HOME", fix.home.join(".agentstack"));
    agentstack::commands::intake_external::run_answered(
        &pkg,
        &fix.proj.join(".agentstack"),
        Some(true),
    )
    .unwrap();

    // The card, in order: origin, unsigned warning, what it adds, attribution.
    assert!(out.contains("origin"), "the origin must be named:\n{out}");
    assert!(
        out.contains("unsigned source"),
        "an unsigned external source must say so:\n{out}"
    );
    assert!(
        out.contains("Apache-2.0") && out.contains("summarize-pro"),
        "attribution must be ON the card, in 'Apache-2.0, from <origin>' shape:\n{out}"
    );
    assert!(
        out.contains("LICENSE/NOTICE text comes with it"),
        "carrying the NOTICE text is the part a bare SPDX tag does not satisfy, \
         so the card must say it happened:\n{out}"
    );
    assert!(
        out.contains("nothing is active"),
        "staging must be described as inert:\n{out}"
    );

    // It landed where the ordinary funnel can see it.
    let landed = fix.proj.join(".agentstack/skills/summarize-pro/SKILL.md");
    assert!(landed.exists(), "content must land on a yes:\n{out}");
    // The LICENSE text came too — mechanically, not by promise.
    assert!(
        fix.proj
            .join(".agentstack/skills/summarize-pro/LICENSE")
            .exists(),
        "the LICENSE file itself must travel with the content:\n{out}"
    );
    assert!(
        !fix.proj.join(".agentstack/quarantine").exists(),
        "quarantine must be emptied once accepted:\n{out}"
    );
}

/// Two packages coexist. Each keeps its own directory.
///
/// The bug this pins: entries were staged relative to the PACKAGE rather than
/// to the kind's root, so a package's `SKILL.md` adopted to `skills/SKILL.md`
/// with the skill's own directory gone — and the second import collided with
/// the first over a file neither of them was. The end-to-end witness caught it
/// only because it checked where the file landed instead of trusting the
/// command's "2 file(s)" summary.
#[test]
fn two_imported_packages_keep_their_own_directories() {
    let _env = agentstack::util::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let fix = fixture();
    let first = fix.eve_package();
    let second = fix.supply.join("translate");
    fs::create_dir_all(&second).unwrap();
    fs::write(
        second.join("SKILL.md"),
        "---\nname: translate\nlicense: MIT\n---\n\nTranslate it.\n",
    )
    .unwrap();

    std::env::set_var("HOME", &fix.home);
    std::env::set_var("AGENTSTACK_HOME", fix.home.join(".agentstack"));
    let dir = fix.proj.join(".agentstack");
    agentstack::commands::intake_external::run_answered(&first, &dir, Some(true)).unwrap();
    agentstack::commands::intake_external::run_answered(second.to_str().unwrap(), &dir, Some(true))
        .expect("the second import must not collide with the first");
    let (a, b) = (String::new(), String::new());
    let _ = (&a, &b);

    assert!(
        fix.proj
            .join(".agentstack/skills/summarize-pro/SKILL.md")
            .exists(),
        "the first package must keep its directory:\n{a}"
    );
    assert!(
        fix.proj
            .join(".agentstack/skills/translate/SKILL.md")
            .exists(),
        "and so must the second:\n{b}"
    );
    // And they are not the same file wearing two names.
    let one =
        fs::read_to_string(fix.proj.join(".agentstack/skills/summarize-pro/SKILL.md")).unwrap();
    let two = fs::read_to_string(fix.proj.join(".agentstack/skills/translate/SKILL.md")).unwrap();
    assert_ne!(one, two, "each package must keep its own content");
}

/// **The declined twin.** Same input, same command, no yes — and the project is
/// byte-identical with nothing left staged. This is the Phase 1 property, and
/// it is what makes "intake never becomes activation" a fact rather than a
/// slogan.
#[test]
fn the_declined_twin_leaves_nothing_anywhere() {
    let fix = fixture();
    let pkg = fix.eve_package();
    let before = fix.snapshot();

    // No `--write`, no terminal: the answer is no.
    let (out, ok) = fix.run(&["add", "from", &pkg]);
    assert!(ok, "a decline is not an error:\n{out}");

    // The user still saw the card — declining is an informed act.
    assert!(
        out.contains("Apache-2.0"),
        "the card must be shown before the decision:\n{out}"
    );

    assert_eq!(
        fix.snapshot(),
        before,
        "a declined intake must leave the project byte-identical:\n{out}"
    );
    assert!(
        !fix.proj.join(".agentstack/quarantine").exists(),
        "and nothing staged for later — a decline is not a deferral:\n{out}"
    );
    assert!(
        !fix.proj.join(".agentstack/skills").exists(),
        "nothing may reach the intake directories:\n{out}"
    );
}

/// A connection definition shipping a LIVE credential: the value is replaced
/// with a `${REF}` on the way in, the user is told the source did that, and
/// the raw secret is written nowhere.
#[test]
fn a_fetched_credential_becomes_a_ref_and_is_never_written_down() {
    let fix = fixture();
    fs::create_dir_all(&fix.supply).unwrap();
    let conn = fix.supply.join("connection.json");
    fs::write(
        &conn,
        r#"{"name":"web-search","command":"npx","args":["-y","@acme/search-mcp"],
            "env":{"SEARCH_API_KEY":"sk-live-9f3a2b7c1d8e4f6a0b2c3d4e"}}"#,
    )
    .unwrap();

    let (out, ok) = fix.run(&["add", "from", conn.to_str().unwrap(), "--write"]);
    assert!(ok, "{out}");

    assert!(
        out.contains("${SEARCH_API_KEY}"),
        "the credential must be shown as a ref:\n{out}"
    );
    assert!(
        !out.contains("sk-live-9f3a2b7c1d8e4f6a0b2c3d4e"),
        "the raw credential must never be printed:\n{out}"
    );
    assert!(
        out.contains("NOT kept"),
        "the user must be told the source shipped a live value and that it was \
         discarded — that is the whole value of having done it:\n{out}"
    );

    // And nowhere on disk, in any file, under the project.
    for (path, bytes) in fix.snapshot() {
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            !text.contains("sk-live-9f3a2b7c1d8e4f6a0b2c3d4e"),
            "the credential reached {path}"
        );
    }
}

/// Reading a registry listing installs nothing. Browsing and taking are
/// different acts; collapsing them is exactly the ungated behaviour this funnel
/// exists to not reproduce.
#[test]
fn listing_a_registry_installs_nothing() {
    let fix = fixture();
    fs::create_dir_all(&fix.supply).unwrap();
    let reg = fix.supply.join("registry.json");
    fs::write(
        &reg,
        r#"[{"name":"summarize-pro","kind":"skill","license":"Apache-2.0",
             "description":"Summarize long documents."},
            {"name":"web-search","kind":"server","description":"Search the web."}]"#,
    )
    .unwrap();
    let before = fix.snapshot();

    // Even WITH --write, which is the accept flag everywhere else.
    let (out, ok) = fix.run(&["add", "from", reg.to_str().unwrap(), "--write"]);
    assert!(ok, "{out}");

    assert!(
        out.contains("summarize-pro") && out.contains("web-search"),
        "the listing must be shown:\n{out}"
    );
    assert!(
        out.contains("nothing was fetched or staged"),
        "and it must say plainly that nothing happened:\n{out}"
    );
    assert_eq!(
        fix.snapshot(),
        before,
        "reading a catalog must never install from it, even with --write:\n{out}"
    );
}

/// Invariant 7 at the door: a package whose file walks upward is refused, and
/// nothing is written outside the project.
#[test]
fn a_traversing_package_is_refused() {
    let fix = fixture();
    let pkg = fix.supply.join("evil");
    fs::create_dir_all(pkg.join("nested")).unwrap();
    fs::write(pkg.join("SKILL.md"), "---\nname: evil\n---\nhi\n").unwrap();
    // A real symlink pointing out of the package — the shape a tarball uses to
    // escape. It must be skipped, not followed and staged.
    #[cfg(unix)]
    std::os::unix::fs::symlink("/etc/hosts", pkg.join("nested/escape.md")).ok();

    let (out, _ok) = fix.run(&["add", "from", pkg.to_str().unwrap(), "--write"]);

    let staged_escape = fix.proj.join(".agentstack/skills/evil/nested/escape.md");
    assert!(
        !staged_escape.exists(),
        "a symlink out of the package must not be followed into the project:\n{out}"
    );
    // Whatever else happened, nothing from outside the package came along.
    for (path, bytes) in fix.snapshot() {
        assert!(
            !String::from_utf8_lossy(&bytes).contains("localhost"),
            "content from outside the package reached {path}"
        );
    }
}

/// F16 witness (FINDINGS.md): `adopt` must never write THROUGH a symlinked
/// destination. A repo shipping `.agentstack/skills` as a link to a sensitive
/// directory would otherwise receive every adopted byte at the link's target.
/// The tamper is the destination link, not the source (source links were
/// already skipped).
#[test]
fn adopt_refuses_a_symlinked_destination() {
    let _env = agentstack::util::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let fix = fixture();
    std::env::set_var("HOME", &fix.home);
    std::env::set_var("AGENTSTACK_HOME", fix.home.join(".agentstack"));

    // The attacker's target: a directory outside the project.
    let target = fix._tmp.path().join("victim");
    fs::create_dir_all(&target).unwrap();
    // The repo ships `.agentstack/skills` as a link to it.
    let skills = fix.proj.join(".agentstack/skills");
    std::os::unix::fs::symlink(&target, &skills).unwrap();

    let pkg = fix.eve_package();
    let err = agentstack::commands::intake_external::run_answered(
        &pkg,
        &fix.proj.join(".agentstack"),
        Some(true),
    );
    assert!(
        err.is_err(),
        "adopt through a symlinked destination succeeded"
    );
    // Nothing reached the link's target.
    assert!(
        !target.join("summarize-pro/SKILL.md").exists(),
        "bytes were written through the destination symlink"
    );
}

/// F3 witness: content that arrived through `add from` is recorded as
/// received, so the intake scanner never labels those exact bytes "your own
/// work". The tamper is the laundering route — adopt lands received files
/// untracked in git, which is precisely what used to read as local work.
#[test]
fn received_content_is_not_labeled_local_work() {
    let _env = agentstack::util::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let fix = fixture();
    std::env::set_var("HOME", &fix.home);
    std::env::set_var("AGENTSTACK_HOME", fix.home.join(".agentstack"));
    let pkg = fix.eve_package();
    let dir = fix.proj.join(".agentstack");

    agentstack::commands::intake_external::run_answered(&pkg, &dir, Some(true)).unwrap();

    // The adopted skill is now in the intake dir, untracked in git. Its
    // provenance must NOT be locally-authored — it arrived from a stranger.
    let base = agentstack::manifest::project_root_of(&dir);
    let loaded = agentstack::manifest::load_from_dir(&dir).unwrap();
    let found = agentstack::intake::scan(&dir, &base, &loaded.manifest);
    let item = found
        .items
        .iter()
        .find(|i| i.name == "summarize-pro")
        .expect("the received skill is seen by intake");
    assert!(
        !item.provenance.is_local(),
        "received content was labeled the user's own work: {:?}",
        item.provenance.reason()
    );
    assert!(
        item.provenance.reason().contains("receive"),
        "the reason should name the arrival route: {:?}",
        item.provenance.reason()
    );
}

/// Serve one HTTP response, once, from a background thread. Returns the URL.
/// The body is served at any path; the caller controls what shape it is.
fn serve_once(body: &'static str, content_type: &'static str) -> String {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        if let Some(Ok(mut s)) = listener.incoming().next() {
            let mut tmp = [0u8; 2048];
            let _ = s.read(&mut tmp);
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = s.write_all(resp.as_bytes());
        }
    });
    format!("http://{addr}/acme.json")
}

/// F2 witness (FINDINGS.md): a URL serving object-shaped connection JSON with
/// a live credential must go through the SAME shape detection + redaction the
/// local path gets — not be staged verbatim as a SKILL.md. The finding's
/// bypass was the network branch specifically; every prior test used a local
/// path. The tamper is the transport: identical bytes, delivered over http.
#[test]
fn a_url_connection_is_redacted_not_staged_as_a_skill() {
    let _env = agentstack::util::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let fix = fixture();
    std::env::set_var("HOME", &fix.home);
    std::env::set_var("AGENTSTACK_HOME", fix.home.join(".agentstack"));

    let url = serve_once(
        r#"{"name":"web-search","command":"npx","args":["-y","@acme/search-mcp"],
            "env":{"SEARCH_API_KEY":"sk-live-9f3a2b7c1d8e4f6a0b2c3d4e"}}"#,
        "application/json",
    );

    // Headless connection report needs no answer (nothing is staged), so the
    // real binary can drive it.
    let (out, ok) = fix.run(&["add", "from", &url]);
    assert!(ok, "{out}");
    // It was recognized as a CONNECTION, not staged as skill content…
    assert!(
        out.contains("MCP server definition"),
        "a URL connection must be recognized as one, not staged as a skill:\n{out}"
    );
    // …the live key was turned into a ${REF} and named as replaced…
    assert!(
        out.contains("SEARCH_API_KEY") && out.contains("were NOT kept"),
        "the credential must be redacted to a ${{REF}} on the way in:\n{out}"
    );
    // …and the raw secret is written NOWHERE under the project.
    for (path, bytes) in fix.snapshot() {
        assert!(
            !String::from_utf8_lossy(&bytes).contains("sk-live-"),
            "a live credential from a URL reached {path}"
        );
    }
    // No SKILL.md holding the connection body was staged.
    assert!(
        !fix.proj.join(".agentstack/skills/acme/SKILL.md").exists(),
        "a URL connection was staged as skill content:\n{out}"
    );
}
