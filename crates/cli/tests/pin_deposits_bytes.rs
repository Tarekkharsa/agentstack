// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Phase 2 witness: **capturing the approved bytes is a property of pinning.**
//!
//! A re-gate can only show "3 lines changed" if the bytes the user last
//! approved are still on disk. Git-sourced content already lands in
//! `~/.agentstack/store/content/<digest>/` during resolve; path-sourced content
//! — which is exactly what the Phase 1 drop-a-file funnel produces — did not.
//!
//! The fix is deliberately NOT "the four lock-writing call sites remember to
//! deposit." That shape is what produced two real defects already this phase (a
//! capability kind disclosed nowhere, and a structural lint satisfied by test
//! code). Instead the deposit lives inside `Store::pin`, the function that turns
//! a resolved checksum into the typed pin a lock entry carries: a lock entry
//! cannot be constructed without a pin, and a pin cannot be obtained from a
//! `Resolved` without the deposit happening.
//!
//! So this witness is stated UNIVERSALLY rather than per command. It does not
//! assert "`lock` deposits" or "`use --write` deposits" — it asserts, over the
//! whole lockfile after a write, that EVERY path-sourced entry has a store
//! object under its checksum. A future command that writes lock entries some
//! new way is covered without this file changing.
//!
//! Failure posture (design note, `docs/design/consent-card.md`): the deposit is
//! best-effort. A CAS write failure degrades the future diff card to the honest
//! "no snapshot recorded" message and never blocks the pin or the lock write.
//! This witness therefore runs on a healthy filesystem and pins the property;
//! the pathological case is handled by the card's fallback, not here.

use std::fs;
use std::process::{Command, Stdio};

fn run(bin: &str, args: &[&str], home: &std::path::Path, cwd: &std::path::Path) -> (String, bool) {
    let out = Command::new(bin)
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
    (text, out.status.success())
}

/// Every `[[skill]]` row in the lock whose `source = "path"`, as
/// `(name, checksum)`. Parsed with a deliberately dumb line scanner rather than
/// a TOML crate: this witness should not go green because a parser silently
/// tolerated a shape change in the lockfile it is meant to be checking.
fn path_entries(lock_text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for block in lock_text.split("[[skill]]").skip(1) {
        let field = |key: &str| -> Option<String> {
            block.lines().find_map(|l| {
                let l = l.trim();
                let rest = l.strip_prefix(key)?.trim_start();
                let rest = rest.strip_prefix('=')?.trim();
                Some(rest.trim_matches('"').to_string())
            })
        };
        // Stop at the next table so a following [[server]] can't bleed in.
        let block = block.split("\n[[").next().unwrap_or(block);
        if !block.contains("source = \"path\"") {
            continue;
        }
        if let (Some(name), Some(checksum)) = (field("name"), field("checksum")) {
            out.push((name, checksum));
        }
    }
    out
}

fn write_project(proj: &std::path::Path) {
    let a = proj.join(".agentstack");
    fs::create_dir_all(a.join("skills/summarize")).unwrap();
    fs::create_dir_all(a.join("skills/review")).unwrap();
    fs::write(
        a.join("skills/summarize/SKILL.md"),
        "---\ndescription: summarizes\n---\n# Summarize\nfirst body\n",
    )
    .unwrap();
    fs::write(
        a.join("skills/review/SKILL.md"),
        "---\ndescription: reviews\n---\n# Review\nanother body\n",
    )
    .unwrap();
    fs::write(
        a.join("agentstack.toml"),
        r#"version = 1

[skills.summarize]
path = "./skills/summarize"

[skills.review]
path = "./skills/review"
"#,
    )
    .unwrap();
}

/// Assert the universal property over whatever the lock currently holds.
fn assert_every_path_pin_has_bytes(home: &std::path::Path, proj: &std::path::Path, after: &str) {
    let lock_text = fs::read_to_string(proj.join(".agentstack/agentstack.lock"))
        .unwrap_or_else(|e| panic!("no lockfile after {after}: {e}"));
    let entries = path_entries(&lock_text);
    assert!(
        !entries.is_empty(),
        "after {after} the lock recorded no path-sourced skills, so this witness \
         would be vacuous — the fixture or the lock format changed:\n{lock_text}"
    );
    let content_root = home.join(".agentstack/store/content");
    let mut missing = Vec::new();
    for (name, checksum) in &entries {
        // The store is keyed by the bare hex, which is exactly what the lock
        // records as `checksum` (minus any `sha256:` prefix the format carries).
        let hex = checksum.rsplit(':').next().unwrap_or(checksum);
        let dir = content_root.join(hex);
        if !dir.is_dir() {
            missing.push(format!("{name} @ {hex}"));
        }
    }
    assert!(
        missing.is_empty(),
        "after {after}, these path-sourced pins have no bytes in \
         store/content/ — a later re-gate could not show what changed: {missing:?}"
    );
}

/// CONSENT WITNESS (Phase 2, prior bytes). NEVER weaken this to a per-command
/// assertion: the point is that the property holds for ANY lock write, because
/// the deposit happens inside pinning rather than at the call sites.
#[test]
fn every_path_sourced_pin_in_the_lock_has_its_bytes_in_the_content_store() {
    let bin = env!("CARGO_BIN_EXE_agentstack");
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let proj = tmp.path().join("proj");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&proj).unwrap();
    write_project(&proj);

    // Path 1: `lock`.
    let (out, ok) = run(bin, &["lock"], &home, &proj);
    assert!(ok, "lock failed:\n{out}");
    assert_every_path_pin_has_bytes(&home, &proj, "`agentstack lock`");

    // Path 2: `install` — a different builder (install::locked_entry) reaching
    // the same pinning act.
    let (out, ok) = run(bin, &["install"], &home, &proj);
    assert!(ok, "install failed:\n{out}");
    assert_every_path_pin_has_bytes(&home, &proj, "`agentstack install`");

    // Editing a skill produces a NEW digest, so the property must hold for the
    // new pin too — the content store is append-only and per-digest, which is
    // precisely what lets a re-gate hold both sides of a diff at once.
    let before: Vec<(String, String)> =
        path_entries(&fs::read_to_string(proj.join(".agentstack/agentstack.lock")).unwrap());
    fs::write(
        proj.join(".agentstack/skills/summarize/SKILL.md"),
        "---\ndescription: summarizes\n---\n# Summarize\nfirst body\nAND A NEW LINE\n",
    )
    .unwrap();
    let (out, ok) = run(bin, &["lock"], &home, &proj);
    assert!(ok, "re-lock failed:\n{out}");
    assert_every_path_pin_has_bytes(&home, &proj, "an edit + re-lock");

    // Both sides survive: the OLD pin's bytes are still there beside the new
    // one. Without this, "show me what changed" has nothing to diff against.
    let content_root = home.join(".agentstack/store/content");
    let old_summarize = before
        .iter()
        .find(|(n, _)| n == "summarize")
        .map(|(_, c)| c.rsplit(':').next().unwrap_or(c).to_string())
        .expect("summarize was pinned before the edit");
    assert!(
        content_root.join(&old_summarize).is_dir(),
        "the previously approved bytes were evicted by the re-lock — a re-gate \
         diff needs both sides"
    );
    let old_body = fs::read_to_string(content_root.join(&old_summarize).join("SKILL.md"))
        .expect("the old snapshot is readable");
    assert!(
        !old_body.contains("AND A NEW LINE"),
        "the stored snapshot tracked the live edit instead of preserving the \
         approved bytes — it must be a copy, never a link to the project dir"
    );
}
