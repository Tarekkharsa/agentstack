// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! **The undo surfaces may only promise the undo they actually have.**
//!
//! One constant, `agentstack::history::SKILLS_ARE_NOT_RECORDED`, three places
//! it has to hold, and the mechanism underneath that makes it true. Those were
//! three separate test binaries with byte-identical `run` and `strip_ansi`
//! helpers; they are one binary now, in three clearly marked groups.
//!
//! The shared subject: the change ledger holds a file's BYTES. A delivered
//! skill is a linked DIRECTORY, and `rollback` deletes by path with no
//! ownership test — so widening the ledger to cover skills would let a
//! rollback take a directory the user made by hand. G31 narrowed the PROMISE
//! instead of widening the ledger. Each surface that names `restore` has to
//! carry that narrowing, and the negative controls keep the bound a fact about
//! a particular project rather than a permanent disclaimer people learn to
//! skip.
//!
//! - **Group 1 — `x uninstall`.** The closing line was true of every file edit
//!   the uninstall made and false of the skills it pruned alongside them.
//! - **Group 2 — `session start` / `session end`.** The temporary-session
//!   banner makes the same offer about the same ledger.
//! - **Group 3 — the mechanism.** Why the ledger cannot be widened: no bytes
//!   are captured for a delivered skill, rollback cannot bring one back, and
//!   rollback deletes by path with no ownership test (control: `render::skills`
//!   DOES carry an ownership test and leaves the identical user directory
//!   alone).
//!
//! Groups 1 and 2 spawn the real binary — the claim is about what a person
//! reads at the end of a command, so the evidence has to be the process's own
//! output. Group 3 runs in-process and mutates `HOME`, so it holds
//! [`ENV_LOCK`]; the spawning groups pass every variable to the child
//! explicitly (`env_clear`) and are unaffected by it.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Mutex;

use agentstack::adapter::descriptor::SkillStrategy;
use agentstack::commands::restore;
use agentstack::history;
use agentstack::history::SKILLS_ARE_NOT_RECORDED;
use agentstack::render::{skills, PriorTrust};
use agentstack::state::{State, TargetState};

/// Group 3 mutates the process-global `HOME`; serialize those tests.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Run the binary against `home`/`cwd` and return its combined output, with
/// Run the binary against `home`/`cwd` and return its combined output, with
/// ANSI styling removed — every notice below is printed through `.dimmed()`,
/// and a test that matched escape codes would be asserting on the colour.
fn run(args: &[&str], home: &Path, cwd: &Path) -> (String, bool) {
    let out = Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("HOME", home)
        .env("AGENTSTACK_HOME", home.join(".agentstack"))
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null())
        .output()
        .expect("spawn agentstack");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (strip_ansi(&text), out.status.success())
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // CSI ... final byte in @-~; every sequence the CLI emits is one.
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

// ══ Group 1 — `x uninstall` ═══════════════════════════════════════════════

/// A project with an instructions fragment (a managed region in `CLAUDE.md` —
/// an ordinary FILE edit the ledger does cover) and, when `with_skill`, an
/// inline skill in a toolset. Both are needed: the notice is about the
/// difference between them.
fn uninstall_project(root: &Path, with_skill: bool) {
    fs::create_dir_all(root.join("instructions")).unwrap();
    fs::write(
        root.join("instructions/house.md"),
        "House rule: be brief.\n",
    )
    .unwrap();

    let mut manifest = String::from(
        "version = 1\n\n\
         [targets]\ndefault = [\"claude-code\"]\n\n\
         [delivery]\nrender_locally = true\n\n\
         [instructions.house]\npath = \"./instructions/house.md\"\n\n",
    );
    if with_skill {
        fs::create_dir_all(root.join("skills/greet")).unwrap();
        fs::write(
            root.join("skills/greet/SKILL.md"),
            "---\nname: greet\ndescription: say hello\n---\n# greet\n",
        )
        .unwrap();
        manifest.push_str(
            "[skills.greet]\npath = \"./skills/greet\"\n\n\
             [toolsets.dev]\nskills = [\"greet\"]\n",
        );
    }
    fs::write(root.join("agentstack.toml"), manifest).unwrap();
}

