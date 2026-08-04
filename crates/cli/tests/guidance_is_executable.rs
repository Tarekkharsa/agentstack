// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! **Every command the product suggests is a contract.**
//!
//! If AgentStack tells a person or an agent to run something, then running it
//! must be *possible* and must *make progress*. Nothing in the type system
//! says so today: user-facing guidance is ~100 independent string literals
//! spread over ~20 files, with no shared source and no executable contract.
//!
//! That is not a hypothetical. One semantic change — `agentstack lock` became
//! preview-by-default, so the writing form is `lock --write` — silently
//! invalidated roughly a hundred of those strings. Every one stayed
//! grammatical English, the workspace compiled, and the whole suite passed.
//! It took two adversarial review passes and three repair rounds to find them,
//! and each round opened a fresh dead end while closing the old one. Sweeping
//! the strings by hand is not a cure: the next semantic change breaks them
//! again, just as quietly.
//!
//! This file is the cure. It drives the real binary over a matrix of real
//! project states, harvests every command-shaped string the product emits,
//! and asserts three properties, strongest first:
//!
//! * **(a0) A MACHINE FIELD CARRIES A COMMAND, OR NOTHING** — every string in
//!   a machine-readable guidance field is a runnable `agentstack …`
//!   invocation, placeholder-free. Prose in a machine field is a reproduced
//!   infinite loop for a JSON driver, and it used to be *dropped* here in
//!   silence: the harvest kept only strings that already looked like commands,
//!   so the defect could not even be counted. Nothing is dropped now.
//! * **(a) PARSES** — every harvested command parses against the real clap
//!   tree. A suggestion naming a retired verb or a removed flag fails here.
//! * **(b) IS NOT A NO-OP WHERE A WRITE IS REQUIRED** — a command offered as
//!   the machine-readable *fix* for a blocking finding, whose own clap node
//!   declares a `--write` flag, must carry that flag. This is the assertion
//!   that would have caught the entire `lock` regression at commit time, and
//!   it is derived from the clap tree rather than from a hand list, so a new
//!   preview-by-default command is covered on the day it lands.
//! * **(c) CONVERGES** — for EVERY state in the matrix whose machine field
//!   names a command, that command is executed verbatim and the project must
//!   measurably move. Detection is by observable state, never by exit code:
//!   one state in the matrix exists because there the offered command exits 0
//!   and prints a green tick while the blocking condition stands untouched.
//! * **(e) IS DISCOVERABLE** — every harvested command's verb can be found
//!   from `agentstack --help`: listed there directly, named there as reachable,
//!   or listed one step away under `agentstack x`. A surface may never name a
//!   command a person cannot find. This is the rule the command-visibility
//!   split could break and that (a) structurally cannot see: hiding a command
//!   from `--help` does not stop it parsing, so a guard built only on the clap
//!   tree sails straight through the defect. The discoverable set is DERIVED
//!   from the real binary's help output on every run — never hardcoded here, or
//!   the rule would rot the moment the visible set changed.
//! * **(d) THE SURFACES AGREE** — `doctor --json`, `status --json` and
//!   `trust --preview` describe ONE project, so their machine fields may not
//!   contradict each other. This is the general form of the last two rounds of
//!   defects: every string involved parsed, wrote, and carried no placeholder,
//!   and the bug was that two surfaces said different things at once.
//!
//! Two deliberate asymmetries, both load-bearing:
//!
//! * A placeholder (`<query>`, `<name>`, …) is *fine* in human-facing prose —
//!   "agentstack search <query>" is how you teach a shape. It is *not* fine in
//!   a machine-readable fix field, where a caller is expected to execute the
//!   string verbatim. Level (a) substitutes a sentinel for placeholders before
//!   parsing; a separate, stricter assertion forbids placeholders in the
//!   machine fields specifically.
//! * Human prose may offer a preview (`agentstack lock` to *look*). A machine
//!   fix for a blocking finding may not — see (b).
//!
//! Scope, stated so the gaps read as decisions:
//!
//! * Convergence (c) now runs over the WHOLE matrix, plus the three dedicated
//!   drift/never-pinned tests below. It used to run for the two drift shapes
//!   and the never-pinned one — exactly the states that already worked — so
//!   the check written to prevent the loop class never executed a machine
//!   field in the states where that class kept reappearing, and the guard
//!   passed green through two repair loops while the class escaped twice more.
//!   Exactly one state is exempt and it is named in the run's own NOT COVERED
//!   list: a fix of `agentstack trust .` needs a reviewed digest a human
//!   supplies and cannot be driven from a stdin-null spawn. A `null` machine
//!   field is likewise recorded rather than asserted — there is no command to
//!   execute — and rule (d) is what guards those states instead.
//! * The matrix carries two MISSING-BODY states. It had none, and that hole is
//!   where the final defect lived: a declared body absent from disk, where
//!   `trust --preview` correctly emitted `fix: null` while `doctor` and
//!   `status` named `agentstack lock --write`. One of the two is behind an
//!   exit-0 green tick, which is why (c) compares state and not exit status.
//! * Guidance emitted by interactive-only paths (the `init` wizard, TTY
//!   confirms) is out of reach of a spawned, stdin-null binary and is not
//!   harvested. `docs_commands.rs` covers the same class for documentation
//!   prose.
//! * Nothing here is silent: every state, surface, skipped surface, and
//!   unjudged string is printed by [`report`] on a passing run, and the
//!   "not covered" line is computed from that record alone. A guard that
//!   quietly narrows its own coverage is how this class of bug survived three
//!   rounds — and an earlier version of THIS file printed "nothing — every
//!   surface answered in every state" while dropping machine-field prose on
//!   the floor and never reaching a healthy project at all.
//! * The matrix reaches the healthy rungs (`healthy-ungrouped`,
//!   `healthy-grouped`). It has to: the strings a *working* project emits are
//!   guidance too, and the one defect that escaped every earlier round lived
//!   there, where no unhealthy fixture could ever see it.
//! * One rung stays out of reach and is named rather than papered over:
//!   `Rung::Group` on `status`. `status` answers the Group rung only after a
//!   bridge is registered, and `run` spawns with `env_clear` and
//!   `PATH=/usr/bin:/bin` on purpose, so no harness is ever detected and the
//!   "register the bridge" branch precedes the ladder in every state this
//!   file can build. A placeholder reached a machine field behind exactly
//!   that shadow. The rules that judge it are therefore asserted directly, on
//!   the emitted payload, by
//!   `the_harvest_tags_a_nested_command_carrier_as_a_machine_field`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use clap::{CommandFactory, Parser};

// ---------------------------------------------------------------------------
// Running the real binary
// ---------------------------------------------------------------------------

struct Out {
    text: String,
    ok: bool,
}

/// Spawn the real `agentstack` with an isolated HOME and a minimal PATH.
///
/// `env_clear` matters: these assertions are about what the product says to a
/// user in a clean environment, and an inherited `AGENTSTACK_*` or a developer
/// HOME would silently change which findings appear.
fn run(args: &[&str], home: &Path, proj: &Path) -> Out {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(args)
        .current_dir(proj)
        .env_clear()
        .env("HOME", home)
        .env("AGENTSTACK_HOME", home.join(".agentstack"))
        .env("PATH", "/usr/bin:/bin")
        // Colour codes would land inside harvested strings; ask for none, and
        // strip anything that arrives anyway (see `strip_ansi`).
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        // No terminal: the non-interactive path is the one a script and an
        // agent take, and it is the one whose guidance must be executable.
        .stdin(std::process::Stdio::null())
        .output()
        .expect("spawn agentstack");
    Out {
        text: format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
        ok: out.status.success(),
    }
}

/// Drop CSI escape sequences. Belt and braces next to `NO_COLOR`: a single
/// stray `\x1b[1m` inside a harvested token turns a real failure into a
/// confusing one.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

// ---------------------------------------------------------------------------
// The state matrix
// ---------------------------------------------------------------------------

/// A manifest with one of every kind that produces guidance, so the surfaces
/// have something to be unhappy about in more than one section.
const FULL_MANIFEST: &str = r#"version = 1

[servers.filesystem]
type = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "."]

[skills.summarize]
path = "./skills/summarize"

[instructions.house-rules]
path = "./instructions/house-rules.md"

[settings.claude-code]
permissions = { allow = ["Bash(git status)"] }
"#;

/// Instructions + settings only: the pure rendered lane, no servers at all.
const RENDERED_ONLY_MANIFEST: &str = r#"version = 1

[instructions.house-rules]
path = "./instructions/house-rules.md"

[settings.claude-code]
permissions = { allow = ["Bash(git status)"] }
"#;

/// A skill declared in `[skills]` but named by no toolset, whose body is never
/// written to disk — beside one that pins cleanly.
///
/// This is the nastier of the two missing-body shapes. Because every OTHER
/// declared item pins, `agentstack lock --write` exits 0 and prints a green
/// tick while the blocking condition stands, so an exit-code check alone can
/// not see the dead end. Only a state comparison can.
const ORPHAN_MISSING_BODY_MANIFEST: &str = r#"version = 1

[skills.summarize]
path = "./skills/summarize"

[skills.orphan]
path = "./skills/orphan"

[toolsets.dev]
skills = ["summarize"]
"#;

/// Servers declared and nothing else — the "declared live, no bridge
/// connected" shape.
const SERVERS_ONLY_MANIFEST: &str = r#"version = 1

[servers.filesystem]
type = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "."]

[servers.docs]
type = "http"
url = "https://api.example.com/mcp/docs"
"#;

struct State {
    name: &'static str,
    home: PathBuf,
    proj: PathBuf,
}

/// A skill body with no frontmatter `description:` — the shape doctor advises
/// about, kept for the unhealthy states so that advice stays harvested.
const SKILL_UNDESCRIBED: &str = "# Summarize\nbody\n";

/// A well-formed skill: real frontmatter with a `description:`, so a project
/// built from it can actually reach the healthy rungs instead of stopping at
/// the advisory.
const SKILL_DESCRIBED: &str = "---\nname: summarize\ndescription: Summarize a document into bullet points.\n---\n\n# Summarize\nbody\n";

fn write_project(root: &Path, manifest: &str) -> (PathBuf, PathBuf) {
    write_project_with_skill(root, manifest, SKILL_UNDESCRIBED)
}

fn write_project_with_skill(root: &Path, manifest: &str, skill: &str) -> (PathBuf, PathBuf) {
    let home = root.join("home");
    let proj = root.join("proj");
    let a = proj.join(".agentstack");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(a.join("skills/summarize")).unwrap();
    fs::create_dir_all(a.join("instructions")).unwrap();
    fs::write(a.join("skills/summarize/SKILL.md"), skill).unwrap();
    fs::write(a.join("instructions/house-rules.md"), "Prefer boring.\n").unwrap();
    fs::write(a.join("agentstack.toml"), manifest).unwrap();
    (home, proj)
}

/// Pin the surface. Uses `lock --write` on purpose: the *writing* form is the
/// point of the whole file, and a fixture that guessed wrong would build the
/// wrong states.
fn lock(home: &Path, proj: &Path) {
    let out = run(&["lock", "--write"], home, proj);
    assert!(out.ok, "fixture: lock --write failed:\n{}", out.text);
}

/// Consent to exactly the digest the review emitted — the non-interactive
/// grant a panel drives. This test never constructs a grant by any other
/// route; it only *reads* the surfaces afterwards.
fn grant(home: &Path, proj: &Path) {
    let preview = run(&["trust", "--preview"], home, proj);
    assert!(preview.ok, "fixture: trust --preview:\n{}", preview.text);
    let v: serde_json::Value =
        serde_json::from_str(&strip_ansi(&preview.text)).expect("preview is JSON");
    let digest = v["surface_digest"].as_str().unwrap().to_string();
    let out = run(
        &["trust", "--yes", "--consented-digest", &digest],
        home,
        proj,
    );
    assert!(out.ok, "fixture: trust grant failed:\n{}", out.text);
}

/// Render the setup into the CLIs. Without this the ladder never leaves the
/// `apply` rung, and the healthy guidance strings are never emitted at all.
fn apply(home: &Path, proj: &Path) {
    let out = run(&["apply", "--write"], home, proj);
    assert!(out.ok, "fixture: apply --write failed:\n{}", out.text);
}

