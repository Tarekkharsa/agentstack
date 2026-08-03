// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Linked library sources — several folders, one ordered list.
//!
//! The contract is `docs/design/linked-library-sources.md`: any folder can be a
//! library source, the list is ordered, and the first source holding a name
//! wins (`PATH` semantics). Each test below is one of that document's claims,
//! and the fourth is the one that has to hold for the rest to be safe: the
//! order decides *selection*, never what an already-locked project *serves*.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use agentstack::cli::UseArgs;
use agentstack::commands::{lib as lib_cmd, use_profile};
use agentstack::library::{Kind, Library};
use agentstack::resolve::{self, ResolveMode};
use agentstack::sources::Sources;
use agentstack::store::Store;

// HOME / AGENTSTACK_HOME are process-global; serialize the binary against
// itself exactly as every other home-mutating test file does.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// An isolated machine: empty HOME, empty `AGENTSTACK_HOME`, nothing linked.
fn isolate(tmp: &Path) -> PathBuf {
    let home = tmp.join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    let ash = home.join(".agentstack");
    std::env::set_var("AGENTSTACK_HOME", &ash);
    ash
}

/// Link folders as sources, in this order. Written as the real file, so the
/// tests exercise the same load path the CLI does.
fn link(sources: &[(&str, &Path)]) {
    let mut s = Sources::default();
    for (name, root) in sources {
        fs::create_dir_all(root).unwrap();
        s.link(name, root, false, None).unwrap();
    }
    // `link` materializes the implicit `local` entry the first time a second
    // folder is linked; these tests want exactly the list they named.
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

/// Put a skill in a library source through the ordinary library write path.
fn seed_skill(tmp: &Path, root: &Path, name: &str, body: &str) {
    let src = tmp.join(format!(
        "staged-{}-{name}",
        root.file_name().unwrap().to_string_lossy()
    ));
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("SKILL.md"), body).unwrap();
    lib_cmd::add_skill(
        root,
        name,
        lib_cmd::LibSource::Path(&src),
        true,
        true,
        false,
    )
    .unwrap();
}

/// A project that references one library skill by name from one toolset.
fn project_referencing(proj: &Path, reference: &str) {
    fs::create_dir_all(proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        format!(
            "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
             [profiles.p]\nskills = [\"{reference}\"]\n"
        ),
    )
    .unwrap();
}

fn use_args(write: bool) -> UseArgs {
    UseArgs {
        profile: Some("p".into()),
        targets: vec!["claude-code".into()],
        scope: None,
        write,
        allow_unresolved: false,
        prune_foreign: false,
        no_gitignore: true,
        list: false,
        json: false,
        quiet: true,
    }
}

/// Resolve a reference the way every consumer does — merged library, primary
/// root as the base — and hand back the body it landed on.
fn resolved_body(reference: &str, proj: &Path) -> String {
    let sources = Sources::load().unwrap();
    let library = Library::load_linked(&sources.linked()).unwrap();
    let loaded = agentstack::manifest::load_from_dir(proj).unwrap();
    let r = resolve::resolve_skill(
        &loaded.manifest,
        proj,
        &library,
        &sources.primary().root,
        &Store::default_store(),
        reference,
        ResolveMode::PathOnly,
    )
    .unwrap();
    fs::read_to_string(r.path.join("SKILL.md")).unwrap()
}

/// Claim: the ordered list resolves like `PATH` — the first source holding the
/// name wins, and nothing merges.
#[test]
fn several_sources_resolve_in_order_and_the_first_match_wins() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    isolate(tmp.path());
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    link(&[("a", &a), ("b", &b)]);
    seed_skill(tmp.path(), &a, "review", "# from a\n");
    seed_skill(tmp.path(), &b, "review", "# from b\n");
    // Only `b` holds this one — a later source still contributes its own names.
    seed_skill(tmp.path(), &b, "only-b", "# only in b\n");

    let proj = tmp.path().join("proj");
    project_referencing(&proj, "review");

    assert_eq!(resolved_body("review", &proj), "# from a\n");
    assert_eq!(resolved_body("only-b", &proj), "# only in b\n");

    // And the merged index is the winners, once each — not a union with
    // duplicates and not a merge of two entries into one.
    let library = Library::load_linked(&Sources::load().unwrap().linked()).unwrap();
    assert_eq!(
        library.skills.iter().filter(|s| s.name == "review").count(),
        1
    );
}

/// Claim: a shadowed name is reported, never hidden — the winner AND the
/// shadowed source are both visible, with the reference that pins the other.
#[test]
fn a_shadowed_name_is_reported_not_hidden() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let ash = isolate(tmp.path());
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    link(&[("a", &a), ("b", &b)]);
    seed_skill(tmp.path(), &a, "review", "# from a\n");
    seed_skill(tmp.path(), &b, "review", "# from b\n");

    let library = Library::load_linked(&Sources::load().unwrap().linked()).unwrap();
    let c = library
        .linked
        .collisions
        .iter()
        .find(|c| c.name == "review" && c.kind == Kind::Skill)
        .expect("the shared name is recorded as a collision");
    assert_eq!(c.winner, "a");
    assert_eq!(c.shadowed, vec!["b".to_string()]);
    assert_eq!(c.qualified_shadowed(), "b:review");

    // And it reaches a user: `lib sources` names both sides and the reference
    // that takes the shadowed copy.
    let out = run_cli(&ash, tmp.path(), &["lib", "sources"]);
    assert!(out.contains("review"), "{out}");
    assert!(out.contains("a used"), "the winner is named: {out}");
    assert!(out.contains("b shadowed"), "the loser is named: {out}");
    assert!(out.contains("b:review"), "the way to pin the other: {out}");
}