/// Pin the surface, grant trust bound to the exact reviewed bytes, then render.
/// The full ceremony rather than a hand-written state file: skills only
/// materialize through the trust gate, and the notice must describe a delivery
/// that really happened.
fn deliver(home: &Path, root: &Path, with_skill: bool) {
    let (out, ok) = run(&["lock", "--write"], home, root);
    assert!(ok, "lock: {out}");

    let (preview, ok) = run(&["trust", "--preview"], home, root);
    assert!(ok, "trust --preview: {preview}");
    let surface: serde_json::Value =
        serde_json::from_str(&preview).expect("trust --preview emits JSON");
    let digest = surface
        .pointer("/data/surface_digest")
        .or_else(|| surface.get("surface_digest"))
        .and_then(|v| v.as_str())
        .expect("the review surface carries the digest a grant must present")
        .to_string();
    let (out, ok) = run(&["trust", "--yes", "--consented", &digest], home, root);
    assert!(ok, "trust --yes: {out}");

    if with_skill {
        let (out, ok) = run(&["use", "dev", "--write"], home, root);
        assert!(ok, "use --write: {out}");
        assert!(
            root.join(".claude/skills/greet").symlink_metadata().is_ok(),
            "the premise: the toolset materialized a skill: {out}"
        );
    }
    let (out, ok) = run(&["x", "instructions", "--write"], home, root);
    assert!(ok, "instructions --write: {out}");
    assert!(root.join("CLAUDE.md").exists(), "the premise: {out}");
}

#[test]
fn uninstall_says_the_restore_it_names_will_not_bring_skills_back() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let root = tmp.path().join("proj");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&root).unwrap();
    uninstall_project(&root, /*with_skill=*/ true);
    deliver(&home, &root, true);

    // `--keep-home` keeps the ledger, which is the only case where the closing
    // line offers `restore` at all.
    let (out, ok) = run(&["x", "uninstall", "--write", "--keep-home"], &home, &root);
    assert!(ok, "{out}");
    assert!(
        out.contains("Empty skill parent (if empty after cleanup)"),
        "the preview/write plan must disclose the conditional parent cleanup: {out}"
    );
    assert!(
        out.contains("undo with `agentstack restore`"),
        "the promise under test is still made: {out}"
    );
    assert!(
        out.contains(SKILLS_ARE_NOT_RECORDED),
        "the closing line must carry the SHARED reason sentence, so this surface cannot drift \
         from `undo` and `x restore`: {out}"
    );
    assert!(
        out.contains("greet"),
        "and name the skill it is about, as the other Undo surfaces do: {out}"
    );
    assert!(
        out.contains("re-materialize"),
        "and name the way back, since `restore` is not it: {out}"
    );

    // The behaviour the notice describes. Run the restore it named and look at
    // what returns — this is what makes the wording a claim rather than a
    // slogan.
    assert!(
        !root.join(".claude/skills/greet").exists(),
        "the uninstall pruned the delivered skill"
    );
    assert!(
        !root.join(".claude").exists(),
        "uninstall must remove the project-local CLI parent once its managed skills dir \
         was the last thing in it"
    );
    let (out, ok) = run(&["x", "restore", "--last", "--write"], &home, &root);
    assert!(ok, "the named restore runs: {out}");
    assert!(
        root.join("CLAUDE.md").exists(),
        "the file edit comes back — that half of the promise was always true: {out}"
    );
    assert!(
        !root.join(".claude/skills/greet").exists(),
        "the skill does NOT come back. If this ever fails, the ledger has learned to carry \
         skills and this notice — plus G31 — should be revisited rather than patched: {out}"
    );
}

#[test]
fn uninstall_keeps_a_skill_parent_that_contains_user_content() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let root = tmp.path().join("proj");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&root).unwrap();
    uninstall_project(&root, /*with_skill=*/ true);
    deliver(&home, &root, true);
    fs::write(root.join(".claude/notes.txt"), "user-owned\n").unwrap();

    let (out, ok) = run(&["x", "uninstall", "--write", "--keep-home"], &home, &root);
    assert!(ok, "{out}");
    assert_eq!(
        fs::read_to_string(root.join(".claude/notes.txt")).unwrap(),
        "user-owned\n",
        "an otherwise-empty CLI parent is prunable, but user content makes it untouchable"
    );
    assert!(
        root.join(".claude").exists(),
        "the parent that holds user content must survive uninstall"
    );
}

