// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! **`x uninstall` may only promise the undo it actually has.**
//!
//! The closing line — "Its own state is still under ~/.agentstack (undo with
//! `agentstack restore`)" — was true of every file edit the uninstall made and
//! false of the skills it pruned alongside them. The skills leg is
//! `capture: false` (`commands::unrender::Removal::capture`), for the reason
//! G31 established and `skills_are_outside_the_undo_ledger.rs` proves: the
//! change ledger holds a file's BYTES, a delivered skill is a linked directory,
//! and `rollback` deletes by path with no ownership test — so widening the
//! ledger would take a skill directory a user made by hand. G31 narrowed the
//! promise instead. This surface had not been narrowed with it.
//!
//! Both tests spawn the real binary: the claim is about what a person reads at
//! the end of an uninstall, so the evidence has to be the process's own output.
//!
//! - [`uninstall_says_the_restore_it_names_will_not_bring_skills_back`] runs the
//!   named `restore` and shows what does and does not come back, so the notice
//!   is pinned to the behaviour rather than to its own wording.
//! - [`a_project_with_no_materialized_skills_gets_no_notice`] is the negative
//!   control: the bound stays a fact about this project, never a permanent
//!   disclaimer people learn to skip.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use agentstack::history::SKILLS_ARE_NOT_RECORDED;

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

/// A project with an instructions fragment (a managed region in `CLAUDE.md` —
/// an ordinary FILE edit the ledger does cover) and, when `with_skill`, an
/// inline skill in a toolset. Both are needed: the notice is about the
/// difference between them.
fn project(root: &Path, with_skill: bool) {
    fs::create_dir_all(root.join("instructions")).unwrap();
    fs::write(
        root.join("instructions/house.md"),
        "House rule: be brief.\n",
    )
    .unwrap();

    let mut manifest = String::from(
        "version = 1\n\n\
         [targets]\ndefault = [\"claude-code\"]\n\n\
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
    let (out, ok) = run(
        &["trust", "--yes", "--consented-digest", &digest],
        home,
        root,
    );
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
    project(&root, /*with_skill=*/ true);
    deliver(&home, &root, true);

    // `--keep-home` keeps the ledger, which is the only case where the closing
    // line offers `restore` at all.
    let (out, ok) = run(&["x", "uninstall", "--write", "--keep-home"], &home, &root);
    assert!(ok, "{out}");
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
    project(&root, /*with_skill=*/ false);
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