/// Claim: `<source>:<name>` resolves only in the source it names, and the
/// order cannot move it — before or after a reorder.
#[test]
fn a_fully_qualified_reference_ignores_order() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    isolate(tmp.path());
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    link(&[("a", &a), ("b", &b)]);
    seed_skill(tmp.path(), &a, "review", "# from a\n");
    seed_skill(tmp.path(), &b, "review", "# from b\n");

    let proj = tmp.path().join("proj");
    project_referencing(&proj, "b:review");

    assert_eq!(resolved_body("b:review", &proj), "# from b\n");

    // Reorder so `b` wins bare names too — the qualified answer is identical,
    // and so is the qualified answer for the source that is now second.
    let mut sources = Sources::load().unwrap();
    sources
        .reorder(&["b".to_string(), "a".to_string()])
        .unwrap();
    sources.save().unwrap();
    assert_eq!(resolved_body("b:review", &proj), "# from b\n");
    assert_eq!(resolved_body("a:review", &proj), "# from a\n");
    assert_eq!(
        resolved_body("review", &proj),
        "# from b\n",
        "the bare name follows the order — that is the difference being witnessed"
    );

    // A qualifier naming a source nobody linked is its own mistake, with its
    // own message: no such source, not "no such skill".
    let sources = Sources::load().unwrap();
    let library = Library::load_linked(&sources.linked()).unwrap();
    let loaded = agentstack::manifest::load_from_dir(&proj).unwrap();
    let err = resolve::resolve_skill(
        &loaded.manifest,
        &proj,
        &library,
        &sources.primary().root,
        &Store::default_store(),
        "ghost:review",
        ResolveMode::PathOnly,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("not linked"), "{err}");
}

/// **The decisive safety witness.** Precedence decides selection. Serving reads
/// the bytes the lock pins, from the content store — so reordering the sources
/// cannot change what an already-locked project serves, and a pinned name that
/// would now resolve to different content re-gates instead of swapping.
#[test]
fn reordering_sources_cannot_change_what_a_locked_project_serves() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    isolate(tmp.path());
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    link(&[("a", &a), ("b", &b)]);
    seed_skill(tmp.path(), &a, "review", "# from a\n");
    seed_skill(tmp.path(), &b, "review", "# from b\n");

    let proj = tmp.path().join("proj");
    project_referencing(&proj, "review");

    // Activate: this pins `a`'s bytes and materializes them.
    use_profile::run(&use_args(true), Some(&proj)).unwrap();
    let lock_path = proj.join("agentstack.lock");
    let lock_before = fs::read_to_string(&lock_path).unwrap();
    let served = proj.join(".claude/skills/review/SKILL.md");
    assert_eq!(fs::read_to_string(&served).unwrap(), "# from a\n");

    // Put `b` in front. Nothing about this touches the project.
    let mut sources = Sources::load().unwrap();
    sources
        .reorder(&["b".to_string(), "a".to_string()])
        .unwrap();
    sources.save().unwrap();

    assert_eq!(
        fs::read_to_string(&lock_path).unwrap(),
        lock_before,
        "a reorder is machine state — it may not rewrite any project's lock"
    );
    assert_eq!(
        fs::read_to_string(&served).unwrap(),
        "# from a\n",
        "what is served is the pinned bytes, not whatever the sources now say"
    );

    // The paired half: the pinned name NOW resolves to different content. That
    // must re-gate, not swap. Activation fails closed and names the drift, and
    // the refusal leaves the lock exactly as it was.
    let err = use_profile::run(&use_args(true), Some(&proj))
        .unwrap_err()
        .to_string();
    assert!(err.contains("review"), "the gate names the skill: {err}");
    assert!(
        err.contains("drift") || err.contains("changed"),
        "the gate names the divergence: {err}"
    );
    assert_eq!(
        fs::read_to_string(&lock_path).unwrap(),
        lock_before,
        "a blocked activation must never absorb the new source's bytes"
    );
    assert_eq!(
        fs::read_to_string(&served).unwrap(),
        "# from a\n",
        "and it must not have swapped the served bytes on the way out"
    );
}