#[test]
fn uninstall_prunes_a_legacy_empty_skill_parent_without_managed_skill_state() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let root = tmp.path().join("proj");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&root).unwrap();
    uninstall_project(&root, /*with_skill=*/ false);
    fs::create_dir_all(root.join(".claude")).unwrap();

    let (out, ok) = run(
        &[
            "x",
            "uninstall",
            "--scope",
            "project",
            "--write",
            "--keep-home",
        ],
        &home,
        &root,
    );
    assert!(ok, "{out}");
    assert!(
        !root.join(".claude").exists(),
        "a prior version's empty skill namespace should be cleaned without a surviving \
         managed-skill record: {out}"
    );
}

/// NEGATIVE CONTROL. Same command, same closing line, same `restore` promise —
/// but this project materialized no skills, so there is nothing the promise
/// overstates and nothing is printed. A notice that appeared here would be a
/// disclaimer, and a disclaimer on every uninstall is read by no one.
#[test]
fn a_project_with_no_materialized_skills_gets_no_notice() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let root = tmp.path().join("proj");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&root).unwrap();
    uninstall_project(&root, /*with_skill=*/ false);
    deliver(&home, &root, false);

    let (out, ok) = run(&["x", "uninstall", "--write", "--keep-home"], &home, &root);
    assert!(ok, "{out}");
    assert!(
        out.contains("undo with `agentstack restore`"),
        "control: the same closing line is reached — so the assertion below is about the \
         notice, not about a different code path: {out}"
    );
    assert!(
        !out.contains(SKILLS_ARE_NOT_RECORDED),
        "control: no materialized skills, no notice: {out}"
    );
}

// ══ Group 2 — temporary sessions ══════════════════════════════════════════

/// The one line of `session start`'s output that opens with "temporary:" —
/// the banner under test.
///
/// Asserted on by itself rather than on the whole run, because the activation
/// `session start` performs prints its OWN (already correct) undo line naming
/// `x restore`, scoped to "server configs only". A whole-output search for
/// "restore" would collide with that surface and prove nothing about this one.
fn temporary_banner(out: &str) -> &str {
    out.lines()
        .find(|l| l.contains("temporary:"))
        .unwrap_or_else(|| panic!("session start opens with the temporary banner: {out}"))
}

/// A project with a server and, when `with_skill`, an inline skill in the same
/// toolset. Both are needed in the positive case: the point is the difference
/// between them under one `restore`.
///
/// `render_locally` is pinned on purpose. Servers default to the live lane, and
/// a session that writes no server config would record no ledger entry — a real
/// failure mode, and one this test exercises in passing, but not the one that
/// separates a captured FILE from an uncaptured skill. Forcing the local render
/// puts a genuine ledger entry next to the skill so `restore` has something to
/// succeed at.
fn session_project(root: &Path, with_skill: bool) {
    let mut manifest = String::from(
        "version = 1\n\n\
         [targets]\ndefault = [\"claude-code\"]\n\n\
         [delivery]\nrender_locally = true\n\n\
         [servers.echo]\ntype = \"stdio\"\ncommand = \"echo\"\nargs = [\"hi\"]\n\n",
    );
    if with_skill {
        fs::create_dir_all(root.join("skills/greet")).unwrap();
        fs::write(
            root.join("skills/greet/SKILL.md"),
            "---\nname: greet\ndescription: say hello\n---\n# greet\n",
        )
        .unwrap();
        manifest.push_str(
            "[skills.greet]\npath = \"./skills/greet\"\n\n\
             [toolsets.dev]\nservers = [\"echo\"]\nskills = [\"greet\"]\n",
        );
    } else {
        manifest.push_str("[toolsets.dev]\nservers = [\"echo\"]\n");
    }
    fs::write(root.join("agentstack.toml"), manifest).unwrap();
}

/// Pin the surface and grant trust bound to the exact reviewed bytes. The full
/// ceremony rather than a hand-written state file: `session start` refuses an
/// untrusted project outright, and skills only materialize through that gate.
fn trust(home: &Path, root: &Path) {
    let (out, ok) = run(&["lock", "--write"], home, root);
    assert!(ok, "lock: {out}");

    let (preview, ok) = run(&["trust", "--preview"], home, root);
    assert!(ok, "trust --preview: {preview}");
    let surface: serde_json::Value =
        serde_json::from_str(&preview).expect("trust --preview emits JSON");
    let digest = surface
        .pointer("/data/surface_digest")
        .or_else(|| surface.get("surface_digest"))
        .and_then(|v| v.as_str())
        .expect("the review surface carries the digest a grant must present")
        .to_string();
    let (out, ok) = run(&["trust", "--yes", "--consented", &digest], home, root);
    assert!(ok, "trust --yes: {out}");
}

