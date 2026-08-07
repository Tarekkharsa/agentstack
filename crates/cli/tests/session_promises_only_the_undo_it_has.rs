// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! **`x session start` may only promise the undo it actually has.**
//!
//! The opening banner used to read: "temporary: `agentstack x session end` (or
//! `agentstack x restore --last --write`) puts every file back." The first half
//! is true. The parenthetical was not, and it was the half a scripted caller
//! would reach for:
//!
//! - `session::start` captures ONLY the server-config snapshots into the
//!   history ledger. The skills it materializes are tracked in the session
//!   store and taken off by `session end` through its own mechanism — the G31
//!   boundary [`skills_are_outside_the_undo_ledger.rs`] and
//!   [`uninstall_promises_only_the_undo_it_has.rs`] both pin: the ledger holds
//!   a file's BYTES, and a delivered skill is a symlink to a directory.
//! - So the named restore replays the file edits and leaves the skills on
//!   disk. And a session whose only effect is materialized skills records no
//!   ledger entry at all, so that restore fails outright.
//!
//! Both tests spawn the real binary, because the claim is about what a person
//! reads when a session starts, and the correction has to be the process's own
//! output.
//!
//! - [`the_restore_that_was_offered_leaves_the_skills_on_disk`] is the spine:
//!   it runs the restore that used to be offered and shows on disk what does
//!   and does not come back, then runs the command the banner DOES name and
//!   shows that one is the whole revert. The wording is pinned to that
//!   behaviour rather than to itself.
//! - [`a_session_that_materializes_no_skills_gets_no_notice`] is the negative
//!   control: like every other Undo surface, the bound stays a fact about this
//!   session instead of a disclaimer people learn to skip.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use agentstack::history::SKILLS_ARE_NOT_RECORDED;

/// Run the binary against `home`/`cwd` and return its combined output, with
/// ANSI styling removed — the banner is printed through `.dimmed()`, and a test
/// that matched escape codes would be asserting on the colour.
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
fn project(root: &Path, with_skill: bool) {
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
    let (out, ok) = run(
        &["trust", "--yes", "--consented-digest", &digest],
        home,
        root,
    );
    assert!(ok, "trust --yes: {out}");
}

#[test]
fn the_restore_that_was_offered_leaves_the_skills_on_disk() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let root = tmp.path().join("proj");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&root).unwrap();
    project(&root, /*with_skill=*/ true);
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
    project(&root, /*with_skill=*/ false);
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
/// `uninstall_promises_only_the_undo_it_has.rs`, which runs the uninstall and
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
