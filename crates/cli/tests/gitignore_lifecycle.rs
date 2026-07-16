// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The managed `.gitignore` block across the profile lifecycle.
//!
//! Entries are anchored on OUTCOMES + persistent records, never on what the
//! manifest declares: a path is ignored iff agentstack wrote it this run or a
//! record (state / on-disk managed marker) says it owns the path now. So a
//! blocked write hides nothing, `apply` and `use` emit the same block on an
//! unchanged setup, and full deactivation leaves a committed block intact.
//! Serialized-by-design: these mutate process-global HOME / env vars.

use std::fs;
use std::sync::Mutex;

use agentstack::cli::{ApplyArgs, UseArgs};
use agentstack::commands::{apply, use_profile};
use agentstack::library::{Library, LibraryServer};
use agentstack::scope::Scope;
use agentstack::state::{target_key, State};

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn use_args(profile: &str) -> UseArgs {
    UseArgs {
        profile: profile.into(),
        targets: vec![],
        scope: Some(Scope::Project),
        write: true,
        allow_unresolved: false,
        no_gitignore: false,
        prune_foreign: false,
    }
}

fn apply_args() -> ApplyArgs {
    ApplyArgs {
        targets: vec!["claude-code".into(), "cursor".into()],
        profile: None,
        dry_run: false,
        write: true,
        scope: Some(Scope::Project),
        allow_unresolved: false,
        no_gitignore: false,
        prune_foreign: false,
    }
}

/// Point HOME / AGENTSTACK_HOME at a scratch dir for the duration of a test.
fn set_home(home: &std::path::Path) {
    fs::create_dir_all(home).unwrap();
    std::env::set_var("HOME", home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));
}

fn clear_home() {
    std::env::remove_var("AGENTSTACK_HOME");
    std::env::remove_var("HOME");
}

/// Install a central-library server `kibana` (a `${REF}` header) under
/// `<home>/.agentstack/lib` — referenced by name only, never inlined.
fn install_library_server(home: &std::path::Path, url: &str) {
    let lib_home = home.join(".agentstack/lib");
    fs::create_dir_all(lib_home.join("servers")).unwrap();
    fs::write(
        lib_home.join("servers/kibana.toml"),
        format!("type = \"http\"\nurl = \"{url}\"\n\n[headers]\nAuthorization = \"Bearer ${{KIBANA_TOKEN}}\"\n"),
    )
    .unwrap();
    let mut lib = Library::default();
    lib.upsert_server(LibraryServer {
        name: "kibana".into(),
        checksum: None,
        version: None,
        provenance: Some("consolidated:codex".into()),
    });
    lib.save(&lib_home).unwrap();
}

/// `apply` and `use` emit the SAME managed block on an established setup —
/// otherwise alternating them rewrites a possibly-committed `.gitignore` (and
/// un-ignores whichever artifact the other command doesn't produce). The two
/// converge once each artifact has a record: `apply` compiles CLAUDE.md (the
/// marker `use` reads) and `use` materializes skills (the state `apply` reads).
#[test]
fn apply_and_use_emit_an_identical_block() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    set_home(&tmp.path().join("home"));

    let proj = tmp.path().join("proj");
    fs::create_dir_all(proj.join(".git")).unwrap();
    fs::create_dir_all(proj.join("skills/local")).unwrap();
    fs::write(proj.join("skills/local/SKILL.md"), "# local\n").unwrap();
    fs::create_dir_all(proj.join("instr")).unwrap();
    fs::write(proj.join("instr/house.md"), "Be concise.\n").unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\", \"cursor\"]\n\
         [servers.demo]\ntype = \"http\"\nurl = \"https://x/mcp\"\n\
         [skills.local]\npath = \"./skills/local\"\n\
         [instructions.house]\npath = \"./instr/house.md\"\n\
         [profiles.default]\nservers = [\"demo\"]\nskills = [\"local\"]\n",
    )
    .unwrap();

    // Establish every artifact once: apply compiles CLAUDE.md + writes config,
    // use materializes skills + records them.
    apply::run(&apply_args(), Some(&proj)).unwrap();
    use_profile::run(&use_args("default"), Some(&proj)).unwrap();
    let after_use = fs::read_to_string(proj.join(".gitignore")).unwrap();
    apply::run(&apply_args(), Some(&proj)).unwrap();
    let after_apply = fs::read_to_string(proj.join(".gitignore")).unwrap();

    assert_eq!(
        after_apply, after_use,
        "apply and use must produce the same managed block — no churn"
    );
    for entry in [
        "/.mcp.json",
        "/.claude/skills/",
        "/CLAUDE.md",
        "/.cursor/mcp.json", // config-only adapter must appear from BOTH commands
    ] {
        assert!(
            after_use.contains(entry),
            "block missing {entry}: {after_use}"
        );
    }

    clear_home();
}