#[test]
fn the_restore_that_was_offered_leaves_the_skills_on_disk() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let root = tmp.path().join("proj");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&root).unwrap();
    session_project(&root, /*with_skill=*/ true);
    trust(&home, &root);

    let (out, ok) = run(&["x", "session", "start", "dev"], &home, &root);
    assert!(ok, "{out}");

    // The wording. The banner names the command that is the whole revert, and
    // must not offer `x restore` beside it as an equal.
    let banner = temporary_banner(&out);
    assert!(
        banner.contains("agentstack x session end"),
        "the banner still names the way back before the first byte changes: {banner}"
    );
    assert!(
        !banner.contains("restore"),
        "and must not offer `x restore` as an alternative — the assertions below are what \
         makes that offer false: {banner}"
    );
    assert!(
        out.contains(SKILLS_ARE_NOT_RECORDED),
        "the report carries the SHARED reason sentence, so this surface cannot drift from \
         `undo`, `x restore` and `x uninstall`: {out}"
    );

    // The premise: one captured FILE and one uncaptured skill, both on disk.
    let mcp = root.join(".mcp.json");
    let skill = root.join(".claude/skills/greet");
    assert!(mcp.exists(), "the session wrote a server config: {out}");
    assert!(
        skill.symlink_metadata().is_ok(),
        "the session materialized a skill: {out}"
    );

    // The behaviour. Run the restore that used to be offered and look at what
    // returns — this is what makes the banner a claim rather than a slogan.
    let (restore, ok) = run(&["x", "restore", "--last", "--write"], &home, &root);
    assert!(ok, "the restore runs: {restore}");
    assert!(
        !mcp.exists(),
        "the FILE goes back to its pre-session state (it did not exist), which is why the \
         old parenthetical read as true: {restore}"
    );
    assert!(
        skill.symlink_metadata().is_ok(),
        "the skill does NOT come back off — so `restore` never 'put every file back'. If this \
         ever fails, the ledger has learned to carry skills and this banner, plus G31, should \
         be revisited rather than patched: {restore}"
    );

    // And the command the banner DOES name is the whole revert.
    let (end, ok) = run(&["x", "session", "end"], &home, &root);
    assert!(ok, "{end}");
    assert!(
        skill.symlink_metadata().is_err(),
        "`session end` takes the skill off through its own mechanism — the reason it is the \
         only command the banner names: {end}"
    );
}

/// NEGATIVE CONTROL. Same command, same banner, same restore — but this session
/// materializes no skills, so there is nothing the promise overstates and the
/// bound is not printed. A notice on every `session start` is read by no one.
#[test]
fn a_session_that_materializes_no_skills_gets_no_notice() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let root = tmp.path().join("proj");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&root).unwrap();
    session_project(&root, /*with_skill=*/ false);
    trust(&home, &root);

    let (out, ok) = run(&["x", "session", "start", "dev"], &home, &root);
    assert!(ok, "{out}");
    assert!(
        temporary_banner(&out).contains("agentstack x session end"),
        "control: the same banner is reached — so the assertion below is about the notice, \
         not about a different code path: {out}"
    );
    assert!(
        root.join(".mcp.json").exists(),
        "control: this session really did activate something: {out}"
    );
    assert!(
        !out.contains(SKILLS_ARE_NOT_RECORDED),
        "control: no materialized skills, no notice: {out}"
    );
}