/// Name a toolset, non-interactively, via the same preview → consented-digest
/// ceremony a panel drives. This is what takes a rendered project off the
/// "group these for a task" rung and onto the verified one.
fn create_toolset(home: &Path, proj: &Path, name: &str, server: &str) {
    let preview = run(
        &["toolset", "create", name, "--server", server, "--preview"],
        home,
        proj,
    );
    assert!(
        preview.ok,
        "fixture: toolset create --preview:\n{}",
        preview.text
    );
    let v: serde_json::Value =
        serde_json::from_str(&strip_ansi(&preview.text)).expect("toolset preview is JSON");
    // The digest key is read by shape rather than assumed: the preview
    // envelope carries it under whichever of these names this version uses,
    // and a fixture that guessed would fail opaquely.
    let digest = ["consent_digest", "digest", "surface_digest"]
        .iter()
        .find_map(|k| v[*k].as_str())
        .unwrap_or_else(|| panic!("toolset preview carries no consent digest:\n{v:#}"))
        .to_string();
    let out = run(
        &[
            "toolset",
            "create",
            name,
            "--server",
            server,
            "--yes",
            "--consented",
            &digest,
        ],
        home,
        proj,
    );
    assert!(out.ok, "fixture: toolset create --yes:\n{}", out.text);
}

/// Content drift: the bytes a lock pinned and a grant was bound to change
/// underneath both. Nothing new is declared — only the body moves.
fn drift_content(proj: &Path) {
    fs::write(
        proj.join(".agentstack/skills/summarize/SKILL.md"),
        "# Summarize\nbody, but different bytes\n",
    )
    .unwrap();
}

/// Content drift *plus* a newly declared capability, so the consented surface
/// itself has to be re-gated. This is the harder and more realistic shape: a
/// branch lands, the manifest grows a server, and a file the lock pinned
/// changed in the same commit.
fn drift_content_and_surface(proj: &Path) {
    drift_content(proj);
    let manifest = proj.join(".agentstack/agentstack.toml");
    let mut body = fs::read_to_string(&manifest).unwrap();
    body.push_str(
        "\n[servers.notes]\ntype = \"stdio\"\ncommand = \"node\"\nargs = [\"notes.js\"]\n",
    );
    fs::write(&manifest, body).unwrap();
}

/// Build every state in the matrix under one temp root.
///
/// The matrix is deliberately focused rather than exhaustive: these seven are
/// the shapes whose guidance differs. Adding a state costs one directory and
/// one surface sweep, so extend it freely — the assertions are state-agnostic.
fn matrix(root: &Path) -> Vec<State> {
    let mut states = Vec::new();
    let mut add = |name: &'static str, build: &dyn Fn(&Path) -> (PathBuf, PathBuf)| {
        let dir = root.join(name);
        fs::create_dir_all(&dir).unwrap();
        let (home, proj) = build(&dir);
        states.push(State { name, home, proj });
    };

    // 1. Initialized, nothing declared.
    add("empty-manifest", &|d| write_project(d, "version = 1\n"));

    // 2. Capabilities declared, nothing pinned.
    add("declared-unpinned", &|d| write_project(d, FULL_MANIFEST));

    // 3. Pinned, never consented.
    add("pinned-untrusted", &|d| {
        let (home, proj) = write_project(d, FULL_MANIFEST);
        lock(&home, &proj);
        (home, proj)
    });

    // 4. Pinned and consented — the healthy shape.
    add("trusted-healthy", &|d| {
        let (home, proj) = write_project(d, FULL_MANIFEST);
        lock(&home, &proj);
        grant(&home, &proj);
        (home, proj)
    });

    // 5. Consented, then the pinned content moved underneath it. This is the
    //    state whose fix must WRITE, and the one convergence is asserted on.
    add("content-drifted", &|d| {
        let (home, proj) = write_project(d, FULL_MANIFEST);
        lock(&home, &proj);
        grant(&home, &proj);
        drift_content_and_surface(&proj);
        (home, proj)
    });

    // 6. The pure rendered lane: files only, no servers.
    add("rendered-only", &|d| {
        write_project(d, RENDERED_ONLY_MANIFEST)
    });

    // 7. Servers declared, no bridge connected anywhere.
    add("servers-no-bridge", &|d| {
        write_project(d, SERVERS_ONLY_MANIFEST)
    });

    // 8. FULLY HEALTHY, UNGROUPED — the `Rung::Group` state.
    //
    //    Everything above stops on a rung BELOW health: something is
    //    unpinned, unconsented, drifted or unrendered. So for seven states
    //    this guard never saw a single healthy-project string, and passed over
    //    the exact defect it was written to catch (a machine field naming
    //    `toolset create <name> --server <server>`). A described skill, a
    //    lock, a grant and a real `apply --write` are what it takes to get
    //    here; each one is a fixture line, and none of them is optional.
    add("healthy-ungrouped", &|d| {
        let (home, proj) = write_project_with_skill(d, FULL_MANIFEST, SKILL_DESCRIBED);
        lock(&home, &proj);
        grant(&home, &proj);
        apply(&home, &proj);
        (home, proj)
    });

    // 9. FULLY HEALTHY AND GROUPED — the `Rung::Verified` state, the top of
    //    the setup ladder, where the product has nothing left to ask for.
    //    "Nothing left to ask for" is its own guidance shape and its own way
    //    to be wrong.
    add("healthy-grouped", &|d| {
        let (home, proj) = write_project_with_skill(d, FULL_MANIFEST, SKILL_DESCRIBED);
        lock(&home, &proj);
        grant(&home, &proj);
        apply(&home, &proj);
        create_toolset(&home, &proj, "dev", "filesystem");
        // Naming a toolset re-locks, which makes the earlier consent stale —
        // so without this second grant the state falls back to "review it
        // again" and never stands on the verified rung at all. Re-granting is
        // the same non-interactive ceremony, not a shortcut around one.
        grant(&home, &proj);
        (home, proj)
    });

    // 10. INLINE SKILL, DECLARED BODY ABSENT FROM DISK.
    //
    //     The matrix had no missing-body state at all, and that hole is where
    //     the final defect of the whole exercise lived: `trust --preview`
    //     honoured `ContentDrift::fix = None` while `doctor` and `status`
    //     named `agentstack lock --write` over bytes that do not exist. Every
    //     rule needed to catch it was already written and live; no state ever
    //     produced the payload they judge. A guard whose matrix cannot reach
    //     the defect's state is green for the same reason the product was
    //     wrong.
    add("inline-body-missing", &|d| {
        let (home, proj) = write_project(d, FULL_MANIFEST);
        fs::remove_dir_all(proj.join(".agentstack/skills/summarize")).unwrap();
        (home, proj)
    });

    // 11. THE SAME CONDITION BEHIND AN EXIT-0 GREEN TICK.
    //
    //     One skill outside every toolset, its body missing, beside one that
    //     pins. `lock --write` succeeds — it pins everything it can find —
    //     and the blocking condition survives it untouched. So a driver that
    //     judges progress by exit status sees a success and re-polls into the
    //     identical field forever. The convergence check below therefore
    //     compares STATE, never exit codes.
    add("orphan-body-missing", &|d| {
        let (home, proj) =
            write_project_with_skill(d, ORPHAN_MISSING_BODY_MANIFEST, SKILL_DESCRIBED);
        let out = run(&["lock", "--write"], &home, &proj);
        assert!(
            out.ok,
            "fixture: `lock --write` is expected to exit 0 over a missing orphan body — that \
             exit-0 shape IS the state under test:\n{}",
            out.text
        );
        (home, proj)
    });

    states
}

// ---------------------------------------------------------------------------
// Harvest
// ---------------------------------------------------------------------------

/// Where a harvested command came from, and therefore how strictly it is
/// judged.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Audience {
    /// Prose a person reads. Placeholders are legitimate; a preview-only
    /// command is legitimate ("run `agentstack lock` to see what would pin").
    Human,
    /// A JSON field a caller executes verbatim. Placeholders are NOT
    /// legitimate, and if the finding it answers is blocking, it must write.
    Machine,
}

#[derive(Clone, Debug)]
struct Harvested {
    state: String,
    surface: String,
    /// JSON pointer, or the human line the command was lifted from.
    origin: String,
    audience: Audience,
    /// The command exactly as the product printed it, `agentstack` included.
    command: String,
    /// True when this string is offered as the answer to a finding that
    /// blocks the project (an error, or a non-ready readiness).
    blocking: bool,
}

/// Placeholder tokens that stand for a value the reader supplies.
fn is_placeholder(tok: &str) -> bool {
    let t = tok.trim_matches(|c| c == '[' || c == ']');
    (t.starts_with('<') && t.ends_with('>')) || (t.starts_with('{') && t.ends_with('}'))
}

/// Words that end a command and begin prose again. A remedy line is written
/// for a human ("agentstack adopt, then rerun doctor"), so the command has to
/// be cut out of the sentence around it.
const PROSE_STOPS: &[&str] = &[
    "—", "·", "then", "or", "and", "to", "so", "which", "if", "before", "after", "first",
];

/// Lift one command out of a fragment of prose that begins with `agentstack`.
///
/// Returns `None` when nothing survives the cut. Tokens are taken until a
/// prose stop word, a parenthesis, or a sentence-ending punctuation mark.
/// Where an `agentstack` INVOCATION begins in `fragment`, as opposed to the
/// same ten letters inside a longer word or a path.
///
/// This is precision, not leniency, and the distinction matters because it is
/// the only kind of change to this file that could quietly turn a guard off.
/// The product legitimately prints absolute paths in prose — a missing skill
/// body is reported as `… declares a body at /tmp/x/proj/.agentstack/skills/…`
/// — and a plain substring search lifted `agentstack/./skills/orphan that is
/// not present on disk` out of the middle of that sentence and demanded it
/// parse as a command. Nothing was ever suggested there. So the match is
/// anchored: the run of letters must start a token (start of fragment, or
/// preceded by a separator) and must be FOLLOWED by a space, since every real
/// invocation has at least a verb after it. A bare trailing `agentstack` is
/// the product's own name and was already discarded downstream.
fn invocation_start(fragment: &str) -> Option<usize> {
    let bytes = fragment.as_bytes();
    let mut from = 0usize;
    while let Some(rel) = fragment[from..].find("agentstack") {
        let at = from + rel;
        let before_ok = at == 0
            || matches!(
                bytes[at - 1],
                b' ' | b'\t' | b'`' | b'"' | b'\'' | b'(' | b'[' | b'{'
            );
        let after = at + "agentstack".len();
        let after_ok = bytes.get(after) == Some(&b' ');
        if before_ok && after_ok {
            return Some(at);
        }
        from = at + 1;
    }
    None
}

fn command_from_fragment(fragment: &str) -> Option<String> {
    let start = invocation_start(fragment)?;
    let mut tail = &fragment[start..];
    // Two or more spaces is a column separator, not a word gap: the `Next:`
    // line and doctor's `next:` tail both print `<command>   <why>` in
    // columns. Cut there, or the rationale gets parsed as arguments.
    if let Some(gap) = tail.find("  ") {
        tail = &tail[..gap];
    }
    let mut toks: Vec<String> = Vec::new();
    for raw in tail.split_whitespace() {
        let tok = raw.trim_matches(|c: char| c == '`' || c == '"' || c == '\'');
        if tok.is_empty() {
            continue;
        }
        if PROSE_STOPS.contains(&tok) || tok.starts_with('(') {
            break;
        }
        let ended = tok.ends_with(['.', ',', ';', ':']) && !tok.ends_with("..");
        let tok = tok.trim_end_matches(['.', ',', ';', ':']);
        if tok.is_empty() {
            break;
        }
        toks.push(tok.to_string());
        if ended {
            break;
        }
    }
    if toks.len() < 2 {
        // Bare "agentstack" is the product's own name in a sentence, not a
        // suggestion to run anything.
        return None;
    }
    Some(toks.join(" "))
}

/// Every backtick span and every `↳` fix column in a block of human output.
///
/// `↳` is the codebase-wide convention for "and here is what fixes it", so it
/// is harvested as a first-class origin rather than hoping the fix also
/// happened to be backticked.
fn harvest_human(text: &str) -> Vec<(String, String)> {
    let text = strip_ansi(text);
    let mut found = Vec::new();
    for line in text.lines() {
        // Fix columns. A single line can carry several, separated by `·`.
        if let Some((_, fixes)) = line.split_once("↳ ") {
            for chunk in fixes.split('·') {
                if let Some(cmd) = command_from_fragment(chunk) {
                    found.push((line.trim().to_string(), cmd));
                }
            }
        }
        // Backtick spans.
        let mut rest = line;
        while let Some(open) = rest.find('`') {
            let after = &rest[open + 1..];
            let Some(close) = after.find('`') else { break };
            let span = &after[..close];
            if span.trim_start().starts_with("agentstack") {
                if let Some(cmd) = command_from_fragment(span) {
                    found.push((line.trim().to_string(), cmd));
                }
            }
            rest = &after[close + 1..];
        }
        // The `Next:` / `next:` line prints its command unquoted.
        for marker in ["Next:", "next:"] {
            if let Some((_, tail)) = line.split_once(marker) {
                if let Some(cmd) = command_from_fragment(tail) {
                    found.push((line.trim().to_string(), cmd));
                }
            }
        }
    }
    found.sort();
    found.dedup();
    found
}

