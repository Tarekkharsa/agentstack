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

/// Every `[[instruction]]` row's `(name, checksum)`. Same deliberately dumb
/// scanner as `path_entries`, and for the same reason: this witness must not go
/// green because a parser tolerated a lockfile shape change.
fn instruction_entries(lock_text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for block in lock_text.split("[[instruction]]").skip(1) {
        let block = block.split("\n[[").next().unwrap_or(block);
        let field = |key: &str| -> Option<String> {
            block.lines().find_map(|l| {
                let rest = l.trim().strip_prefix(key)?.trim_start();
                Some(rest.strip_prefix('=')?.trim().trim_matches('"').to_string())
            })
        };
        if let (Some(name), Some(checksum)) = (field("name"), field("checksum")) {
            out.push((name, checksum));
        }
    }
    out
}

/// Every checksum a `[[table]]` kind pins, as `(label, checksum)`. Same
/// deliberately dumb scanner as the two above, and the same reason: this
/// witness must not go green because a parser tolerated a shape change.
/// `keys` names the checksum fields to collect, because a `[[workflow]]` row
/// pins TWO — its source and its approved blueprint.
fn table_entries(lock_text: &str, table: &str, keys: &[&str]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for block in lock_text.split(table).skip(1) {
        let block = block.split("\n[[").next().unwrap_or(block);
        let field = |key: &str| -> Option<String> {
            block.lines().find_map(|l| {
                let rest = l.trim().strip_prefix(key)?.trim_start();
                Some(rest.strip_prefix('=')?.trim().trim_matches('"').to_string())
            })
        };
        let name = field("name").unwrap_or_else(|| table.to_string());
        for key in keys {
            if let Some(checksum) = field(key) {
                out.push((format!("{name} ({key})"), checksum));
            }
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
    fs::create_dir_all(a.join("instructions")).unwrap();
    fs::write(a.join("instructions/house.md"), "Prefer boring code.\n").unwrap();
    // G21: the three kinds that used to pin with no store object behind them —
    // an extension (a directory tree), a workflow (one script), and that
    // workflow's approved blueprint.
    fs::create_dir_all(a.join("extensions/checkpoint")).unwrap();
    fs::write(
        a.join("extensions/checkpoint/index.ts"),
        "export default function (pi) {} // v1\n",
    )
    .unwrap();
    fs::write(
        a.join("extensions/checkpoint/package.json"),
        "{\"name\":\"checkpoint\"}\n",
    )
    .unwrap();
    fs::create_dir_all(a.join("workflows")).unwrap();
    fs::write(a.join("workflows/pipeline.js"), "exports.run = () => 1;\n").unwrap();
    fs::write(
        a.join("workflows/pipeline.blueprint.json"),
        "{\"pattern\":\"fan-out\",\"goal\":\"review a diff\",\"nodes\":[]}\n",
    )
    .unwrap();
    fs::write(
        a.join("agentstack.toml"),
        r#"version = 1

[skills.summarize]
path = "./skills/summarize"

[skills.review]
path = "./skills/review"

[instructions.house]
path = "./instructions/house.md"

[extensions.checkpoint]
path = "./extensions/checkpoint"
target = "pi"

[workflows.pipeline]
path = "./workflows/pipeline.js"
roles = ["planner"]
blueprint = "./workflows/pipeline.blueprint.json"

[settings.claude-code]
model = "opus"

[toolsets.planner]
skills = ["summarize"]
"#,
    )
    .unwrap();
}

/// Assert the universal property over whatever the lock currently holds.
fn assert_every_path_pin_has_bytes(home: &std::path::Path, proj: &std::path::Path, after: &str) {
    let lock_text = fs::read_to_string(proj.join(".agentstack/agentstack.lock"))
        .unwrap_or_else(|e| panic!("no lockfile after {after}: {e}"));
    // One assertion, two kinds: skills pin a tree digest and instructions pin
    // raw file bytes, but BOTH must have their approved bytes on disk or a
    // re-gate cannot show what changed.
    let mut entries = path_entries(&lock_text);
    let instructions = instruction_entries(&lock_text);
    // G21: extensions and workflows pin the STRICT integrity-root digest and a
    // workflow additionally pins its approved blueprint. All three used to be
    // digested straight into the lockfile with no store object, so a re-gate on
    // them had a pin and no approved copy to diff against.
    let extensions = table_entries(&lock_text, "[[extension]]", &["checksum"]);
    let workflows = table_entries(
        &lock_text,
        "[[workflow]]",
        &["checksum", "blueprint_checksum"],
    );
    // G18: a settings key pins the canonical bytes of its DECLARED value, which
    // live in the manifest rather than in a file — the same shape a server
    // definition pins. It belongs to this universal property for the same
    // reason: a re-gate over an edited `permissions` block can only show what
    // moved if the approved bytes are still on disk.
    let settings = table_entries(&lock_text, "[[setting]]", &["checksum"]);
    assert!(
        !entries.is_empty()
            && !instructions.is_empty()
            && !extensions.is_empty()
            && !settings.is_empty()
            && workflows.len() >= 2,
        "after {after} the lock is missing one of the pinned kinds, so this \
         witness would be vacuous — the fixture or the lock format \
         changed:\n{lock_text}"
    );
    entries.extend(instructions);
    entries.extend(extensions);
    entries.extend(workflows);
    entries.extend(settings);
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
        "after {after}, these pins have no bytes in \
         store/content/ — a later re-gate could not show what changed: {missing:?}"
    );
}

/// G21 BACKWARD-COMPATIBILITY WITNESS: a lockfile written before deposits
/// existed carries pins with no store object, and a project that upgraded
/// mid-flight must keep working — the missing copy degrades to the honest
/// "not recorded" answer, never a failure.
///
/// Simulated by emptying `store/content/` after a lock. That is exactly the
/// on-disk state an older lock leaves (and the state a pruned store leaves),
/// and the lockfile format is unchanged, so it is the whole compatibility
/// surface there is to test.
#[test]
fn a_lock_whose_deposits_are_absent_still_works() {
    let bin = env!("CARGO_BIN_EXE_agentstack");
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let proj = tmp.path().join("proj");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&proj).unwrap();
    write_project(&proj);

    let (out, ok) = run(bin, &["lock", "--write"], &home, &proj);
    assert!(ok, "lock failed:\n{out}");
    let lock_before = fs::read_to_string(proj.join(".agentstack/agentstack.lock")).unwrap();

    // Every deposit disappears — the "older lock" state.
    let content_root = home.join(".agentstack/store/content");
    assert!(content_root.is_dir(), "nothing was deposited to remove");
    fs::remove_dir_all(&content_root).unwrap();

    // A read path still answers.
    let (out, ok) = run(bin, &["trust", ".", "--preview"], &home, &proj);
    assert!(ok, "trust --preview failed with no deposits:\n{out}");
    assert!(out.contains("surface_digest"), "{out}");

    // And re-locking is a no-op on the pins: the deposit is a side effect of
    // pinning, not a lockfile field, so the bytes on disk do not move.
    let (out, ok) = run(bin, &["lock", "--write"], &home, &proj);
    assert!(ok, "re-lock failed:\n{out}");
    assert_eq!(
        fs::read_to_string(proj.join(".agentstack/agentstack.lock")).unwrap(),
        lock_before,
        "adding deposits changed the lockfile format"
    );
    // The re-lock also backfilled what was removed — no migration needed.
    assert_every_path_pin_has_bytes(&home, &proj, "a re-lock over an older lock");
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
    let (out, ok) = run(bin, &["lock", "--write"], &home, &proj);
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
    let (out, ok) = run(bin, &["lock", "--write"], &home, &proj);
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
