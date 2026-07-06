//! The managed .gitignore block across the profile lifecycle: activation
//! writes stable directory-level entries (no per-skill churn), and
//! deactivation (`use off --write`) leaves the block intact — stripping it
//! would dirty a committed .gitignore in team repos.
//! Serialized-by-design: one test, phases in order (mutates process HOME).

use std::fs;
use std::sync::Mutex;

use agentstack::cli::{ApplyArgs, UseArgs};
use agentstack::commands::{apply, use_profile};
use agentstack::scope::Scope;

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
        targets: vec!["claude-code".into()],
        profile: None,
        dry_run: false,
        write: true,
        scope: Some(Scope::Project),
        allow_unresolved: false,
        no_gitignore: false,
        prune_foreign: false,
    }
}

/// `apply` and `use` must emit the SAME managed block — otherwise alternating
/// them rewrites a possibly-committed `.gitignore` (and un-ignores whichever
/// artifact the other command doesn't know about).
#[test]
fn apply_and_use_emit_an_identical_block() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    let proj = tmp.path().join("proj");
    fs::create_dir_all(proj.join(".git")).unwrap();
    fs::create_dir_all(proj.join("skills/local")).unwrap();
    fs::write(proj.join("skills/local/SKILL.md"), "# local\n").unwrap();
    fs::create_dir_all(proj.join("instr")).unwrap();
    fs::write(proj.join("instr/house.md"), "Be concise.\n").unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [servers.demo]\ntype = \"http\"\nurl = \"https://x/mcp\"\n\
         [skills.local]\npath = \"./skills/local\"\n\
         [instructions.house]\npath = \"./instr/house.md\"\n\
         [profiles.default]\nservers = [\"demo\"]\nskills = [\"local\"]\n",
    )
    .unwrap();

    apply::run(&apply_args(), Some(&proj)).unwrap();
    let after_apply = fs::read_to_string(proj.join(".gitignore")).unwrap();
    use_profile::run(&use_args("default"), Some(&proj)).unwrap();
    let after_use = fs::read_to_string(proj.join(".gitignore")).unwrap();

    assert_eq!(
        after_apply, after_use,
        "apply and use must produce the same managed block — no churn"
    );
    for entry in ["/.mcp.json", "/.claude/skills/", "/CLAUDE.md"] {
        assert!(
            after_use.contains(entry),
            "block missing {entry}: {after_use}"
        );
    }

    std::env::remove_var("AGENTSTACK_HOME");
    std::env::remove_var("HOME");
}

#[test]
fn activation_writes_stable_entries_and_deactivation_keeps_the_block() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

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
         [profiles.srv]\nservers = [\"kibana\"]\nskills = []\n\
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

    // Phase 2: switch to a server-only profile — skills get pruned, but the
    // block must stay byte-identical: a target that still manages anything
    // emits its full stable entry pair, so profile membership (skills vs
    // servers) can never churn a committed file.
    use_profile::run(&use_args("srv"), Some(&proj)).unwrap();
    let after_srv = fs::read_to_string(proj.join(".gitignore")).unwrap();
    assert_eq!(
        after_srv, after_use,
        "switching to a server-only profile must not rewrite the managed block"
    );

    // Phase 3: deactivate via the empty profile — artifacts are pruned but the
    // block stays byte-identical (a committed .gitignore must not go dirty).
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

    std::env::remove_var("AGENTSTACK_HOME");
    std::env::remove_var("HOME");
}
