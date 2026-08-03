// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Instructions that target CLI and model (`docs/design/instruction-variants.md`).
//!
//! Six claims, one witness each. Three are about *which bytes* a harness gets —
//! precedence, the unknown-model fallback, and resolution across linked library
//! sources. Three are about *saying only what is known* — every variant pinned
//! and a drifted one failing closed, an unconfirmed channel never presented as
//! confirmed, and a harness with no instruction channel named rather than
//! omitted.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use agentstack::adapter::Registry;
use agentstack::instructions::Selecting;
use agentstack::library::{Library, LibraryInstruction};
use agentstack::render::instructions::plan_instructions;
use agentstack::resolve::{instruction_lock_status_with, InstructionLockStatus};
use agentstack::scope::Scope;
use agentstack::sources::Sources;

// HOME / AGENTSTACK_HOME are process-global; serialize this binary against
// itself exactly as every other home-mutating test file does.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// An isolated machine plus a project directory holding `manifest`.
fn project(tmp: &Path, manifest: &str) -> PathBuf {
    let home = tmp.join("home");
    fs::create_dir_all(&home).unwrap();
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));
    }
    project_at(tmp, manifest)
}

/// A second project on the SAME machine — deliberately not re-isolating, so
/// anything already linked stays linked.
fn project_at(under: &Path, manifest: &str) -> PathBuf {
    let proj = under.join("proj");
    fs::create_dir_all(proj.join(".agentstack")).unwrap();
    fs::write(proj.join(".agentstack/agentstack.toml"), manifest).unwrap();
    proj
}

/// Write a fragment body under the project's `.agentstack/`.
fn body(proj: &Path, rel: &str, text: &str) {
    let path = proj.join(".agentstack").join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// The manifest every precedence test resolves against: one fragment with all
/// three variant shapes plus a base body.
const FOUR_LEVELS: &str = r#"
version = 1

[instructions.house]
path = "./instructions/house.md"

[[instructions.house.variant]]
cli = "claude-code"
model = "opus"
path = "./instructions/house.claude-opus.md"

[[instructions.house.variant]]
cli = "codex"
path = "./instructions/house.codex.md"

[[instructions.house.variant]]
model = "opus"
path = "./instructions/house.opus.md"
"#;

fn seed_four_levels(proj: &Path) {
    body(proj, "instructions/house.md", "BASE BODY\n");
    body(
        proj,
        "instructions/house.claude-opus.md",
        "CLAUDE PLUS OPUS\n",
    );
    body(proj, "instructions/house.codex.md", "CODEX ONLY\n");
    body(proj, "instructions/house.opus.md", "OPUS ONLY\n");
}

/// Compile `cli`'s managed region for a project whose selected toolset is
/// `toolset`, and return the prose that landed in it.
fn compiled(proj: &Path, cli: &str, toolset: Option<&str>) -> String {
    let registry = Registry::load().unwrap();
    let desc = registry.get(cli).unwrap();
    let loaded = agentstack::manifest::load_from_dir(&proj.join(".agentstack")).unwrap();
    let sel = Selecting {
        library: Library::load_default_or_warn(),
        toolset: toolset.map(str::to_string),
    };
    let plan = plan_instructions(
        &loaded.manifest,
        desc,
        Scope::Project,
        &proj.join(".agentstack"),
        &[],
        &sel,
    )
    .expect("this harness has an instruction file");
    plan.proposed
}

// ----------------------------------------------------------------- claim 1

/// All four precedence levels, on one fragment: exact `(cli, model)` beats
/// `(cli)` beats `(model)` beats the base body — and an identical selector
/// declared twice resolves to the first one, deterministically.
#[test]
fn the_most_specific_matching_variant_wins() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();

    // `[settings.<cli>] model` is one of the two declarations that name a
    // model, and the one that needs no command-line selection.
    let manifest = format!(
        "{FOUR_LEVELS}\n[settings.claude-code]\nmodel = \"opus\"\n\
         [settings.codex]\nmodel = \"opus\"\n"
    );
    let proj = project(tmp.path(), &manifest);
    seed_four_levels(&proj);

    // 1 · exact (cli, model).
    assert!(
        compiled(&proj, "claude-code", None).contains("CLAUDE PLUS OPUS"),
        "an exact (cli, model) variant must win over both single-key variants"
    );
    // 2 · (cli) beats (model): codex has a cli-only variant, and the model is
    //     opus, for which a model-only variant also exists.
    let codex = compiled(&proj, "codex", None);
    assert!(
        codex.contains("CODEX ONLY"),
        "cli must outrank model: {codex}"
    );
    assert!(!codex.contains("OPUS ONLY"));

    // 3 · (model) alone, on a CLI with no variant of its own.
    let opencode = project(
        tmp.path().join("m3").as_path(),
        &format!("{FOUR_LEVELS}\n[settings.opencode]\nmodel = \"opus\"\n"),
    );
    seed_four_levels(&opencode);
    assert!(
        compiled(&opencode, "opencode", None).contains("OPUS ONLY"),
        "a model-only variant carries a CLI that has none of its own"
    );

    // 4 · the base body, when nothing matches at all.
    let plain = project(
        tmp.path().join("m4").as_path(),
        &format!("{FOUR_LEVELS}\n[settings.opencode]\nmodel = \"sonnet\"\n"),
    );
    seed_four_levels(&plain);
    assert!(
        compiled(&plain, "opencode", None).contains("BASE BODY"),
        "no (cli) and no (model) match leaves the base body"
    );

    // Ties break deterministically: two identical selectors resolve to the
    // FIRST declared, the same first-match rule linked sources use.
    let tied = project(
        tmp.path().join("tie").as_path(),
        r#"
version = 1
[instructions.house]
path = "./instructions/house.md"
[[instructions.house.variant]]
cli = "codex"
path = "./instructions/first.md"
[[instructions.house.variant]]
cli = "codex"
path = "./instructions/second.md"
"#,
    );
    body(&tied, "instructions/house.md", "BASE\n");
    body(&tied, "instructions/first.md", "FIRST DECLARED\n");
    body(&tied, "instructions/second.md", "SECOND DECLARED\n");
    let out = compiled(&tied, "codex", None);
    assert!(out.contains("FIRST DECLARED"), "{out}");
    assert!(!out.contains("SECOND DECLARED"));
}

