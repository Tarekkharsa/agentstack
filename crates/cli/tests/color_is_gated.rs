// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! TODO gap P8-G1 — ANSI colour was unconditional: `.green()`/`.bold()` wrote
//! escapes into files, pipes and CI logs, `NO_COLOR=1` included.
//!
//! These tests run the real binary with stdout on a pipe, which is exactly the
//! condition the defect described, and read the RAW BYTES rather than trusting
//! a rendering. The gate is `agentstack_core::paint`; the rules it applies are
//! unit-tested there as a table. What can only be checked from out here is that
//! the gate actually reaches the screens — that every `.dimmed()` in ~52 files
//! goes through it — so each case below is a whole command's real output.
//!
//! The negative control matters as much as the rest: a gate that disabled
//! colour unconditionally would pass every "no escapes" assertion and be just
//! as wrong. `CLICOLOR_FORCE=1` proves the colour path is intact and that the
//! answer is still a decision.

use std::process::Command;

/// The condition every case shares: stdout is a pipe, never a terminal, so the
/// TTY leg of the decision is "no" throughout and the env vars are the only
/// thing under test. Returns raw stdout+stderr bytes and the exit code, read
/// from the child directly — never through a shell pipeline, which would
/// report the wrong process's status.
fn run(env: &[(&str, &str)], args: &[&str]) -> (Vec<u8>, Option<i32>) {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_agentstack"));
    cmd.args(args)
        .current_dir(&proj)
        .env("HOME", &home)
        .env("AGENTSTACK_HOME", home.join(".agentstack"))
        // Inherited values would decide the test for it.
        .env_remove("NO_COLOR")
        .env_remove("CLICOLOR_FORCE")
        .env_remove("FORCE_COLOR")
        .env("TERM", "xterm-256color");
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("the binary runs");
    let mut bytes = out.stdout;
    bytes.extend_from_slice(&out.stderr);
    (bytes, out.status.code())
}

/// Count of CSI introducers — the shape every colour sequence this workspace
/// emits begins with. Counting bytes, not eyeballing a string, is the point:
/// an escape is invisible in a test failure message otherwise.
fn escapes(bytes: &[u8]) -> usize {
    bytes.windows(2).filter(|w| w == b"\x1b[").count()
}

fn assert_clean(label: &str, bytes: &[u8]) {
    assert_eq!(
        escapes(bytes),
        0,
        "{label}: expected no ANSI escapes, found {} in:\n{}",
        escapes(bytes),
        String::from_utf8_lossy(bytes).escape_debug()
    );
}

/// The defect as reported: output redirected to a file or a pipe carries no
/// colour, on several screens, not just the one that was captured.
#[test]
fn a_pipe_gets_no_colour() {
    for args in [
        vec!["status"],
        vec!["doctor"],
        vec!["--help"],
        vec!["adapters"],
        vec![],
    ] {
        let (bytes, _) = run(&[], &args);
        assert_clean(&format!("agentstack {}", args.join(" ")), &bytes);
    }
}

/// <https://no-color.org>: any value that is not empty disables colour. Checked
/// with a value that is not `1`, because "NO_COLOR=0" meaning "no colour" is
/// the counter-intuitive half of that convention and the half implementations
/// get wrong.
#[test]
fn no_color_disables_colour() {
    for value in ["1", "0", "anything"] {
        let (bytes, code) = run(&[("NO_COLOR", value)], &["status"]);
        assert_clean(&format!("NO_COLOR={value}"), &bytes);
        assert_eq!(code, Some(0), "NO_COLOR={value} must not change the exit");
    }
}

/// The negative control. Without this, "no escapes anywhere" would also pass
/// against a binary that had lost colour entirely.
///
/// `CLICOLOR_FORCE=1` is also the answer for someone piping into a pager who
/// *wants* the colour, which is why the gate has an opt-in at all. It outranks
/// `NO_COLOR` — it is the more specific, more deliberate request — and this
/// pins that order so it cannot be flipped silently.
#[test]
fn clicolor_force_keeps_colour_through_a_pipe() {
    let (forced, code) = run(&[("CLICOLOR_FORCE", "1")], &["status"]);
    assert!(
        escapes(&forced) > 0,
        "CLICOLOR_FORCE=1 must still colour a pipe, found none in:\n{}",
        String::from_utf8_lossy(&forced).escape_debug()
    );
    assert_eq!(code, Some(0));

    let (both, _) = run(&[("CLICOLOR_FORCE", "1"), ("NO_COLOR", "1")], &["status"]);
    assert_eq!(
        both, forced,
        "CLICOLOR_FORCE outranks NO_COLOR; the two together must equal forced alone"
    );

    // ...and the same screen, ungated, is the same text without the escapes.
    let (plain, _) = run(&[], &["status"]);
    let stripped: Vec<u8> = strip_ansi(&forced);
    assert_eq!(
        String::from_utf8_lossy(&stripped),
        String::from_utf8_lossy(&plain),
        "the gate must remove colour and nothing else"
    );
}