/// One string value found in a JSON body, with everything the classification
/// needs already decided by the walk.
struct JsonString {
    /// JSON pointer to the value.
    ptr: String,
    /// Is this field part of the machine-readable guidance contract — a field
    /// a driver executes verbatim?
    machine: bool,
    /// Machine-named, but overridden by a `next_action` twin in the same
    /// object: display prose that a UI renders and never execs. Carries the
    /// reason, for the coverage report.
    display_twin: bool,
    value: String,
}

/// Keys that name a piece of guidance. Matched by *shape*, not by a fixed
/// list: anything containing `next` (`next_action`, `next_step`, a future
/// `next_actions`) plus the two established remedy words. A guidance field
/// invented tomorrow under any of these spellings is covered with no edit.
fn is_guidance_key(k: &str) -> bool {
    k.contains("next") || k == "fix" || k == "remedy"
}

/// Inside a guidance object (`next_action: {command, step, why}`), these are
/// the keys that carry the command. `why` and its siblings are the rationale
/// printed beside it and are prose by contract — sweeping them in as machine
/// fields would make the strict rules below fire on correct English.
const COMMAND_CARRIERS: &[&str] = &["command", "step", "cmd", "run", "fix"];

/// Walk a JSON body and collect EVERY string, tagged with whether it belongs
/// to the machine-readable guidance contract.
///
/// Discovery by shape, not a hardcoded field list, in two directions:
///
/// * A key whose name is guidance-shaped ([`is_guidance_key`]) is a machine
///   field, and so is a command-carrying key nested directly inside one. A
///   field added tomorrow is covered on the day it lands.
/// * Nothing is dropped. The previous version only kept strings that already
///   *looked* like commands, so a machine field holding prose vanished with no
///   trace — which is precisely how a JSON driver was handed English and
///   looped forever. Every string now leaves this walk; the caller decides.
///
/// The one subtraction is explicit and reported: when an object carries a
/// `next_action` key, that key IS its machine contract, and any *other*
/// guidance-named sibling is the human sentence the same surface prints
/// (`doctor --json` documents `next_step` as "guidance for a UI that renders
/// text, never something to exec"). Those are tagged `display_twin` and land
/// in the coverage report as skips rather than being judged by the machine
/// rules — a subtraction that only ever applies where a real machine twin
/// exists to be judged in its place.
fn harvest_json_strings(
    v: &serde_json::Value,
    ptr: &str,
    machine: bool,
    display_twin: bool,
    out: &mut Vec<JsonString>,
) {
    match v {
        serde_json::Value::Object(map) => {
            let has_twin = map.contains_key("next_action");
            for (k, child) in map {
                let (child_machine, child_twin) = if machine {
                    // Already inside a guidance field: only the command
                    // carriers below it stay machine.
                    (COMMAND_CARRIERS.contains(&k.as_str()), display_twin)
                } else if is_guidance_key(k) {
                    // A display twin is NOT a machine field: it is reported as
                    // a skip instead, and its `next_action` sibling carries
                    // the contract this run asserts.
                    let twin = has_twin && k != "next_action";
                    (!twin, twin)
                } else {
                    (false, false)
                };
                harvest_json_strings(child, &format!("{ptr}/{k}"), child_machine, child_twin, out);
            }
        }
        serde_json::Value::Array(items) => {
            // Arrays inherit their key's classification: `next_steps: [...]`
            // is a list of machine fields, one per element.
            for (ix, child) in items.iter().enumerate() {
                harvest_json_strings(child, &format!("{ptr}/{ix}"), machine, display_twin, out);
            }
        }
        serde_json::Value::String(s) => out.push(JsonString {
            ptr: ptr.to_string(),
            machine,
            display_twin,
            value: s.clone(),
        }),
        _ => {}
    }
}

/// Is this string, in its entirety, an `agentstack …` invocation — the thing a
/// machine field promises to be?
fn is_whole_command(s: &str) -> bool {
    let t = s.trim();
    if !t.starts_with("agentstack ") {
        return false;
    }
    // Not "starts with a command": a machine field is executed verbatim, so
    // `agentstack apply --write to render your setup` is NOT a command — it is
    // a sentence with a command in it, and exec'ing it fails. EVERY token has
    // to be argv-shaped.
    let toks: Vec<&str> = t.split_whitespace().collect();
    toks.len() >= 2
        && toks.iter().all(|tok| {
            // `agentstack trust .` — a lone `.` is a path argument, not the
            // full stop that ends a sentence.
            let punctuated = *tok != "." && tok.ends_with(['.', ',', ';', ':']);
            !PROSE_STOPS.contains(tok) && !tok.starts_with('(') && !tok.contains('`') && !punctuated
        })
}

/// The surfaces swept in every state. `(label, argv, is_json)`.
///
/// Every guidance-producing, non-interactive, read-only surface the CLI has.
/// `trust --preview` is included because it is the review a panel renders
/// before asking for consent — and it emits guidance of its own.
const SURFACES: &[(&str, &[&str], bool)] = &[
    ("status", &["status"], false),
    ("status --json", &["status", "--json"], true),
    ("doctor", &["doctor"], false),
    ("doctor --json", &["doctor", "--json"], true),
    ("trust --preview", &["trust", "--preview"], true),
    ("delivery", &["delivery"], false),
    ("delivery --json", &["delivery", "--json"], true),
    // The bare invocation: the orientation screen someone sees by typing the
    // product's name, which has its own "Next:" line.
    ("(bare)", &[], false),
];

/// Is this state's report blocking — i.e. is the product saying something is
/// wrong that the reader must repair?
fn is_blocking(doctor_json: &serde_json::Value) -> bool {
    let errors = doctor_json["errors"].as_u64().unwrap_or(0);
    let readiness = doctor_json["readiness"].as_str().unwrap_or("");
    errors > 0 || !matches!(readiness, "ready" | "healthy" | "ok")
}

// ---------------------------------------------------------------------------
// The clap tree: what parses, and what writes
// ---------------------------------------------------------------------------

/// Sentinel substituted for a placeholder token before parsing. It must be a
/// plausible value for a name, a path, a query and a digest at once, which is
/// why it is a bare word rather than anything punctuated.
const SENTINEL: &str = "sentinel";

/// Does the clap node reached by `path` declare a `--write` flag?
///
/// Derived, not tabulated. "This command can write, and the caller did not ask
/// it to" is exactly the shape of the `lock` regression, and deriving it means
/// the next command that becomes preview-by-default is covered the day its
/// flag appears — nobody has to remember to update a list here.
fn takes_write_flag(cmd: &clap::Command, path: &[String]) -> bool {
    let mut node = cmd.clone();
    for seg in path {
        match node.find_subcommand(seg) {
            Some(sub) => node = sub.clone(),
            None => return false,
        }
    }
    // Bound to a local: the iterator borrows `node`, which is dropped at the
    // end of this block, so the answer has to be computed before that.
    let writes = node.get_arguments().any(|a| a.get_long() == Some("write"));
    writes
}

/// The subcommand path of a harvested command: the leading tokens that clap
/// resolves as subcommands, stopping at the first flag or free value.
fn subcommand_path(cmd: &clap::Command, tokens: &[String]) -> Vec<String> {
    let mut node = cmd.clone();
    let mut path = Vec::new();
    for tok in tokens {
        if tok.starts_with('-') {
            break;
        }
        match node.find_subcommand(tok) {
            Some(sub) => {
                path.push(tok.clone());
                node = sub.clone();
            }
            None => break,
        }
    }
    path
}

fn tokens_of(command: &str) -> Vec<String> {
    command.split_whitespace().map(str::to_string).collect()
}

/// Normalize a harvested command the way the BINARY does before clap sees it.
///
/// `agentstack x <cmd> …` is not a nested clap subcommand: `main` strips the
/// leading `x` from argv and there is exactly one parse tree and one dispatch
/// arm. So a guidance string naming the namespaced spelling would fail rule (a)
/// — "unrecognized subcommand 'x'" — while running perfectly in a terminal,
/// and the guard would be reporting an artefact of its own shortcut.
///
/// This is not a relaxation, and the distinction is the whole point: the
/// product's OWN [`agentstack::cli::strip_namespace`] is called here rather
/// than a copy of its rule, so the guard cannot drift from the binary. What the
/// stripping does NOT do is excuse anything: `agentstack x nonexistent` still
/// fails to parse (pinned by `the_guard_still_rejects_a_bad_namespaced_command`),
/// and that the two spellings really reach the same place is asserted
/// separately, against the real binary, by `the_x_namespace_is_a_pure_alias`.
fn as_argv(tokens: &[String]) -> Vec<String> {
    agentstack::cli::strip_namespace(tokens).unwrap_or_else(|| tokens.to_vec())
}

// ---------------------------------------------------------------------------
// (e) Discoverability: what a person can find from `agentstack --help`
// ---------------------------------------------------------------------------

/// A bare command name, as it appears in a help listing.
fn is_ident(s: &str) -> bool {
    !s.is_empty()
        && s.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The text after the last run of two-or-more spaces — the value column of a
/// help line like `  Set up      up · adapters · settings`.
fn value_column(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut last = 0usize;
    let mut i = 0usize;
    while i + 1 < bytes.len() {
        if bytes[i] == b' ' && bytes[i + 1] == b' ' {
            let mut j = i;
            while j < bytes.len() && bytes[j] == b' ' {
                j += 1;
            }
            last = j;
            i = j;
        } else {
            i += 1;
        }
    }
    line[last..].trim()
}

/// Command names listed on one `a · b · c` help line.
///
/// Deliberately strict, because a LOOSE reading here would WIDEN the
/// discoverable set and quietly weaken rule (e) — the one direction in which a
/// parsing bug in this file turns an assertion off. A line carrying a bracket,
/// a colon or a backtick is prose (`Setup (what you have) · Toolset (…)`) and
/// is refused outright; every remaining chunk must be a bare identifier; and
/// the caller keeps only the names the clap tree actually resolves, so a word
/// lifted out of a sentence can never enter the set.
fn dot_list_idents(line: &str) -> Vec<String> {
    if line.contains(['(', ')', ':', '`', ',']) {
        return Vec::new();
    }
    let body = value_column(line);
    if body.is_empty() {
        return Vec::new();
    }
    let chunks: Vec<&str> = body.split('·').map(str::trim).collect();
    if chunks.iter().all(|c| is_ident(c)) {
        chunks.iter().map(|c| (*c).to_string()).collect()
    } else {
        Vec::new()
    }
}

/// The commands clap lists under `Commands:` on a help screen.
fn listed_commands(help: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut inside = false;
    for line in help.lines() {
        if line.trim_end() == "Commands:" {
            inside = true;
            continue;
        }
        if inside {
            if line.trim().is_empty() {
                break;
            }
            // Two-space indent is a command; four is one of its subcommands.
            if !line.starts_with("  ") || line.starts_with("    ") {
                continue;
            }
            if let Some(first) = line.split_whitespace().next() {
                if is_ident(first) && first != "help" {
                    out.insert(first.to_string());
                }
            }
        }
    }
    out
}

/// Every top-level command named anywhere in `--help --all`, including the
/// hidden ones and the t3code fixed actions.
fn all_help_commands(help_all: &str, tree: &clap::Command) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in help_all.lines() {
        if !line.starts_with("  ") || line.starts_with("    ") {
            continue;
        }
        if let Some(first) = line.split_whitespace().next() {
            if is_ident(first) && first != "help" && tree.find_subcommand(first).is_some() {
                out.insert(first.to_string());
            }
        }
        for name in dot_list_idents(line) {
            if tree.find_subcommand(&name).is_some() {
                out.insert(name);
            }
        }
    }
    out
}

/// What a person can find, and how — derived from the real binary every run.
struct Discoverable {
    /// Listed by clap under `Commands:` on plain `--help`.
    visible: BTreeSet<String>,
    /// Named on the plain `--help` screen as reachable both ways.
    named_on_help: BTreeSet<String>,
    /// Listed in the grouped `agentstack x` toolbox.
    under_x: BTreeSet<String>,
    /// Every top-level command `--help --all` names.
    in_help_all: BTreeSet<String>,
}

impl Discoverable {
    /// The union: everything reachable from `--help` directly or in one step.
    fn reachable(&self) -> BTreeSet<String> {
        let mut u = self.visible.clone();
        u.extend(self.named_on_help.iter().cloned());
        u.extend(self.under_x.iter().cloned());
        u
    }