/// `x uninstall --help` is the same sentence as the command body's closing
/// line, and it was narrowed in the same way — so it is checked here rather
/// than left to a reader's eye.
///
/// Help text is STATIC: `--help` has no project to inspect, so unlike every
/// other Undo surface this one cannot be conditional. It therefore states the
/// boundary once, using the shared constant, and this test is the thing that
/// keeps it from being edited back into a bare "restore still works".
///
/// The behaviour behind it is already pinned by
/// group 1 above, which runs the uninstall and
/// the restore it names; this asserts only that the help makes the same promise
/// that test proved the command can keep.
#[test]
fn uninstall_help_carries_the_same_bound_as_the_command_body() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();

    let (out, ok) = run(&["x", "uninstall", "--help"], &home, tmp.path());
    assert!(ok, "{out}");
    assert!(
        out.contains("`agentstack restore` still works afterwards"),
        "the promise under test is still made: {out}"
    );
    assert!(
        out.contains(SKILLS_ARE_NOT_RECORDED),
        "and is bounded by the SHARED reason sentence, not a paraphrase of it: {out}"
    );
    assert!(
        out.contains("re-materialize"),
        "and names the way back, since `restore` is not it: {out}"
    );
}

// ══ Group 3 — why the ledger cannot be widened ════════════════════════════

fn setup_home(home: &Path) {
    fs::create_dir_all(home).unwrap();
    std::env::set_var("HOME", home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));
}

/// A project whose skills are trusted, so materialization exercises MECHANICS
/// rather than failing at the trust gate (which has its own witnesses in
/// `red_team_skills_trust_gate.rs`).
fn trusted_project(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    fs::write(dir.join("agentstack.toml"), "version = 1\n").unwrap();
    agentstack::trust::trust_unreviewed(dir).unwrap();
}

/// A skill source on disk: what `use --write` links to or copies from.
fn skill_source(root: &Path, name: &str) -> std::path::PathBuf {
    let dir = root.join("lib").join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("SKILL.md"), "# skill\n").unwrap();
    dir
}

/// Materialize `name` into `skills_dir` exactly as `use --write` does.
fn materialize(skills_dir: &Path, name: &str, source: &Path, strategy: SkillStrategy, proj: &Path) {
    let plan = skills::plan(
        skills_dir.to_path_buf(),
        strategy,
        vec![(name.to_string(), source.to_path_buf())],
        &[],
        proj,
        PriorTrust::STRICT,
    )
    .unwrap();
    skills::materialize(&plan).unwrap();
}

// ---------------------------------------------------------------------------
// 1. The ledger stores bytes. A delivered skill has none to store.
// ---------------------------------------------------------------------------

/// `history::capture` is `fs::read_to_string`, so it can only describe a FILE.
/// A delivered skill is a symlink to a directory (the default strategy) or a
/// copied directory tree; either way `before` comes out `None` — the ledger
/// holds no record of what was there, which is the whole substance of an undo.
#[test]
fn the_ledger_has_no_bytes_for_a_delivered_skill() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    setup_home(&tmp.path().join("home"));
    let proj = tmp.path().join("proj");
    trusted_project(&proj);
    let source = skill_source(tmp.path(), "greet");

    for (strategy, dirname) in [
        (SkillStrategy::Symlink, "linked"),
        (SkillStrategy::Copy, "copied"),
    ] {
        let skills_dir = proj.join(dirname);
        materialize(&skills_dir, "greet", &source, strategy, &proj);
        let delivered = skills_dir.join("greet");
        assert!(
            delivered.symlink_metadata().is_ok(),
            "{dirname}: the skill was delivered"
        );

        let captured = history::capture(&delivered, "Claude Code · skills");
        assert!(
            captured.before.is_none(),
            "{dirname}: the ledger claimed to hold bytes for a delivered skill — it cannot, \
             and an undo built on that claim would be reverting to a state it never saw"
        );
    }

    // NEGATIVE CONTROL. The same capture on the artifact the ledger is FOR — an
    // ordinary config file — does hold its bytes. So the assertions above fail
    // for the shape of a skill, not for a broken capture.
    let config = proj.join(".mcp.json");
    fs::write(&config, "{\"mcpServers\":{}}").unwrap();
    let captured = history::capture(&config, "Claude Code · servers");
    assert_eq!(
        captured.before.as_deref(),
        Some("{\"mcpServers\":{}}"),
        "control: a config file's pre-write bytes ARE captured"
    );

    std::env::remove_var("AGENTSTACK_HOME");
}

// ---------------------------------------------------------------------------
// 2. And the undo half cannot act on one either.
// ---------------------------------------------------------------------------