/// Activation writes stable, directory-level entries (no per-skill churn),
/// re-activating the same profile is a no-op, and full deactivation leaves the
/// block byte-identical — stripping it would dirty a committed `.gitignore`.
#[test]
fn activation_writes_stable_entries_and_deactivation_keeps_the_block() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    set_home(&tmp.path().join("home"));

    let proj = tmp.path().join("proj");
    fs::create_dir_all(proj.join(".git")).unwrap(); // ensure_block needs a repo
    fs::create_dir_all(proj.join("skills/local-notes")).unwrap();
    fs::write(proj.join("skills/local-notes/SKILL.md"), "# local\n").unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [servers.kibana]\ntype = \"http\"\nurl = \"https://k/mcp\"\n\
         [skills.local-notes]\npath = \"./skills/local-notes\"\n\
         [profiles.p]\nservers = [\"kibana\"]\nskills = [\"local-notes\"]\n\
         [profiles.off]\nservers = []\nskills = []\n",
    )
    .unwrap();

    // Phase 1: activate — the block lists the skills DIR and the config file,
    // never a per-skill line, so membership changes can't churn it.
    use_profile::run(&use_args("p"), Some(&proj)).unwrap();
    let after_use = fs::read_to_string(proj.join(".gitignore")).unwrap();
    assert!(
        after_use.contains("/.claude/skills/\n"),
        "directory-level skills entry: {after_use}"
    );
    assert!(after_use.contains("/.mcp.json"), "{after_use}");
    assert!(
        !after_use.contains("/.claude/skills/local-notes"),
        "no per-skill entries: {after_use}"
    );

    // Phase 2: re-activate the same profile — nothing changed, so the block
    // must stay byte-identical (both flags read the records the first run left).
    use_profile::run(&use_args("p"), Some(&proj)).unwrap();
    let after_reactivate = fs::read_to_string(proj.join(".gitignore")).unwrap();
    assert_eq!(
        after_reactivate, after_use,
        "re-activating an unchanged profile must not rewrite the managed block"
    );

    // Phase 3: deactivate via the empty profile — every managed record clears,
    // so the entry set is empty and the block is left intact (a committed
    // .gitignore must not go dirty) even as the emptied skills dir is removed.
    use_profile::run(&use_args("off"), Some(&proj)).unwrap();
    let after_off = fs::read_to_string(proj.join(".gitignore")).unwrap();
    assert_eq!(
        after_off, after_use,
        "deactivation must leave the managed block untouched"
    );
    assert!(
        !proj.join(".claude/skills").exists(),
        "deactivation removes the emptied managed skills dir"
    );

    clear_home();
}

/// Names-only pattern (the documented shape): a profile references a central
/// library server with NO inline `[servers.*]`. `use --write` resolves and
/// writes `.mcp.json` WITH the secret, so its `/.mcp.json` ignore entry must be
/// emitted from the resolved server_map — not from a `manifest.servers` gate.
#[test]
fn names_only_profile_ignores_the_resolved_mcp_config() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    set_home(&home);
    std::env::set_var("KIBANA_TOKEN", "secret-value");
    install_library_server(&home, "https://central-kibana/mcp");

    let proj = tmp.path().join("proj");
    fs::create_dir_all(proj.join(".git")).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [profiles.dev]\nservers = [\"kibana\"]\n",
    )
    .unwrap();

    use_profile::run(&use_args("dev"), Some(&proj)).unwrap();

    // The write happened WITH the resolved secret …
    let cfg = fs::read_to_string(proj.join(".mcp.json")).unwrap();
    assert!(cfg.contains("secret-value"), "secret written: {cfg}");
    // … so the config file must be in the managed block.
    let gitignore = fs::read_to_string(proj.join(".gitignore")).unwrap();
    assert!(
        gitignore.contains("/.mcp.json"),
        "names-only write must ignore the secret-carrying config: {gitignore}"
    );

    std::env::remove_var("KIBANA_TOKEN");
    clear_home();
}

