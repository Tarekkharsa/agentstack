// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Phase 2 consent witness, leg 1: **the card never discloses less than the
//! surface the grant records.**
//!
//! The bounding rule for the review card is that it may not show less than the
//! old preview did. Anchoring a test to a hand-copied list of yesterday's lines
//! would rot, and worse, would certify yesterday's omissions — `[hooks.*]` and
//! `[settings.*]` were declared, re-gated the trust digest when edited, and
//! appeared on the screen NOWHERE. So the invariant is stated against the
//! machine's own record instead:
//!
//! > every item the grant persists into the consented surface must appear, by
//! > name, in the review the human just read.
//!
//! That record is `trust::prior_surface`, written from the same `diff.mark`
//! calls that render the card's lines — so the two cannot drift apart without
//! this test noticing, and any capability kind added later is covered the
//! moment it marks, with no list to remember to update.
//!
//! Leg 2 (`:review` in `tools/check-structure.py`) covers the case this test
//! structurally cannot: a kind that never marks at all is invisible here,
//! because it is absent from both sides of the comparison.
//!
//! Spawns the real binary, because the claim is about what the terminal prints.

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
        // No terminal: the non-interactive consent gate is the real path here,
        // which is why the grant below must present a consented digest.
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

/// A project declaring one of every kind the card can disclose. Deliberately
/// maximal: coverage here is fixture-relative, so the fixture is the part that
/// must stay honest. A kind added to the manifest model without a line here is
/// caught by leg 2, not by this file.
fn write_fixture(proj: &std::path::Path) {
    let a = proj.join(".agentstack");
    fs::create_dir_all(a.join("skills/summarize")).unwrap();
    fs::create_dir_all(a.join("instructions")).unwrap();
    fs::write(a.join("skills/summarize/SKILL.md"), "# Summarize\nbody\n").unwrap();
    fs::write(
        a.join("instructions/house-rules.md"),
        "Prefer boring code.\n",
    )
    .unwrap();
    fs::write(
        a.join("agentstack.toml"),
        r#"version = 1

[servers.filesystem]
type = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "."]

[servers.docs]
type = "http"
url = "https://api.example.com/mcp/docs"

[skills.summarize]
path = "./skills/summarize"

[instructions.house-rules]
path = "./instructions/house-rules.md"

[hooks.pre-commit]
event = "PreToolUse"
matcher = "Bash"
command = "./scripts/check.sh"
args = ["--strict"]

[settings.claude-code]
permissions = { allow = ["Bash(git status)"] }
"#,
    )
    .unwrap();
}

/// CONSENT WITNESS (Phase 2, the card). NEVER delete or weaken this test: it is
/// the end-to-end form of "the card shows everything the grant records."
#[test]
fn every_recorded_surface_item_appears_in_the_review_the_human_read() {
    let bin = env!("CARGO_BIN_EXE_agentstack");
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let proj = tmp.path().join("proj");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&proj).unwrap();
    write_fixture(&proj);

    let (lock_out, lock_ok) = run(bin, &["lock"], &home, &proj);
    assert!(lock_ok, "lock failed:\n{lock_out}");

    // The digest the human would have reviewed, taken from the read-only
    // preview exactly as a non-interactive caller must.
    let (preview, preview_ok) = run(bin, &["trust", "--preview"], &home, &proj);
    assert!(preview_ok, "preview failed:\n{preview}");
    let v: serde_json::Value = serde_json::from_str(&preview).expect("preview is JSON");
    let digest = v["surface_digest"]
        .as_str()
        .expect("preview carries surface_digest")
        .to_string();

    // The grant prints the WHOLE review before the gate, then records the
    // surface. Capturing its stdout captures precisely what a human read.
    let (card, granted) = run(
        bin,
        &["trust", "--yes", "--consented-digest", &digest],
        &home,
        &proj,
    );
    assert!(granted, "grant failed:\n{card}");

    // The machine's own record of what consent covered. Read from the store
    // the child process wrote, rather than calling the library in-process:
    // `prior_surface` resolves AGENTSTACK_HOME from the *test* process, and
    // mutating global env here would make this witness racy against every
    // other test in the binary. Reading the file is also the stricter check —
    // it asserts on what was actually persisted to disk.
    let store: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(home.join(".agentstack/trust.json")).unwrap())
            .expect("trust store is JSON");
    let entry = store["trusted"]
        .as_object()
        .expect("store has a trusted map")
        .values()
        .next()
        .expect("exactly one project was granted");
    let recorded: Vec<(String, String)> = entry["surface"]
        .as_array()
        .expect("the grant recorded a surface")
        .iter()
        .map(|it| {
            (
                it["kind"].as_str().unwrap_or_default().to_string(),
                it["name"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    assert!(
        !recorded.is_empty(),
        "the grant recorded no surface at all — the comparison below would be vacuous"
    );

    // `mark` receives RAW names while the card prints sanitized copies, so the
    // comparison is sanitized-to-sanitized or an exotic name fails spuriously.
    let mut missing: Vec<String> = Vec::new();
    for (kind, name) in &recorded {
        if name.is_empty() {
            // Aggregate rows (secrets, policy) carry no name; their presence is
            // asserted separately below, by the text the card owes them.
            continue;
        }
        let shown = agentstack::text::sanitize_line(name);
        if !card.contains(&shown) {
            missing.push(format!("{kind} {shown}"));
        }
    }
    assert!(
        missing.is_empty(),
        "the grant recorded these items but the review never showed them: {missing:?}\n\
         --- what the human read ---\n{card}"
    );

    // The kinds that shipped undisclosed are named explicitly, so a future
    // refactor that drops them again fails on a sentence a human can read
    // rather than only on a generic coverage count.
    for owed in ["pre-commit", "claude-code"] {
        assert!(
            card.contains(owed),
            "the card omitted {owed:?} — this is the exact gap Phase 2 closed:\n{card}"
        );
    }
    // The wildcard hook scope must be spelled out, never left as a bare `[*]`
    // for the reader to decode — it is the WIDEST scope, and it was rendered
    // as the least alarming glyph.
    assert!(
        card.contains("every hook-capable CLI"),
        "the hook's wildcard target was not explained in words:\n{card}"
    );
    // The machine ceiling is a constant fact the card owes every reader.
    assert!(
        card.contains("machine policy ceiling"),
        "the card dropped the machine-ceiling line:\n{card}"
    );
}