/// With `before: None`, `history::rollback` deletes — via `fs::remove_file`,
/// which cannot remove a directory. So a copied skill recorded in the ledger
/// would produce an undo that ERRORS, leaving the user mid-revert. This is the
/// conclusion `x unrender` reached first: its skills leg is `capture: false`.
#[test]
fn rollback_cannot_take_back_a_delivered_skill_directory() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    setup_home(&tmp.path().join("home"));
    let proj = tmp.path().join("proj");
    trusted_project(&proj);
    let source = skill_source(tmp.path(), "greet");

    let skills_dir = proj.join("copied");
    materialize(&skills_dir, "greet", &source, SkillStrategy::Copy, &proj);
    let delivered = skills_dir.join("greet");

    let captured = history::capture(&delivered, "Claude Code · skills");
    assert!(
        history::rollback(std::slice::from_ref(&captured)).is_err(),
        "rollback appeared to take back a copied skill directory — if this ever passes, \
         re-open G31: the seam may have grown a way to carry skills"
    );
    assert!(
        delivered.join("SKILL.md").exists(),
        "the failed rollback left the delivery in place"
    );

    // NEGATIVE CONTROL. The identical call on a file the ledger DOES cover
    // restores it byte for byte — the seam works, skills just do not fit it.
    let config = proj.join(".mcp.json");
    fs::write(&config, "original").unwrap();
    let captured = history::capture(&config, "Claude Code · servers");
    fs::write(&config, "overwritten").unwrap();
    history::rollback(std::slice::from_ref(&captured)).unwrap();
    assert_eq!(
        fs::read_to_string(&config).unwrap(),
        "original",
        "control: rollback restores a captured file exactly"
    );

    std::env::remove_var("AGENTSTACK_HOME");
}

// ---------------------------------------------------------------------------
// 3. The deciding question, answered: no.
// ---------------------------------------------------------------------------

/// **Why widening the ledger would be unsafe, not merely awkward.**
///
/// `history::rollback` acts on a path and carries no ownership test at all: a
/// `before: None` entry deletes whatever is at that path NOW. Every capture of
/// a delivered skill is a `before: None` entry (test 1), so a skills-aware
/// ledger would delete a hand-made skill directory the user put there after the
/// activation — bytes we did not write and cannot prove we wrote.
///
/// This is trust-adjacent code, so that alone settles it: narrow the promise.
#[test]
fn rollback_deletes_by_path_with_no_ownership_test() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    setup_home(&tmp.path().join("home"));
    let proj = tmp.path().join("proj");
    trusted_project(&proj);

    // The path a skill would have been recorded at, captured while absent —
    // exactly the `before: None` every skill capture produces.
    let path = proj.join("skills-note.md");
    let captured = history::capture(&path, "Claude Code · skills");
    assert!(captured.before.is_none());

    // The user then writes their OWN file at that path.
    fs::write(&path, "mine, written by hand\n").unwrap();
    history::rollback(std::slice::from_ref(&captured)).unwrap();
    assert!(
        !path.exists(),
        "the ledger is expected to delete by path; if it ever learns ownership, G31 can be \
         revisited"
    );

    // NEGATIVE CONTROL. `render::skills` — the module that DOES know what we
    // own — leaves the user's identical directory alone under the same prune it
    // uses for our own deliveries. The ownership test exists; it just lives
    // where the ledger cannot reach it without new recording machinery.
    let skills_dir = proj.join("claude-skills");
    fs::create_dir_all(skills_dir.join("greet")).unwrap();
    fs::write(skills_dir.join("greet/SKILL.md"), "user's own\n").unwrap();
    let plan = skills::plan(
        skills_dir.clone(),
        SkillStrategy::Symlink,
        Vec::new(),
        &["greet".to_string()],
        &proj,
        PriorTrust::STRICT,
    )
    .unwrap();
    skills::materialize(&plan).unwrap();
    assert_eq!(
        fs::read_to_string(skills_dir.join("greet/SKILL.md")).unwrap(),
        "user's own\n",
        "control: the skills prune refuses to remove a directory it cannot prove is ours"
    );

    std::env::remove_var("AGENTSTACK_HOME");
}

// ---------------------------------------------------------------------------
// 4. The narrowed promise, where the user meets it.
// ---------------------------------------------------------------------------