    fn how(&self, name: &str) -> &'static str {
        if self.visible.contains(name) {
            "listed on `agentstack --help`"
        } else if self.named_on_help.contains(name) {
            "named on the `agentstack --help` screen"
        } else if self.under_x.contains(name) {
            "listed under `agentstack x`"
        } else {
            "NOT DISCOVERABLE"
        }
    }
}

/// Spawn the real binary's three help screens and read the sets out of them.
///
/// Nothing here is a list this file maintains. The visible set is whatever the
/// binary prints today, so promoting or hiding a command changes what rule (e)
/// enforces on the same commit that changes the product — which is the only way
/// a discoverability rule can stay honest.
fn discoverable(home: &Path, proj: &Path, tree: &clap::Command) -> Discoverable {
    let help = strip_ansi(&run(&["--help"], home, proj).text);
    let x = strip_ansi(&run(&["x"], home, proj).text);
    let all = strip_ansi(&run(&["--help", "--all"], home, proj).text);

    let visible = listed_commands(&help);
    assert!(
        !visible.is_empty(),
        "read no commands at all out of `agentstack --help` — the extraction broke, not the \
         product:\n{help}"
    );

    let mut named_on_help = BTreeSet::new();
    for line in help.lines() {
        for name in dot_list_idents(line) {
            if tree.find_subcommand(&name).is_some() {
                named_on_help.insert(name);
            }
        }
    }

    let mut under_x = BTreeSet::new();
    for line in x.lines() {
        for name in dot_list_idents(line) {
            if tree.find_subcommand(&name).is_some() {
                under_x.insert(name);
            }
        }
    }
    assert!(
        !under_x.is_empty(),
        "read no commands at all out of `agentstack x` — the extraction broke, not the \
         product:\n{x}"
    );

    Discoverable {
        visible,
        named_on_help,
        under_x,
        in_help_all: all_help_commands(&all, tree),
    }
}

/// Rule (e), as one function so the guard can be pointed at itself (see
/// `the_guard_catches_an_undiscoverable_command`).
///
/// `None` means the command's verb can be found from `agentstack --help`.
fn discoverability_violation(h: &Harvested, reachable: &BTreeSet<String>) -> Option<String> {
    // `agentstack x install` is discoverable BY CONSTRUCTION — naming the
    // namespace is one of the two ways this rule accepts. Judge what follows.
    let toks = as_argv(&tokens_of(&h.command));
    let verb = toks.get(1)?;
    if verb.starts_with('-') || !is_ident(verb) || reachable.contains(verb) {
        return None;
    }
    // Name the whole leading command path, not just the verb: `secret set` is
    // what the fix column printed and what the reader would have to find.
    let path: Vec<&str> = toks[1..]
        .iter()
        .take_while(|t| is_ident(t))
        .map(String::as_str)
        .collect();
    let path = path.join(" ");
    Some(format!(
        "\n  `agentstack {path}` is named by {} ({}) in state `{}` but is not discoverable from \
         `agentstack --help` — list it, or name it as `agentstack x {path}`.\n  \
         why     : a surface may never name a command a reader cannot find. Hiding a command does \
         not stop it PARSING, so rule (a) cannot see this; the reader is simply told to run \
         something that appears on no help screen they were pointed at.",
        h.surface, h.origin, h.state,
    ))
}

// ---------------------------------------------------------------------------
// Reporting
// ---------------------------------------------------------------------------

/// Coverage the run did NOT achieve, printed on success.
///
/// A guard that narrows itself in silence reads as "everything is covered".
/// Anything skipped, allow-listed or unparsed lands here and is printed.
#[derive(Default)]
struct Skips(Vec<String>);

impl Skips {
    fn note(&mut self, what: String) {
        self.0.push(what);
    }
}

// ---------------------------------------------------------------------------
// The guard
// ---------------------------------------------------------------------------

#[test]
fn every_suggested_command_parses_and_makes_progress() {
    let tmp = tempfile::tempdir().unwrap();
    let states = matrix(tmp.path());
    let clap_tree = agentstack::cli::Cli::command();
    // What a reader can find, read off the real binary's help screens. Derived
    // once; used by rule (e) below and printed in the coverage report.
    let help_home = tmp.path().join("help-home");
    let help_proj = tmp.path().join("help-proj");
    fs::create_dir_all(&help_home).unwrap();
    fs::create_dir_all(&help_proj).unwrap();
    let disco = discoverable(&help_home, &help_proj, &clap_tree);

    let mut skips = Skips::default();
    // Seeded, not discovered. The structural limit documented in this file's
    // header is real on EVERY run, so it belongs in the record on every run —
    // otherwise the report can print "not covered: nothing" while a known
    // blind spot stands, which is the precise failure the comment above
    // `report` warns about. A guard may only claim the coverage it can show.
    skips.note(
        "`Rung::Group` on `status` — `run` spawns with `env_clear` and \
         PATH=/usr/bin:/bin, so no harness is ever detected and the \
         register-the-bridge branch precedes the ladder in every state this \
         file can build. Asserted directly instead by \
         `the_harvest_tags_a_nested_command_carrier_as_a_machine_field`."
            .to_string(),
    );
    let mut harvested: Vec<Harvested> = Vec::new();
    let mut non_command_machine_fields: Vec<String> = Vec::new();
    let mut per_state_surfaces: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    // Every JSON body this run parsed, kept so the cross-surface agreement
    // rule costs no extra spawns.
    let mut payloads: BTreeMap<(&str, &str), serde_json::Value> = BTreeMap::new();

    for state in &states {
        // The blocking reading comes from doctor --json, the surface whose job
        // is to say whether anything is wrong.
        let doctor = run(&["doctor", "--json"], &state.home, &state.proj);
        let doctor_json: serde_json::Value = serde_json::from_str(&strip_ansi(&doctor.text))
            .unwrap_or_else(|e| {
                panic!(
                    "[{}] doctor --json is not JSON ({e}):\n{}",
                    state.name, doctor.text
                )
            });
        let blocking = is_blocking(&doctor_json);

        for (label, argv, is_json) in SURFACES {
            let out = run(argv, &state.home, &state.proj);
            let text = strip_ansi(&out.text);
            per_state_surfaces
                .entry(state.name)
                .or_default()
                .push((*label).to_string());

            if *is_json {
                match serde_json::from_str::<serde_json::Value>(&text) {
                    Ok(v) => {
                        payloads.insert((state.name, *label), v.clone());
                        let mut found = Vec::new();
                        harvest_json_strings(&v, "", false, false, &mut found);
                        for s in found {
                            let at = format!("[{}] {label} {}", state.name, s.ptr);
                            if s.machine {
                                if is_whole_command(&s.value) {
                                    harvested.push(Harvested {
                                        state: state.name.to_string(),
                                        surface: (*label).to_string(),
                                        origin: s.ptr,
                                        audience: Audience::Machine,
                                        command: s.value.trim().to_string(),
                                        blocking,
                                    });
                                } else if let Some(msg) =
                                    non_command_violation(state.name, label, &s.ptr, &s.value)
                                {
                                    // Defect (a), made loud. A machine field
                                    // holding anything but a command used to
                                    // vanish here without a trace.
                                    non_command_machine_fields.push(msg);
                                }
                            } else if s.display_twin {
                                skips.note(format!(
                                    "{at}: display prose, not judged as a machine field — its \
                                     object carries a `next_action` twin, which IS asserted; \
                                     value {:?}",
                                    s.value
                                ));
                            } else {
                                // An ordinary prose field. Judged by the human
                                // rules, exactly as the terminal text is.
                                for (_, command) in harvest_human(&s.value) {
                                    harvested.push(Harvested {
                                        state: state.name.to_string(),
                                        surface: (*label).to_string(),
                                        origin: s.ptr.clone(),
                                        audience: Audience::Human,
                                        command,
                                        blocking,
                                    });
                                }
                            }
                        }
                    }
                    Err(e) => {
                        // Not a failure by itself: a surface may legitimately
                        // refuse in a given state. But it IS lost coverage, so
                        // it is printed rather than swallowed.
                        skips.note(format!(
                            "[{}] {label}: no JSON to harvest ({e}); exit_ok={}",
                            state.name, out.ok
                        ));
                    }
                }
            } else {
                for (line, command) in harvest_human(&text) {
                    harvested.push(Harvested {
                        state: state.name.to_string(),
                        surface: (*label).to_string(),
                        origin: line,
                        audience: Audience::Human,
                        command,
                        blocking,
                    });
                }
            }
        }
    }

    assert!(
        !harvested.is_empty(),
        "harvested nothing at all — the extraction broke, not the product"
    );

    // ── (a0) A MACHINE FIELD CARRIES A COMMAND, OR NOTHING ────────────────
    //
    // Asserted FIRST, because everything below it assumes the field holds a
    // command at all. A machine field holding prose ("review the errors
    // above", "add `description:` so agents can find it") is the same dead end
    // as a preview offered in place of a write: the caller executes it, fails,
    // re-polls, and reads the identical string. `null` is the honest answer
    // when there is nothing to run.
    assert!(
        non_command_machine_fields.is_empty(),
        "{} machine-readable field(s) do not carry a runnable command:{}",
        non_command_machine_fields.len(),
        non_command_machine_fields.join("\n")
    );

    // ── (a) PARSES ─────────────────────────────────────────────────────────
    //
    // Placeholders are substituted with a sentinel first: `<query>` is a
    // legitimate thing to print AT A HUMAN, and level (a) is only asking
    // "does this verb, with these flags, exist?".
    let mut failures: Vec<String> = Vec::new();
    for h in &harvested {
        let mut argv: Vec<String> = vec!["agentstack".to_string()];
        let mut had_placeholder = false;
        for tok in tokens_of(&h.command).into_iter().skip(1) {
            if is_placeholder(&tok) {
                had_placeholder = true;
                argv.push(SENTINEL.to_string());
            } else {
                argv.push(tok);
            }
        }
        // Normalized exactly as the binary normalizes it — see `as_argv`.
        let argv = as_argv(&argv);
        if let Err(e) = agentstack::cli::Cli::try_parse_from(&argv) {
            failures.push(format!(
                "\n  state    : {}\n  surface  : {}\n  origin   : {}\n  command  : `{}`\n  why      : does not parse against the clap tree{}\n  clap says: {}",
                h.state,
                h.surface,
                h.origin,
                h.command,
                if had_placeholder {
                    " (placeholders were replaced with `sentinel` first, so this is not a placeholder problem)"
                } else {
                    ""
                },
                e.to_string().lines().next().unwrap_or("").trim(),
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "AgentStack suggested {} command(s) that cannot be run:{}\n\n\
         Every string the product offers is a promise that running it is possible.",
        failures.len(),
        failures.join("\n")
    );

    // ── (a') PLACEHOLDERS ARE HUMAN-ONLY ──────────────────────────────────
    //
    // A person reads `agentstack search <query>` and knows to substitute. A
    // machine field is executed verbatim, so a placeholder there is a command
    // that cannot run.
    let mut placeholder_failures: Vec<String> = Vec::new();
    for h in harvested.iter().filter(|h| h.audience == Audience::Machine) {
        if let Some(msg) = placeholder_violation(h) {
            placeholder_failures.push(msg);
        }
    }
    assert!(
        placeholder_failures.is_empty(),
        "{} machine-readable fix field(s) carry a placeholder:{}",
        placeholder_failures.len(),
        placeholder_failures.join("\n")
    );

    // ── (b) NOT A NO-OP WHERE A WRITE IS REQUIRED ─────────────────────────
    //
    // THE ASSERTION THAT WOULD HAVE CAUGHT THE `lock` REGRESSION.
    //
    // If a command is offered as the machine-readable fix for a blocking
    // finding, and its own clap node declares `--write`, then the suggestion
    // must carry `--write`. Without it the caller runs a preview, sees a
    // plan, changes nothing, and is handed the identical finding on the next
    // poll — a dead end that reads like progress.
    //
    // The write-capable set is derived from the clap tree (see
    // `takes_write_flag`), which is why nothing here needs updating when a
    // command's write semantics change: the flag IS the semantics. Commands
    // whose repair is genuinely read-only have no `--write` node and are not
    // considered.
    let mut noop_failures: Vec<String> = Vec::new();
    for h in harvested
        .iter()
        .filter(|h| h.audience == Audience::Machine && h.blocking)
    {
        if let Some(msg) = noop_violation(&clap_tree, h) {
            noop_failures.push(msg);
        }
    }
    assert!(
        noop_failures.is_empty(),
        "{} machine-readable fix(es) for blocking findings are no-ops:{}\n\n\
         This is the exact shape of the `agentstack lock` regression: a command that used to \
         write became preview-by-default, and ~100 guidance strings kept naming the preview form.",
        noop_failures.len(),
        noop_failures.join("\n")
    );

    // ── (e) IS DISCOVERABLE ───────────────────────────────────────────────
    //
    // THE RULE THE COMMAND-VISIBILITY SPLIT COULD BREAK.
    //
    // Levels (a0)–(b) judge whether a named command RUNS. None of them can see
    // whether a reader could ever have FOUND it: a hidden command parses
    // exactly as well as a listed one, so a guard built on the clap tree alone
    // passes straight through "the fix column names `agentstack secret set`,
    // and `agentstack --help` does not mention `secret`".
    //
    // The discoverable set is read off the real binary's own help screens (see
    // `discoverable`), never written down here. Promote or hide a command and
    // this rule changes with it on the same commit.
    let reach = disco.reachable();
    let mut undiscoverable: Vec<String> = Vec::new();
    for h in &harvested {
        let toks = as_argv(&tokens_of(&h.command));
        match toks.get(1) {
            Some(verb) if verb.starts_with('-') || !is_ident(verb) => skips.note(format!(
                "[{}] {} {}: rule (e) did not judge `{}` — its first argument ({verb:?}) is not a \
                 command name, so there is no verb to look for on a help screen.",
                h.state, h.surface, h.origin, h.command
            )),
            Some(_) => {
                if let Some(msg) = discoverability_violation(h, &reach) {
                    undiscoverable.push(msg);
                }
            }
            None => skips.note(format!(
                "[{}] {} {}: rule (e) did not judge `{}` — it carries no verb at all.",
                h.state, h.surface, h.origin, h.command
            )),
        }
    }
    assert!(
        undiscoverable.is_empty(),
        "{} named command(s) cannot be found from `agentstack --help`:{}\n\n\
         Discoverable today: {} listed on --help, {} named on the --help screen, {} under \
         `agentstack x`.\n\
         Naming a command a reader cannot find is the defect the command-visibility split exists \
         to REMOVE, not to spread.",
        undiscoverable.len(),
        undiscoverable.join("\n"),
        disco.visible.len(),
        disco.named_on_help.len(),
        disco.under_x.len(),
    );

    // ── (d) THE THREE MACHINE SURFACES AGREE ──────────────────────────────
    //
    // THE GENERAL FORM OF THE LAST TWO ROUNDS OF DEFECTS.
    //
    // Levels (a0)–(b) judge each string on its own. They cannot see the shape
    // that actually escaped twice: one project, three surfaces, and two
    // different answers. `trust --preview` correctly emitted `fix: null` over
    // a declared body absent from disk; `doctor --json` and `status --json`
    // named `agentstack lock --write` at the same moment. Each string passed
    // every per-string rule — `lock --write` parses, writes, and carries no
    // placeholder — and the contradiction between the surfaces was the whole
    // defect.
    //
    // See `agreement_violations` for the three rules and why each is stated
    // the way it is.
    let mut disagreements: Vec<String> = Vec::new();
    for state in &states {
        disagreements.extend(agreement_violations(
            state.name,
            payloads.get(&(state.name, "doctor --json")),
            payloads.get(&(state.name, "status --json")),
            payloads.get(&(state.name, "trust --preview")),
            &mut skips,
        ));
    }
    assert!(
        disagreements.is_empty(),
        "{} cross-surface disagreement(s) — two surfaces describing ONE project handed a driver \
         different answers:{}\n\n\
         Whichever surface a driver happens to poll decides whether it converges or loops. That \
         is not a guidance bug in one string; it is the product contradicting itself.",
        disagreements.len(),
        disagreements.join("\n")
    );

    // ── (c) CONVERGES — NOW FOR EVERY STATE THAT NAMES A COMMAND ──────────
    //
    // Level (c) used to run for the two drift shapes and `declared-unpinned`
    // only — precisely the states that already worked. So the guard written to
    // prevent the loop class never executed a machine field in the states
    // where the class kept reappearing.
    //
    // It now runs for every state in the matrix whose `doctor --json`
    // `next_action` names a command. Detection is BY STATE, never by exit
    // code: `orphan-body-missing` is the state where the offered command exits
    // 0, prints a green tick, and leaves the blocking condition exactly where
    // it was.
    //
    // This pass MUTATES the matrix, so it runs last.
    let mut loops: Vec<String> = Vec::new();
    for state in &states {
        if let Some(msg) = converge_once(state, &mut skips) {
            loops.push(msg);
        }
    }
    assert!(
        loops.is_empty(),
        "{} state(s) hand a driver a command that does not move them:{}",
        loops.len(),
        loops.join("\n")
    );

    report(&states, &per_state_surfaces, &harvested, &skips, &disco);
}

// ---------------------------------------------------------------------------
// (e) Non-regression: the two help screens may not disagree, and the namespace
//     must be a pure alias
// ---------------------------------------------------------------------------

/// A hidden command is still a command.
///
/// `--help --all` is the complete map, and the visibility split moved commands
/// off the everyday screen rather than removing them. So every name that screen
/// prints must still resolve and still answer its own `--help`. If one does
/// not, the map names something that no longer exists — the same defect as an
/// unrunnable fix line, one level up.
#[test]
fn every_command_in_help_all_is_runnable() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let proj = tmp.path().join("proj");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&proj).unwrap();
    let tree = agentstack::cli::Cli::command();
    let disco = discoverable(&home, &proj, &tree);
    assert!(
        disco.in_help_all.len() >= disco.visible.len(),
        "`--help --all` listed fewer commands ({}) than plain `--help` ({}) — the extraction \
         broke, not the product",
        disco.in_help_all.len(),
        disco.visible.len()
    );

    let mut broken: Vec<String> = Vec::new();
    for name in &disco.in_help_all {
        let out = run(&[name, "--help"], &home, &proj);
        if !out.ok {
            broken.push(format!(
                "\n  `agentstack {name}` is listed by `agentstack --help --all`, but \
                 `agentstack {name} --help` failed:\n{}",
                out.text.trim()
            ));
        }
    }
    assert!(
        broken.is_empty(),
        "{} command(s) named on the complete map are not runnable:{}\n\n\
         Hiding a command from the everyday screen must never remove it.",
        broken.len(),
        broken.join("\n")
    );
    println!(
        "`--help --all` names {} runnable top-level command(s)",
        disco.in_help_all.len()
    );
}

/// The two lists cannot disagree.
///
/// Everything on the everyday screen must also be on the complete map, and
/// everything the `x` toolbox lists must be a real command. Two help screens
/// that disagree about what exists send a reader to the wrong one.
#[test]
fn the_visible_set_and_the_complete_map_agree() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let proj = tmp.path().join("proj");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&proj).unwrap();
    let tree = agentstack::cli::Cli::command();
    let disco = discoverable(&home, &proj, &tree);

    let missing: Vec<&String> = disco
        .visible
        .iter()
        .filter(|c| !disco.in_help_all.contains(*c))
        .collect();
    assert!(
        missing.is_empty(),
        "{missing:?} are listed on `agentstack --help` but absent from `agentstack --help --all`. \
         The complete map must be a superset of the everyday screen."
    );

    // Everything named as reachable, from either screen, must resolve.
    for name in disco.reachable() {
        assert!(
            tree.find_subcommand(&name).is_some(),
            "`{name}` is named on a help screen but is not a command"
        );
    }
    println!(
        "visible {} ⊆ complete map {} ; `agentstack x` lists {} ; --help also names {}",
        disco.visible.len(),
        disco.in_help_all.len(),
        disco.under_x.len(),
        disco.named_on_help.len()
    );
}