// ----------------------------------------------------------------- claim 2

/// With no model declared anywhere, a `model` selector never matches — the
/// least specific matching body is used, and the surface says the model is
/// unknown rather than defaulting into a claim.
#[test]
fn an_unknown_model_falls_back_to_the_least_specific_variant_and_says_so() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    // No `[settings.*] model`, and no toolset is named below.
    let proj = project(tmp.path(), FOUR_LEVELS);
    seed_four_levels(&proj);

    // claude-code has an exact (cli, model) variant and nothing cli-only, so
    // an unknown model falls all the way to the base body.
    let claude = compiled(&proj, "claude-code", None);
    assert!(
        claude.contains("BASE BODY"),
        "an unknown model must never match a model selector: {claude}"
    );
    assert!(!claude.contains("CLAUDE PLUS OPUS"));
    assert!(!claude.contains("OPUS ONLY"));

    // codex keeps its cli-only variant: the model simply is not consulted.
    assert!(compiled(&proj, "codex", None).contains("CODEX ONLY"));

    // And the surface SAYS so. `[toolsets.backend]` declares no model, so
    // naming it changes nothing — a toolset is a selection, not a guess.
    let named = compiled(&proj, "claude-code", Some("backend"));
    assert!(named.contains("BASE BODY"));

    let json = agentstack::commands::overview::status_body(Some(&proj)).unwrap();
    let rows = json["project"]["instruction_channels"].as_array().unwrap();
    let claude_row = rows
        .iter()
        .find(|r| r["id"] == "claude-code")
        .expect("claude-code is a targeted harness");
    assert_eq!(
        claude_row["selection"]["model"],
        serde_json::Value::Null,
        "an unknown model is reported as null, never as a default: {claude_row}"
    );
    assert_eq!(claude_row["selection"]["model_source"], "unknown");
    let sentence = claude_row["sentence"].as_str().unwrap();
    assert!(
        sentence.contains("model unknown"),
        "status must say the model is unknown: {sentence}"
    );

    // The other declaration that names a model is honoured and named as such.
    let with_setting = project(
        tmp.path().join("set").as_path(),
        &format!("{FOUR_LEVELS}\n[settings.claude-code]\nmodel = \"opus\"\n"),
    );
    seed_four_levels(&with_setting);
    let json = agentstack::commands::overview::status_body(Some(&with_setting)).unwrap();
    let rows = json["project"]["instruction_channels"].as_array().unwrap();
    let row = rows.iter().find(|r| r["id"] == "claude-code").unwrap();
    assert_eq!(row["selection"]["model"], "opus");
    assert_eq!(row["selection"]["model_source"], "settings");
}

