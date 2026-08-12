// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! P7 — the library rot view, end to end through the real binary.
//!
//! A central library solves distribution but makes CAPABILITY ROT worse: an
//! index that only ever grows becomes the mess scattered static configs were.
//! `agentstack lib list` therefore answers two questions, not one: what is in
//! here, and what is dead?
//!
//! Two claims are witnessed here, both of which decide whether the feature
//! helps or harms:
//!
//! 1. **The honesty rule** (invariant 8 at the library level). An entry with no
//!    usage history reads "no data", NEVER "0 uses". "Never used" and "we have
//!    no record" are different claims and the difference decides whether
//!    someone deletes a skill. The unit tests in `commands::lib::rot_tests`
//!    pin the rendering; this pins the real command's real output.
//! 2. **One drift opinion.** A library entry whose bytes moved after the
//!    project pinned it is marked DRIFTED — and it is marked because
//!    `resolve::skill_lock_status`, the same seam the trust gate and
//!    `trust --preview` read, says so. There is no second drift computation to
//!    disagree with the gate.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

fn run(bin: &str, args: &[&str], home: &Path, cwd: &Path) -> (String, bool) {
    let out = Command::new(bin)
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("HOME", home)
        .env("AGENTSTACK_HOME", home.join(".agentstack"))
        .env("PATH", "/usr/bin:/bin")
        .env("NO_COLOR", "1")
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

/// Write a skill body under `src/<name>/SKILL.md`.
fn author(src: &Path, name: &str, description: &str, body: &str) {
    let dir = src.join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n\n{body}\n"),
    )
    .unwrap();
}

/// The row for one entry, so assertions read against a line rather than the
/// whole page.
fn row<'a>(out: &'a str, name: &str) -> &'a str {
    out.lines()
        .find(|l| l.split_whitespace().nth(1) == Some(name))
        .unwrap_or_else(|| panic!("no rot row for '{name}' in:\n{out}"))
}