/// `agentstack x <cmd>` is the SAME command as `agentstack <cmd>`.
///
/// The namespace is a way to find a command, not a second one. If the two
/// spellings ever diverged — different flags, different help, a different
/// dispatch arm — then rule (e) would be pointing readers at something that is
/// not what they were told to run, and every "reachable in one step" claim on
/// the help screen would be false.
#[test]
fn the_x_namespace_is_a_pure_alias() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let proj = tmp.path().join("proj");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&proj).unwrap();
    let tree = agentstack::cli::Cli::command();
    let disco = discoverable(&home, &proj, &tree);

    let mut divergent: Vec<String> = Vec::new();
    for name in &disco.under_x {
        let direct = run(&[name, "--help"], &home, &proj);
        let namespaced = run(&["x", name, "--help"], &home, &proj);
        if direct.ok != namespaced.ok || strip_ansi(&direct.text) != strip_ansi(&namespaced.text) {
            divergent.push(format!(
                "\n  `agentstack x {name}` and `agentstack {name}` do not reach the same \
                 place.\n  direct     (ok={}):\n{}\n  namespaced (ok={}):\n{}",
                direct.ok,
                direct.text.trim(),
                namespaced.ok,
                namespaced.text.trim()
            ));
        }
    }
    assert!(
        divergent.is_empty(),
        "{} namespaced command(s) diverge from their own name:{}\n\n\
         `agentstack x <cmd>` is advertised as the same command reached one hop away. If it is \
         not, the help screen is lying and rule (e)'s one-step claim is void.",
        divergent.len(),
        divergent.join("\n")
    );
    println!(
        "`agentstack x <cmd>` == `agentstack <cmd>` for all {} namespaced command(s)",
        disco.under_x.len()
    );
}

/// Rule (e), pointed at the defect it exists to prevent — two-sided.
///
/// The bad case is built by REMOVING a command from the live, derived
/// discoverable set rather than by editing the product: that is exactly what
/// hiding `secret` from `--help` would do to this rule's input, and it keeps
/// the demonstration honest without a source edit that a concurrent change
/// could race. The corrected case then uses the set as the binary really
/// prints it, so the rule cannot rot into one that fires on everything.
#[test]
fn the_guard_catches_an_undiscoverable_command() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let proj = tmp.path().join("proj");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&proj).unwrap();
    let tree = agentstack::cli::Cli::command();
    let disco = discoverable(&home, &proj, &tree);
    let live = disco.reachable();

    // `agentstack secret set <name>` is printed by doctor's fix column at five
    // sites. It is discoverable today; it was NOT before this split, and that
    // is the historical defect.
    let h = Harvested {
        state: "declared-unpinned".into(),
        surface: "doctor".into(),
        origin: "↳ agentstack secret set OPENAI_API_KEY".into(),
        audience: Audience::Human,
        command: "agentstack secret set OPENAI_API_KEY".into(),
        blocking: true,
    };
    assert!(
        live.contains("secret"),
        "fixture: `secret` must be discoverable today, or this two-sided check has no positive \
         side left. Reachable set: {live:?}"
    );
    assert!(
        discoverability_violation(&h, &live).is_none(),
        "`agentstack secret set` is discoverable from the real `--help` today and must pass"
    );

    // Now hide it, exactly as the historical state did.
    let mut hidden = live.clone();
    hidden.remove("secret");
    let msg = discoverability_violation(&h, &hidden)
        .expect("the guard must flag a command guidance names that no help screen lists");
    assert!(
        msg.contains("agentstack secret set")
            && msg.contains("doctor")
            && msg.contains("not discoverable from `agentstack --help`")
            && msg.contains("agentstack x secret set"),
        "the failure must name the command, the surface that emitted it, and what is missing:\n{msg}"
    );
    println!("guard self-check — a named but undiscoverable command:{msg}");

    // …and a command reached one hop away under `agentstack x` passes, or the
    // rule would demand every verb be promoted to the everyday screen.
    let namespaced = disco
        .under_x
        .iter()
        .find(|c| !disco.visible.contains(*c))
        .expect("the `x` toolbox must list something the everyday screen does not")
        .clone();
    let one_hop = Harvested {
        command: format!("agentstack {namespaced} --help"),
        ..h
    };
    assert!(
        discoverability_violation(&one_hop, &live).is_none(),
        "`{namespaced}` is listed under `agentstack x`, which IS discoverable in one step; \
         flagging it would force every verb onto the everyday screen and undo the split"
    );
}