// ----------------------------------------------------------------- claim 3

/// Every variant body gets its own pin, and moving any of them — including one
/// nothing currently selects — reads as drift and refuses the write.
#[test]
fn every_variant_body_is_pinned_and_a_drifted_variant_fails_closed() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    // No model is declared, so NOTHING selects the two model-bearing variants.
    // They are still content, so they are still pinned.
    let proj = project(tmp.path(), FOUR_LEVELS);
    seed_four_levels(&proj);
    let dir = proj.join(".agentstack");

    agentstack::commands::lock::run(&lock_args(), Some(&proj)).expect("lock pins the fragment");

    let lock_text = fs::read_to_string(dir.join("agentstack.lock")).unwrap();
    assert!(
        lock_text.contains("[[instruction.variant]]"),
        "variants must reach the lock: {lock_text}"
    );
    let lock = agentstack::lock::Lock::load(&dir).unwrap();
    let entry = lock
        .get_instruction("house")
        .expect("the fragment is pinned");
    assert_eq!(
        entry.variants.len(),
        3,
        "every declared variant carries its own digest, selected or not"
    );
    // Each pin covers its own bytes — three different bodies, three different
    // digests, none of them the base's.
    let mut digests: Vec<&str> = entry.variants.iter().map(|v| v.checksum.hex()).collect();
    digests.push(entry.checksum.hex());
    digests.sort_unstable();
    let before = digests.len();
    digests.dedup();
    assert_eq!(
        before,
        digests.len(),
        "each body is pinned to its own bytes"
    );

    let loaded = agentstack::manifest::load_from_dir(&dir).unwrap();
    let instr = &loaded.manifest.instructions["house"];
    let library = Library::default();
    assert_eq!(
        instruction_lock_status_with("house", instr, &dir, &lock, &library),
        InstructionLockStatus::Matches
    );

    // Drift the variant NOTHING selects. It must still be caught.
    body(&proj, "instructions/house.opus.md", "OPUS ONLY, EDITED\n");
    assert!(
        matches!(
            instruction_lock_status_with("house", instr, &dir, &lock, &library),
            InstructionLockStatus::ChecksumDrift { .. }
        ),
        "an unselected variant's bytes moving is still drift — consent is over \
         content, not over what happened to be chosen today"
    );

    // And the write fails closed at the gate `apply`/`instructions` share.
    let err = agentstack::commands::instructions::run(
        &agentstack::cli::InstructionsArgs {
            targets: Vec::new(),
            toolset: None,
            scope: Some(Scope::Project),
            write: true,
        },
        Some(&proj),
    )
    .expect_err("a drifted variant must refuse the write");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("house"),
        "the refusal names the fragment: {msg}"
    );
}

fn lock_args() -> agentstack::cli::LockArgs {
    agentstack::cli::LockArgs {
        quiet: true,
        profile: None,
        update: None,
        upgrade: None,
        all: false,
        with_instructions: false,
        yes: false,
        write: false,
    }
}

// ----------------------------------------------------------------- claim 4

