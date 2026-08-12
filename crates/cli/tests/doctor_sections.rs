// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! What `doctor::collect` says, section by section.
//!
//! Six binaries used to hold these thirteen tests, one per concern, each with
//! its own `ENV_LOCK` and its own near-identical `section_lines` helper. They
//! are all the same shape of test — set up a fake machine, call
//! `doctor::collect` in-process, read one section out of the JSON — and every
//! one of them mutates the SAME process globals (`HOME`, `AGENTSTACK_HOME`,
//! `PATH`). Splitting them across binaries bought no isolation that the lock in
//! this file does not buy, and cost a compile-and-link unit each.
//!
//! The concerns themselves stay separate and every test is kept verbatim —
//! this is a binary merge, not a coverage merge. Each section below names the
//! defect its tests pin.
//!
//! ## The hazard this file's fixture exists to remove
//!
//! `cargo nextest` runs process-per-test, so merging files is free there. Plain
//! `cargo test` — the local loop CLAUDE.md prescribes — runs a binary's tests as
//! THREADS IN ONE PROCESS, where `set_var` outlives the test that called it. Two
//! of the six merged files set `PATH` (one to an empty dir, one to a stub `bin`
//! inside a temp dir that is then deleted) and four did not set it at all. Merged
//! naively, a test that does not set `PATH` would inherit whatever the previous
//! test left behind — a dangling temp path — and `is_installed()` would answer
//! differently depending on test ORDER.
//!
//! So [`machine`] always sets all three variables, and always points `PATH` at a
//! directory it owns. No test here may leave a global unset for the next one.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use agentstack::commands::doctor;
use agentstack::commands::lib::{add_skill, LibSource};

/// Every test in this binary mutates the process-global `HOME`,
/// `AGENTSTACK_HOME` and `PATH`; serialize them.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// A fake machine: an isolated home, an isolated agentstack home, an empty
/// `PATH` this fixture owns, and an empty project directory.
struct Machine {
    /// The fake `$HOME`. `AGENTSTACK_HOME` is `<home>/.agentstack`.
    home: PathBuf,
    /// The ONLY directory on `PATH`. Empty unless a test plants a stub in it,
    /// so no adapter counts as installed and detection does not vary with the
    /// developer's own `PATH`.
    bin: PathBuf,
    /// An empty project dir — each test writes the manifest it needs.
    proj: PathBuf,
}

fn machine(tmp: &Path) -> Machine {
    let home = tmp.join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    let bin = tmp.join("bin");
    fs::create_dir_all(&bin).unwrap();
    std::env::set_var("PATH", &bin);

    let proj = tmp.join("proj");
    fs::create_dir_all(&proj).unwrap();

    Machine { home, bin, proj }
}

impl Machine {
    /// The ordinary manifest: one target, nothing else.
    fn claude_code_target(&self) -> &Path {
        fs::write(
            self.proj.join("agentstack.toml"),
            "version = 1\n[targets]\ndefault = [\"claude-code\"]\n",
        )
        .unwrap();
        &self.proj
    }