/// The namespace normalization is not a hole in rule (a).
///
/// `as_argv` removes a leading `x` before clap parses, because the binary does.
/// That is the one edit in this round that could switch an assertion off by
/// accident, so both directions are pinned: the namespaced spelling of a REAL
/// command must parse, and the namespaced spelling of a command that does not
/// exist — or a misspelled flag behind the namespace — must still fail.
#[test]
fn the_guard_still_rejects_a_bad_namespaced_command() {
    let parses = |cmd: &str| {
        let argv = as_argv(&tokens_of(cmd));
        agentstack::cli::Cli::try_parse_from(&argv).is_ok()
    };
    assert!(
        parses("agentstack x install"),
        "the namespaced spelling of a real command must parse, or every guidance string that \
         names `agentstack x <cmd>` fails rule (a) for a reason that is about this file rather \
         than about the product"
    );
    for bad in [
        "agentstack x nonexistent-verb",
        "agentstack x lock --no-such-flag",
        "agentstack x x lock",
    ] {
        assert!(
            !parses(bad),
            "`{bad}` must still be rejected — stripping the namespace normalizes ONE leading `x`, \
             it does not excuse what follows"
        );
    }
    // …and only the FIRST argument is a namespace. `agentstack apply x` is an
    // argument to `apply`, and normalizing it away would hide a real defect.
    let toks = tokens_of("agentstack apply x");
    assert_eq!(
        as_argv(&toks),
        toks,
        "a later `x` is an argument, not the namespace"
    );
}

/// The derivation itself, pinned two-sided.
///
/// Rule (e) is only as strong as what [`dot_list_idents`] refuses to read. A
/// loose reading WIDENS the discoverable set, which is the one way a change to
/// this file could switch the rule off without touching an assertion — so the
/// prose the help screens really print is pinned as unreadable, and the listing
/// lines as readable.
#[test]
fn the_help_derivation_reads_listings_and_not_prose() {
    for prose in [
        "Four ideas cover the whole product: Setup (what you have) · Toolset (what this",
        "task needs) · Status (is it ready) · Undo (how to take it back).",
        "  agentstack x                   the rest of the toolbox",
        "  agentstack --help --all",
    ] {
        assert_eq!(
            dot_list_idents(prose),
            Vec::<String>::new(),
            "prose is not a command listing: {prose:?}"
        );
    }
    assert_eq!(
        dot_list_idents("  gateway · guard · install · instructions · lib · self · session · up"),
        vec![
            "gateway",
            "guard",
            "install",
            "instructions",
            "lib",
            "self",
            "session",
            "up"
        ],
        "a `·`-separated listing must be read, or rule (e) would flag the very commands such a \
         line exists to make findable. This shape is what `agentstack x` prints today; the plain \
         `--help` screen no longer carries a second copy of it."
    );
    assert_eq!(
        dot_list_idents("  Set up      up · adapters · settings · self · completions"),
        vec!["up", "adapters", "settings", "self", "completions"],
        "a grouped `agentstack x` line must be read"
    );
    assert_eq!(
        dot_list_idents("  Undo        restore"),
        vec!["restore"],
        "a one-command group is still a listing"
    );

    let clap_like = "Commands:\n  init     Setup: find the CLIs\n  status   Status: where it stands\n  help     Print this message\n\nOptions:\n  -h\n";
    let listed = listed_commands(clap_like);
    assert!(
        listed.contains("init") && listed.contains("status") && !listed.contains("help"),
        "the `Commands:` block is the visible set, minus clap's own `help`: {listed:?}"
    );
}

// ---------------------------------------------------------------------------
// (d) Cross-surface agreement
// ---------------------------------------------------------------------------

/// The commands that answer the PIN/TRUST rung — the one rung all three
/// surfaces speak about, and therefore the one where silence on one surface
/// and a command on another is a contradiction rather than a difference of
/// subject.
///
/// `trust --preview` is not a general guidance surface: it says nothing about
/// rendering, activation or grouping, so `doctor` naming `apply --write` while
/// the preview is silent is two surfaces answering two questions. Restricting
/// rule 2 and rule 3 to this set is what keeps the assertion a contradiction
/// detector instead of a noise generator.
const TRUST_RUNG_COMMANDS: &[&str] = &["agentstack lock --write", "agentstack trust ."];

/// Read a surface's machine field. `Ok(None)` is the honest "nothing to run".
fn machine_field<'a>(body: Option<&'a serde_json::Value>, ptr: &str) -> Option<&'a str> {
    body?.pointer(ptr)?.as_str()
}

/// The three rules, and every one of them derived from a defect that shipped.
///
/// 1. **`doctor` and `status` never name two different trust-rung commands.**
///    Deliberately not "are always equal". Strict equality was written first
///    and measured against the live matrix, where it flagged three states in
///    which the two surfaces are on genuinely different rungs — `status` ranks
///    a pending intake first for a person looking at a project, `doctor` ranks
///    the consent gate first for a person asking what is wrong. Neither is
///    wrong and neither contradicts the other, so an equality assertion would
///    have demanded the product change correct behaviour to satisfy the guard.
///    What IS a contradiction is both surfaces answering the SAME rung with
///    DIFFERENT commands, and that is what this clause says. Every difference
///    it does not judge is written to the coverage report, so the narrowing is
///    on the record rather than in a comment.
/// 2. **A terminal trust rung is terminal everywhere.** When the preview
///    reports blockers and NOT ONE of them carries a repairing command, no
///    other surface may name a trust-rung command for the same project. This
///    is the defect of the final round, stated generally: `fix: null` beside
///    `next_action: "agentstack lock --write"` is one project with two answers,
///    and the loud one loops.
/// 3. **When the preview does name a fix, the others must name the same one.**
///    Two surfaces agreeing that the trust rung is actionable but disagreeing
///    about which command acts on it is the same contradiction with the
///    polarity flipped, and it is not otherwise detectable: both strings parse,
///    both write, neither carries a placeholder.
///
/// States where a surface produced no JSON at all are recorded as skips rather
/// than passed over, so the coverage report cannot imply this rule ran
/// everywhere when it did not.
fn agreement_violations(
    state: &str,
    doctor: Option<&serde_json::Value>,
    status: Option<&serde_json::Value>,
    preview: Option<&serde_json::Value>,
    skips: &mut Skips,
) -> Vec<String> {
    let mut out = Vec::new();
    for (label, present) in [
        ("doctor --json", doctor.is_some()),
        ("status --json", status.is_some()),
        ("trust --preview", preview.is_some()),
    ] {
        if !present {
            skips.note(format!(
                "[{state}] {label}: emitted no JSON, so the cross-surface agreement rule (d) \
                 could not compare it against the other two in this state"
            ));
        }
    }

    let d = machine_field(doctor, "/next_action");
    let s = machine_field(status, "/next_action/command");
    let p = machine_field(preview, "/fix");

    // Rule 1, restricted to the rung the two surfaces actually share. See the
    // doc comment: outside the trust rung they legitimately rank different
    // questions first, and the un-asserted part is recorded as a skip.
    let d_trust = d.is_some_and(|c| TRUST_RUNG_COMMANDS.contains(&c));
    let s_trust = s.is_some_and(|c| TRUST_RUNG_COMMANDS.contains(&c));
    if doctor.is_some() && status.is_some() {
        if d_trust && s_trust && d != s {
            out.push(format!(
                "\n  state    : {state}\n  surface A: doctor --json  /next_action     = {d:?}\n  \
                 sentence : {:?}\n  \
                 surface B: status --json  /next_action/command = {s:?}\n  \
                 sentence : {:?}\n  \
                 why      : BOTH name a trust-rung command and the two commands differ. They are \
                 answering the same question at the same moment about the same project, so which \
                 command a driver runs depends only on which surface it happened to poll.",
                machine_field(doctor, "/next_step"),
                machine_field(status, "/next_action/sentence"),
            ));
        } else if d != s {
            skips.note(format!(
                "[{state}] rule (d) clause 1: `doctor --json` next_action = {d:?} and \
                 `status --json` next_action.command = {s:?} differ OUTSIDE the trust rung, and \
                 that difference is NOT asserted. The two surfaces share the setup ladder but \
                 rank the rungs above it (intake, consent) for their own audience, so equality \
                 is not a contract the product makes there and asserting it would flag correct \
                 behaviour. What IS asserted for this state is clauses 2 and 3."
            ));
        }
    }

    // Rules 2 and 3 need to know whether the preview is speaking about the
    // trust rung at all. An empty blocker list means "nothing to say here",
    // which is silence, not a contradicting `null`.
    let blockers = preview
        .and_then(|v| v.get("blockers"))
        .and_then(|b| b.as_array());
    let blocked = blockers.is_some_and(|b| !b.is_empty());

    for (label, field, value) in [
        ("doctor --json", "/next_action", d),
        ("status --json", "/next_action/command", s),
    ] {
        let Some(cmd) = value else { continue };
        if !TRUST_RUNG_COMMANDS.contains(&cmd) {
            continue;
        }
        match p {
            // Rule 2.
            None if blocked => out.push(format!(
                "\n  state    : {state}\n  surface A: trust --preview /fix    = null (with {} \
                 blocker(s), none of which carries a repairing command)\n  \
                 surface B: {label} {field} = {cmd:?}\n  \
                 why      : one project, two answers about the SAME rung. The preview says no \
                 command repairs this condition; {label} names one anyway. A driver polling {label} \
                 runs it, observes nothing change, and re-polls into the identical field — the \
                 exact loop `fix: null` exists to end. Emit null on both, or a working command on \
                 both.",
                blockers.map(|b| b.len()).unwrap_or(0)
            )),
            // Rule 3.
            Some(fix) if fix != cmd => out.push(format!(
                "\n  state    : {state}\n  surface A: trust --preview /fix    = {fix:?}\n  \
                 surface B: {label} {field} = {cmd:?}\n  \
                 why      : both surfaces agree the trust rung is actionable and name DIFFERENT \
                 commands for it. Whichever one a driver happens to read decides what it runs."
            )),
            _ => {}
        }
    }
    out
}

// ---------------------------------------------------------------------------
// (c) Convergence, generalized over the matrix
// ---------------------------------------------------------------------------

/// Everything about a project a driver can observe through `doctor --json`.
///
/// Convergence is judged on this tuple and on nothing else. Exit status is
/// deliberately absent: `orphan-body-missing` is the state where the offered
/// command exits 0 and prints a green tick over an untouched blocking
/// condition, so an exit-code check would call that state converged.
fn observable(v: &serde_json::Value) -> String {
    format!(
        "next_action={:?} next_step={:?} state={:?} readiness={:?} errors={} warnings={}",
        v["next_action"], v["next_step"], v["state"], v["readiness"], v["errors"], v["warnings"]
    )
}

/// Execute one state's machine field verbatim and require the state to move.
///
/// Returns the failure message, or `None` when the state converged, was
/// terminal (a `null` machine field: there is nothing to execute and nothing
/// to loop on), or was recorded as a skip.
fn converge_once(state: &State, skips: &mut Skips) -> Option<String> {
    let before: serde_json::Value = serde_json::from_str(&strip_ansi(
        &run(&["doctor", "--json"], &state.home, &state.proj).text,
    ))
    .ok()?;
    let Some(fix) = before["next_action"].as_str().map(str::to_string) else {
        // Not a gap: `null` is the terminal answer, and a driver that reads it
        // stops. There is no command to execute, so there is nothing for a
        // convergence check to say. Rule (d) is what guards this state, and it
        // ran above.
        skips.note(format!(
            "[{}] convergence (c): `doctor --json` `next_action` is null — terminal by contract, \
             so there is no command to execute and nothing for (c) to say. What holds this state \
             is rule (d) — which, where the trust rung is terminal, forbids ANY surface naming a \
             trust-rung command over it — together with (a0)/(a)/(a')/(b) on every other string \
             the state emits.",
            state.name
        ));
        return None;
    };
    if fix == "agentstack trust ." {
        // A structural limit of a stdin-null spawn, named rather than papered
        // over: the grant needs a reviewed digest a human supplies, so this
        // command cannot be driven from here. The end-to-end walk through it
        // is asserted by `tests/trust_content_drift.rs`.
        skips.note(format!(
            "[{}] convergence (c): the offered fix is `agentstack trust .`, which needs a \
             reviewed digest and cannot be driven from a stdin-null spawn. Asserted end to end \
             instead by `tests/trust_content_drift.rs`.",
            state.name
        ));
        return None;
    }

    let snapshot_before = observable(&before);
    let argv: Vec<&str> = fix.split_whitespace().skip(1).collect();
    let out = run(&argv, &state.home, &state.proj);
    let after: serde_json::Value = serde_json::from_str(&strip_ansi(
        &run(&["doctor", "--json"], &state.home, &state.proj).text,
    ))
    .ok()?;
    let snapshot_after = observable(&after);
    if snapshot_before != snapshot_after {
        return None;
    }
    Some(format!(
        "\n  state    : {}\n  command  : `{fix}`\n  exit_ok  : {}\n  before   : {snapshot_before}\n  \
         after    : {snapshot_after}\n  \
         why      : `doctor --json` named this as the ONE thing to run; running it verbatim left \
         every observable field identical, and the SAME command is named again. That is a \
         reproduced infinite loop. Note the exit status: a command may succeed and still change \
         nothing, which is why this check compares state and never exit codes.\n\n\
         Output of the offered command:\n{}",
        state.name,
        out.ok,
        out.text.trim()
    ))
}