/// The honesty rule, against real `status` output: a harness whose live channel
/// is unconfirmed is labelled unconfirmed and said to be unused, and the word
/// "confirmed" appears for exactly the one harness the research confirmed.
#[test]
fn an_unconfirmed_channel_is_never_claimed_as_confirmed() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project(
        tmp.path(),
        "version = 1\n\
         [targets]\ndefault = [\"claude-code\", \"codex\"]\n\
         [instructions.house]\npath = \"./instructions/house.md\"\n",
    );
    body(&proj, "instructions/house.md", "be kind\n");

    let json = agentstack::commands::overview::status_body(Some(&proj)).unwrap();
    let rows = json["project"]["instruction_channels"].as_array().unwrap();

    let codex = rows.iter().find(|r| r["id"] == "codex").unwrap();
    assert_eq!(
        codex["live_channel"]["confirmation"], "unconfirmed",
        "Codex's MCP instructions consumption is unverified: {codex}"
    );
    assert_eq!(
        codex["live_channel"]["used"], false,
        "an unconfirmed channel is never used as though it worked"
    );
    let sentence = codex["sentence"].as_str().unwrap();
    assert!(
        sentence.contains("unconfirmed"),
        "the sentence must carry the word: {sentence}"
    );
    assert!(
        !sentence.contains("confirmed for this tool"),
        "an unconfirmed channel must never borrow the confirmed wording: {sentence}"
    );

    // The one confirmed channel says confirmed — AND says it is not used, so
    // "confirmed" can never be read as "serving".
    let claude = rows.iter().find(|r| r["id"] == "claude-code").unwrap();
    assert_eq!(claude["live_channel"]["confirmation"], "confirmed");
    assert_eq!(claude["live_channel"]["used"], false);
    let sentence = claude["sentence"].as_str().unwrap();
    assert!(
        sentence.contains("not used for house rules"),
        "a confirmed channel must still say it is not carrying them: {sentence}"
    );

    // And the disproven claim is gone from the routing copy.
    let delivery = format!("{}", json["project"]["delivery"]);
    assert!(
        !delivery.contains("cannot inject"),
        "the disproven MCP claim must not survive anywhere: {delivery}"
    );

    // The feature is advertised, so a panel can gate on it.
    let features = json["features"].as_array();
    // `status_body` is the un-enveloped body; the envelope is where features
    // live, so assert against the shipped constant instead.
    assert!(features.is_none());
    assert!(agentstack::ui_contract::FEATURES.contains(&"instruction-channels-v1"));
}

// ----------------------------------------------------------------- claim 5

/// Seven of thirteen adapters carry no instruction channel at all. They are
/// reported, not omitted — an adapter that silently disappears from a coverage
/// list reads as covered.
#[test]
fn a_harness_with_no_instruction_channel_is_reported_as_such() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project(
        tmp.path(),
        "version = 1\n\
         [targets]\ndefault = [\"claude-code\", \"gemini\", \"cursor\"]\n\
         [instructions.house]\npath = \"./instructions/house.md\"\n",
    );
    body(&proj, "instructions/house.md", "be kind\n");

    let json = agentstack::commands::overview::status_body(Some(&proj)).unwrap();
    let rows = json["project"]["instruction_channels"].as_array().unwrap();

    for id in ["gemini", "cursor"] {
        let row = rows
            .iter()
            .find(|r| r["id"] == id)
            .unwrap_or_else(|| panic!("{id} must appear rather than be omitted: {rows:?}"));
        assert_eq!(
            row["file"],
            serde_json::Value::Null,
            "{id} has no instruction destination"
        );
        assert_eq!(row["live_channel"], serde_json::Value::Null);
        let sentence = row["sentence"].as_str().unwrap();
        assert!(
            sentence.contains("no instruction channel"),
            "{id} must say so plainly: {sentence}"
        );
        assert!(
            sentence.contains("do not reach this tool"),
            "and say what that means: {sentence}"
        );
    }

    // The registry itself is the source of that fact, so the count is not a
    // number this test invented.
    let registry = Registry::load().unwrap();
    let with = registry.iter().filter(|d| d.instructions.is_some()).count();
    assert_eq!(
        with, 6,
        "six of the registered adapters carry an instruction channel; if this \
         changes, the honesty matrix in docs/design/instruction-variants.md \
         changes with it"
    );
}