/// The undo inventory names the materialized skills it cannot reach, so a
/// person — and a panel drawing an Undo affordance from this exact value —
/// learns the boundary from the surface that has it, not by discovering it
/// afterwards while recovering.
#[test]
fn the_undo_surfaces_name_the_skills_they_cannot_reach() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    setup_home(&tmp.path().join("home"));
    let proj = tmp.path().join("proj");
    trusted_project(&proj);
    let other = tmp.path().join("other");
    trusted_project(&other);

    // What `use --write` leaves behind: skill ownership recorded per target in
    // the state ledger, and NOTHING in the history ledger.
    let mut state = State::load().unwrap();
    for id in ["claude-code", "codex"] {
        state.targets.insert(
            agentstack::state::target_key(id, agentstack::scope::Scope::Project, &proj),
            TargetState {
                managed_skills: vec!["greet".to_string()],
                ..Default::default()
            },
        );
    }
    // A DIFFERENT project's delivery, to prove the note is project-scoped: the
    // state ledger is machine-global, and naming another project's skills here
    // would be a new lie in place of the old one.
    state.targets.insert(
        agentstack::state::target_key("claude-code", agentstack::scope::Scope::Project, &other),
        TargetState {
            managed_skills: vec!["not-ours".to_string()],
            ..Default::default()
        },
    );
    state.save().unwrap();
    assert!(
        history::list().is_empty(),
        "the premise: materializing skills records nothing in the history ledger"
    );

    let registry = agentstack::adapter::Registry::load().unwrap();
    let inventory = restore::list_json_value(&registry, &proj);
    let named: Vec<&str> = inventory["skills_not_recorded"]
        .as_array()
        .expect("the inventory declares its own boundary")
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(
        named,
        vec!["greet"],
        "the inventory names this project's unreachable skill, once, and not another \
         project's"
    );

    // NEGATIVE CONTROL. A project that materialized no skills gets no note —
    // the boundary is a fact about the project, not a permanent disclaimer
    // that would train users to ignore it.
    let clean = tmp.path().join("clean");
    trusted_project(&clean);
    let inventory = restore::list_json_value(&registry, &clean);
    assert!(
        inventory["skills_not_recorded"]
            .as_array()
            .unwrap()
            .is_empty(),
        "control: no materialized skills, no note"
    );

    std::env::remove_var("AGENTSTACK_HOME");
}

/// C3: `undo` and `x restore` do not list the same set, and each must say so.
///
/// The history ledger is machine-global; `undo` filters it to writes that
/// landed inside the project you are standing in, so that a timeline can never
/// offer to revert a repository you are not looking at. The cost is that a
/// machine-scope write — `x gateway connect --all --write`, `apply --scope
/// global` — is recorded and undoable but absent from `undo`. A walkthrough hit
/// exactly that: `undo` showed one entry, `x restore` showed more.
///
/// The scope difference stays. What changed is that both surfaces now state
/// their own boundary, so neither list reads as the complete record.
#[test]
fn undo_and_restore_each_state_which_writes_they_cover() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let root = tmp.path().join("proj");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&root).unwrap();
    // `use --write` is what actually lands a history entry, so the fixture
    // needs the skill: an empty ledger would prove nothing about either list.
    uninstall_project(&root, /*with_skill=*/ true);
    deliver(&home, &root, true);

    // Land a real ledger entry. `render-locally --write` records a project
    // change (crates/cli/src/commands/delivery.rs), which is enough to put
    // both lists into their populated branch.
    let (out, _) = run(
        &["x", "delivery", "render-locally", "--write"],
        &home,
        &root,
    );
    let populated = out.contains("wrote") || out.contains("rendered");

    let (out, ok) = run(&["undo"], &home, &root);
    assert!(ok, "{out}");
    // Both branches — a populated timeline and an empty one — have to name the
    // wider list, because "nothing recorded to undo for this project" is the
    // reading most likely to be mistaken for "nothing was written".
    assert!(
        out.contains("agentstack x restore --list"),
        "undo must name where the machine-wide writes are: {out}"
    );
    if out.contains("recent changes") {
        assert!(
            out.contains("this project only"),
            "a populated timeline must bound itself: {out}"
        );
    }

    let (out, ok) = run(&["x", "restore", "--list"], &home, &root);
    assert!(ok, "{out}");
    if populated || out.contains("Recorded changes") {
        assert!(
            out.contains("everything on this machine"),
            "restore must state the wider scope that makes it the answer: {out}"
        );
    }
}
