// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Progressive disclosure: every doctor check always runs (the JSON carries all
//! sections and the error/warning counters are display-independent), but each
//! section is tagged `relevant` so the default terminal report can hide the
//! ones for features a project doesn't use. These tests pin the tagging.

use std::fs;

use agentstack::commands::doctor;

static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn relevant(report: &serde_json::Value, title: &str) -> bool {
    report["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["title"] == title)
        .unwrap_or_else(|| panic!("section '{title}' missing from doctor JSON"))["relevant"]
        .as_bool()
        .unwrap()
}

#[test]
fn unused_feature_sections_are_tagged_irrelevant_but_still_reported() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    // A near-empty project: one target, nothing else — no servers, skills,
    // profiles, instructions, packs, or bridge registration.
    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n",
    )
    .unwrap();

    let report = doctor::collect(Some(&proj)).unwrap();

    // Unused features are tagged irrelevant — but their sections still exist
    // in the JSON: checks ran, nothing was skipped.
    for title in [
        "Zero-files gateway",
        "Secrets",
        "Drift",
        "Instructions",
        "Quirks",
        "Skills",
        "Content scan",
        "Reproducibility",
    ] {
        assert!(
            !relevant(&report, title),
            "'{title}' must be tagged irrelevant for a project that doesn't use it"
        );
    }

    // The baseline and the machine-policy summary stay relevant always.
    assert!(relevant(&report, "Adapters & CLIs"));
    assert!(relevant(&report, "Machine policy"));
}

#[test]
fn used_features_stay_relevant() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    let proj = tmp.path().join("proj");
    fs::create_dir_all(proj.join("skills/helper")).unwrap();
    fs::write(proj.join("skills/helper/SKILL.md"), "# helper\n").unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [servers.demo]\ntype = \"http\"\nurl = \"https://demo.example/mcp\"\n\
         headers = { Authorization = \"Bearer ${DEMO_TOKEN}\" }\n\
         [skills.helper]\npath = \"./skills/helper\"\n\
         [profiles.p]\nskills = [\"helper\"]\n",
    )
    .unwrap();

    let report = doctor::collect(Some(&proj)).unwrap();

    for title in [
        "Secrets",
        "Drift",
        "Skills",
        "Content scan",
        "Reproducibility",
    ] {
        assert!(
            relevant(&report, title),
            "'{title}' must stay relevant for a project that uses the feature"
        );
    }
}