/// The level-(b) rule, as one function so the guard can be pointed at itself
/// (see `the_guard_catches_the_historical_lock_regression`).
///
/// Returns the failure message when `h` is a machine-readable fix for a
/// blocking finding whose clap node declares `--write` and which does not ask
/// for it. `None` means the suggestion is fine.
fn noop_violation(clap_tree: &clap::Command, h: &Harvested) -> Option<String> {
    // Namespace-normalized first, or rule (b) would go BLIND on every
    // `agentstack x <cmd>` string: `x` resolves to no clap node, the path comes
    // back empty, and a preview-only fix would be waved through.
    let toks = as_argv(&tokens_of(&h.command));
    let path = subcommand_path(clap_tree, &toks[1..]);
    if path.is_empty() || !takes_write_flag(clap_tree, &path) {
        return None;
    }
    if toks.iter().any(|t| t == "--write" || t == "-w") {
        return None;
    }
    Some(format!(
        "\n  state   : {}\n  surface : {}\n  field   : {}\n  command : `{}`\n  why     : offered as the fix for a BLOCKING finding, but `agentstack {}` \
         only previews without `--write` — running it verbatim changes nothing and the \
         finding comes back unchanged. Did you mean `{} --write`?",
        h.state,
        h.surface,
        h.origin,
        h.command,
        path.join(" "),
        h.command,
    ))
}

/// The placeholder rule, as one function so the guard can be pointed at itself
/// (see `the_guard_catches_a_placeholder_and_a_non_command`).
///
/// A person reads `agentstack search <query>` and knows to substitute. A
/// machine field is executed verbatim, so a placeholder there is a command
/// that cannot run — and cannot ever *become* runnable, however many times the
/// caller re-polls. That is the shape of the reproduced loop: a driver ran
/// `--server <server>`, got `no server '<server>'`, and polled forever.
fn placeholder_violation(h: &Harvested) -> Option<String> {
    // Any `<…>` token, not only a whole-token one: `--server=<name>` is just
    // as unrunnable as a bare `<name>`, and the rule this file's header states
    // is "placeholder-free", with no qualifier.
    let bad: Vec<String> = tokens_of(&h.command)
        .into_iter()
        .filter(|t| is_placeholder(t) || (t.contains('<') && t.contains('>')))
        .collect();
    if bad.is_empty() {
        return None;
    }
    Some(format!(
        "\n  state   : {}\n  surface : {}\n  field   : {}\n  command : `{}`\n  why     : a machine-readable field is executed verbatim, so the \
         placeholder(s) {:?} make it unrunnable. Placeholders belong in human prose only — \
         emit a concrete value here, or emit `null`.",
        h.state, h.surface, h.origin, h.command, bad
    ))
}

/// The "a machine field carries a command, or nothing" rule, as one function
/// for the same reason.
///
/// `None` means the value is fine (it is a runnable command).
fn non_command_violation(state: &str, surface: &str, field: &str, value: &str) -> Option<String> {
    if is_whole_command(value) {
        return None;
    }
    Some(format!(
        "\n  state   : {state}\n  surface : {surface}\n  field   : {field}\n  value   : {value:?}\n  why     : a machine-readable field is executed VERBATIM by a driver. \
         This value is not a runnable `agentstack …` command, so a caller that executes it gets \
         an error, re-polls, and is handed the same value again — a reproduced infinite loop. \
         Emit a runnable command, or emit `null`.",
    ))
}

/// The guard, pointed at the exact bug it exists to prevent.
///
/// `agentstack lock` used to write. When it became preview-by-default, ~100
/// guidance strings kept naming the preview form, and every one of them still
/// compiled, still read as English, and still passed the suite. This asserts
/// that the level-(b) rule flags that string — and that it does NOT flag the
/// corrected one — so the guard cannot rot into a no-op itself.
#[test]
fn the_guard_catches_the_historical_lock_regression() {
    let clap_tree = agentstack::cli::Cli::command();
    let historical = Harvested {
        state: "content-drifted".into(),
        surface: "doctor --json".into(),
        origin: "/next_action".into(),
        audience: Audience::Machine,
        command: "agentstack lock".into(),
        blocking: true,
    };
    let msg = noop_violation(&clap_tree, &historical)
        .expect("the guard must flag a bare `agentstack lock` offered as a blocking fix");
    assert!(
        msg.contains("only previews without `--write`") && msg.contains("agentstack lock --write"),
        "the failure message must name the offending string AND the corrected one:\n{msg}"
    );
    println!("guard self-check — this is what the historical bug looks like when caught:{msg}");

    let corrected = Harvested {
        command: "agentstack lock --write".into(),
        ..historical
    };
    assert!(
        noop_violation(&clap_tree, &corrected).is_none(),
        "the corrected form must pass, or the guard is unusable"
    );
}

/// The two newer rules, pointed at the two strings that actually escaped.
///
/// Same construction as the historical self-check above, and for the same
/// reason: these rules are enforced through the SAME functions the main
/// assertion calls, so neither can rot into a no-op while still "passing".
/// Both are asserted two-sided — the bad string must fail AND the corrected
/// one must pass — because a rule that flags everything is as useless as one
/// that flags nothing.
#[test]
fn the_guard_catches_a_placeholder_and_a_non_command_in_a_machine_field() {
    // 1. The placeholder. This exact string sat in a machine field and the
    //    seven-state matrix never reached the rung that emits it.
    let shape = Harvested {
        state: "healthy-ungrouped".into(),
        surface: "doctor --json".into(),
        origin: "/next_action".into(),
        audience: Audience::Machine,
        command: "agentstack toolset create <name> --server <server>".into(),
        blocking: false,
    };
    let msg = placeholder_violation(&shape)
        .expect("the guard must flag a placeholder in a machine-readable field");
    assert!(
        msg.contains("<name>") && msg.contains("<server>"),
        "the failure must name the offending placeholder(s):\n{msg}"
    );
    println!("guard self-check — a placeholder in a machine field:{msg}");
    let concrete = Harvested {
        command: "agentstack toolset create dev --server filesystem".into(),
        ..shape
    };
    assert!(
        placeholder_violation(&concrete).is_none(),
        "a concrete command must pass, or the guard is unusable"
    );

    // 2. The non-command: prose in a machine field, the defect that used to be
    //    dropped silently with no coverage note at all.
    let prose = non_command_violation(
        "healthy-ungrouped",
        "doctor --json",
        "/next_action",
        "review the errors above",
    )
    .expect("the guard must flag prose in a machine-readable field");
    assert!(
        prose.contains("review the errors above"),
        "the failure must quote the offending value:\n{prose}"
    );
    println!("guard self-check — prose in a machine field:{prose}");
    // A sentence that merely CONTAINS a command is still prose.
    assert!(
        non_command_violation(
            "s",
            "x",
            "/next_action",
            "agentstack apply --write to render your setup",
        )
        .is_some(),
        "a command wrapped in a sentence is not executable verbatim"
    );
    // And the two runnable shapes must pass, including the lone `.` path
    // argument, which is not a full stop.
    for good in ["agentstack lock --write", "agentstack trust ."] {
        assert!(
            non_command_violation("s", "x", "/next_action", good).is_none(),
            "`{good}` is runnable and must pass, or the guard is unusable"
        );
    }
}

/// The extractor's token boundary, two-sided.
///
/// Any narrowing of what this file HARVESTS is a way to switch the guard off
/// without touching an assertion, so the one narrowing it makes is pinned here
/// in both directions: a path that merely contains the letters must yield
/// nothing, and every real shape the product prints must still be lifted.
#[test]
fn the_harvest_lifts_invocations_and_not_paths() {
    for prose in [
        "inline skill 'orphan' declares a body at /tmp/p/.agentstack/./skills/orphan that is not present on disk",
        "see /home/u/.agentstack/agentstack.toml",
        "the agentstack.toml file is the manifest",
    ] {
        assert_eq!(
            command_from_fragment(prose),
            None,
            "a path or a filename is not a suggestion to run anything: {prose:?}"
        );
    }
    for (prose, want) in [
        (
            "run `agentstack lock --write` to pin",
            "agentstack lock --write",
        ),
        ("agentstack apply --write", "agentstack apply --write"),
        // Pre-existing and harmless: a trailing `.` is cut as sentence
        // punctuation, so the harvested form is the verb alone. `agentstack
        // trust` parses and is judged; only the path argument is lost.
        // `is_whole_command`, which judges MACHINE fields, keeps the `.`.
        ("↳ agentstack trust .", "agentstack trust"),
        (
            "Next: agentstack search <query>",
            "agentstack search <query>",
        ),
    ] {
        assert_eq!(
            command_from_fragment(prose).as_deref(),
            Some(want),
            "a real invocation must still be harvested from {prose:?}"
        );
    }
}

/// Rule (d), pointed at the exact payload pair that shipped — two-sided.
///
/// A rule that fires on everything is as useless as one that fires on nothing,
/// and rule (d) is the one with the most room to be either: it compares three
/// surfaces that legitimately answer different questions. So each of its three
/// clauses is asserted both ways here, and the legitimate-difference case
/// (`doctor` naming a render while the preview is silent) is asserted to pass.
#[test]
fn the_guard_catches_a_cross_surface_disagreement() {
    let doctor = |cmd: serde_json::Value| serde_json::json!({ "next_action": cmd });
    let status = |cmd: serde_json::Value| serde_json::json!({ "next_action": { "command": cmd } });
    let preview = |fix: serde_json::Value, blockers: usize| {
        serde_json::json!({
            "fix": fix,
            "blockers": (0..blockers)
                .map(|_| serde_json::json!({ "kind": "skill", "fix": serde_json::Value::Null }))
                .collect::<Vec<_>>(),
        })
    };
    let mut sink = Skips::default();

    // Rule 2 — THE DEFECT OF THE FINAL ROUND. A declared body that is not on
    // disk: the preview says no command repairs it, doctor and status name
    // `lock --write` anyway.
    let d = doctor(serde_json::json!("agentstack lock --write"));
    let s = status(serde_json::json!("agentstack lock --write"));
    let p = preview(serde_json::Value::Null, 1);
    let msgs = agreement_violations(
        "inline-body-missing",
        Some(&d),
        Some(&s),
        Some(&p),
        &mut sink,
    );
    assert_eq!(
        msgs.len(),
        2,
        "both loud surfaces must be named, not just the first:\n{msgs:#?}"
    );
    for m in &msgs {
        assert!(
            m.contains("trust --preview") && m.contains("null"),
            "the failure must name BOTH surfaces and BOTH values:\n{m}"
        );
    }
    assert!(
        msgs.iter().any(|m| m.contains("doctor --json"))
            && msgs.iter().any(|m| m.contains("status --json")),
        "both disagreeing surfaces must be reported:\n{msgs:#?}"
    );
    println!(
        "guard self-check — the cross-surface contradiction:{}",
        msgs.join("")
    );

    // …and the corrected payload — all three null — must pass.
    assert!(
        agreement_violations(
            "inline-body-missing",
            Some(&doctor(serde_json::Value::Null)),
            Some(&status(serde_json::Value::Null)),
            Some(&p),
            &mut sink,
        )
        .is_empty(),
        "three surfaces answering `null` together is the corrected shape and must pass"
    );

    // Rule 1 — both surfaces answer the TRUST rung and answer it differently.
    let msgs = agreement_violations(
        "s",
        Some(&doctor(serde_json::json!("agentstack lock --write"))),
        Some(&status(serde_json::json!("agentstack trust ."))),
        None,
        &mut sink,
    );
    assert!(
        msgs.iter()
            .any(|m| m.contains("agentstack lock --write") && m.contains("agentstack trust .")),
        "rule 1 must quote both commands:\n{msgs:#?}"
    );
    // …and its deliberate narrowing, asserted so it cannot silently widen back
    // into a false positive: two surfaces on DIFFERENT rungs are not a
    // contradiction, and the difference is recorded rather than judged.
    let mut narrowing = Skips::default();
    assert!(
        agreement_violations(
            "empty-manifest",
            Some(&doctor(serde_json::json!("agentstack trust ."))),
            Some(&status(serde_json::json!("agentstack yes"))),
            None,
            &mut narrowing,
        )
        .is_empty(),
        "`status` ranking a pending intake first while `doctor` ranks the consent gate first is \
         two surfaces on two rungs, not a contradiction"
    );
    assert!(
        narrowing.0.iter().any(|n| n.contains("NOT asserted")),
        "every difference clause 1 declines to judge must land in the coverage report:\n{:#?}",
        narrowing.0
    );

    // Rule 3 — both name a trust-rung command, and they differ.
    let msgs = agreement_violations(
        "s",
        Some(&doctor(serde_json::json!("agentstack trust ."))),
        Some(&status(serde_json::json!("agentstack trust ."))),
        Some(&preview(serde_json::json!("agentstack lock --write"), 1)),
        &mut sink,
    );
    assert!(
        msgs.iter().any(|m| m.contains("DIFFERENT")),
        "rule 3 must fire when the preview and the ladder name different trust-rung commands:\n{msgs:#?}"
    );

    // THE NON-VIOLATION, asserted just as hard. `trust --preview` says nothing
    // about rendering, so a silent preview beside `apply --write` is two
    // surfaces answering two questions — not a contradiction. Without this the
    // rule would fire on healthy projects and get switched off.
    assert!(
        agreement_violations(
            "rendered-only",
            Some(&doctor(serde_json::json!("agentstack apply --write"))),
            Some(&status(serde_json::json!("agentstack apply --write"))),
            Some(&preview(serde_json::Value::Null, 1)),
            &mut sink,
        )
        .is_empty(),
        "a preview silent about a rung it does not speak about is not a disagreement"
    );
}