/// A run whose writes are ALL blocked (unresolved secret, no --allow-unresolved)
/// records nothing and must contribute NO managed block — otherwise it would
/// hide a hand-maintained `.mcp.json` from `git status`.
#[test]
fn blocked_write_emits_no_managed_block() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    set_home(&tmp.path().join("home"));

    let proj = tmp.path().join("proj");
    fs::create_dir_all(proj.join(".git")).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [servers.kibana]\ntype = \"http\"\nurl = \"https://k/mcp\"\n\
         headers = { Authorization = \"Bearer ${NOPE_TOKEN}\" }\n",
    )
    .unwrap();

    let args = ApplyArgs {
        targets: vec!["claude-code".into()],
        profile: None,
        dry_run: false,
        write: true,
        scope: Some(Scope::Project),
        allow_unresolved: false,
        no_gitignore: false,
        prune_foreign: false,
    };
    apply::run(&args, Some(&proj)).unwrap();

    assert!(
        !proj.join(".mcp.json").exists(),
        "blocked write must not create the config"
    );
    let gitignore = fs::read_to_string(proj.join(".gitignore")).unwrap_or_default();
    assert!(
        !gitignore.contains("agentstack"),
        "a fully-blocked run must not write a managed block: {gitignore}"
    );

    clear_home();
}

/// Alternating `apply --write` and `use --write` on an established, unchanged
/// setup yields a byte-identical block every time — both derive the flags from
/// the same persistent records.
#[test]
fn apply_and_use_alternate_without_churn() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    set_home(&tmp.path().join("home"));

    let proj = tmp.path().join("proj");
    fs::create_dir_all(proj.join(".git")).unwrap();
    fs::create_dir_all(proj.join("skills/local")).unwrap();
    fs::write(proj.join("skills/local/SKILL.md"), "# local\n").unwrap();
    fs::create_dir_all(proj.join("instr")).unwrap();
    fs::write(proj.join("instr/house.md"), "Be concise.\n").unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\", \"cursor\"]\n\
         [servers.demo]\ntype = \"http\"\nurl = \"https://x/mcp\"\n\
         [skills.local]\npath = \"./skills/local\"\n\
         [instructions.house]\npath = \"./instr/house.md\"\n\
         [profiles.default]\nservers = [\"demo\"]\nskills = [\"local\"]\n",
    )
    .unwrap();

    // Establish, then alternate — capture the block after each of the last four.
    apply::run(&apply_args(), Some(&proj)).unwrap();
    use_profile::run(&use_args("default"), Some(&proj)).unwrap();
    let baseline = fs::read_to_string(proj.join(".gitignore")).unwrap();
    for _ in 0..2 {
        apply::run(&apply_args(), Some(&proj)).unwrap();
        assert_eq!(
            fs::read_to_string(proj.join(".gitignore")).unwrap(),
            baseline,
            "apply churned the block"
        );
        use_profile::run(&use_args("default"), Some(&proj)).unwrap();
        assert_eq!(
            fs::read_to_string(proj.join(".gitignore")).unwrap(),
            baseline,
            "use churned the block"
        );
    }

    clear_home();
}

/// A managed leftover config keeps its `/.mcp.json` entry: even with our own
/// server set empty, a kept-foreign record means a managed file is on disk, so
/// the entry must NOT silently drop from a block that still lists the skills
/// dir (a non-empty set rewrites the whole block).
#[test]
fn leftover_managed_config_keeps_mcp_entry() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    set_home(&tmp.path().join("home"));

    let proj = tmp.path().join("proj");
    fs::create_dir_all(proj.join(".git")).unwrap();
    fs::create_dir_all(proj.join("skills/local")).unwrap();
    fs::write(proj.join("skills/local/SKILL.md"), "# local\n").unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [skills.local]\npath = \"./skills/local\"\n\
         [profiles.p]\nservers = []\nskills = [\"local\"]\n",
    )
    .unwrap();

    // Pre-seed a kept-foreign server for this target — a managed config file
    // another manifest left on disk. Our own server set is empty.
    let mut state = State::load().unwrap();
    let key = target_key("claude-code", Scope::Project, &proj);
    state.record_kept_foreign(&key, vec!["foreign-srv".into()]);
    state.save().unwrap();

    use_profile::run(&use_args("p"), Some(&proj)).unwrap();

    let gitignore = fs::read_to_string(proj.join(".gitignore")).unwrap();
    assert!(
        gitignore.contains("/.claude/skills/"),
        "skills dir active: {gitignore}"
    );
    assert!(
        gitignore.contains("/.mcp.json"),
        "kept-foreign leftover config must stay ignored: {gitignore}"
    );
    // The kept-foreign record survived the run (still reachable for prune).
    let state = State::load().unwrap();
    assert_eq!(state.kept_foreign(&key), vec!["foreign-srv".to_string()]);

    clear_home();
}
