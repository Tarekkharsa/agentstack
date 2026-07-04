//! The managed .gitignore block across the profile lifecycle: activation
//! writes stable directory-level entries (no per-skill churn), and
//! deactivation (`use off --write`) leaves the block intact — stripping it
//! would dirty a committed .gitignore in team repos.
//! Serialized-by-design: one test, phases in order (mutates process HOME).

use std::fs;

use agentstack::cli::UseArgs;
use agentstack::commands::use_profile;
use agentstack::scope::Scope;

fn use_args(profile: &str) -> UseArgs {
    UseArgs {
        profile: profile.into(),
        targets: vec![],
        scope: Some(Scope::Project),
        write: true,
        allow_unresolved: false,
        no_gitignore: false,
    }
}

#[test]
fn activation_writes_stable_entries_and_deactivation_keeps_the_block() {
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