/// The CLASSIFICATION half of the same guard, checked against the payload
/// shape that got past it.
///
/// `placeholder_violation` and `non_command_violation` were both live and both
/// correct, and a placeholder still reached a machine field: `status --json`
/// emitted `next_action: {command, step, why}` with the filtered command
/// beside an UNfiltered `step`, and no state in the matrix reached the rung
/// that fills it. Two independent reasons, and only one of them is fixable by
/// a fixture:
///
/// * The rung is unreachable here. `status` answers the Group rung only once
///   a bridge is registered, and `run` deliberately spawns with `env_clear`
///   and `PATH=/usr/bin:/bin`, so no harness is ever detected and the bridge
///   branch precedes the ladder in every state this file can build. That is a
///   real coverage limit, recorded in the header, not a rule that is missing.
/// * The rule itself is only useful if the WALK tags such a value as machine.
///   That half needs no fixture at all, so it is asserted directly, against
///   the exact body the product emitted.
#[test]
fn the_harvest_tags_a_nested_command_carrier_as_a_machine_field() {
    let body: serde_json::Value = serde_json::json!({
        "next_action": {
            "command": serde_json::Value::Null,
            "step": "agentstack toolset create <name> --server <server>",
            "why": "group these for a task, then switch between toolsets",
        }
    });
    let mut found = Vec::new();
    harvest_json_strings(&body, "", false, false, &mut found);
    let step = found
        .iter()
        .find(|s| s.ptr == "/next_action/step")
        .expect("the walk must reach the nested value at all");
    assert!(
        step.machine && !step.display_twin,
        "a command carrier nested in a guidance object IS a machine field \
         (ptr={}, machine={}, display_twin={})",
        step.ptr,
        step.machine,
        step.display_twin
    );
    // …and, being machine, it is judged: this is the failure the sweep would
    // print if a state ever reached that rung again with this shape.
    let h = Harvested {
        state: "synthetic".into(),
        surface: "status --json".into(),
        origin: step.ptr.clone(),
        audience: Audience::Machine,
        command: step.value.clone(),
        blocking: false,
    };
    let msg = placeholder_violation(&h)
        .expect("a placeholder in a nested machine carrier must be flagged");
    println!("guard self-check — nested carrier classified and judged:{msg}");

    // The prose sibling stays prose: `why` must NOT be swept in, or the strict
    // rules would fire on correct English and the guard would be turned off.
    let why = found
        .iter()
        .find(|s| s.ptr == "/next_action/why")
        .expect("the walk keeps every string");
    assert!(!why.machine, "`why` is rationale, never a command carrier");
}

// ---------------------------------------------------------------------------
// (c) CONVERGES
// ---------------------------------------------------------------------------
//
// Levels (a) and (b) prove a suggestion is runnable and non-empty. Only
// EXECUTION proves it repairs anything.
//
// What is traded away: convergence is asserted for the two drift shapes and
// the never-pinned shape, not for every state in the matrix. Executing each state's fix would multiply
// this file's runtime by the size of the matrix for little extra signal, and
// drift is the path the historical regression actually broke. Every other
// state is asserted at levels (a) and (b) — a decision on the record, not an
// oversight.

/// Build a drifted project, run whatever `doctor --json` names as the ONE next
/// action, verbatim, and require the project to be measurably better after.
fn assert_the_offered_fix_converges(label: &str, prepare: &dyn Fn(&Path, &Path)) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("drift");
    fs::create_dir_all(&dir).unwrap();
    let (home, proj) = write_project(&dir, FULL_MANIFEST);
    prepare(&home, &proj);

    let before: serde_json::Value =
        serde_json::from_str(&strip_ansi(&run(&["doctor", "--json"], &home, &proj).text)).unwrap();
    let before_errors = before["errors"].as_u64().unwrap_or(0);
    let before_warnings = before["warnings"].as_u64().unwrap_or(0);
    let before_readiness = before["readiness"].as_str().unwrap_or("").to_string();
    assert!(
        is_blocking(&before),
        "fixture problem, not a product problem: the project ({label}) is not blocking:\n{before:#}"
    );

    let fix = before["next_action"]
        .as_str()
        .expect("doctor --json must always carry exactly one next_action")
        .to_string();
    let argv: Vec<&str> = fix.split_whitespace().skip(1).collect();
    let out = run(&argv, &home, &proj);
    if !out.ok {
        // A refusal is only acceptable when it is about this test's missing
        // terminal. When the refusal itself names a DIFFERENT command as the
        // step that had to come first, the guidance skipped a rung: the caller
        // was handed step 2 while step 1 was still outstanding, which is the
        // same dead end as a preview offered in place of a write.
        let prior: Vec<String> = harvest_human(&out.text)
            .into_iter()
            .map(|(_, c)| c)
            .filter(|c| *c != fix)
            .collect();
        assert!(
            prior.is_empty(),
            "wrong rung. `doctor --json` named `{fix}` as the ONE next action for a blocked \
             project, but running it verbatim REFUSED and pointed at a step that had to come \
             first:\n  offered  : `{fix}`\n  required first: {prior:?}\n\nFull refusal:\n{}\n\n\
             `doctor`'s next_action contract says it is never a command that would refuse. A \
             caller that executes it verbatim gets an error instead of progress.",
            out.text
        );
        panic!(
            "the fix `{fix}` that doctor offered for the project ({label}) FAILED to run:\n{}",
            out.text
        );
    }

    let after: serde_json::Value =
        serde_json::from_str(&strip_ansi(&run(&["doctor", "--json"], &home, &proj).text)).unwrap();
    let after_errors = after["errors"].as_u64().unwrap_or(0);
    let after_readiness = after["readiness"].as_str().unwrap_or("").to_string();
    let after_warnings = after["warnings"].as_u64().unwrap_or(0);
    let after_fix = after["next_action"].as_str().unwrap_or("").to_string();
    // "Progress" is the ladder advancing, not a counter falling. Repairing one
    // rung legitimately exposes the next: re-pinning drifted bytes changes the
    // lockfile, which makes the existing consent stale, so the warning count
    // can rise by one while the project is strictly closer to health. What a
    // dead end actually looks like is a caller executing the offered fix and
    // being handed THE SAME command again — so a changed `next_action` counts,
    // and an unchanged one fails even if a counter moved.
    //
    // This step-level check is deliberately not a whole loop: `trust .` needs
    // a reviewed digest and cannot be driven from a stdin-null spawn. The
    // end-to-end walk from drift all the way to health is asserted by
    // `tests/trust_content_drift.rs::a_json_only_driver_converges_from_drift_to_health`.
    let progressed = after_fix != fix
        || after_errors < before_errors
        || after_warnings < before_warnings
        || after_readiness != before_readiness;
    assert!(
        progressed,
        "no progress. Running the offered fix verbatim left the project in the same place:\n  \
         command  : `{fix}`\n  \
         next     : `{after_fix}` (unchanged)\n  \
         errors   : {before_errors} → {after_errors}\n  \
         warnings : {before_warnings} → {after_warnings}\n  \
         readiness: {before_readiness} → {after_readiness}\n\n\
         A fix that a caller can execute and re-poll into the identical finding is a dead end. \
         If the repair needs a write, the suggestion must carry the flag that writes."
    );
    println!(
        "converged: `{fix}` moved next_action → `{after_fix}`, errors {before_errors} → \
         {after_errors}, warnings {before_warnings} → {after_warnings}, \
         readiness {before_readiness} → {after_readiness}"
    );
}

/// Bytes moved under the grant, nothing new declared.
#[test]
fn the_content_drift_fix_converges() {
    assert_the_offered_fix_converges("content-drifted", &|home, proj| {
        lock(home, proj);
        grant(home, proj);
        drift_content(proj);
    });
}

/// Never pinned at all — a declared surface with no lockfile. Cheap to add
/// (no lock, no grant, no drift) and it closes the convergence gap the drift
/// pair left open: level (c) ran for drift only, so a machine field naming a
/// command that cannot repair an UNPINNED surface was invisible to it.
#[test]
fn the_never_pinned_fix_converges() {
    assert_the_offered_fix_converges("declared-unpinned", &|_home, _proj| {});
}

/// Bytes moved AND the declared surface grew, so the grant itself must be
/// re-gated. Kept separate from the case above because the two take different
/// rungs of the guidance ladder, and a regression in one is invisible in the
/// other.
#[test]
fn the_regate_drift_fix_converges() {
    assert_the_offered_fix_converges("content-drifted + surface changed", &|home, proj| {
        lock(home, proj);
        grant(home, proj);
        drift_content_and_surface(proj);
    });
}

/// Print the coverage this run achieved, and everything it did not.
fn report(
    states: &[State],
    surfaces: &BTreeMap<&str, Vec<String>>,
    harvested: &[Harvested],
    skips: &Skips,
    disco: &Discoverable,
) {
    let mut s = String::new();
    let _ = writeln!(s, "\n── guidance-is-executable: coverage ──");
    let _ = writeln!(s, "states ({}):", states.len());
    for st in states {
        let n = harvested.iter().filter(|h| h.state == st.name).count();
        let empty = Vec::new();
        let sw = surfaces.get(st.name).unwrap_or(&empty);
        let _ = writeln!(
            s,
            "  {:<18} {n:>3} command(s) over {} surface(s): {}",
            st.name,
            sw.len(),
            sw.join(", ")
        );
    }
    let machine = harvested
        .iter()
        .filter(|h| h.audience == Audience::Machine)
        .count();
    let _ = writeln!(
        s,
        "audiences: {machine} machine-readable field(s), {} human string(s)",
        harvested.len() - machine
    );
    let distinct: BTreeSet<&str> = harvested.iter().map(|h| h.command.as_str()).collect();
    let _ = writeln!(s, "distinct commands asserted ({}):", distinct.len());
    for c in &distinct {
        let _ = writeln!(s, "  {c}");
    }
    // Rule (e)'s input, printed in full: the set is derived every run, so the
    // report shows what was actually enforced rather than what this file
    // remembers.
    let _ = writeln!(
        s,
        "discoverability (e): {} listed on --help, {} also named there, {} under `agentstack x`, \
         {} on the complete map",
        disco.visible.len(),
        disco.named_on_help.len(),
        disco.under_x.len(),
        disco.in_help_all.len()
    );
    let verbs: BTreeSet<String> = harvested
        .iter()
        .filter_map(|h| as_argv(&tokens_of(&h.command)).into_iter().nth(1))
        .filter(|t| is_ident(t))
        .collect();
    let _ = writeln!(s, "verbs named by a surface ({}):", verbs.len());
    for v in &verbs {
        let _ = writeln!(s, "  {v:<12} {}", disco.how(v));
    }
    // Computed from what actually happened, never from an assumption. The
    // previous version printed "nothing — every surface answered in every
    // state" whenever no surface had *failed to parse as JSON*, while strings
    // it silently dropped never reached this counter at all. A guard that
    // reports its own coverage from anything but its own record is worse than
    // one with no report: it manufactures confidence.
    // No "covered everything" branch exists any more, by construction: the
    // caller seeds this list with the known structural limit, so the list is
    // never empty and the guard can never claim total coverage.
    let _ = writeln!(
        s,
        "NOT COVERED ({}) — a documented structural limit, or a string this run \
         saw and did not judge:",
        skips.0.len()
    );
    for k in &skips.0 {
        let _ = writeln!(s, "  {k}");
    }
    println!("{s}");
}