// ----------------------------------------------------------------- claim 6

/// A sourceless fragment resolves its bodies — base AND variants — from the
/// linked library sources, first match wins, exactly as every other library
/// kind does.
#[test]
fn variants_resolve_across_linked_library_sources() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project(
        tmp.path(),
        "version = 1\n\
         [targets]\ndefault = [\"claude-code\"]\n\
         [instructions.house]\n\
         [settings.claude-code]\nmodel = \"opus\"\n",
    );

    let team = tmp.path().join("team");
    let personal = tmp.path().join("personal");
    seed_library_instruction(&team, "house", "TEAM BASE\n", Some("TEAM OPUS\n"));
    seed_library_instruction(
        &personal,
        "house",
        "PERSONAL BASE\n",
        Some("PERSONAL OPUS\n"),
    );

    // team first: it wins the bare name, and its VARIANT is what compiles.
    link(&[("team", &team), ("personal", &personal)]);
    let out = compiled(&proj, "claude-code", None);
    assert!(
        out.contains("TEAM OPUS"),
        "the winning source's variant is what reaches the harness: {out}"
    );

    // Reorder: the other source now wins the name, and its variant follows.
    link(&[("personal", &personal), ("team", &team)]);
    let out = compiled(&proj, "claude-code", None);
    assert!(
        out.contains("PERSONAL OPUS"),
        "precedence decides selection across sources: {out}"
    );

    // The collision is surfaced, never hidden — the same rule item 3 fixed.
    let library = Library::load_default().unwrap();
    assert!(
        library
            .linked
            .collisions
            .iter()
            .any(|c| c.name == "house" && c.kind == agentstack::library::Kind::Instruction),
        "a house rule held by two sources is a collision like any other"
    );

    // And a model that matches nothing falls to the winning source's BASE body,
    // not to the other source's variant.
    let plain = project_at(
        tmp.path().join("plain").as_path(),
        "version = 1\n\
         [targets]\ndefault = [\"claude-code\"]\n\
         [instructions.house]\n",
    );
    let out = compiled(&plain, "claude-code", None);
    assert!(out.contains("PERSONAL BASE"), "{out}");
}

/// Write a library instruction body (`instructions/<name>/instruction.toml`
/// plus its markdown) into `root`, and index it.
fn seed_library_instruction(root: &Path, name: &str, base: &str, opus: Option<&str>) {
    let dir = root.join("instructions").join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("base.md"), base).unwrap();
    let mut decl = String::from("path = \"base.md\"\n");
    if let Some(text) = opus {
        fs::write(dir.join("opus.md"), text).unwrap();
        decl.push_str(
            "\n[[variant]]\ncli = \"claude-code\"\nmodel = \"opus\"\npath = \"opus.md\"\n",
        );
    }
    fs::write(dir.join("instruction.toml"), decl).unwrap();

    let mut library = Library::load(root).unwrap_or_default();
    library.upsert_instruction(LibraryInstruction {
        name: name.to_string(),
        description: None,
        provenance: Some("test".into()),
    });
    library.save(root).unwrap();
}

/// Link folders as sources, in this order — the real file, so the tests
/// exercise the same load path the CLI does.
fn link(sources: &[(&str, &Path)]) {
    let mut s = Sources::default();
    for (name, root) in sources {
        fs::create_dir_all(root).unwrap();
        s.link(name, root, false, None).unwrap();
    }
    s.sources
        .retain(|e| sources.iter().any(|(n, _)| n == &e.name));
    s.sources.sort_by_key(|e| {
        sources
            .iter()
            .position(|(n, _)| n == &e.name)
            .unwrap_or(usize::MAX)
    });
    s.save().unwrap();
}