    /// Put a working `claude` on `PATH`, so `is_installed()` is true for
    /// exactly one adapter.
    fn install_claude_stub(&self) {
        let claude = self.bin.join("claude");
        fs::write(&claude, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&claude, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
}

/// The `(level, msg)` pairs of one titled section of the doctor JSON.
fn section_lines(report: &serde_json::Value, title: &str) -> Vec<(String, String)> {
    report["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["title"] == title)
        .unwrap_or_else(|| panic!("no '{title}' section in {report}"))["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            (
                l["level"].as_str().unwrap_or_default().to_string(),
                l["msg"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// Just the `msg` strings of one titled section — absent section reads empty.
fn section_msgs(report: &serde_json::Value, title: &str) -> Vec<String> {
    report["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["title"] == title)
        .map(|s| {
            s["lines"]
                .as_array()
                .unwrap()
                .iter()
                .map(|l| l["msg"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Whether a titled section is tagged as worth showing by default.
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

// ── A CLI whose config is on disk but whose binary is not installed ─────────
//
// This used to be a `warn`, which made it a thing the user was told to fix.
// Nothing can be fixed: uninstalling an editor leaves its config directory
// behind, so the line fires on a machine where nothing is wrong, and as a
// warning it never cleared — a healthy project sat permanently at "needs
// attention" over somebody else's leftovers.
//
// It is an advisory now: still stated (we would render for a tool that cannot
// launch), but counted in its own total, never the recommended next action,
// and it does not move `state` off ready. The severity had no test at all
// before, which is how it stayed wrong; these pin it.

/// A home with `~/.claude.json` present and nothing on `PATH`.
fn leftover_claude_config(m: &Machine) -> &Path {
    fs::write(m.home.join(".claude.json"), "{}\n").unwrap();
    m.claude_code_target()
}

fn adapter_line<'a>(report: &'a serde_json::Value, needle: &str) -> &'a serde_json::Value {
    report["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["title"] == "Adapters & CLIs")
        .expect("Adapters & CLIs section missing")["lines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["msg"].as_str().unwrap_or_default().contains(needle))
        .unwrap_or_else(|| panic!("no adapter line containing '{needle}'"))
}

#[test]
fn config_without_its_binary_is_an_advisory_not_a_warning() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let m = machine(tmp.path());
    let proj = leftover_claude_config(&m);

    let report = doctor::collect(Some(proj)).unwrap();
    let line = adapter_line(&report, "config present but binary not on PATH");

    assert_eq!(
        line["level"], "advisory",
        "a leftover config is a fact, not a fault: {line}"
    );
}

#[test]
fn it_does_not_count_as_a_warning_or_become_the_next_action() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let m = machine(tmp.path());
    let proj = leftover_claude_config(&m);

    let report = doctor::collect(Some(proj)).unwrap();

    // The counters are what the panel's chip and the CI gate read. Folding an
    // advisory into `warnings` is precisely the permanent-orange problem the
    // advisory tier exists to remove.
    assert!(
        report["advisories"].as_u64().unwrap() >= 1,
        "advisory not counted: {report}"
    );

    // And it must never be the one thing the user is told to start with —
    // there is no command that fixes it.
    let next = report["next_action"].as_str().unwrap_or_default();
    assert!(
        !next.contains("PATH"),
        "an unfixable line was recommended as the next action: {next}"
    );
}

// ── A CLI that is installed but has never been configured ──────────────────
//
// The mirror of the pair above: there the config outlived its binary, here the
// binary has arrived and no config exists yet. It is the ordinary state of a
// freshly installed machine, and `doctor` described it with a claim about a
// file that is not there:
//
//   ✓ Claude Code    installed · ~/.claude.json parses
//
// `read_config_value` is a READ, not an assertion of existence — a missing or
// empty file is `Ok(None)`, and the branch matched `Ok(_)`. So every detected
// CLI on a fresh machine got a tick for parsing a file that has never existed,
// in the terminal report AND in `sections[].lines[].msg`, which is what a panel
// renders. It also contradicted `init` in the same run, which reports the same
// tools honestly as "binary on PATH — no config files found": one binary, two
// surfaces, opposite claims about one file.
//
// Three states now have three sentences — a file that parsed, no file yet, and
// an adapter with no config concept at all (Pi) — and none of them is a fault,
// so the level is unchanged in every case.

/// The defect, stated directly: no surface may say a file parses when no such
/// file exists.
#[test]
fn a_config_that_does_not_exist_is_never_reported_as_parsing() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let m = machine(tmp.path());
    m.install_claude_stub();
    let proj = m.claude_code_target();

    // The premise the assertion rests on: there really is no config here.
    assert!(
        !m.home.join(".claude.json").exists(),
        "fixture wrote a config; the test would pass for the wrong reason"
    );

    let report = doctor::collect(Some(proj)).unwrap();
    let lines = section_msgs(&report, "Adapters & CLIs");
    let claude = lines
        .iter()
        .find(|l| l.contains("Claude Code"))
        .unwrap_or_else(|| panic!("no Claude Code adapter line in {lines:?}"));

    assert!(
        !claude.contains("parses"),
        "claimed a nonexistent file parses: {claude}"
    );
    assert!(
        claude.contains("no config yet"),
        "the honest state is not stated: {claude}"
    );
    // Still installed, and still not a fault — an unconfigured CLI is the
    // ordinary first-run state, not something to repair.
    assert!(claude.contains("installed"), "{claude}");
    assert_eq!(report["errors"], 0, "{report}");
}

/// And the positive case, so the fix cannot be "never say parses again": a
/// config that IS on disk and IS valid keeps its claim.
#[test]
fn a_config_that_exists_still_reports_that_it_parses() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let m = machine(tmp.path());
    m.install_claude_stub();
    let proj = m.claude_code_target();
    fs::write(m.home.join(".claude.json"), r#"{"mcpServers":{}}"#).unwrap();

    let report = doctor::collect(Some(proj)).unwrap();
    let lines = section_msgs(&report, "Adapters & CLIs");
    let claude = lines
        .iter()
        .find(|l| l.contains("Claude Code"))
        .unwrap_or_else(|| panic!("no Claude Code adapter line in {lines:?}"));

    assert!(
        claude.contains("parses"),
        "a real, valid config lost its reading: {claude}"
    );
    assert!(!claude.contains("no config yet"), "{claude}");
}

// ── Dead skill symlinks ────────────────────────────────────────────────────

/// Doctor must surface dead skill symlinks. A broken link in a CLI's skills dir
/// loads nothing and `consolidate` skips it, so without this check the skill
/// just silently stops existing (the real-world case: two pi skills linked into
/// an empty `~/.agents/skills/`). The line warns under "Skills", naming the
/// missing target and the exact fix (remove the link / reinstall).
#[test]
fn broken_skill_symlink_warns_with_fix() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let m = machine(tmp.path());

    // Claude Code counts as detected via its config file.
    fs::write(m.home.join(".claude.json"), "{}\n").unwrap();
    // Its global skills dir holds one healthy skill and one dead link.
    let skills = m.home.join(".claude/skills");
    fs::create_dir_all(skills.join("good")).unwrap();
    fs::write(skills.join("good/SKILL.md"), "# good\n").unwrap();
    let gone = m.home.join(".agents/skills/find-skills");
    std::os::unix::fs::symlink(&gone, skills.join("find-skills")).unwrap();

    let proj = m.claude_code_target();

    let report = doctor::collect(Some(proj)).unwrap();
    let lines = section_lines(&report, "Skills");
    let broken: Vec<_> = lines
        .iter()
        .filter(|(_, msg)| msg.contains("broken skill link"))
        .collect();
    assert_eq!(broken.len(), 1, "one dead link, got: {lines:?}");
    let (level, msg) = broken[0];
    assert_eq!(level, "warn");
    assert!(msg.contains("'find-skills'"), "names the link: {msg}");
    assert!(
        msg.contains(&gone.display().to_string()) && msg.contains("target missing"),
        "names the missing target: {msg}"
    );
    assert!(
        msg.contains(&format!("rm {}", skills.join("find-skills").display()))
            && msg.contains("reinstall"),
        "carries the fix: {msg}"
    );
    // The healthy skill must not be flagged.
    assert!(
        !lines.iter().any(|(_, m)| m.contains("'good'")),
        "{lines:?}"
    );
}

// ── Profile-referenced library skills ──────────────────────────────────────

/// `doctor`'s Skills section must consider the same name set a trust review
/// covers — inline `[skills.*]` PLUS profile-referenced (library) names — not
/// just inline entries. Regression for the "no skills defined" contradiction:
/// the Reproducibility section listed a pinned library skill the Skills section
/// claimed didn't exist.
#[test]
fn skills_section_counts_profile_referenced_library_skills() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let m = machine(tmp.path());

    // Seed a skill into the central library only.
    let src = tmp.path().join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(
        src.join("SKILL.md"),
        "---\ndescription: SQL review\n---\n# body\n",
    )
    .unwrap();
    let lib_home = m.home.join(".agentstack/lib");
    add_skill(
        &lib_home,
        "sql-review",
        LibSource::Path(&src),
        false,
        true,
        false,
    )
    .unwrap();

    // A project that references the library skill through a profile — NO inline
    // `[skills.*]` entry, which is exactly the case that used to read as empty.
    fs::write(
        m.proj.join("agentstack.toml"),
        "version = 1\n[profiles.dev]\nskills = [\"sql-review\"]\n",
    )
    .unwrap();

    let report = doctor::collect(Some(&m.proj)).unwrap();
    let skills = section_msgs(&report, "Skills");
    // The profile-referenced library skill is now checked and present…
    assert!(
        skills.iter().any(|l| l.contains("sql-review")),
        "Skills section must list the library skill: {skills:?}"
    );
    // …instead of claiming there are no skills at all.
    assert!(
        !skills.iter().any(|l| l == "no skills defined"),
        "Skills section must not report empty: {skills:?}"
    );
}

// ── Progressive disclosure ─────────────────────────────────────────────────
//
// Every doctor check always runs (the JSON carries all sections and the
// error/warning counters are display-independent), but each section is tagged
// `relevant` so the default terminal report can hide the ones for features a
// project doesn't use. These pin the tagging.

#[test]
fn unused_feature_sections_are_tagged_irrelevant_but_still_reported() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let m = machine(tmp.path());
    // A near-empty project: one target, nothing else — no servers, skills,
    // profiles, instructions, packs, or bridge registration.
    let proj = m.claude_code_target();

    let report = doctor::collect(Some(proj)).unwrap();

    // Unused features are tagged irrelevant — but their sections still exist
    // in the JSON: checks ran, nothing was skipped. "Machine policy" joins
    // them only in the fully-unused case: no machine policy file AND no
    // project [policy] (Stage 1.4 — the ordinary journey shows no policy
    // vocabulary until the feature exists).
    for title in [
        "Zero-files gateway",
        "Secrets",
        "Drift",
        "Instructions",
        "Quirks",
        "Skills",
        "Content scan",
        "Reproducibility",
        "Machine policy",
    ] {
        assert!(
            !relevant(&report, title),
            "'{title}' must be tagged irrelevant for a project that doesn't use it"
        );
    }

    // The baseline stays relevant always.
    assert!(relevant(&report, "Adapters & CLIs"));
}

#[test]
fn machine_policy_stays_relevant_once_a_machine_policy_exists() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let m = machine(tmp.path());
    fs::create_dir_all(m.home.join(".agentstack")).unwrap();

    // A machine manifest with a policy layer: the one-word summary is now a
    // real fact about this machine and must never be hidden as noise.
    fs::write(
        m.home.join(".agentstack/agentstack.toml"),
        "version = 1\n[policy.egress]\n\"*\" = [\"!*\"]\n",
    )
    .unwrap();

    let proj = m.claude_code_target();

    let report = doctor::collect(Some(proj)).unwrap();
    assert!(relevant(&report, "Machine policy"));
}

#[test]
fn used_features_stay_relevant() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let m = machine(tmp.path());

    fs::create_dir_all(m.proj.join("skills/helper")).unwrap();
    fs::write(m.proj.join("skills/helper/SKILL.md"), "# helper\n").unwrap();
    fs::write(
        m.proj.join("agentstack.toml"),
        // `Drift` is a RENDERED-lane section: it compares what `apply` would
        // write against disk. Under the default routing this project's MCP
        // servers are served live, so there is nothing rendered to compare and
        // the section is correctly irrelevant — the same rule the zero-files
        // branch has always applied. This fixture's subject is "a feature in
        // use keeps its section", so it opts into local rendering to put the
        // feature genuinely in use.
        "version = 1\n[delivery]\nrender_locally = true\n\
         [targets]\ndefault = [\"claude-code\"]\n\
         [servers.demo]\ntype = \"http\"\nurl = \"https://demo.example/mcp\"\n\
         headers = { Authorization = \"Bearer ${DEMO_TOKEN}\" }\n\
         [skills.helper]\npath = \"./skills/helper\"\n\
         [profiles.p]\nskills = [\"helper\"]\n",
    )
    .unwrap();

    let report = doctor::collect(Some(&m.proj)).unwrap();

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

// ── `doctor-mode-v1` / `doctor-liveness-v1`: mode, activation, liveness ────
//
// `agentstack status` has printed both for a while; `doctor --json` did not, so
// every JSON consumer was blind to them. That matters because a panel that
// shows an on-disk path is making a claim only `static` makes true — and
// because "never activated" is the difference between a project that is set up
// and one that has never written anything, which nothing in the JSON said.
//
// …plus `doctor-liveness-v1`, the runtime reading BESIDE it. A build once
// answered `live` / `not_live` in `activation` itself, under this same contract
// name and the same `schema_version`, so every consumer gating on the two
// documented words read a locked project as never activated. That is the drift
// these tests pin: `activation` is lockfile-derived and keeps its words,
// `live_state` is the lease-derived one, and a consumer of either gets an
// explicit name to negotiate.

/// The `.agentstack/`-scoped project these two tests share.
fn scoped_project(m: &Machine) -> PathBuf {
    fs::create_dir_all(m.proj.join(".agentstack")).unwrap();
    fs::write(
        m.proj.join(".agentstack/agentstack.toml"),
        "version = 1\n[servers.demo]\ntype = \"stdio\"\ncommand = \"echo\"\n",
    )
    .unwrap();
    m.proj.clone()
}

#[test]
fn a_never_activated_project_says_so() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let m = machine(tmp.path());
    let proj = scoped_project(&m);

    let report = doctor::collect(Some(&proj)).unwrap();

    // No lockfile and nothing rendered: the default mode, and the fact that
    // nothing has ever been written for this project.
    assert_eq!(report["mode"], "static", "{report}");
    assert_eq!(report["activation"], "never_activated", "{report}");
    assert_eq!(report["locked"], false, "{report}");
    // The runtime reading is its own field and answers its own question.
    assert_eq!(report["live_state"], "not_live", "{report}");
}

#[test]
fn a_lockfile_flips_activation() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let m = machine(tmp.path());
    let proj = scoped_project(&m);
    fs::write(proj.join(".agentstack/agentstack.lock"), "version = 1\n").unwrap();

    let report = doctor::collect(Some(&proj)).unwrap();
    assert_eq!(report["activation"], "locked", "{report}");
    assert_eq!(report["locked"], true, "{report}");
    // A pin is not a connection: nothing is serving this project, and the
    // liveness field is the only one allowed to say so. Collapsing the two —
    // which one build did, by answering `not_live` in `activation` — is how a
    // consumer of the documented words reads a locked project as never
    // activated.
    assert_eq!(report["live_state"], "not_live", "{report}");
}

/// The pre-manifest payload is hand-written rather than built from the report,
/// so it is the one path where a promised key can silently go missing.
#[test]
fn the_no_project_payload_carries_both_keys_as_nulls() {
    let src = include_str!("../src/commands/doctor.rs");
    for key in [
        "\"activation\": serde_json::Value::Null",
        "\"live_state\": serde_json::Value::Null",
    ] {
        assert!(
            src.contains(key),
            "the needs_setup payload must carry {key} — a key that vanishes on \
             the least-informed path is worse than one that never existed"
        );
    }
}

#[test]
fn the_contract_name_is_advertised() {
    // Without the name, a UI cannot distinguish an older binary's absent keys
    // from this binary's legitimate nulls, so it cannot use either.
    assert!(
        agentstack::ui_contract::FEATURES.contains(&"doctor-mode-v1"),
        "doctor-mode-v1 missing from FEATURES"
    );
    // The runtime reading is additive, so it gets its own name rather than new
    // words inside `activation`; without the name a panel reading `live_state`
    // would be sniffing a field.
    assert!(
        agentstack::ui_contract::FEATURES.contains(&"doctor-liveness-v1"),
        "doctor-liveness-v1 missing from FEATURES"
    );
    // Additive means additive: the envelope's version must not move, because a
    // bump tells every panel to disable itself.
    assert_eq!(agentstack::ui_contract::SCHEMA_VERSION, 1);
}