/// Claim: git is the productized option for versioning a source, never a
/// requirement. A plain folder is a first-class source — it works end to end,
/// and nothing tells the user it is misconfigured.
#[test]
fn a_plain_non_git_linked_folder_is_first_class() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let ash = isolate(tmp.path());
    let plain = tmp.path().join("plain-folder");
    link(&[("plain", &plain)]);
    seed_skill(tmp.path(), &plain, "review", "# plain\n");
    assert!(!plain.join(".git").exists(), "precondition: not a git repo");

    // End to end: it resolves, it locks, it materializes.
    let proj = tmp.path().join("proj");
    project_referencing(&proj, "review");
    use_profile::run(&use_args(true), Some(&proj)).unwrap();
    assert_eq!(
        fs::read_to_string(proj.join(".claude/skills/review/SKILL.md")).unwrap(),
        "# plain\n"
    );

    // `doctor` says nothing that reads as "this folder is set up wrong".
    let report = run_cli(&ash, &proj, &["doctor"]);
    for wrong in ["not a git repo", "not a git repository", "misconfigur"] {
        assert!(
            !report.to_lowercase().contains(wrong),
            "doctor implied a plain library source is broken ({wrong}):\n{report}"
        );
    }

    // And `lib sync` — the git feature — declines honestly: it names what it
    // needs, calls the folder fine, and offers the opt-in.
    let sync = run_cli(&ash, &proj, &["lib", "sync"]);
    assert!(sync.contains("plain folder"), "{sync}");
    assert!(sync.contains("which is fine"), "{sync}");
    assert!(sync.contains("lib sync --init"), "{sync}");
}

/// Claim: `init` imports the CLI configs it finds into a linked library folder,
/// and the project stays clean — a manifest that references by name, with no
/// server definitions and no capability files of its own.
#[test]
fn init_imports_existing_cli_config_into_a_linked_folder() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let ash = isolate(tmp.path());
    let team = tmp.path().join("team");
    link(&[("team", &team)]);

    // One ordinary machine-global CLI config to import.
    fs::write(
        tmp.path().join("home/.claude.json"),
        r#"{"mcpServers":{"search":{"command":"/usr/bin/env","args":["npx","search-mcp"]}}}"#,
    )
    .unwrap();

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    run_cli(&ash, &proj, &["init", "--yes", "--secrets", "skip"]);

    // The definition landed in the linked folder — the FIRST source, not
    // `~/.agentstack/lib`, which this machine never linked.
    let def = team.join("servers/search.toml");
    assert!(def.exists(), "the server definition landed in the source");
    assert!(fs::read_to_string(&def).unwrap().contains("npx"));
    let library = Library::load(&team).unwrap();
    assert!(library.get_server("search").is_some(), "and it is indexed");
    assert!(
        !ash.join("lib/servers/search.toml").exists(),
        "nothing went to the unlinked default library"
    );

    // The project is clean: a name reference, no definitions, no capability
    // files of its own.
    let loaded = agentstack::manifest::load_from_dir(&proj.join(".agentstack")).unwrap();
    assert!(
        loaded.manifest.servers.is_empty(),
        "no inline server definitions in the project"
    );
    assert!(
        loaded
            .manifest
            .profiles
            .values()
            .any(|p| p.servers.iter().any(|s| s == "search")),
        "the project references the imported server by name"
    );
    assert!(!proj.join(".agentstack/servers").exists());
    assert!(!proj.join(".agentstack/skills").exists());
}

/// The no-regression witness: a machine that never linked anything has one
/// library, at the same place, recorded the same way, and resolving through the
/// merged view is byte-identical to reading that single index.
#[test]
fn a_single_library_setup_behaves_exactly_as_before() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let ash = isolate(tmp.path());
    // Deliberately no `link` call: no source list exists on this machine.
    assert!(!ash.join("sources.toml").exists());

    let lib_home = ash.join("lib");
    seed_skill(tmp.path(), &lib_home, "review", "# central\n");

    let sources = Sources::load().unwrap();
    assert_eq!(sources.linked().len(), 1);
    assert_eq!(sources.primary().root, lib_home);

    // The index the merged view exposes is the single index, entry for entry —
    // including the RELATIVE path form, which is what keeps lock entries and
    // rendered artifacts identical to a pre-linked-sources build.
    let single = Library::load(&lib_home).unwrap();
    let merged = Library::load_default().unwrap();
    assert_eq!(merged.skills, single.skills);
    assert_eq!(
        merged.get("review").unwrap().path.as_deref(),
        Some("review")
    );
    assert!(merged.linked.collisions.is_empty());

    // And the implicit source still answers to its name, so a qualified
    // reference is available before anything is linked.
    let proj = tmp.path().join("proj");
    project_referencing(&proj, "review");
    assert_eq!(resolved_body("review", &proj), "# central\n");
    assert_eq!(resolved_body("local:review", &proj), "# central\n");

    // A library write with nothing linked still lands in `~/.agentstack/lib`.
    assert!(lib_home.join("skills/review/SKILL.md").exists());
}

/// Run the real binary with this machine's isolated home. Used only where the
/// claim is about what a **user** is told, which is exactly what a library call
/// cannot witness.
fn run_cli(agentstack_home: &Path, cwd: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("HOME", agentstack_home.parent().unwrap_or(agentstack_home))
        .env("AGENTSTACK_HOME", agentstack_home)
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("NO_COLOR", "1")
        .output()
        .expect("running agentstack");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}
