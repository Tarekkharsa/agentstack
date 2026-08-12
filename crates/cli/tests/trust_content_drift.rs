// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! `trust-content-drift-v1`: no JSON surface reports a healthy project over
//! content that has drifted from its pin.
//!
//! The consent digest covers the manifest, the local overlay, and the lockfile
//! — not the bodies those bytes pin. So editing an approved skill in place used
//! to leave `trust --preview` saying `state: "trusted"` with an empty blocker
//! list and the edited item marked `change: "unchanged"`, and `status --json`
//! saying `trust: "trusted"`, while `doctor` errored and `agentstack trust`
//! refused. The gate was right and the reporting lied — an invariant-8 breach
//! on exactly the surfaces a driver polls.
//!
//! Two things are witnessed here, and the second is the one that matters most
//! in a year: the surfaces tell the truth, AND a program that reads only JSON
//! can converge. If a driver ever has to read a human sentence to make
//! progress, the loop below stops moving.

use std::fs;
use std::path::Path;

fn run(bin: &str, args: &[&str], home: &Path, cwd: &Path) -> (String, bool) {
    let out = std::process::Command::new(bin)
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

fn json(bin: &str, args: &[&str], home: &Path, proj: &Path) -> serde_json::Value {
    let (text, ok) = run(bin, args, home, proj);
    assert!(ok, "{args:?} failed:\n{text}");
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{args:?} is not JSON ({e}):\n{text}"))
}

fn preview(bin: &str, home: &Path, proj: &Path) -> serde_json::Value {
    json(bin, &["trust", "--preview"], home, proj)
}

fn status(bin: &str, home: &Path, proj: &Path) -> serde_json::Value {
    json(bin, &["status", "--json"], home, proj)
}

/// The non-interactive grant a panel drives: consent bound to the digest the
/// preview just emitted.
fn grant(bin: &str, home: &Path, proj: &Path) -> (String, bool) {
    let digest = preview(bin, home, proj)["surface_digest"]
        .as_str()
        .unwrap()
        .to_string();
    run(bin, &["trust", "--yes", "--consented", &digest], home, proj)
}

fn fixture(tmp: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let home = tmp.join("home");
    let proj = tmp.join("proj");
    let a = proj.join(".agentstack");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(a.join("skills/summarize")).unwrap();
    fs::write(
        a.join("skills/summarize/SKILL.md"),
        "---\nname: summarize\ndescription: sums things up\n---\n\nbody v1\n",
    )
    .unwrap();
    fs::write(
        a.join("agentstack.toml"),
        "version = 1\n\n[skills.summarize]\npath = \"./skills/summarize\"\n",
    )
    .unwrap();
    (home, proj)
}

/// Edit the approved body in place — the manifest and lockfile bytes, and so
/// the trust digest, are untouched.
fn drift(proj: &Path) {
    fs::write(
        proj.join(".agentstack/skills/summarize/SKILL.md"),
        "---\nname: summarize\ndescription: sums things up\n---\n\nbody v2\n",
    )
    .unwrap();
}

fn pinned_and_trusted(bin: &str, home: &Path, proj: &Path) {
    let (text, ok) = run(bin, &["lock", "--write"], home, proj);
    assert!(ok, "lock failed:\n{text}");
    let (text, ok) = grant(bin, home, proj);
    assert!(ok, "grant failed:\n{text}");
}

/// The defect, on both machine surfaces at once.
#[test]
fn drifted_content_is_never_reported_healthy_on_any_json_surface() {
    let bin = env!("CARGO_BIN_EXE_agentstack");
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = fixture(tmp.path());
    pinned_and_trusted(bin, &home, &proj);

    // Healthy before the edit — otherwise the assertions below prove nothing.
    assert_eq!(preview(bin, &home, &proj)["state"], "trusted");
    assert_eq!(status(bin, &home, &proj)["project"]["trust"], "trusted");

    drift(&proj);

    let v = preview(bin, &home, &proj);
    let features: Vec<&str> = v["features"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f.as_str())
        .collect();
    assert!(
        features.contains(&"trust-content-drift-v1"),
        "the payload is not advertised: {features:?}"
    );
    assert_eq!(v["state"], "drifted", "preview still claims health: {v}");
    assert_eq!(v["grantable"], false);
    let blockers = v["blockers"].as_array().unwrap();
    assert_eq!(blockers.len(), 1, "one blocker expected: {v}");
    assert_eq!(blockers[0]["kind"], "skill");
    assert_eq!(blockers[0]["name"], "summarize");
    // Every blocked machine surface carries a command that can be run verbatim.
    assert_eq!(blockers[0]["fix"], "agentstack lock --write");
    assert_eq!(v["fix"], "agentstack lock --write");
    assert_eq!(v["next_step"]["command"], "agentstack lock --write");
    assert_eq!(v["content_drift"].as_array().unwrap().len(), 1);

    // The card item itself: `change` keyed on identity read `unchanged` here,
    // because "where the body comes from" had not moved. Drift outranks it.
    let item = v["review"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["kind"] == "skill" && i["name"] == "summarize")
        .unwrap();
    assert_eq!(item["change"], "drifted", "the item still reads clean: {v}");
    assert_eq!(item["drifted"], true);
    assert_eq!(item["fix"], "agentstack lock --write");
    let group = v["review"]["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["kind"] == "skill")
        .unwrap();
    assert_eq!(group["counts"]["drifted"], 1);
    assert_eq!(group["change"], "changed");

    let s = status(bin, &home, &proj);
    assert_eq!(
        s["project"]["trust"], "drifted",
        "status still claims trusted: {s}"
    );
    assert_eq!(s["project"]["content_drift"].as_array().unwrap().len(), 1);
    assert_eq!(s["next_action"]["command"], "agentstack lock --write");
}

/// The gate is what the reporting must agree WITH: it still refuses, and the
/// refusal is what makes a `trusted` reading a lie rather than a difference of
/// opinion.
#[test]
fn the_grant_still_refuses_drifted_content() {
    let bin = env!("CARGO_BIN_EXE_agentstack");
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = fixture(tmp.path());
    pinned_and_trusted(bin, &home, &proj);
    drift(&proj);

    let (text, ok) = grant(bin, &home, &proj);
    assert!(!ok, "the grant accepted drifted content:\n{text}");
    assert!(
        text.contains("skill content drifted from lock"),
        "the refusal changed its reason:\n{text}"
    );
}

/// A program reading ONLY JSON converges: read, execute the field verbatim,
/// re-read, until every surface reports health. No human sentence is parsed.
#[test]
fn a_json_only_driver_converges_from_drift_to_health() {
    let bin = env!("CARGO_BIN_EXE_agentstack");
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = fixture(tmp.path());
    pinned_and_trusted(bin, &home, &proj);
    drift(&proj);

    for step in 0..5 {
        let v = preview(bin, &home, &proj);
        let s = status(bin, &home, &proj);
        if v["state"] == "trusted"
            && s["project"]["trust"] == "trusted"
            && v["blockers"].as_array().unwrap().is_empty()
        {
            return;
        }
        // One field, run verbatim. Consent is the exception every contract
        // makes: it is not a "fix", so the driver presents the digest the
        // payload already carries rather than being handed a grant command.
        if let Some(fix) = v["fix"].as_str() {
            let args: Vec<&str> = fix.split_whitespace().skip(1).collect();
            let (text, ok) = run(bin, &args, &home, &proj);
            assert!(ok, "step {step}: `{fix}` failed:\n{text}");
        } else {
            let (text, ok) = grant(bin, &home, &proj);
            assert!(ok, "step {step}: grant failed:\n{text}");
        }
    }
    panic!("the JSON-only loop did not converge");
}

// ---------------------------------------------------------------------------
// The never-pinned half of the same flag: the detector must report exactly the
// set the GATE refuses over — no wider, or `grantable: false` blocks a consent
// `agentstack trust --yes` accepts, and a panel gating its Approve control on
// that field refuses the one answer only a human may give.
// ---------------------------------------------------------------------------

/// A machine whose central library holds one path-sourced skill, and a project
/// that names it through a toolset — so the skill resolves with
/// `SkillOrigin::Library`, the origin the gate does NOT block on.
fn library_fixture(tmp: &Path, with_body: bool) -> (std::path::PathBuf, std::path::PathBuf) {
    let home = tmp.join("home");
    let proj = tmp.join("proj");
    let lib = home.join(".agentstack/lib");
    fs::create_dir_all(proj.join(".agentstack")).unwrap();
    fs::create_dir_all(&lib).unwrap();
    fs::write(
        lib.join("library.toml"),
        "version = 1\n\n[[skill]]\nname = \"libhelper\"\nsource = \"path\"\npath = \"libhelper\"\n",
    )
    .unwrap();
    if with_body {
        fs::create_dir_all(lib.join("skills/libhelper")).unwrap();
        fs::write(
            lib.join("skills/libhelper/SKILL.md"),
            "---\nname: libhelper\ndescription: helps\n---\n\nbody\n",
        )
        .unwrap();
    }
    fs::write(
        proj.join(".agentstack/agentstack.toml"),
        "version = 1\n\n[project]\nname = \"p\"\n\n[toolsets.dev]\nskills = [\"libhelper\"]\n",
    )
    .unwrap();
    (home, proj)
}

/// F2. The reporting projection claimed a refusal the gate does not make.
#[test]
fn an_unpinned_library_skill_is_grantable_because_the_gate_grants_it() {
    let bin = env!("CARGO_BIN_EXE_agentstack");
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = library_fixture(tmp.path(), true);

    let v = preview(bin, &home, &proj);
    assert_eq!(
        v["surface_unpinned"].as_array().unwrap().len(),
        0,
        "a library-origin unpinned skill is a yellow advisory at the gate, not a blocker:\n{v:#}"
    );
    assert_eq!(v["grantable"], serde_json::json!(true), "{v:#}");
    assert!(v["blockers"].as_array().unwrap().is_empty(), "{v:#}");

    // The claim and the enforcement, checked against each other.
    let (text, ok) = grant(bin, &home, &proj);
    assert!(
        ok,
        "the gate refused what `grantable: true` promised:\n{text}"
    );
}

/// F1. A body that is not on disk cannot be pinned, so no machine field may
/// name a pinning command over it — that is a poll-and-run loop with no exit.
#[test]
fn a_body_absent_from_disk_never_names_a_command_that_cannot_repair_it() {
    let bin = env!("CARGO_BIN_EXE_agentstack");
    let tmp = tempfile::tempdir().unwrap();

    // Library origin: not a gate blocker at all, so nothing is instructed.
    let (home, proj) = library_fixture(tmp.path(), false);
    let v = preview(bin, &home, &proj);
    assert!(
        v["surface_unpinned"].as_array().unwrap().is_empty(),
        "{v:#}"
    );
    assert_eq!(v["next_step"], serde_json::Value::Null, "{v:#}");
    let s = status(bin, &home, &proj);
    assert!(
        s["project"]["surface_unpinned"]
            .as_array()
            .unwrap()
            .is_empty(),
        "{s:#}"
    );

    // Inline origin: a genuine blocker, but still no fix — `lock --write`
    // resolves before it pins and exits non-zero here, unchanged, forever.
    let tmp2 = tempfile::tempdir().unwrap();
    let (home, proj) = fixture(tmp2.path());
    fs::remove_dir_all(proj.join(".agentstack/skills/summarize")).unwrap();
    let v = preview(bin, &home, &proj);
    assert_eq!(v["grantable"], serde_json::json!(false), "{v:#}");
    let b = &v["blockers"].as_array().unwrap()[0];
    assert_eq!(b["fix"], serde_json::Value::Null, "{v:#}");
    assert!(
        b["reason"]
            .as_str()
            .unwrap()
            .contains("not present on disk"),
        "{v:#}"
    );
    assert_eq!(v["fix"], serde_json::Value::Null, "{v:#}");
    assert_eq!(v["next_step"], serde_json::Value::Null, "{v:#}");
    let (text, ok) = run(bin, &["lock", "--write"], &home, &proj);
    assert!(!ok, "`lock --write` unexpectedly succeeded:\n{text}");
}

/// The other direction of the same narrowing: an INLINE unpinned skill is a
/// real refusal and must keep its blocker, its name, and its working fix.
#[test]
fn an_unpinned_inline_skill_still_blocks_and_still_names_its_fix() {
    let bin = env!("CARGO_BIN_EXE_agentstack");
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = fixture(tmp.path());

    let v = preview(bin, &home, &proj);
    assert_eq!(v["grantable"], serde_json::json!(false), "{v:#}");
    let b = &v["surface_unpinned"].as_array().unwrap()[0];
    assert_eq!(b["kind"], "skill");
    assert_eq!(b["name"], "summarize");
    assert_eq!(b["fix"], "agentstack lock --write");
    assert_eq!(v["next_step"]["command"], "agentstack lock --write");

    let (text, ok) = grant(bin, &home, &proj);
    assert!(!ok, "the gate granted an unpinned inline skill:\n{text}");
    assert!(text.contains("isn't fully pinned"), "{text}");

    // …and the loop it names converges.
    let (text, ok) = run(bin, &["lock", "--write"], &home, &proj);
    assert!(ok, "{text}");
    let v = preview(bin, &home, &proj);
    assert_eq!(v["grantable"], serde_json::json!(true), "{v:#}");
    let (text, ok) = grant(bin, &home, &proj);
    assert!(ok, "{text}");
}

/// Round 7, finding 1, across all three machine surfaces at once.
///
/// `trust --preview` honoured `ContentDrift::fix = None`; `doctor --json` and
/// `status --json` threw it away and named `agentstack lock --write` anyway.
/// One project, two answers, and the loud one was wrong. The worse shape is
/// the second fixture: with every OTHER declared item pinned, that command
/// exits 0 and prints a green tick while the blocking condition stands, so a
/// driver cannot detect failure by exit code either.
#[test]
fn no_surface_names_a_command_for_a_body_that_is_not_on_disk() {
    let bin = env!("CARGO_BIN_EXE_agentstack");

    // 1a — the only declared skill, body directory removed.
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = fixture(tmp.path());
    fs::remove_dir_all(proj.join(".agentstack/skills/summarize")).unwrap();
    agree_on_no_command(bin, &home, &proj, "summarize");

    // 1b — a skill declared outside every toolset, body missing, beside one
    // that pins cleanly. One `lock --write` (exit 0) reaches the same state.
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = fixture(tmp.path());
    fs::write(
        proj.join(".agentstack/agentstack.toml"),
        "version = 1\n\n[skills.summarize]\npath = \"./skills/summarize\"\n\n\
         [skills.orphan]\npath = \"./skills/orphan\"\n\n\
         [toolsets.dev]\nskills = [\"summarize\"]\n",
    )
    .unwrap();
    let (text, ok) = run(bin, &["lock", "--write"], &home, &proj);
    assert!(ok, "`lock --write` is expected to exit 0 here:\n{text}");
    agree_on_no_command(bin, &home, &proj, "orphan");
}

/// All three surfaces answer "no command", each still says what is wrong, and
/// a driver polling either of the two that used to loop now terminates.
fn agree_on_no_command(bin: &str, home: &Path, proj: &Path, name: &str) {
    let v = preview(bin, home, proj);
    assert_eq!(v["fix"], serde_json::Value::Null, "{v:#}");
    assert_eq!(v["next_step"], serde_json::Value::Null, "{v:#}");

    let d = json(bin, &["doctor", "--json"], home, proj);
    assert_eq!(d["next_action"], serde_json::Value::Null, "{d:#}");
    let s = status(bin, home, proj);
    assert_eq!(
        s["next_action"]["command"],
        serde_json::Value::Null,
        "{s:#}"
    );

    // The machine field goes quiet; the prose must not.
    for sentence in [
        d["next_step"].as_str().unwrap_or_default(),
        s["next_action"]["sentence"].as_str().unwrap_or_default(),
    ] {
        assert!(
            sentence.contains(name) && sentence.contains("not present on disk"),
            "the human sentence lost the finding: {sentence:?}"
        );
    }

    // And the per-item `fix` is null on the itemised `status` list too, not
    // just in the headline.
    let items = s["project"]["surface_unpinned"].as_array().unwrap();
    assert!(!items.is_empty(), "{s:#}");
    assert!(
        items.iter().all(|i| i["fix"].is_null()),
        "an item no command repairs must not carry one: {s:#}"
    );
}