/// `TERM=dumb` names a terminal that cannot render escapes. Cheap, conventional,
/// and the one case where stdout could legitimately be a TTY and still deserve
/// no colour.
#[test]
fn term_dumb_disables_colour() {
    let (bytes, _) = run(&[("TERM", "dumb")], &["status"]);
    assert_clean("TERM=dumb", &bytes);
}

/// The machine surface must be unaffected in both directions: no escapes when
/// colour is off (it never had any — checked so a future `.dimmed()` in a JSON
/// path is caught), and no escapes when colour is FORCED either, because a
/// coloured JSON document is unparseable.
#[test]
fn json_stays_machine_parseable() {
    for env in [vec![], vec![("CLICOLOR_FORCE", "1")]] {
        for args in [vec!["status", "--json"], vec!["doctor", "--json"]] {
            let (bytes, _) = run(&env, &args);
            let label = format!("{args:?} with {env:?}");
            assert_clean(&label, &bytes);
            let text = String::from_utf8(bytes).expect("JSON output is UTF-8");
            serde_json::from_str::<serde_json::Value>(&text)
                .unwrap_or_else(|e| panic!("{label} must parse as JSON: {e}\n{text}"));
        }
    }
}

/// Files allowed to import upstream `owo_colors` directly, and why. Anything
/// not on this list is a bypass of the gate.
///
/// The list is EMPTY, and that is the finished state. The last two exceptions —
/// `commands/runs.rs` and `commands/uninstall.rs`, each one unconverted
/// `use owo_colors::OwoColorize;` left behind because another agent held the
/// file when the gate landed — are converted, so `owo-colors` is no longer a
/// dependency of `crates/cli` at all. In this crate the rule is now enforced by
/// the compiler: an ungated import does not resolve. The list stays because the
/// scan below still needs it, and because an exception should have to be added
/// on purpose, with a reason, rather than appear as a diff nobody reads.
///
/// If you are here to add one, the conversion you are avoiding is one line:
///
/// ```text
/// -use owo_colors::OwoColorize;
/// +use agentstack_core::paint::OwoColorize;
/// ```
const MAY_IMPORT_UPSTREAM: &[(&str, &str)] = &[];

/// The gate is only as good as the habit around it. One stray
/// `use owo_colors::OwoColorize;` in a new file silently restores the exact
/// defect P8-G1 describes, in that file only, and no behavioural test would
/// catch it — the screens it colours might not be among the ones sampled above.
///
/// So this reads the source rather than the output. It lives here, beside the
/// behavioural witnesses, rather than in `tools/check-structure.py`: that tool
/// enforces CRATE-level structure (which crate may depend on which, parsed out
/// of ARCHITECTURE.md), and this is a within-crate rule about one trait, with
/// no architectural edge to record. Keeping it here also means the whole P8-G1
/// story — the gate's behaviour and the rule that keeps it reachable — fails in
/// one `cargo test --test color_is_gated`.
#[test]
fn every_screen_uses_the_gated_trait() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("crates/cli sits two levels below the repo root")
        .to_path_buf();

    // The cli crate holds ~1250 of the colour call sites. `core` is scanned too:
    // it is where the gate lives, so it is the one crate that MUST reach
    // upstream — but only from `paint.rs`, which is the delegation itself.
    let mut offenders: Vec<String> = Vec::new();
    let mut scanned = 0usize;
    for crate_src in ["crates/cli/src", "crates/core/src"] {
        for path in rust_files(&repo.join(crate_src)) {
            scanned += 1;
            let text = std::fs::read_to_string(&path).unwrap();
            // The import is what grants the trait's methods; a bare mention in
            // prose (this codebase comments heavily, including about this very
            // gate) is not a bypass.
            if !text.contains("use owo_colors::") {
                continue;
            }
            let rel = path
                .strip_prefix(&repo)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            // `paint.rs` IS the sanctioned delegation: it wraps upstream so the
            // emitted bytes stay identical when colour is on.
            if rel == "crates/core/src/paint.rs" {
                continue;
            }
            if MAY_IMPORT_UPSTREAM.iter().any(|(f, _)| *f == rel) {
                continue;
            }
            offenders.push(rel);
        }
    }

    assert!(
        scanned > 100,
        "scanned only {scanned} files — the walk is broken, not the codebase"
    );
    assert!(
        offenders.is_empty(),
        "these files import upstream `owo_colors` directly, which is unconditional \
         and ignores NO_COLOR (TODO gap P8-G1):\n  {}\n\nUse the gated trait instead:\n  \
         -use owo_colors::OwoColorize;\n  +use agentstack_core::paint::OwoColorize;\n\n\
         If a file genuinely needs upstream, add it to MAY_IMPORT_UPSTREAM in this \
         file with the reason.",
        offenders.join("\n  ")
    );
}

fn rust_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    out
}

/// Remove SGR sequences only — enough to compare the forced and gated renders
/// of the same screen.
fn strip_ansi(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b && bytes.get(i + 1) == Some(&b'[') {
            i += 2;
            while i < bytes.len() && bytes[i] != b'm' {
                i += 1;
            }
            i += 1;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    out
}