#[test]
fn lib_list_names_what_is_dead_without_ever_inventing_a_zero() {
    let bin = env!("CARGO_BIN_EXE_agentstack");
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let proj = tmp.path().join("proj");
    let src = tmp.path().join("src");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&proj).unwrap();
    fs::create_dir_all(&src).unwrap();

    author(&src, "incident", "Run the incident checklist", "body v1");
    author(&src, "pdf-extract", "Pull tables out of PDFs", "body v1");
    author(&src, "legacy-deploy", "The 2024 deploy dance", "body v1");

    for name in ["incident", "pdf-extract", "legacy-deploy"] {
        let path = src.join(name);
        let (out, ok) = run(
            bin,
            &["lib", "add", &path.display().to_string(), "--write"],
            &home,
            &proj,
        );
        assert!(ok, "lib add {name} failed:\n{out}");
    }

    // ── Before any project exists: nothing has been measured, so every row
    // must say so rather than claiming the entries are dead. This is the case
    // the honesty rule exists for — a fresh machine's library is not rot.
    let (out, ok) = run(bin, &["lib", "list"], &home, &proj);
    assert!(ok, "lib list failed:\n{out}");
    assert!(
        !out.contains("0 uses") && !out.contains("NEVER USED"),
        "a library with no usage history claimed entries were unused:\n{out}"
    );
    assert!(row(&out, "incident").contains("no data"), "{out}");
    assert!(
        out.contains("no usage history at all — no data, not \"unused\""),
        "{out}"
    );

    // ── A project that pins and activates two of the three.
    let (out, ok) = run(bin, &["init", "--yes", "--secrets", "skip"], &home, &proj);
    assert!(ok, "init failed:\n{out}");
    let manifest = proj.join(".agentstack/agentstack.toml");
    // Appended, not substituted: a machine with no agent CLIs installed gets a
    // bare manifest with no `[toolsets]` section to substitute into.
    let mut text = fs::read_to_string(&manifest).unwrap();
    text.push_str("\n[toolsets.default]\nskills = [\"incident\", \"pdf-extract\"]\n");
    fs::write(&manifest, text).unwrap();
    let (out, ok) = run(bin, &["lock", "--write"], &home, &proj);
    assert!(ok, "lock failed:\n{out}");
    // The manifest edit above re-gated consent, and an activation delivers
    // content — so review and grant, exactly as a human would, or the trust
    // gate (`render::skills::trust_refusal`) blocks the very materialization
    // the usage meter is here to count.
    let (preview, ok) = run(bin, &["trust", "--preview"], &home, &proj);
    assert!(ok, "trust --preview failed:\n{preview}");
    let digest = serde_json::from_str::<serde_json::Value>(&preview)
        .unwrap_or_else(|e| panic!("trust --preview is not JSON ({e}):\n{preview}"))
        ["surface_digest"]
        .as_str()
        .expect("preview carries a surface digest")
        .to_string();
    let (out, ok) = run(
        bin,
        &["trust", "--yes", "--consented", &digest],
        &home,
        &proj,
    );
    assert!(ok, "grant failed:\n{out}");
    // Activation is what the usage meter counts; ignore the delivery outcome,
    // which depends on which harnesses this machine has.
    let _ = run(bin, &["use", "default", "--write"], &home, &proj);

    // ── The library copy of one pinned skill moves after the pin. Only the
    // shared lock-status seam decides this; nothing here re-digests anything.
    let drifted = home.join(".agentstack/lib/skills/incident/SKILL.md");
    let body = fs::read_to_string(&drifted).unwrap();
    fs::write(&drifted, format!("{body}\nedited after the pin\n")).unwrap();

    let (out, ok) = run(bin, &["lib", "list"], &home, &proj);
    assert!(ok, "lib list failed:\n{out}");

    // What it is, and which linked source it came from.
    assert!(row(&out, "incident").starts_with("  skill"), "{out}");
    assert!(row(&out, "incident").contains("local"), "{out}");

    // Used, and drifted — one reading, stated where a person looks.
    assert!(
        row(&out, "incident").contains("DRIFTED — content changed since it was pinned"),
        "{out}"
    );
    // Used, and still matching its pin.
    assert!(row(&out, "pdf-extract").contains("matches lock"), "{out}");
    assert!(!row(&out, "pdf-extract").contains("DRIFTED"), "{out}");

    // Never used — now a supportable claim, because the activation meter
    // exists and has history that does not include this entry.
    assert!(row(&out, "legacy-deploy").contains("NEVER USED"), "{out}");

    // The honesty rule again, on the axis the meter cannot answer: activation
    // counts carry no timestamps, so "last used" stays "no data" even for the
    // entries that plainly WERE used. It never degrades to a zero or a dash.
    assert!(row(&out, "incident").contains("last: no data"), "{out}");
    assert!(
        !out.contains("0 uses"),
        "rendered a zero for absent data:\n{out}"
    );

    // The one-line summary, and the reversible action that answers it.
    assert!(
        out.contains("1 of 3 measurable entries never used."),
        "{out}"
    );
    assert!(
        out.contains("1 entry drifted from this project's lock"),
        "{out}"
    );
    assert!(
        out.contains("agentstack lib remove <name> --write"),
        "{out}"
    );
    assert!(out.contains("agentstack lib trash"), "{out}");
}

/// An empty library says "nothing installed" and stops: a health report about
/// nothing is noise, and a summary line over zero entries would be the kind of
/// hollow number this view exists to avoid.
#[test]
fn an_empty_library_gets_no_rot_report() {
    let bin = env!("CARGO_BIN_EXE_agentstack");
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let proj = tmp.path().join("proj");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&proj).unwrap();

    let (out, ok) = run(bin, &["lib", "list"], &home, &proj);
    assert!(ok, "lib list failed:\n{out}");
    assert!(
        out.contains("No skills, servers, extensions, or hooks"),
        "{out}"
    );
    assert!(!out.contains("What is dead in here"), "{out}");
    assert!(!out.contains("NEVER USED"), "{out}");
}
