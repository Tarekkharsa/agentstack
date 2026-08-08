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
//! * **(f) THE DELIVERY CLAIMS AGREE, AND MATCH THE DISK** — rules (a)–(e)
//!   judge the COMMANDS a surface names and nothing else, so the guard passed
//!   green through three separate findings of one shape: a delivery claim
//!   computed from `delivery::Plan` alone, without the per-harness bridge
//!   reading and without looking at disk. `use --write` wrote a server config
//!   while four surfaces said nothing was on disk, and `why` reported "live"
//!   where `doctor` reported "no bridge". Rule (f) lives in its own sweep at
//!   the bottom of this file, over the four states this matrix does not have.
//! * **(d) THE SURFACES AGREE** — `doctor --json`, `status --json` and
//!   `trust --preview` describe ONE project, so their machine fields may not
//!   contradict each other. This is the general form of the last two rounds of
//!   defects: every string involved parsed, wrote, and carried no placeholder,
//!   and the bug was that two surfaces said different things at once. Its
//!   fourth clause is stranger and stronger: the consent gate must be able to
//!   SEE the project the other surfaces describe. Every ladder in the product
//!   routes through `agentstack trust .`, and that command is EXEMPT from
//!   convergence (c) — a grant needs a reviewed digest a stdin-null spawn
//!   cannot supply — so a state where `trust` cannot find the manifest is a
//!   state where `status` names it forever and no other rule here can look.
//! * **(g) A WRITING COMMAND DOES NOT NAME ITSELF** — the closing `next:` line
//!   of `up`, `apply --write`, `use --write`, `lock --write` and
//!   `adopt --write` is the guidance a reader meets most often: it arrives the
//!   moment they finish a step. None of it was swept, because each of those
//!   commands destroys the state it was read in and covering them costs a
//!   fresh project per (state, surface) pair. "Expensive" is not "covered", so
//!   they now have their own sweep, and its own rule: a command whose parting
//!   advice is to run the command is the recurring fault at its purest, and
//!   (a), (b) and (e) are all structurally blind to it — the string parses, it
//!   writes, and it is perfectly discoverable.
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
//! **FOUR LIVE DEFECTS OF THIS EXACT FAMILY WERE FOUND BY WIDENING THIS FILE,
//! AND THEY WERE PUT ON THE LEDGER RATHER THAN IN A CLOSED BUG REPORT.** See
//! [`KNOWN_DEFECTS`] and the reproducer beside each one. The ledger is not an
//! allow-list: every entry must STILL REPRODUCE or the run fails and demands
//! the entry's deletion, so a repaired defect cannot leave a suppression behind
//! it. Their one-line shapes, because the pattern is the point:
//!
//! * **G33, repaired** — a machine field naming `agentstack init` where `init`
//!   refuses without a terminal. It now answers an explicit `null`: no runnable
//!   spelling of `init` is fit for a driver (`--dry-run` is a no-op preview,
//!   `--secrets <store>` is a placeholder, and `--yes` imports CLI configs and
//!   lifts live token values with no prompt), and the human sentence still
//!   names the verb.
//! * **G34, repaired** — `status` naming `agentstack yes` where `yes` refuses
//!   without a terminal. It now names `agentstack adopt --write`: the INERT
//!   half of that work, which declares the drop and grants nothing, leaving the
//!   trust gate where it was. The funnel survives in the human `why`.
//! * **G35, repaired** — `agentstack adopt`, the PREVIEW form of a command that
//!   declares `--write`, offered as the fix, exiting 0 and changing nothing
//!   (the `lock` regression, alive again in a second verb). Both surfaces now
//!   name the writing form.
//! * **STILL LIVE** — a machine manifest whose consent gate reports that the
//!   manifest does not exist. That one keeps its ledger entry and its
//!   `#[ignore]`d reproducer.
//!
//! Scope, stated so the gaps read as decisions:
//!
//! * Convergence (c) runs over the WHOLE matrix and over BOTH machine
//!   surfaces. It used to run for two drift shapes; then for every state, but
//!   reading `doctor --json` alone. That was still a hole with a defect in it:
//!   rule (d) clause 1 explicitly declines to require that `doctor` and
//!   `status` name the same command, so a command only `status` names was
//!   executed by nothing — and `status` is the surface a panel polls. In three
//!   states of this matrix `doctor` answers `null` while `status` hands a
//!   driver a command that refuses. Each field is now driven from a PRISTINE
//!   state, rebuilt rather than copied: a grant is keyed by the project's path,
//!   so `cp -r` yields an untrusted project wearing a trusted one's name.
//!   Exactly one command is exempt and it is named in the run's own NOT
//!   COVERED list: `agentstack trust .` needs a reviewed digest a human
//!   supplies. A `null` machine field is likewise recorded rather than
//!   asserted — there is no command to execute — and rule (d) guards those
//!   states instead.
//! * The matrix carries two MISSING-BODY states. It had none, and that hole is
//!   where the final defect lived: a declared body absent from disk, where
//!   `trust --preview` correctly emitted `fix: null` while `doctor` and
//!   `status` named `agentstack lock --write`. One of the two is behind an
//!   exit-0 green tick, which is why (c) compares state and not exit status.
//! * The matrix is no longer "a focused set of shapes somebody thought of". It
//!   is enumerated against the product's OWN routing — every arm of
//!   `overview::next_step` and every rung of `overview::Rung` — and each state
//!   names the arm it exists to reach. Three arms had never been reached by any
//!   state: `undeclared_drops` (routes to `agentstack yes`), `unimported_native`
//!   (routes to `agentstack adopt`), and the unadopted directory with no
//!   manifest at all (routes to `agentstack init`). All three were carrying a
//!   live defect. An arm with no state is a guidance branch no assertion in
//!   this file has ever read, which is the whole mechanism by which this fault
//!   class survived ten closures.
//! * Guidance emitted by interactive-only paths (the `init` wizard, TTY
//!   confirms) is out of reach of a spawned, stdin-null binary and is not
//!   harvested. `docs_commands.rs` covers the same class for documentation
//!   prose. Note the distinction the run's NOT COVERED list draws: the verbs
//!   `init`, `trust` and `yes` are all REACHED here and their non-interactive
//!   REFUSALS are harvested and judged; what is out of reach is the wizard text
//!   a terminal would show.
//! * The aligned remedy column an error prints (`  create one here:
//!   agentstack init`) is NOT harvested, and the reason is recorded on every
//!   run rather than left implicit: those lines carry placeholders no single
//!   sentinel can satisfy — a subcommand slot (`--manifest-dir <dir>
//!   <command>`) and an enumerated value (`--secrets <env|keychain|skip>`) —
//!   so reading them without a per-argument sentinel derived from the clap tree
//!   would make rule (a) fail on correct English, and the pressure would be to
//!   loosen rule (a). A named gap, not a hidden one.
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
//! * `Rung::Group` on `status` used to be recorded here as structurally out of
//!   reach, on the reasoning that `run` spawns with `env_clear` and
//!   `PATH=/usr/bin:/bin` so no harness is ever detected. That reasoning was
//!   WRONG, and the way it was wrong is worth keeping: harness detection does
//!   not read PATH, it reads the harness's own config file, and `~/.claude.json`
//!   lands inside an isolated HOME — which rule (f), further down this same
//!   file, had already been doing for its own states. So the branch was
//!   reachable all along and the gap was in the fixtures, not in the isolation.
//!   `bridge-registered-group` now stands on that rung and the sweep judges the
//!   real payload; `harness-no-bridge` covers the rung below it. A blind spot
//!   recorded as structural is worse than one recorded as work outstanding: the
//!   first stops anybody looking.
//!   `the_harvest_tags_a_nested_command_carrier_as_a_machine_field` still
//!   asserts the classification half directly, because a fixture cannot prove a
//!   walk tags a field correctly.
//!
//! ---------------------------------------------------------------------------
//! WHAT THIS FENCE COVERS, AND WHAT IT DOES NOT
//! ---------------------------------------------------------------------------
//!
//! Kept here as a flat list on purpose. The prose above explains WHY; a reader
//! deciding whether a new bug could have been caught needs a table, and a table
//! that lives anywhere but next to the assertions goes stale. The same list is
//! recomputed and printed by [`report`] on every run from the run's own record,
//! so if the two ever disagree, believe the run.
//!
//! **STATES SWEPT (21).** Each names the routing arm it exists to reach.
//!   no-project · empty-manifest · declared-unpinned · pinned-untrusted ·
//!   trusted-healthy · content-drifted · surface-stale · trust-revoked ·
//!   rendered-only · servers-no-bridge · hooks-only · machine-manifest ·
//!   dropped-undeclared · native-unimported · bridge-registered-group ·
//!   harness-no-bridge · session-active · healthy-ungrouped · healthy-grouped ·
//!   inline-body-missing · orphan-body-missing
//!
//! **SURFACES SWEPT IN EVERY STATE (13).** status · status --json · doctor ·
//!   doctor --json · trust --preview · trust · delivery · delivery --json ·
//!   lock · apply --dry-run · use · adopt · the bare invocation.
//!
//! **WRITING SURFACES (5), swept over 6 states = 30 fresh projects.**
//!   up · apply --write · use --write · lock --write · adopt --write.
//!
//! **NOT COVERED — every one of these is a decision, and each is re-printed by
//! the run so it cannot be forgotten:**
//!
//! 1. INTERACTIVE OUTPUT. The `init` wizard, the TTY trust confirm, and the
//!    `yes` review are unreachable from a stdin-null spawn. Their verbs ARE
//!    reached and their non-interactive REFUSALS are judged; the terminal text
//!    is not. `docs_commands.rs` covers the same class for documentation prose.
//! 2. THE ALIGNED REMEDY COLUMN of an error (`  create one here:   agentstack
//!    init`). Not harvested, because those lines carry placeholders no single
//!    SENTINEL can satisfy — a subcommand slot (`--manifest-dir <dir>
//!    <command>`) and an enumerated value (`--secrets <env|keychain|skip>`) —
//!    so reading them today would fail rule (a) on correct English. Closing
//!    this needs a per-argument sentinel derived from the clap tree.
//! 3. `agentstack x gateway connect --all --write` REFUSING under this file's
//!    spawn when no harness config is present. That is a fact about the
//!    isolated machine, not about the guidance, so it is not asserted. The
//!    three harness-detecting states are what put the branch under the rules.
//! 4. SECRET-RESOLUTION states (`${REF}` unresolved). They need a keychain or a
//!    project `.env` this file does not build; the `Secrets` section's guidance
//!    is pinned by the `secret` command's own tests.
//! 5. THE WHOLE WALK TO HEALTH. Convergence (c) is one step at a time, because
//!    `agentstack trust .` needs a reviewed digest a human supplies. The
//!    end-to-end walk is
//!    `tests/trust_content_drift.rs::a_json_only_driver_converges_from_drift_to_health`.
//! 6. THE WRITING SWEEP IS 6 STATES WIDE, NOT 21. Every pair costs a fresh
//!    project, so the grid is bounded by runtime rather than by principle. The
//!    15 states left out are listed by name in that test's own NOT COVERED
//!    block on every run.
//! 7. A HARNESS THAT IS INSTALLED rather than merely CONFIGURED. Detection
//!    reads config files, which the fixtures supply; nothing here puts a binary
//!    on PATH, and `run` clears PATH on purpose.
//! 8. `apply`/`use`/`session start` DELIVERY CLAIMS while writing. Rule (f)
//!    sweeps read-only surfaces; those three are pinned by
//!    `tests/abandoned_render_is_named.rs` and `tests/use_honours_delivery.rs`.
//! 9. RULE (f) CLAUSE 2 JUDGES SERVER ARTIFACTS ONLY. Claims about
//!    instructions, settings, skills and extensions are outside its walk.
//!
//! **RUNTIME.** ~25 s wall for the whole file on a warm build (21 states × 13
//! surfaces, 30 writing pairs, two convergence passes over freshly built
//! matrices). It was ~8 s before this widening. That is the price of executing
//! real commands against real fixtures, and it is paid once per CI run.

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

/// Hooks and nothing else.
///
/// Hooks are the one capability that always gets the full consent ceremony, so
/// this manifest's guidance lane is the trust gate with no server, no skill and
/// no instruction beside it to dilute what the surfaces say about it.
const HOOKS_ONLY_MANIFEST: &str = r#"version = 1

[hooks.format]
event = "PostToolUse"
matcher = "Edit"
command = "echo"
args = ["formatted"]
"#;

/// `FULL_MANIFEST` plus a named toolset — the shape `session start` needs,
/// since a session loads a toolset by name and refuses without one.
const TOOLSET_MANIFEST: &str = r#"version = 1

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

[toolsets.dev]
servers = ["filesystem"]
skills = ["summarize"]
"#;

/// A server configured natively in the project, which this manifest does not
/// declare — the input to the `adopt` rung.
const FOREIGN_MCP_JSON: &str =
    r#"{"mcpServers":{"legacy":{"type":"stdio","command":"node","args":["legacy.js"]}}}"#;

struct State {
    name: &'static str,
    home: PathBuf,
    proj: PathBuf,
}

/// A state, as a NAME and a BUILDER rather than as a built directory.
///
/// The split exists because convergence MUTATES: running the offered command is
/// the whole point of rule (c), and a state that has already been driven once
/// cannot answer the same question again. `status --json` and `doctor --json`
/// do not always name the same command (see rule (d) clause 1), so executing
/// both machine fields honestly needs two pristine copies of the same state —
/// and a copied DIRECTORY is not one: a grant is keyed by the project's path,
/// so `cp -r` silently produces an untrusted project wearing a trusted one's
/// name. Rebuilding from the spec is the only faithful way to get a second
/// copy, which is why the builders are plain `fn` pointers.
struct StateSpec {
    name: &'static str,
    build: fn(&Path) -> (PathBuf, PathBuf),
}

impl StateSpec {
    fn build_under(&self, root: &Path) -> State {
        let dir = root.join(self.name);
        fs::create_dir_all(&dir).unwrap();
        let (home, proj) = (self.build)(&dir);
        State {
            name: self.name,
            home,
            proj,
        }
    }
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

/// A directory with no manifest at or above it — the UNADOPTED state.
///
/// The matrix began at "initialized, nothing declared" and so never held the
/// state every first-time reader is in. It is the one state whose answer is
/// `agentstack init`, and therefore the only state that can judge whether that
/// answer is executable.
fn no_project(root: &Path) -> (PathBuf, PathBuf) {
    let home = root.join("home");
    let proj = root.join("proj");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&proj).unwrap();
    (home, proj)
}

/// The MACHINE manifest: the setup lives at `~/.agentstack/agentstack.toml`
/// and the working directory IS the home.
///
/// A different scope, not a different project. The manifest home and
/// `AGENTSTACK_HOME` coincide here, which changes what `apply --scope` defaults
/// to and which files the rendered lane targets, so every surface answers from
/// a different reading of "where does this land". Nothing in the matrix
/// exercised it: every other state is a repo manifest.
fn machine_manifest(root: &Path) -> (PathBuf, PathBuf) {
    let home = root.join("home");
    let a = home.join(".agentstack");
    fs::create_dir_all(a.join("skills/summarize")).unwrap();
    fs::create_dir_all(a.join("instructions")).unwrap();
    fs::write(a.join("skills/summarize/SKILL.md"), SKILL_DESCRIBED).unwrap();
    fs::write(a.join("instructions/house-rules.md"), "Prefer boring.\n").unwrap();
    fs::write(a.join("agentstack.toml"), FULL_MANIFEST).unwrap();
    (home.clone(), home)
}

/// Make a harness DETECTABLE under the isolated HOME, and say whether the
/// AgentStack bridge is registered in it.
///
/// `run` spawns with `env_clear` and `PATH=/usr/bin:/bin` on purpose, so no
/// harness is ever found on PATH — which is why this file's header used to
/// record the whole bridge-and-Group branch as structurally out of reach.
/// Detection does not read PATH: it reads the harness's own config file, and
/// `~/.claude.json` lands inside an isolated HOME. Writing one is therefore not
/// a way around the isolation; it is the same reading the product performs,
/// supplied honestly. Rule (f) already relies on exactly this.
fn detect_claude_code(home: &Path, with_bridge: bool) {
    let body = if with_bridge {
        CLAUDE_BRIDGE_CONFIG
    } else {
        r#"{"mcpServers":{}}"#
    };
    fs::write(home.join(".claude.json"), body).unwrap();
}

/// Withdraw a grant that was given. Distinct from never having granted one:
/// the project has a trust RECORD saying no, and the surfaces have to route it.
fn revoke(home: &Path, proj: &Path) {
    let out = run(&["trust", "--revoke"], home, proj);
    assert!(out.ok, "fixture: trust --revoke failed:\n{}", out.text);
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

/// Every state in the matrix, as a name and a builder.
///
/// The matrix used to be eleven shapes and was called "deliberately focused".
/// It was not focused; it was the shapes somebody happened to think of, and the
/// gap between it and the states a real project passes through is where this
/// fault class kept living. So the list below is now enumerated against the
/// product's OWN routing — every arm of `overview::next_step` and every rung of
/// `overview::Rung` — and each state names the arm it exists to reach. An arm
/// with no state is a guidance branch no assertion in this file has ever read.
///
/// What is still NOT enumerable here, stated so the gaps read as decisions
/// rather than as coverage:
///
/// * Guidance behind an interactive prompt (the `init` wizard, the TTY trust
///   confirm, `yes`) is out of reach of a stdin-null spawn by construction.
///   `docs_commands.rs` covers the same class for documentation prose.
/// * A harness that is *installed* rather than merely *configured*. Detection
///   reads config files, which [`detect_claude_code`] can supply; it does not
///   simulate a binary on PATH, and `run` clears PATH on purpose.
/// * Secret-resolution states (`${REF}` unresolved on this machine) need a
///   keychain or an `.env` this file does not build. Their guidance is pinned
///   by the `secret` command's own tests.
fn matrix() -> Vec<StateSpec> {
    let mut states: Vec<StateSpec> = Vec::new();
    let mut add = |name: &'static str, build: fn(&Path) -> (PathBuf, PathBuf)| {
        states.push(StateSpec { name, build });
    };

    // 0. UNADOPTED — no manifest at or above the working directory.
    //
    //    The state every first-time reader is in, and the matrix did not have
    //    it. It is the only state whose answer is `agentstack init`, so it is
    //    the only state in which that answer can be judged at all.
    add("no-project", no_project);

    // 1. Initialized, nothing declared.
    add("empty-manifest", |d| write_project(d, "version = 1\n"));

    // 2. Capabilities declared, nothing pinned.
    add("declared-unpinned", |d| write_project(d, FULL_MANIFEST));

    // 3. Pinned, never consented.
    add("pinned-untrusted", |d| {
        let (home, proj) = write_project(d, FULL_MANIFEST);
        lock(&home, &proj);
        (home, proj)
    });

    // 4. Pinned and consented — the healthy shape.
    add("trusted-healthy", |d| {
        let (home, proj) = write_project(d, FULL_MANIFEST);
        lock(&home, &proj);
        grant(&home, &proj);
        (home, proj)
    });

    // 5. Consented, then the pinned content moved underneath it. This is the
    //    state whose fix must WRITE, and the one convergence is asserted on.
    add("content-drifted", |d| {
        let (home, proj) = write_project(d, FULL_MANIFEST);
        lock(&home, &proj);
        grant(&home, &proj);
        drift_content_and_surface(&proj);
        (home, proj)
    });

    // 5b. SURFACE STALE WITHOUT CONTENT DRIFT. The declared surface grew after
    //     the grant and not one pinned byte moved — `TrustState::Changed`
    //     reached by the other door. The matrix only ever reached it through
    //     `drift_content_and_surface`, where a content finding is present too,
    //     so no state ever showed what the surfaces say when re-consent is the
    //     ONLY outstanding thing.
    add("surface-stale", |d| {
        let (home, proj) = write_project_with_skill(d, FULL_MANIFEST, SKILL_DESCRIBED);
        lock(&home, &proj);
        grant(&home, &proj);
        let manifest = proj.join(".agentstack/agentstack.toml");
        let mut body = fs::read_to_string(&manifest).unwrap();
        body.push_str(
            "\n[servers.notes]\ntype = \"stdio\"\ncommand = \"node\"\nargs = [\"notes.js\"]\n",
        );
        fs::write(&manifest, body).unwrap();
        (home, proj)
    });

    // 5c. TRUST WITHDRAWN. Not the same as never granted: there is a trust
    //     record and it says no. `TrustState::Untrusted` reached from above
    //     rather than from below.
    add("trust-revoked", |d| {
        let (home, proj) = write_project_with_skill(d, FULL_MANIFEST, SKILL_DESCRIBED);
        lock(&home, &proj);
        grant(&home, &proj);
        revoke(&home, &proj);
        (home, proj)
    });

    // 6. The pure rendered lane: files only, no servers.
    add("rendered-only", |d| {
        write_project(d, RENDERED_ONLY_MANIFEST)
    });

    // 7. Servers declared, no bridge connected anywhere.
    add("servers-no-bridge", |d| {
        write_project(d, SERVERS_ONLY_MANIFEST)
    });

    // 7b. HOOKS AND NOTHING ELSE.
    //
    //     Hooks are the one capability that always gets the full consent
    //     ceremony, and no state declared one. So every hook sentence any
    //     surface prints — including the review card `trust` shows before it
    //     refuses — was unharvested, and the trust gate was only ever read
    //     over servers and skills.
    add("hooks-only", |d| {
        let home = d.join("home");
        let proj = d.join("proj");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(proj.join(".agentstack")).unwrap();
        fs::write(
            proj.join(".agentstack/agentstack.toml"),
            HOOKS_ONLY_MANIFEST,
        )
        .unwrap();
        (home, proj)
    });

    // 7c. THE MACHINE MANIFEST — a different SCOPE, not a different project.
    add("machine-manifest", machine_manifest);

    // 7d. DROPPED FILES WAITING. A body sitting in `.agentstack/skills/` that
    //     the manifest does not declare: the `undeclared_drops` arm, which
    //     outranks every other rung in `next_step` and which no state reached
    //     on purpose. It is the arm that routes to `agentstack yes`.
    add("dropped-undeclared", |d| {
        let home = d.join("home");
        let proj = d.join("proj");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(proj.join(".agentstack/skills/summarize")).unwrap();
        fs::write(
            proj.join(".agentstack/skills/summarize/SKILL.md"),
            SKILL_DESCRIBED,
        )
        .unwrap();
        fs::write(proj.join(".agentstack/agentstack.toml"), "version = 1\n").unwrap();
        (home, proj)
    });

    // 7e. NATIVE SERVERS THIS SETUP DOES NOT COVER — the `unimported_native`
    //     arm, which routes to `agentstack adopt`.
    //
    //     Reaching it needs three things at once: a detected harness, a grant
    //     already in place (the arm sits BELOW the trust gate), and a server
    //     configured natively that the manifest does not declare. No state had
    //     any of the three, so the arm had never been read by any rule in this
    //     file — and it is where a live defect of the recurring family was
    //     still sitting when this sweep was widened.
    add("native-unimported", |d| {
        let (home, proj) = write_project_with_skill(d, FULL_MANIFEST, SKILL_DESCRIBED);
        detect_claude_code(&home, true);
        lock(&home, &proj);
        grant(&home, &proj);
        // After the grant: `.mcp.json` is not part of the trust surface, so
        // writing it here leaves the consent valid and isolates the arm.
        fs::write(proj.join(".mcp.json"), FOREIGN_MCP_JSON).unwrap();
        (home, proj)
    });

    // 7f. THE BRIDGE RUNG, AND `Rung::Group` ON `status` BEHIND IT.
    //
    //     This file's header recorded `Rung::Group` on `status` as
    //     structurally unreachable, because "run spawns with env_clear so no
    //     harness is ever detected". That reasoning was wrong in a way worth
    //     naming: detection does not read PATH, it reads the harness's own
    //     config file, and `~/.claude.json` lands inside an isolated HOME —
    //     which rule (f) further down this same file had already been doing for
    //     its own states. So the branch was reachable all along and the gap was
    //     in the fixtures, not in the isolation. With the bridge registered and
    //     the project rendered, `status` finally answers the Group rung, and
    //     the machine field that a placeholder once reached is judged by the
    //     sweep rather than by a hand-built payload.
    add("bridge-registered-group", |d| {
        let (home, proj) = write_project_with_skill(d, FULL_MANIFEST, SKILL_DESCRIBED);
        detect_claude_code(&home, true);
        lock(&home, &proj);
        grant(&home, &proj);
        apply(&home, &proj);
        (home, proj)
    });

    // 7g. A HARNESS DETECTED WITH NO BRIDGE REGISTERED. The other half of the
    //     pair above: the state whose answer is `gateway connect --all --write`
    //     rather than silence about a bridge nobody could have registered.
    add("harness-no-bridge", |d| {
        let (home, proj) = write_project_with_skill(d, FULL_MANIFEST, SKILL_DESCRIBED);
        detect_claude_code(&home, false);
        lock(&home, &proj);
        grant(&home, &proj);
        (home, proj)
    });

    // 7h. A SESSION IS ACTIVE. Temporarily loaded content is on disk and the
    //     project has an end-state to return to, which is guidance of its own
    //     ("end it with …") and its own way to be wrong.
    add("session-active", |d| {
        let (home, proj) = write_project_with_skill(d, TOOLSET_MANIFEST, SKILL_DESCRIBED);
        lock(&home, &proj);
        grant(&home, &proj);
        let out = run(&["session", "start", "dev"], &home, &proj);
        assert!(out.ok, "fixture: session start dev failed:\n{}", out.text);
        (home, proj)
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
    add("healthy-ungrouped", |d| {
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
    add("healthy-grouped", |d| {
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
    add("inline-body-missing", |d| {
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
    add("orphan-body-missing", |d| {
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
        // A quote INSIDE the punctuation, e.g. `…--server <server>`,` — the
        // outer trim above cannot reach it, and leaving it attached hides a
        // placeholder from `is_placeholder` (`<server>\`` does not end with
        // `>`), which would quietly narrow rule (a'). Strictly a tightening:
        // it can only turn a token back INTO a placeholder, never out of one.
        let tok = tok.trim_end_matches(['`', '"', '\'']);
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
/// EVERY guidance-producing, non-interactive surface that does not WRITE.
///
/// The read/write split is the load-bearing line, and it is not the same line
/// as "read-only command". A preview is a guidance surface — it prints the
/// closing "re-run with `--write`" step that a reader follows next — and it
/// changes nothing, so it can be swept in every state for the price of one
/// spawn. The surfaces that do write (`up`, `apply --write`, `use --write`,
/// `lock --write`, `adopt --write`) cannot: running one destroys the state the
/// sweep is standing in. Those are swept by
/// [`every_closing_next_step_is_executable`], which builds a fresh state per
/// pair, and the split is why they are not simply appended here.
///
/// `trust --preview` is the review a panel renders before asking for consent;
/// bare `trust` is what a script gets instead, and its REFUSAL names commands
/// too — a refusal is guidance, and it was not being read.
const SURFACES: &[(&str, &[&str], bool)] = &[
    ("status", &["status"], false),
    ("status --json", &["status", "--json"], true),
    ("doctor", &["doctor"], false),
    ("doctor --json", &["doctor", "--json"], true),
    ("trust --preview", &["trust", "--preview"], true),
    // The non-interactive refusal path of the grant itself: it prints the
    // whole review card and then names how to proceed without a terminal.
    ("trust", &["trust"], false),
    ("delivery", &["delivery"], false),
    ("delivery --json", &["delivery", "--json"], true),
    // The previews. Each one closes with the step a reader takes next, which is
    // exactly the string family that broke ten times.
    ("lock", &["lock"], false),
    ("apply --dry-run", &["apply", "--dry-run"], false),
    ("use", &["use"], false),
    ("adopt", &["adopt"], false),
    // The bare invocation: the orientation screen someone sees by typing the
    // product's name, which has its own "Next:" line.
    ("(bare)", &[], false),
];

/// The surfaces that WRITE, swept one fresh state at a time by
/// [`every_closing_next_step_is_executable`]. `(label, argv)`.
const WRITING_SURFACES: &[(&str, &[&str])] = &[
    ("up", &["up"]),
    ("apply --write", &["apply", "--write"]),
    ("use --write", &["use", "--write"]),
    ("lock --write", &["lock", "--write"]),
    ("adopt --write", &["adopt", "--write"]),
];

// ---------------------------------------------------------------------------
// Live defects, named — never an allow-list
// ---------------------------------------------------------------------------

/// One guidance defect that is LIVE in the product today.
///
/// Widening this sweep found faults, and there are only three honest things to
/// do with them: fix the product (not this file's job), delete the assertion
/// (which is how the class survived ten closures), or name the defect so the
/// suite stays green AND the bug stays visible. This is the third.
///
/// It is not an allow-list, and the difference is enforced rather than
/// promised: every entry MUST still reproduce. If a defect is repaired and its
/// entry stays, the run fails and says "delete this entry" — so the ledger
/// cannot quietly become the place assertions go to die, which is the failure
/// mode of every suppression list ever written. Each entry also has a
/// dedicated `#[ignore]`d reproducer further down, so `cargo test -- --ignored`
/// prints the bug on demand.
struct KnownDefect {
    /// Matrix state the defect appears in.
    state: &'static str,
    /// The machine field that carries the bad command.
    surface: &'static str,
    /// The command as the product emits it.
    command: &'static str,
    /// What goes wrong when a driver runs it.
    why: &'static str,
}

const KNOWN_DEFECTS: &[KnownDefect] = &[
    // G33 (`agentstack init` in a machine field), G34 (`agentstack yes` in
    // status's machine field) and G35 (the preview form of `adopt` named as the
    // fix) were ENTRIES HERE and are now repaired in the product, so their
    // entries are gone — a ledger that keeps a fixed defect is a suppression
    // with nothing under it. Their reproducers below are no longer ignored:
    // each one now asserts the repaired behaviour instead of the bug.
    KnownDefect {
        state: "machine-manifest",
        surface: "trust --preview",
        command: CONSENT_GATE_BLIND,
        why: "A COMPLETE DEAD END, and the one the `trust .` convergence exemption was hiding. \
              With the manifest at `~/.agentstack/agentstack.toml` and the working directory at \
              `~`, `status --json` reports `manifest.loaded = true` and walks the reader up the \
              ladder — `agentstack lock --write` (exits 0, pins), then `agentstack trust .`. But \
              `trust`, `trust --preview` and `trust .` all exit 1 with \"no agentstack manifest \
              at or above ~ — run `agentstack init` first\" over the manifest the other surfaces \
              just described. The ladder therefore never terminates: `status` names `trust .` \
              forever, and the command it names cannot see the project at all. The `init` it \
              suggests instead is wrong twice over — a manifest exists, and `init` refuses \
              without a terminal.",
    },
];

/// Stand-in for "the surface produced no machine answer at all", so a defect
/// about a MISSING answer can sit on the same ledger as one about a wrong
/// command. There is no command to name here; that absence IS the defect.
const CONSENT_GATE_BLIND: &str = "(no JSON — the consent gate cannot find the manifest)";

/// Is this (state, surface, command) triple a defect already on the ledger?
fn known_defect(state: &str, surface: &str, command: &str) -> Option<&'static KnownDefect> {
    KNOWN_DEFECTS
        .iter()
        .find(|d| d.state == state && d.surface == surface && d.command == command)
}

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
    let specs = matrix();
    let read_root = tmp.path().join("read");
    let states: Vec<State> = specs.iter().map(|s| s.build_under(&read_root)).collect();
    let clap_tree = agentstack::cli::Cli::command();
    // What a reader can find, read off the real binary's help screens. Derived
    // once; used by rule (e) below and printed in the coverage report.
    let help_home = tmp.path().join("help-home");
    let help_proj = tmp.path().join("help-proj");
    fs::create_dir_all(&help_home).unwrap();
    fs::create_dir_all(&help_proj).unwrap();
    let disco = discoverable(&help_home, &help_proj, &clap_tree);

    let mut skips = Skips::default();
    // Seeded, not discovered. A structural limit that holds on EVERY run
    // belongs in the record on every run — otherwise the report can print a
    // short list while a known blind spot stands, which is the precise failure
    // the comment above `report` warns about. A guard may only claim the
    // coverage it can show.
    //
    // `Rung::Group` on `status` used to be seeded here as unreachable. It is
    // not: detection reads the harness's own config file, not PATH, so
    // `bridge-registered-group` now stands on it and the sweep judges the
    // payload directly. What remains unreachable is listed instead.
    for limit in [
        "guidance behind an interactive prompt — the `init` wizard, the TTY trust confirm, and \
         `agentstack yes` — is out of reach of a stdin-null spawn by construction. Every one of \
         those verbs is REACHED in this matrix and its non-interactive refusal IS harvested; what \
         is not covered is the wizard text a terminal would show. `docs_commands.rs` covers the \
         same class for documentation prose.",
        "`agentstack x gateway connect --all --write` is named as a fix by `doctor`, `apply`, \
         `use` and `delivery`, and it refuses under this file's spawn whenever no harness config \
         is present (`no installed harness with MCP support detected`). That refusal is a fact \
         about the ISOLATED MACHINE, not about the guidance, so it is not asserted as a defect. \
         The states that DO detect a harness (`harness-no-bridge`, `native-unimported`, \
         `bridge-registered-group`) are what put the branch under the rules at all.",
        "secret-resolution states (`${REF}` unresolved on this machine) need a keychain or a \
         project `.env` this file does not build, so the guidance the `Secrets` section emits is \
         not swept here. Pinned by the `secret` command's own tests.",
        "THE REMEDY-COLUMN SHAPE IS NOT HARVESTED, and this is a decision with a reason rather \
         than an oversight. `harvest_human` reads exactly three shapes: a backtick span, a `↳` \
         fix column, and a `Next:`/`next:` line. It does NOT read the aligned two-column remedy \
         an error prints — `  create one here:   agentstack init`, `  preview only (writes \
         nothing):  agentstack init --dry-run`, `  choose the secret store:  agentstack init \
         --secrets <env|keychain|skip>`. Widening to it was tried and rejected here: those lines \
         carry placeholders no single SENTINEL can satisfy — one stands in a SUBCOMMAND slot \
         (`agentstack --manifest-dir <dir> <command>`) and one is an enumerated VALUE \
         (`<env|keychain|skip>`), so rule (a) would fail on correct English and the pressure \
         would be to loosen rule (a) rather than to fix the harvest. Reading them needs a \
         per-argument sentinel derived from the clap tree, which is a change to the harvest \
         contract and not to this sweep. Until then: the remedy column of an error message is \
         GUIDANCE THIS FILE DOES NOT JUDGE.",
        "convergence (c) is judged one step at a time, never as a whole walk to health: \
         `agentstack trust .` needs a reviewed digest a human supplies and cannot be driven from \
         a stdin-null spawn. The end-to-end walk is \
         `tests/trust_content_drift.rs::a_json_only_driver_converges_from_drift_to_health`.",
    ] {
        skips.note(limit.to_string());
    }
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
    let mut reproduced: BTreeSet<(&str, &str, &str)> = BTreeSet::new();
    for h in harvested
        .iter()
        .filter(|h| h.audience == Audience::Machine && h.blocking)
    {
        if let Some(msg) = noop_violation(&clap_tree, h) {
            match known_defect(&h.state, &h.surface, &h.command) {
                Some(d) => {
                    reproduced.insert((d.state, d.surface, d.command));
                    skips.note(format!(
                        "LIVE DEFECT, on the ledger and queued — [{}] {} names `{}`\n      why: {}\
                         \n      (rule (b) fired; the assertion is held open by KNOWN_DEFECTS so \
                         the suite stays green and the bug stays visible. Every entry has its own \
                         reproducer at the bottom of this file.)",
                        d.state, d.surface, d.command, d.why
                    ));
                }
                None => noop_failures.push(msg),
            }
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
        let status_body = payloads.get(&(state.name, "status --json"));
        let preview_body = payloads.get(&(state.name, "trust --preview"));
        disagreements.extend(agreement_violations(
            state.name,
            payloads.get(&(state.name, "doctor --json")),
            status_body,
            preview_body,
            &mut skips,
        ));
        // Clause 4, separate because it is the one clause that can fire on the
        // ABSENCE of an answer, and because the `trust .` convergence exemption
        // means it is the only rule in this file that can reach the state it
        // judges.
        if let Some(msg) = consent_gate_blind(state.name, status_body, preview_body) {
            match known_defect(state.name, "trust --preview", CONSENT_GATE_BLIND) {
                Some(d) => {
                    reproduced.insert((d.state, d.surface, d.command));
                    skips.note(format!(
                        "LIVE DEFECT, on the ledger and queued — [{}] the consent gate cannot see \
                         the project\n      why: {}\n      (rule (d) clause 4 fired; held open by \
                         KNOWN_DEFECTS so the suite stays green and the bug stays visible. \
                         Reproducer: `the_consent_gate_cannot_see_a_machine_manifest`.){msg}",
                        d.state, d.why
                    ));
                }
                None => disagreements.push(msg),
            }
        }
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

    // ── (c) CONVERGES — EVERY STATE, AND NOW BOTH MACHINE SURFACES ────────
    //
    // Level (c) used to run for the two drift shapes and `declared-unpinned`
    // only — precisely the states that already worked. It then grew to every
    // state, but still read ONE surface: `doctor --json`.
    //
    // That was a hole with a defect already in it. `doctor --json` and
    // `status --json` do not always name the same command — rule (d) clause 1
    // explicitly declines to require that they do, because they legitimately
    // rank different rungs for different audiences. So a command that only
    // `status` names was executed by nothing, and `status` is the surface a
    // panel polls. In three states of this matrix `doctor` answers `null`
    // (terminal, nothing to run) while `status` hands a driver a command that
    // refuses; the surface that loops was the surface (c) never read.
    //
    // Both fields are therefore driven, each from a PRISTINE state. The status
    // pass gets its own freshly built matrix rather than the one `doctor` just
    // mutated, and rebuilt rather than copied: a grant is keyed by the
    // project's path, so a copied directory is an untrusted project wearing a
    // trusted one's name and every answer downstream of it would be a fixture
    // artefact.
    //
    // Detection is BY STATE, never by exit code: `orphan-body-missing` is the
    // state where the offered command exits 0, prints a green tick, and leaves
    // the blocking condition exactly where it was.
    //
    // This pass MUTATES, so it runs last.
    let mut loops: Vec<String> = Vec::new();
    for state in &states {
        if let Some(msg) = converge_once(
            state,
            "doctor --json",
            &["doctor", "--json"],
            "/next_action",
            &mut reproduced,
            &mut skips,
        ) {
            loops.push(msg);
        }
    }

    let status_root = tmp.path().join("converge-status");
    for spec in &specs {
        let state = spec.build_under(&status_root);
        if let Some(msg) = converge_once(
            &state,
            "status --json",
            &["status", "--json"],
            "/next_action/command",
            &mut reproduced,
            &mut skips,
        ) {
            loops.push(msg);
        }
    }
    assert!(
        loops.is_empty(),
        "{} state(s) hand a driver a command that does not move them:{}",
        loops.len(),
        loops.join("\n")
    );

    // ── THE LEDGER MAY NOT ROT ────────────────────────────────────────────
    //
    // Every entry in `KNOWN_DEFECTS` held an assertion open above. If one of
    // them no longer reproduces, the product was repaired and the entry is now
    // a suppression with nothing under it — which is exactly how a guard turns
    // into a formality. So a stale entry FAILS, and the failure says what to do
    // about it.
    let stale: Vec<String> = KNOWN_DEFECTS
        .iter()
        .filter(|d| !reproduced.contains(&(d.state, d.surface, d.command)))
        .map(|d| format!("\n  [{}] {} `{}`", d.state, d.surface, d.command))
        .collect();
    assert!(
        stale.is_empty(),
        "{} entry/entries in KNOWN_DEFECTS no longer reproduce:{}\n\n\
         Good news, and a required edit: the product was repaired, so DELETE these entries (and \
         their `#[ignore]`d reproducers). An entry that outlives its defect is a suppression with \
         nothing under it, and a ledger that is allowed to hold those is how an assertion \
         quietly stops asserting.",
        stale.len(),
        stale.join("")
    );

    report(&states, &per_state_surfaces, &harvested, &skips, &disco);
}

// ---------------------------------------------------------------------------
// (g) THE CLOSING NEXT STEP OF A WRITING COMMAND
// ---------------------------------------------------------------------------

/// The states the writing sweep runs in. Named, not "all", and the reason is
/// cost rather than taste: every pair here needs a PRISTINE project, because
/// the surface under test writes to it — so the sweep is
/// `states × surfaces` fixture builds, not `states + surfaces`. Six states are
/// chosen to cover one rung each on the way up the ladder; the run prints the
/// full grid, and the states NOT in it are named in the report.
const WRITING_SWEEP_STATES: &[&str] = &[
    "no-project",
    "declared-unpinned",
    "pinned-untrusted",
    "trusted-healthy",
    "native-unimported",
    "bridge-registered-group",
];

/// Every command a WRITING surface names on its way out must be runnable, must
/// be findable, and must not be the command that was just run.
///
/// WHY THIS IS A SEPARATE TEST. `SURFACES` sweeps what a project can be ASKED,
/// in every state, for one spawn each. `up`, `apply --write`, `use --write`,
/// `lock --write` and `adopt --write` cannot join it: each one changes the
/// project, so running one destroys the state the next surface would have been
/// read in. That is a real cost — a fresh fixture per (state, surface) pair —
/// and it is the reason these five were left out of the sweep for as long as
/// they were. "Expensive" is not the same as "covered", and the closing
/// `next:` line of a writing command is guidance a reader follows more often
/// than anything `doctor` prints: it is what they see the moment they finish a
/// step.
///
/// The three rules, and why each one is here:
///
/// * **(a) PARSES** — the same rule the read sweep applies, applied to strings
///   it never saw.
/// * **(e) IS DISCOVERABLE** — likewise. A closing step naming a command that
///   appears on no help screen strands the reader exactly where they were most
///   ready to continue.
/// * **(g) IS NOT ITSELF** — new, and specific to this sweep. A read-only
///   surface naming itself is merely useless; a writing surface naming itself
///   is the recurring fault in its purest form. The reader ran the command,
///   the command finished, and the command's own parting advice is to run the
///   command. Nothing in (a), (b) or (e) can see it: the string parses, it
///   writes, and it is perfectly discoverable — it is just the same step
///   again. Compared on normalized argv, so `agentstack x lock --write` and
///   `agentstack lock --write` count as the same command, which they are.
#[test]
fn every_closing_next_step_is_executable() {
    let tmp = tempfile::tempdir().unwrap();
    let clap_tree = agentstack::cli::Cli::command();
    let help_home = tmp.path().join("help-home");
    let help_proj = tmp.path().join("help-proj");
    fs::create_dir_all(&help_home).unwrap();
    fs::create_dir_all(&help_proj).unwrap();
    let disco = discoverable(&help_home, &help_proj, &clap_tree);
    let reach = disco.reachable();

    let specs = matrix();
    let mut skips = Skips::default();
    skips.note(
        "the aligned remedy column an error prints (`  create one here:   agentstack init`) is \
         not harvested by `harvest_human` and is therefore not judged here either — see the same \
         note, with the reason, in `every_suggested_command_parses_and_makes_progress`. It is why \
         `no-project` shows so few commands below: those surfaces DO name a next step, in a shape \
         this file cannot yet read without loosening rule (a)."
            .to_string(),
    );
    let swept: BTreeSet<&str> = WRITING_SWEEP_STATES.iter().copied().collect();
    for spec in &specs {
        if !swept.contains(spec.name) {
            skips.note(format!(
                "[{}] not in the writing sweep — every pair here costs a fresh project build, so \
                 the grid is deliberately six states wide. This state IS swept by the read \
                 surfaces in `every_suggested_command_parses_and_makes_progress`; what is not \
                 covered for it is the closing step of `up`/`apply --write`/`use --write`/\
                 `lock --write`/`adopt --write` specifically.",
                spec.name
            ));
        }
    }

    let mut failures: Vec<String> = Vec::new();
    let mut grid: Vec<String> = Vec::new();
    let mut total = 0usize;

    for spec in specs.iter().filter(|s| swept.contains(s.name)) {
        for (label, argv) in WRITING_SURFACES {
            // A pristine project per pair. Rebuilt, never copied: a grant is
            // keyed by the project's path, so `cp -r` would hand this sweep an
            // untrusted project wearing a trusted one's name and every closing
            // step read out of it would be a fixture artefact.
            let root = tmp
                .path()
                .join(format!("{}--{}", spec.name, label.replace(' ', "_")));
            fs::create_dir_all(&root).unwrap();
            let state = spec.build_under(&root);
            let out = run(argv, &state.home, &state.proj);
            let text = strip_ansi(&out.text);
            let found = harvest_human(&text);
            grid.push(format!(
                "  {:<24} {:<14} exit_ok={:<5} {} command(s) named",
                spec.name,
                label,
                out.ok,
                found.len()
            ));

            for (line, command) in found {
                total += 1;
                let toks = as_argv(&tokens_of(&command));

                // (a) PARSES.
                let mut parse_argv: Vec<String> = vec!["agentstack".to_string()];
                for tok in toks.iter().skip(1) {
                    parse_argv.push(if is_placeholder(tok) {
                        SENTINEL.to_string()
                    } else {
                        tok.clone()
                    });
                }
                if let Err(e) = agentstack::cli::Cli::try_parse_from(&parse_argv) {
                    failures.push(format!(
                        "\n  state    : {}\n  surface  : agentstack {}\n  line     : {line}\n  \
                         command  : `{command}`\n  why      : (a) does not parse against the clap \
                         tree\n  clap says: {}",
                        spec.name,
                        argv.join(" "),
                        e.to_string().lines().next().unwrap_or("").trim(),
                    ));
                }

                // (e) IS DISCOVERABLE.
                let h = Harvested {
                    state: spec.name.to_string(),
                    surface: format!("agentstack {}", argv.join(" ")),
                    origin: line.clone(),
                    audience: Audience::Human,
                    command: command.clone(),
                    blocking: !out.ok,
                };
                match toks.get(1) {
                    Some(verb) if is_ident(verb) => {
                        if let Some(msg) = discoverability_violation(&h, &reach) {
                            failures.push(msg);
                        }
                    }
                    _ => skips.note(format!(
                        "[{}] {label}: rule (e) did not judge `{command}` — its first argument is \
                         not a command name.",
                        spec.name
                    )),
                }

                // (g) IS NOT ITSELF.
                if let Some(msg) = self_loop_violation(spec.name, argv, &command) {
                    failures.push(msg);
                }
            }
        }
    }

    let mut s = String::new();
    let _ = writeln!(s, "\n── (g): closing next steps of the writing surfaces ──");
    for line in &grid {
        let _ = writeln!(s, "{line}");
    }
    let _ = writeln!(
        s,
        "{} (state, writing surface) pair(s), {total} command(s) judged by (a), (e) and (g)",
        grid.len()
    );
    let _ = writeln!(s, "NOT COVERED ({}):", skips.0.len());
    for k in &skips.0 {
        let _ = writeln!(s, "  {k}");
    }
    println!("{s}");

    assert!(
        !grid.is_empty(),
        "the writing sweep ran no pairs at all — the state names in WRITING_SWEEP_STATES no \
         longer match the matrix, not a product problem"
    );
    assert!(
        failures.is_empty(),
        "{} closing next step(s) of a writing command are unrunnable, unfindable, or a repeat of \
         the command that just ran:{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Rule (g), as one function so the guard can be pointed at itself (see
/// `the_guard_catches_a_surface_that_names_itself`).
///
/// `None` means the named command is a different step from the one that was
/// just run. Comparison is on NORMALIZED argv — `agentstack x lock --write`
/// and `agentstack lock --write` are one command and must count as one — and
/// on the whole argv rather than the verb, because `lock` naming `lock
/// --write` is real progress and flagging it would make the rule useless.
fn self_loop_violation(state: &str, surface_argv: &[&str], command: &str) -> Option<String> {
    let named: Vec<String> = as_argv(&tokens_of(command)).into_iter().skip(1).collect();
    let ran: Vec<String> = as_argv(
        &std::iter::once("agentstack".to_string())
            .chain(surface_argv.iter().map(|s| (*s).to_string()))
            .collect::<Vec<_>>(),
    )
    .into_iter()
    .skip(1)
    .collect();
    if named != ran || named.is_empty() {
        return None;
    }
    Some(format!(
        "\n  state    : {state}\n  surface  : `agentstack {}`\n  command  : `{command}`\n  \
         why      : (g) the command names ITSELF as the next step. The reader ran it, it \
         finished, and its parting advice is to run it again. Nothing in (a), (b) or (e) can see \
         this — the string parses, it writes, and it is discoverable; it is simply the same step \
         a second time. Name the step that actually follows, or say the work is done.",
        surface_argv.join(" "),
    ))
}

/// Rule (g), pointed at itself — two-sided, like every other rule in this file.
///
/// A rule that fires on everything is as useless as one that fires on nothing,
/// and (g) has an obvious way to become the first: `lock` naming `lock --write`
/// is real progress and must pass, while `lock --write` naming `lock --write`
/// is the dead end. The namespaced spelling of the same command must count as
/// the same command, or hiding a loop behind `agentstack x` would switch the
/// rule off.
#[test]
fn the_guard_catches_a_surface_that_names_itself() {
    let msg = self_loop_violation(
        "declared-unpinned",
        &["lock", "--write"],
        "agentstack lock --write",
    )
    .expect("a writing command naming itself must be flagged");
    assert!(
        msg.contains("names ITSELF"),
        "the failure must say what is wrong:\n{msg}"
    );
    println!("guard self-check — a surface that names itself:{msg}");

    // The namespaced spelling is the SAME command.
    assert!(
        self_loop_violation("s", &["lock", "--write"], "agentstack x lock --write").is_some(),
        "`agentstack x lock --write` and `agentstack lock --write` are one command; counting them \
         as two would let a loop hide behind the namespace"
    );

    // …and the shapes that are real progress must pass.
    for (surface, named) in [
        (&["lock"][..], "agentstack lock --write"),
        (&["lock", "--write"][..], "agentstack trust ."),
        (&["apply", "--write"][..], "agentstack use --write"),
        (&["up"][..], "agentstack lock --write"),
    ] {
        assert!(
            self_loop_violation("s", surface, named).is_none(),
            "`{named}` after `agentstack {}` is a DIFFERENT step and must pass, or rule (g) would \
             fire on correct guidance and be switched off",
            surface.join(" ")
        );
    }
}

// ---------------------------------------------------------------------------
// The live defects, each as its own reproducer
// ---------------------------------------------------------------------------
//
// One `#[ignore]`d test per entry in `KNOWN_DEFECTS`. They are ignored so the
// suite stays green while the bugs are queued, and they exist so the bugs stay
// visible: `cargo test -p agentstack --test guidance_is_executable -- --ignored`
// reproduces each one on demand, against the real binary, with the exit code
// read directly rather than through a pipe.

/// G33, CLOSED — an UNADOPTED directory answers a driver with an explicit
/// `null`, and tells the PERSON what to do in prose.
///
/// The defect: both machine fields read `agentstack init`, and `init` refuses
/// without a terminal ("refusing to init without a terminal"). A machine field
/// is executed verbatim by a driver, a driver has no terminal, so it errored,
/// re-polled, and read the identical field forever.
///
/// **Why `null` and not a runnable spelling.** Three exist and each is worse
/// than nothing, which is why this is a repair and not a rewrite of the string:
///
/// * `init --dry-run` writes nothing — the exit-0 poll-and-run loop this file
///   already caught with `agentstack search`.
/// * `init --secrets <env|keychain|skip>` is a placeholder, forbidden in a
///   machine field by rule (a') and by `machine_command`'s angle-bracket rule.
/// * `init --yes` runs, and is exactly what must NOT be offered here. `init`'s
///   own refusal says why: a flagless init "imports your CLI configs and can
///   lift live token values into files, so it never runs without a prompt or an
///   explicit flag". Putting that in a field a panel executes would route a
///   driver around the wall the product just built, in every directory the
///   panel is pointed at.
///
/// So the honest answer is that no machine can take this step. `null` is how
/// this contract says that — `converge_once` below calls it "the terminal
/// answer, and a driver that reads it stops", and `Rung::Group` and
/// `Rung::Verified` already emit it — and the human sentence still names `init`.
///
/// Asserted two-sided on purpose: the machine field is null, the human sentence
/// is present and names the verb, and `init` still refuses (so this stays a
/// test about GUIDANCE and not an accidental claim that `init` became headless).
#[test]
fn an_unadopted_directory_answers_a_driver_with_null_and_a_person_with_prose() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("no-project");
    fs::create_dir_all(&dir).unwrap();
    let (home, proj) = no_project(&dir);

    for (surface, argv, cmd_ptr, prose_ptr) in [
        (
            "doctor --json",
            &["doctor", "--json"][..],
            "/next_action",
            "/next_step",
        ),
        (
            "status --json",
            &["status", "--json"][..],
            "/next_action/command",
            "/next_action/sentence",
        ),
    ] {
        let v: serde_json::Value =
            serde_json::from_str(&strip_ansi(&run(argv, &home, &proj).text)).unwrap();

        // The seam, exactly as `status_honesty.rs` pins it: the key is PRESENT
        // and its value is an explicit null. Never missing, never `""`.
        let machine = v
            .pointer(cmd_ptr)
            .unwrap_or_else(|| panic!("{surface} {cmd_ptr} must be present, not missing:\n{v:#}"));
        assert!(
            machine.is_null(),
            "{surface} {cmd_ptr} must be an explicit null — a driver has no terminal, and every \
             runnable spelling of `init` is either a no-op preview, a placeholder, or an import \
             that lifts live token values without a prompt. Got {machine}"
        );

        // …and the person is not left with nothing.
        let prose = v
            .pointer(prose_ptr)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| {
                panic!("{surface} {prose_ptr} must carry the human sentence:\n{v:#}")
            });
        assert!(
            prose.contains("agentstack init"),
            "{surface} {prose_ptr} must still tell the reader how to adopt this directory, or the \
             null is a dead end rather than a terminal answer: {prose:?}"
        );
    }

    // The premise this repair rests on, re-measured rather than assumed: `init`
    // still refuses without a terminal. If it ever stops refusing, the reasoning
    // above changes and this test should be revisited, not quietly kept green.
    let out = run(&["init"], &home, &proj);
    assert!(
        !out.ok,
        "`agentstack init` no longer refuses without a terminal — the premise of the null moved:\n{}",
        out.text
    );
}

/// G34, CLOSED — a waiting drop hands a driver the INERT half of the work, and
/// keeps the one-confirmation funnel for the person.
///
/// The defect: `status --json` answered `agentstack yes` for every project with
/// a dropped-but-undeclared file (the `undeclared_drops` arm, which outranks
/// every other rung). `yes` is an interactive verb by design and refuses
/// headlessly — "`agentstack yes` needs a terminal — it is a review you read and
/// answer" — so the surface a panel polls handed it a dead end. `doctor --json`
/// answered `null` for the same project, which is why only a check that reads
/// `status` could see it.
///
/// **Why a command and not `null` here.** `null` means "there is nothing to
/// run", and there is: `adopt --write` DECLARES the dropped file and does
/// nothing else. It grants nothing, pins nothing, delivers nothing, and the
/// trust gate still stands between it and any harness — so no consent ceremony
/// is handed to a driver, which is the line that must not be crossed. It is
/// also the first step of the headless path `yes`'s own refusal prints, so the
/// machine field now agrees with what the product already says in words.
///
/// Asserted over all three drop states in the matrix, and by state rather than
/// by exit code: the drop must actually clear.
#[test]
fn a_waiting_drop_hands_a_driver_the_inert_declare_and_not_the_ceremony() {
    let tmp = tempfile::tempdir().unwrap();

    for name in ["empty-manifest", "servers-no-bridge", "dropped-undeclared"] {
        let spec = matrix()
            .into_iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("the matrix must still carry `{name}`"));
        let root = tmp.path().join(name);
        fs::create_dir_all(&root).unwrap();
        let state = spec.build_under(&root);
        let (home, proj) = (state.home, state.proj);

        let v: serde_json::Value =
            serde_json::from_str(&strip_ansi(&run(&["status", "--json"], &home, &proj).text))
                .unwrap();
        assert!(
            !v["intake"].as_array().unwrap().is_empty(),
            "[{name}] fixture: a drop must be waiting, or this is not the state under test:\n{v:#}"
        );
        let cmd = v
            .pointer("/next_action/command")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| panic!("[{name}] status names no command at all:\n{v:#}"));
        assert_eq!(
            cmd, "agentstack adopt --write",
            "[{name}] the drop rung must name the inert declare a driver can run, never the \
             terminal-only funnel"
        );
        assert!(
            v.pointer("/next_action/why")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|w| w.contains("agentstack yes")),
            "[{name}] the funnel must survive for the PERSON, in the `why` where no driver is \
             invited to run it:\n{v:#}"
        );

        // (c) for this rung, measured by state: run it verbatim and the drop is
        // gone. An exit code would not prove it — that is the whole reason the
        // convergence sweep compares observable state.
        let out = run(&["adopt", "--write"], &home, &proj);
        let after: serde_json::Value =
            serde_json::from_str(&strip_ansi(&run(&["status", "--json"], &home, &proj).text))
                .unwrap();
        assert!(
            after["intake"].as_array().unwrap().is_empty(),
            "[{name}] `agentstack adopt --write` left the drop waiting (exit_ok={}), so the rung \
             names itself forever:\n{}",
            out.ok,
            out.text.trim()
        );
    }
}

/// G35, CLOSED — the adopt rung names the WRITING form.
///
/// The defect was the `agentstack lock` regression alive again in a second
/// verb: `adopt` previews by default and declares `--write`, and both machine
/// fields named the bare spelling. Running it exited **0**, printed the manifest
/// diff it WOULD apply, wrote nothing, and the identical command came back on
/// the next poll — an exit-0 infinite loop, the worse of the two shapes because
/// nothing in the output says it failed.
///
/// Asserted by state, never by exit code, for exactly that reason.
#[test]
fn the_adopt_rung_names_the_writing_form() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("native-unimported");
    fs::create_dir_all(&dir).unwrap();
    let (home, proj) = write_project_with_skill(&dir, FULL_MANIFEST, SKILL_DESCRIBED);
    detect_claude_code(&home, true);
    lock(&home, &proj);
    grant(&home, &proj);
    fs::write(proj.join(".mcp.json"), FOREIGN_MCP_JSON).unwrap();

    let tree = agentstack::cli::Cli::command();
    assert!(
        takes_write_flag(&tree, &["adopt".to_string()]),
        "`adopt` must declare `--write`, or this is not the preview-for-a-write shape"
    );

    let before = strip_ansi(&run(&["doctor", "--json"], &home, &proj).text);
    let bv: serde_json::Value = serde_json::from_str(&before).unwrap();
    assert_eq!(
        bv["next_action"].as_str(),
        Some("agentstack adopt --write"),
        "doctor's machine field must name the form that writes"
    );
    let sv: serde_json::Value =
        serde_json::from_str(&strip_ansi(&run(&["status", "--json"], &home, &proj).text)).unwrap();
    assert_eq!(
        sv.pointer("/next_action/command")
            .and_then(serde_json::Value::as_str),
        Some("agentstack adopt --write"),
        "…and so must the surface a panel polls"
    );

    // Run it verbatim and the project must MOVE. Compared on observable state
    // because the defect's whole signature was an exit code of 0.
    let out = run(&["adopt", "--write"], &home, &proj);
    let after = strip_ansi(&run(&["doctor", "--json"], &home, &proj).text);
    let av: serde_json::Value = serde_json::from_str(&after).unwrap();
    assert_ne!(
        observable(&bv),
        observable(&av),
        "the offered command left every observable field identical (exit_ok={}), which is the \
         exit-0 loop this rung was repaired to remove:\n{}",
        out.ok,
        out.text.trim()
    );
}

/// LIVE DEFECT — the consent gate cannot see a MACHINE manifest, and the
/// ladder that points at it therefore never ends.
///
/// State: the manifest at `~/.agentstack/agentstack.toml`, working directory
/// `~` — the machine scope, which every other surface reads correctly.
/// What happens, step by step, each exit code read from the process directly:
///   1. `status --json` → `manifest.loaded = true`, `next_action.command =
///      "agentstack lock --write"`.
///   2. `agentstack lock --write` → exit 0, pins the surface.
///   3. `status --json` → `next_action.command = "agentstack trust ."`.
///   4. `agentstack trust .` → **exit 1**, "no agentstack manifest at or above
///      ~ — run `agentstack init` first".
///   5. re-poll → step 3 again, forever.
///
/// `agentstack trust --preview` and bare `agentstack trust` refuse identically,
/// so no spelling of the gate can see the project. The `agentstack init` the
/// refusal suggests is wrong twice over: a manifest exists, and `init` refuses
/// without a terminal.
#[test]
#[ignore = "live defect: `trust` cannot find a machine manifest that status and doctor both read"]
fn the_consent_gate_cannot_see_a_machine_manifest() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("machine-manifest");
    fs::create_dir_all(&dir).unwrap();
    let (home, proj) = machine_manifest(&dir);

    let status: serde_json::Value =
        serde_json::from_str(&strip_ansi(&run(&["status", "--json"], &home, &proj).text)).unwrap();
    assert_eq!(
        status.pointer("/manifest/loaded"),
        Some(&serde_json::Value::Bool(true)),
        "fixture: `status` must load the machine manifest, or this is not the state under test"
    );

    let pinned = run(&["lock", "--write"], &home, &proj);
    assert!(pinned.ok, "`lock --write` should pin:\n{}", pinned.text);

    let status: serde_json::Value =
        serde_json::from_str(&strip_ansi(&run(&["status", "--json"], &home, &proj).text)).unwrap();
    assert_eq!(
        status
            .pointer("/next_action/command")
            .and_then(serde_json::Value::as_str),
        Some("agentstack trust ."),
        "the ladder must point at the gate, or this is not the state under test"
    );

    for spelling in [
        &["trust", "."][..],
        &["trust", "--preview"][..],
        &["trust"][..],
    ] {
        let out = run(spelling, &home, &proj);
        assert!(
            !out.ok,
            "the defect is that every spelling of the gate REFUSES here; if `agentstack {}` now \
             works, delete this test and the KNOWN_DEFECTS entry",
            spelling.join(" ")
        );
        assert!(
            out.text.contains("no agentstack manifest"),
            "expected the gate to deny the manifest exists:\n{}",
            out.text
        );
    }
    panic!(
        "reproduced: `status` names `agentstack trust .` over a manifest the gate says is not \
         there. Loop with no exit."
    );
}

/// Rule (d) clause 4, pointed at itself — two-sided.
///
/// The direction that matters most is the negative one: a directory with no
/// manifest is a legitimate state whose consent gate legitimately has nothing
/// to preview, and a clause that flagged it would fire on every unadopted
/// project and be switched off within a week.
#[test]
fn the_guard_catches_a_consent_gate_that_cannot_see_the_project() {
    let loaded = serde_json::json!({
        "manifest": { "path": "/home/u/.agentstack/agentstack.toml", "loaded": true }
    });
    let msg = consent_gate_blind("machine-manifest", Some(&loaded), None)
        .expect("a loaded manifest with no preview at all must be flagged");
    assert!(
        msg.contains("the consent gate cannot find it")
            && msg.contains("/home/u/.agentstack/agentstack.toml"),
        "the failure must say what is wrong and name the manifest the gate cannot see:\n{msg}"
    );
    println!("guard self-check — a consent gate blind to its own project:{msg}");

    // A preview that answered is the corrected shape.
    assert!(
        consent_gate_blind(
            "s",
            Some(&loaded),
            Some(&serde_json::json!({ "fix": null }))
        )
        .is_none(),
        "a gate that answers must pass"
    );
    // No manifest is not a defect — it is a state with its own guidance.
    let absent =
        serde_json::json!({ "manifest": { "path": "/p/agentstack.toml", "loaded": false } });
    assert!(
        consent_gate_blind("no-project", Some(&absent), None).is_none(),
        "a directory with no manifest has nothing for the gate to preview; flagging it would make \
         the clause fire on every unadopted project"
    );
    assert!(
        consent_gate_blind("s", None, None).is_none(),
        "no reading at all is not a claim, and must not be judged as one"
    );
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
/// 4. **The consent gate must be able to SEE the project the other surfaces
///    describe.** Where `status --json` reports `manifest.loaded = true`,
///    `trust --preview` must answer at all. This clause is not about two
///    commands disagreeing; it is about one surface reporting on a project
///    while the gate that governs it says the project does not exist — and it
///    is stated separately because nothing else in this file can reach it.
///    Every ladder in the product routes through `agentstack trust .`, and
///    that command is EXEMPT from convergence (c): it needs a reviewed digest
///    a stdin-null spawn cannot supply. So a state where `trust` cannot find
///    the manifest is a state where `status` names `trust .` forever, (c) is
///    forbidden from executing it, and the loop is invisible. The exemption
///    was load-bearing for a real defect, which is why the READ-ONLY half of
///    the same command is now required to work wherever the ladder points at
///    it. `status`'s own `manifest.loaded` is the condition, so a project that
///    genuinely has no manifest is not flagged.
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

/// Rule (d) clause 4, as one function so the guard can be pointed at itself and
/// so the call site can consult the defect ledger before it fails the run.
///
/// `None` means the consent gate can see whatever `status` says is there.
///
/// The condition is `status`'s OWN reading — `manifest.loaded` — so a directory
/// that genuinely has no manifest cannot trip this clause: the flag is false
/// there and the rule says nothing. That matters, because "no manifest" is a
/// legitimate state with its own guidance and flagging it would make the rule
/// fire on correct behaviour.
fn consent_gate_blind(
    state: &str,
    status: Option<&serde_json::Value>,
    preview: Option<&serde_json::Value>,
) -> Option<String> {
    let loaded = status
        .and_then(|v| v.pointer("/manifest/loaded"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if !loaded || preview.is_some() {
        return None;
    }
    Some(format!(
        "\n  state    : {state}\n  surface A: status --json /manifest/loaded = true, path \
         {:?}\n  surface B: trust --preview — no JSON at all\n  \
         why      : one project, and the consent gate cannot find it. `status` and `doctor` \
         report on this manifest and walk the reader toward `agentstack trust .`; `trust` answers \
         that there is no manifest here. Every ladder in the product routes through that command, \
         so the reader is sent to a gate that refuses to look at the project it governs, and the \
         ladder never terminates. Convergence (c) is structurally unable to catch this — \
         `agentstack trust .` is exempt from it, because a grant needs a reviewed digest a \
         stdin-null spawn cannot supply — which is why the READ-ONLY half of the same command is \
         required to answer wherever the ladder points at it.",
        status
            .and_then(|v| v.pointer("/manifest/path"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("?"),
    ))
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

/// Execute one machine field verbatim and require the state to move.
///
/// `surface`/`argv`/`ptr` name WHICH machine field is being driven —
/// `doctor --json` `/next_action`, or `status --json`
/// `/next_action/command`. The two are separate contracts on separate surfaces
/// and they do not always carry the same string, so parameterizing is what
/// stops the check silently covering only whichever one it was written against.
///
/// Progress is always measured through `doctor --json`, whichever surface was
/// driven: it is the surface whose job is to say whether anything is wrong, and
/// using one yardstick for both keeps "moved" comparable.
///
/// Returns the failure message, or `None` when the state converged, was
/// terminal (a `null` machine field: there is nothing to execute and nothing
/// to loop on), or was recorded as a skip.
fn converge_once(
    state: &State,
    surface: &str,
    argv: &[&str],
    ptr: &str,
    reproduced: &mut BTreeSet<(&'static str, &'static str, &'static str)>,
    skips: &mut Skips,
) -> Option<String> {
    let field: serde_json::Value =
        serde_json::from_str(&strip_ansi(&run(argv, &state.home, &state.proj).text)).ok()?;
    let before: serde_json::Value = serde_json::from_str(&strip_ansi(
        &run(&["doctor", "--json"], &state.home, &state.proj).text,
    ))
    .ok()?;
    let Some(fix) = field
        .pointer(ptr)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
    else {
        // Not a gap: `null` is the terminal answer, and a driver that reads it
        // stops. There is no command to execute, so there is nothing for a
        // convergence check to say. Rule (d) is what guards this state, and it
        // ran above.
        skips.note(format!(
            "[{}] convergence (c) on {surface} {ptr}: null — terminal by contract, so there is no \
             command to execute and nothing for (c) to say. What holds this state is rule (d) — \
             which, where the trust rung is terminal, forbids ANY surface naming a trust-rung \
             command over it — together with (a0)/(a)/(a')/(b) on every other string the state \
             emits.",
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
            "[{}] convergence (c) on {surface}: the offered fix is `agentstack trust .`, which \
             needs a reviewed digest and cannot be driven from a stdin-null spawn. Asserted end \
             to end instead by `tests/trust_content_drift.rs`.",
            state.name
        ));
        return None;
    }

    let snapshot_before = observable(&before);
    let fix_argv: Vec<&str> = fix.split_whitespace().skip(1).collect();
    let out = run(&fix_argv, &state.home, &state.proj);
    let after: serde_json::Value = serde_json::from_str(&strip_ansi(
        &run(&["doctor", "--json"], &state.home, &state.proj).text,
    ))
    .ok()?;
    let snapshot_after = observable(&after);
    if snapshot_before != snapshot_after {
        return None;
    }
    let msg = format!(
        "\n  state    : {}\n  surface  : {surface} {ptr}\n  command  : `{fix}`\n  exit_ok  : {}\n  before   : {snapshot_before}\n  \
         after    : {snapshot_after}\n  \
         why      : {surface} named this as the ONE thing to run; running it verbatim left \
         every observable field identical, and the SAME command is named again. That is a \
         reproduced infinite loop. Note the exit status: a command may succeed and still change \
         nothing, which is why this check compares state and never exit codes.\n\n\
         Output of the offered command:\n{}",
        state.name,
        out.ok,
        out.text.trim()
    );
    // A defect already on the ledger holds this assertion open — loudly. See
    // `KNOWN_DEFECTS`: the entry must still reproduce or the run fails, so this
    // is a record of a live bug, never a way to stop looking at it.
    match known_defect(state.name, surface, &fix) {
        Some(d) => {
            reproduced.insert((d.state, d.surface, d.command));
            skips.note(format!(
                "LIVE DEFECT, on the ledger and queued — [{}] {surface} names `{}`\n      why: {}\
                 \n      (rule (c) reproduced the loop below; held open by KNOWN_DEFECTS so the \
                 suite stays green and the bug stays visible.){msg}",
                d.state, d.command, d.why
            ));
            None
        }
        None => Some(msg),
    }
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

// ---------------------------------------------------------------------------
// (f) DELIVERY CLAIMS MUST AGREE, AND MUST MATCH THE DISK
// ---------------------------------------------------------------------------
//
// WHY THIS RULE EXISTS. Rules (a)–(e) judge the COMMANDS a surface names:
// every one parses, writes where a write is required, carries no placeholder,
// and can be found from `--help`. Not one of them looks at what a surface
// CLAIMS ABOUT DELIVERY — whether a capability is served live, whether a file
// was written, whether anything is on disk at all. So the guard passed green
// through three separate findings of one shape, the shape
// `crates/cli/src/commands/delivery.rs` already warns about in prose:
//
//     **a delivery claim computed from `delivery::Plan` alone**, without the
//     per-harness bridge reading and without looking at the disk.
//
// `Plan` describes ROUTING. It knows which lane a capability would travel. It
// does not know whether a bridge is registered (so "served live" may be false)
// and it does not know what is on disk (so "0 project artifacts" may be false).
// A claim derived from it alone is a claim the product cannot back — invariant
// 8, "claims match enforcement", one level up from enforcement into reporting.
//
// The three clauses, each stated as the defect it would have caught:
//
// 1. NO CONTRADICTION. Two surfaces describing ONE project may not make
//    opposite claims about the same harness: one "served live" while another
//    says "planned live (not connected)".
// 2. CLAIMS MATCH THE FILESYSTEM. This is the clause that would have caught
//    finding 1. After each state is built the project is WALKED, and the file
//    list is derived from that walk — never from a set written down here, which
//    would rot the moment a new adapter landed. If a server config is on disk,
//    no surface may say there are no project artifacts. If none is, no surface
//    may name one as written.
// 3. "LIVE" REQUIRES A BRIDGE. A harness with no bridge registered is not being
//    served. `planned live (not connected)` is the product's own correct
//    wording for that state and it must be used consistently.
//
// The four states below are the ones the (a)–(e) matrix does not have, and
// their absence is why the class escaped: the reviewer's exact `use --write`
// reproduction, a config left behind by an earlier render, a live route WITH a
// bridge, and `render_locally` set so that files are expected and their ABSENCE
// would be the defect.

/// Servers and a toolset, nothing else: the default routing sends MCP servers
/// down the live lane for every MCP-capable harness, so this manifest IS the
/// live-routed shape with no override anywhere.
const LIVE_SERVERS_MANIFEST: &str = r#"version = 1

[servers.filesystem]
type = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "."]

[toolsets.dev]
servers = ["filesystem"]
"#;

/// The same project with `[delivery] render_locally` set. Files are EXPECTED
/// here, which is the direction clause 2 cannot otherwise test: in every other
/// state a missing file is correct, so a rule that only ever checked "no file"
/// would pass by doing nothing.
const RENDER_LOCALLY_MANIFEST: &str = r#"version = 1

[delivery]
render_locally = true

[servers.filesystem]
type = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "."]

[toolsets.dev]
servers = ["filesystem"]
"#;

/// The one server every delivery state declares. Used to recognise a server
/// artifact on disk BY ITS CONTENT rather than by a table of file names.
const DECLARED_SERVER: &str = "filesystem";

/// A bridge entry in a harness's own global config, written by the fixture.
///
/// This is what `overview::bridge_registered` reads: the harness config file
/// exists (so the harness is *detected* under an isolated HOME) and carries an
/// `agentstack` entry at the descriptor's MCP location. It is written directly
/// rather than through `agentstack x gateway connect`, which refuses under this
/// file's `env_clear` spawn — see the skip record.
const CLAUDE_BRIDGE_CONFIG: &str =
    r#"{"mcpServers":{"agentstack":{"type":"stdio","command":"agentstack","args":["gateway"]}}}"#;

/// The display name of the one harness whose bridge this file can register.
const BRIDGED_HARNESS: &str = "Claude Code";

struct DeliveryState {
    name: &'static str,
    home: PathBuf,
    proj: PathBuf,
    /// Did the FIXTURE register a bridge for [`BRIDGED_HARNESS`]? Confirmed
    /// against the harness config on disk before it is used, so the ground
    /// truth is a filesystem reading and never a surface's own word.
    bridged: bool,
}

/// Every file under `dir`, walked. Symlinks are recorded but not followed —
/// a rendered skill is a symlink, and following it would leave the project.
fn walk_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.is_dir() {
            walk_files(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// The server-config files a project actually has, DERIVED BY WALKING IT.
///
/// Deliberately not a list of known adapter file names. A hardcoded set is the
/// same mistake as a delivery claim computed from `Plan` alone: it describes
/// what the test expects rather than what is there, and it goes stale on the
/// day an adapter is added. A file outside `.agentstack/` whose bytes name a
/// server this project declares is a server artifact, whichever adapter wrote
/// it and whatever it is called.
fn server_artifacts(proj: &Path) -> Vec<PathBuf> {
    let mut all = Vec::new();
    walk_files(proj, &mut all);
    all.retain(|p| {
        !p.starts_with(proj.join(".agentstack"))
            && fs::read_to_string(p).is_ok_and(|t| t.contains(DECLARED_SERVER))
    });
    all.sort();
    all
}

/// Is a bridge registered on disk for [`BRIDGED_HARNESS`]?
///
/// Read from the harness's own config file, which is where the product reads
/// it. Independent of every surface under test, so it can serve as ground
/// truth for clause 3.
fn bridge_on_disk(home: &Path) -> bool {
    fs::read_to_string(home.join(".claude.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .is_some_and(|v| v.pointer("/mcpServers/agentstack").is_some())
}

/// One delivery claim a surface made, with everything needed to judge it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Claim {
    /// "served live" — a present-tense claim that the capability is reaching
    /// the harness right now.
    ServedLive,
    /// "planned live (not connected)" — the honest form of the same routing.
    PlannedLive,
    /// "0 project artifacts" / "no project artifacts".
    NoArtifacts,
    /// A path named as being on disk.
    OnDisk(String),
}

#[derive(Debug, Clone)]
struct Said {
    surface: String,
    /// The line, or the JSON pointer, the claim was read from.
    origin: String,
    /// The harness the claim is ABOUT, when it names one.
    harness: Option<String>,
    claim: Claim,
    /// The text verbatim, so a failure quotes what the product really said.
    text: String,
}

/// The zero-artifacts phrase, READ OFF THE PRODUCT'S OWN CONSTANT.
///
/// `agentstack::delivery::ZERO_ARTIFACTS` is the sentence every surface prints
/// for that state. A copy of its wording here is a claim computed from a
/// RECORD of the product instead of from the product: reword the constant and
/// a hardcoded copy silently stops matching, clause 2 goes quiet, and the run
/// still passes on other claims — the guard would report coverage it no longer
/// has. So the phrase is derived: the constant's leading clause, up to the
/// qualifier, which is the part any surface prints verbatim.
fn zero_artifacts_phrase() -> &'static str {
    agentstack::delivery::ZERO_ARTIFACTS
        .split(" for the")
        .next()
        .expect("split always yields a first element")
        .trim()
}

/// The claims one line of surface output carries.
///
/// Order matters in one place: `planned live (not connected)` contains the word
/// "live" and must be recognised as the QUALIFIED form before the unqualified
/// one, or the correct wording would be flagged as the defect.
fn claims_in(line: &str) -> Vec<Claim> {
    let mut out = Vec::new();
    if line.contains("planned live") {
        out.push(Claim::PlannedLive);
    } else if line.contains("served live") || line.contains("serving live") {
        out.push(Claim::ServedLive);
    }
    // The product's phrase, plus the negated prose form the same sentence is
    // written in elsewhere ("no project artifacts").
    let zero = zero_artifacts_phrase();
    if line.contains(zero) || line.contains(&zero.replacen('0', "no", 1)) {
        out.push(Claim::NoArtifacts);
    }
    if line.contains("on disk") || line.contains("still on disk") {
        for tok in line.split_whitespace() {
            let tok = tok.trim_matches(|c: char| !c.is_ascii_graphic() || "().,;:`\"'".contains(c));
            if tok.len() > 1 && tok.starts_with('/') {
                out.push(Claim::OnDisk(tok.to_string()));
            }
        }
    }
    out
}

/// Which harnesses a line is about. Empty means a project-wide statement.
///
/// EVERY name on the line, not just one: the product legitimately prints a
/// single claim over a list of twelve harnesses, and attributing it to one of
/// them would leave eleven claims unjudged — a silent narrowing exactly where
/// the class lives.
///
/// The display names are read off `delivery --json` on every run, so a new
/// adapter is covered the day it lands and no name is written down here. Two
/// tools sharing a prefix are safe: "Claude Desktop" does not contain the
/// string "Claude Code", so each is matched only by its own full name.
fn harnesses_named(line: &str, displays: &[String]) -> Vec<String> {
    displays
        .iter()
        .filter(|d| contains_word(line, d))
        .cloned()
        .collect()
}

/// Does `line` contain `name` as a whole word (not inside a longer word)?
///
/// A bare substring test matched the display name "Pi" inside "Pinned", so an
/// unrelated line was attributed to a harness — and, worse, a JSON string
/// carrying an enclosing `display` subject had that correct subject DISCARDED
/// in favour of the false match. The neighbours are checked instead: a match
/// counts only where it is not glued to an alphanumeric character on either
/// side.
fn contains_word(line: &str, name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let glue = |c: char| c.is_alphanumeric();
    line.match_indices(name).any(|(i, _)| {
        let before = line[..i].chars().next_back().is_some_and(glue);
        let after = line[i + name.len()..].chars().next().is_some_and(glue);
        !before && !after
    })
}

/// The read-only surfaces rule (f) sweeps. `(label, argv, is_json)`.
///
/// Every surface named in the rule, plus `why <server> --json`, which is the
/// per-capability answer and the one that carries `live_unconnected` and
/// `abandoned`.
const DELIVERY_SURFACES: &[(&str, &[&str], bool)] = &[
    ("status", &["status"], false),
    ("status --json", &["status", "--json"], true),
    ("doctor", &["doctor"], false),
    ("doctor --json", &["doctor", "--json"], true),
    ("delivery", &["delivery"], false),
    ("delivery --json", &["delivery", "--json"], true),
    ("why --json", &["why", DECLARED_SERVER, "--json"], true),
    ("trust --preview", &["trust", "--preview"], true),
];

/// Walk a JSON body, carrying the harness an enclosing object is ABOUT.
///
/// The subject of a claim is not always inside the claim. `delivery --json`
/// emits `{"display": "Claude Code", "summary": "… served live"}`: the sentence
/// names no harness, and reading it in isolation leaves the product's most
/// machine-readable delivery claim attributed to nobody and judged by nothing.
/// So an object carrying a `display` string sets the subject for everything
/// beneath it, and a name inside the string itself still wins where there is
/// one.
fn json_delivery_strings(
    v: &serde_json::Value,
    ptr: &str,
    subject: Option<&str>,
    out: &mut Vec<(String, Option<String>, String)>,
) {
    match v {
        serde_json::Value::Object(map) => {
            let here = map
                .get("display")
                .and_then(serde_json::Value::as_str)
                .or(subject);
            for (k, child) in map {
                json_delivery_strings(child, &format!("{ptr}/{k}"), here, out);
            }
        }
        serde_json::Value::Array(items) => {
            for (ix, child) in items.iter().enumerate() {
                json_delivery_strings(child, &format!("{ptr}/{ix}"), subject, out);
            }
        }
        serde_json::Value::String(s) => {
            out.push((ptr.to_string(), subject.map(str::to_string), s.clone()))
        }
        _ => {}
    }
}

/// Sweep every delivery surface in one state and collect what it claimed.
fn delivery_claims(state: &DeliveryState, displays: &[String], skips: &mut Skips) -> Vec<Said> {
    let mut said = Vec::new();
    for (label, argv, is_json) in DELIVERY_SURFACES {
        let out = run(argv, &state.home, &state.proj);
        let text = strip_ansi(&out.text);
        let mut push = |origin: String, line: &str, subject: Option<String>| {
            let mut about = harnesses_named(line, displays);
            if about.is_empty() {
                about.extend(subject);
            }
            for claim in claims_in(line) {
                // One record PER HARNESS the claim covers: a single sentence
                // over twelve harnesses is twelve claims, and judging one of
                // them would leave eleven unjudged.
                if about.is_empty() {
                    said.push(Said {
                        surface: (*label).to_string(),
                        origin: origin.clone(),
                        harness: None,
                        claim: claim.clone(),
                        text: line.trim().to_string(),
                    });
                }
                for h in &about {
                    said.push(Said {
                        surface: (*label).to_string(),
                        origin: origin.clone(),
                        harness: Some(h.clone()),
                        claim: claim.clone(),
                        text: line.trim().to_string(),
                    });
                }
            }
        };
        if *is_json {
            match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(v) => {
                    let mut found = Vec::new();
                    json_delivery_strings(&v, "", None, &mut found);
                    for (ptr, subject, value) in found {
                        // A `why` field is the RATIONALE printed beside a claim,
                        // prose by the same contract rule (a0) already applies
                        // to `next_action.why`. It is recorded rather than
                        // judged — and the subtraction is safe only because the
                        // `summary` sibling in the very same object IS judged,
                        // so no harness loses its claim. A skip, never a
                        // silence.
                        if ptr.ends_with("/why") {
                            if !claims_in(&value).is_empty() {
                                skips.note(format!(
                                    "[{}] {label} {ptr}: a delivery-shaped phrase in a `why` \
                                     rationale field, not judged as a claim — the same prose \
                                     contract rule (a0) applies to `next_action.why`. Its \
                                     `summary` sibling in the same object carries the harness's \
                                     real claim and IS judged. Value: {value:?}",
                                    state.name
                                ));
                            }
                            continue;
                        }
                        push(ptr, &value, subject);
                    }
                }
                Err(e) => skips.note(format!(
                    "[{}] {label}: emitted no JSON ({e}), so rule (f) read no delivery claim from \
                     it in this state; exit_ok={}",
                    state.name, out.ok
                )),
            }
        } else {
            for line in text.lines() {
                push(line.trim().to_string(), line, None);
            }
        }
    }
    said
}

/// The three clauses, as one pure function over what was said and what is
/// really there — so the guard can be pointed at itself (see
/// `the_guard_catches_a_delivery_claim_that_does_not_match_the_disk`).
///
/// `bridged` answers "is a bridge registered for this harness", read from the
/// harness config on disk, never from a surface.
fn delivery_violations(
    state: &str,
    said: &[Said],
    artifacts: &[PathBuf],
    bridged: &dyn Fn(&str) -> bool,
    skips: &mut Skips,
) -> Vec<String> {
    let mut out = Vec::new();

    // ── clause 1: NO CONTRADICTION ────────────────────────────────────────
    let harnesses: BTreeSet<&str> = said.iter().filter_map(|s| s.harness.as_deref()).collect();
    for h in &harnesses {
        let live: Vec<&Said> = said
            .iter()
            .filter(|s| s.harness.as_deref() == Some(*h) && s.claim == Claim::ServedLive)
            .collect();
        let planned: Vec<&Said> = said
            .iter()
            .filter(|s| s.harness.as_deref() == Some(*h) && s.claim == Claim::PlannedLive)
            .collect();
        if let (Some(a), Some(b)) = (live.first(), planned.first()) {
            out.push(format!(
                "\n  state    : {state}\n  harness  : {h}\n  \
                 surface A: {} {}\n             says {:?}\n  \
                 surface B: {} {}\n             says {:?}\n  \
                 why      : ONE project, one harness, two opposite delivery claims. `served live` \
                 asserts the capability is reaching the tool right now; `planned live (not \
                 connected)` asserts it is not. Whichever surface a reader or a driver happens to \
                 poll decides which of the two it believes.",
                a.surface, a.origin, a.text, b.surface, b.origin, b.text,
            ));
        }
    }

    // ── clause 2: CLAIMS MATCH THE FILESYSTEM ─────────────────────────────
    //
    // The file list is walked, so this clause is judged against the project as
    // it really is. Both directions are checked: a "nothing on disk" claim
    // beside a file, and an "on disk" claim beside nothing.
    //
    // THE CAP IS DECLARED, not implied. `server_artifacts` keeps only files
    // whose bytes name a declared SERVER, so every claim about instructions,
    // settings, skills or extensions passes this clause untested. A guard that
    // stays silent about its own blind spot reads as coverage it does not
    // have — the exact shape it exists to catch — so the limit is recorded on
    // every run beside the states it did judge.
    skips.note(format!(
        "[{state}] clause 2 judges SERVER artifacts only — `server_artifacts` keeps files whose \
         bytes name the declared server, so claims about instructions, settings, skills and \
         extensions are outside its walk and are NOT checked against the disk here. Widening it \
         needs a per-kind ground truth (a settings claim is judged against the CLI's own settings \
         file, not against a name in the bytes)."
    ));
    for s in said.iter().filter(|s| s.claim == Claim::NoArtifacts) {
        if !artifacts.is_empty() {
            out.push(format!(
                "\n  state    : {state}\n  surface  : {} {}\n  claim    : {:?}\n  \
                 on disk  : {}\n  \
                 why      : the project WAS WALKED and those file(s) hold the declared server. A \
                 surface saying there are no project artifacts for a capability whose config file \
                 is sitting in the repository is a claim computed from `delivery::Plan` alone — \
                 the plan knows the routing and has never looked at the disk. The harness may \
                 still be reading that file.",
                s.surface,
                s.origin,
                s.text,
                artifacts
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
            ));
        }
    }
    for s in said {
        let Claim::OnDisk(path) = &s.claim else {
            continue;
        };
        if !Path::new(path).exists() {
            out.push(format!(
                "\n  state    : {state}\n  surface  : {} {}\n  claim    : {:?}\n  \
                 why      : the surface names `{path}` as being on disk and the walk did not find \
                 it. A reader is sent to remove, inspect or trust a file that is not there.",
                s.surface, s.origin, s.text,
            ));
        }
    }

    // ── clause 3: "LIVE" REQUIRES A BRIDGE ────────────────────────────────
    for s in said.iter().filter(|s| s.claim == Claim::ServedLive) {
        let Some(h) = s.harness.as_deref() else {
            // A project-wide sentence names no harness, so there is no bridge
            // reading to compare it against. Clause 2 is what judges those.
            skips.note(format!(
                "[{state}] {} {}: a `served live` claim naming no harness — clause 3 needs a \
                 per-harness bridge reading and has none, so this string is judged by clause 2 \
                 only. Value: {:?}",
                s.surface, s.origin, s.text
            ));
            continue;
        };
        if !bridged(h) {
            out.push(format!(
                "\n  state    : {state}\n  harness  : {h}\n  surface  : {} {}\n  claim    : {:?}\n  \
                 bridge   : NOT registered (read from the harness's own config file on disk)\n  \
                 why      : nothing is reaching {h}. `served live` is a present-tense claim of \
                 delivery that no bridge backs — invariant 8 at the reporting layer. The product's \
                 own correct wording for this state is `planned live (not connected)`, and other \
                 surfaces already use it.",
                s.surface, s.origin, s.text,
            ));
        }
    }
    out
}

/// Rule (f), over the four states the (a)–(e) matrix does not have.
#[test]
fn delivery_claims_agree_and_match_the_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let mut skips = Skips::default();
    // Seeded, for the same reason the main guard seeds one: a structural limit
    // that holds on EVERY run belongs in the record on every run, or the report
    // can print a short list while a known blind spot stands.
    skips.note(
        "rule (f) clause 3 reaches a REGISTERED bridge for `Claude Code` only. `agentstack x \
         gateway connect --all --write` refuses under this file's `env_clear` spawn (`no \
         installed harness with MCP support detected`), so the bridge is written directly into \
         the harness config the product reads — and `~/.claude.json` is the one harness config \
         path that lands inside an isolated HOME. The other twelve harnesses are covered in the \
         NOT-connected direction only."
            .to_string(),
    );
    skips.note(
        "rule (f) sweeps read-only surfaces. `apply`, `use` and `session start` also make \
         delivery claims while writing; those are pinned by \
         `tests/abandoned_render_is_named.rs` and `tests/use_honours_delivery.rs` instead, \
         because harvesting them here would mutate the state mid-sweep."
            .to_string(),
    );

    let mut states: Vec<DeliveryState> = Vec::new();

    // 1. THE REVIEWER'S EXACT REPRODUCTION. A live-routed project, pinned,
    //    consented, and then `use <toolset> --write` — the verb that wrote a
    //    server config while four surfaces said nothing was on disk.
    {
        let dir = tmp.path().join("use-write-live");
        fs::create_dir_all(&dir).unwrap();
        let (home, proj) = write_project_with_skill(&dir, LIVE_SERVERS_MANIFEST, SKILL_DESCRIBED);
        lock(&home, &proj);
        grant(&home, &proj);
        let out = run(&["use", "dev", "--write"], &home, &proj);
        assert!(out.ok, "fixture: `use dev --write` failed:\n{}", out.text);
        states.push(DeliveryState {
            name: "use-write-live",
            home,
            proj,
            bridged: false,
        });
    }

    // 2. AN ABANDONED RENDER. `render_locally` was set, the files were written,
    //    and the override was then removed — so the project is routed live with
    //    a config AgentStack wrote still sitting in it. No claim of "nothing on
    //    disk" is true here, and `Plan` alone cannot know that.
    {
        let dir = tmp.path().join("abandoned-render");
        fs::create_dir_all(&dir).unwrap();
        let (home, proj) = write_project_with_skill(&dir, RENDER_LOCALLY_MANIFEST, SKILL_DESCRIBED);
        lock(&home, &proj);
        grant(&home, &proj);
        apply(&home, &proj);
        let manifest = proj.join(".agentstack/agentstack.toml");
        let text = fs::read_to_string(&manifest).unwrap();
        fs::write(
            &manifest,
            text.replace("[delivery]\nrender_locally = true\n\n", ""),
        )
        .unwrap();
        assert!(
            !server_artifacts(&proj).is_empty(),
            "fixture: the abandoned state must actually leave a server config on disk, or clause \
             2 has nothing to judge"
        );
        states.push(DeliveryState {
            name: "abandoned-render",
            home,
            proj,
            bridged: false,
        });
    }

    // 3. ROUTED LIVE, WITH THE BRIDGE REGISTERED. The positive side of clause
    //    3: here `served live` is TRUE, and a rule that flagged it anyway would
    //    be a rule that fires on everything.
    {
        let dir = tmp.path().join("live-with-bridge");
        fs::create_dir_all(&dir).unwrap();
        let (home, proj) = write_project_with_skill(&dir, LIVE_SERVERS_MANIFEST, SKILL_DESCRIBED);
        fs::write(home.join(".claude.json"), CLAUDE_BRIDGE_CONFIG).unwrap();
        assert!(
            bridge_on_disk(&home),
            "fixture: the bridge entry must be readable back off disk, or clause 3's positive \
             side is untested"
        );
        lock(&home, &proj);
        grant(&home, &proj);
        states.push(DeliveryState {
            name: "live-with-bridge",
            home,
            proj,
            bridged: true,
        });
    }

    // 4. `render_locally` SET. Files are EXPECTED, so their absence would be
    //    the defect — the direction clause 2 can not otherwise reach, since
    //    everywhere else "no file" is the correct answer.
    {
        let dir = tmp.path().join("render-locally");
        fs::create_dir_all(&dir).unwrap();
        let (home, proj) = write_project_with_skill(&dir, RENDER_LOCALLY_MANIFEST, SKILL_DESCRIBED);
        lock(&home, &proj);
        grant(&home, &proj);
        apply(&home, &proj);
        assert!(
            !server_artifacts(&proj).is_empty(),
            "fixture: `render_locally` plus `apply --write` must put a server config on disk, or \
             the state under test does not exist"
        );
        states.push(DeliveryState {
            name: "render-locally",
            home,
            proj,
            bridged: false,
        });
    }

    // The harness display names, read off the real binary rather than written
    // down, so a new adapter is attributed correctly on the day it lands.
    let displays: Vec<String> = {
        let s = &states[0];
        let text = strip_ansi(&run(&["delivery", "--json"], &s.home, &s.proj).text);
        let v: serde_json::Value = serde_json::from_str(&text).expect("delivery --json is JSON");
        v["harnesses"]
            .as_array()
            .expect("delivery --json carries a harness list")
            .iter()
            .filter_map(|h| h["display"].as_str().map(str::to_string))
            .collect()
    };
    assert!(
        !displays.is_empty(),
        "read no harness display names at all — the extraction broke, not the product"
    );

    let mut violations: Vec<String> = Vec::new();
    let mut per_state: Vec<String> = Vec::new();
    for state in &states {
        assert_eq!(
            bridge_on_disk(&state.home),
            state.bridged,
            "[{}] fixture: the bridge ground truth must be confirmed against the harness config \
             on disk, never assumed",
            state.name
        );
        let artifacts = server_artifacts(&state.proj);
        let said = delivery_claims(state, &displays, &mut skips);
        if said.is_empty() {
            // Legitimate, and recorded rather than asserted away: a project
            // with `render_locally` set has NO live lane at all, so no surface
            // makes a live claim and none of the three clauses has anything to
            // judge. What that state proves is the fixture assertion above —
            // the files really are written — plus the fact that no surface
            // claims otherwise. The whole-run assertion below is what keeps a
            // broken extraction from passing as "nothing to say".
            skips.note(format!(
                "[{}] no delivery claim of any kind was read: this state has no live lane, so \
                 clauses 1 and 3 have no subject and clause 2 is satisfied by silence. The state \
                 is kept because the ABSENCE of a file would be the defect here, and the fixture \
                 asserts the files exist.",
                state.name
            ));
        }
        let bridged = |h: &str| state.bridged && h == BRIDGED_HARNESS;
        violations.extend(delivery_violations(
            state.name, &said, &artifacts, &bridged, &mut skips,
        ));
        per_state.push(format!(
            "  {:<18} {:>3} claim(s), {} server config(s) on disk, bridge={}",
            state.name,
            said.len(),
            artifacts.len(),
            state.bridged
        ));
    }
    // A broken extraction reads as "every surface agreed". This is the one
    // guard against that: over the whole run, delivery claims MUST have been
    // read, or nothing above was judged at all.
    assert!(
        per_state.iter().any(|l| !l.contains("  0 claim(s)")),
        "no delivery claim was read in ANY state — the extraction broke, not the product:\n{}",
        per_state.join("\n")
    );

    let mut s = String::new();
    let _ = writeln!(s, "\n── rule (f): delivery claims ──");
    for line in &per_state {
        let _ = writeln!(s, "{line}");
    }
    let _ = writeln!(
        s,
        "harnesses read off `delivery --json` ({}): {}",
        displays.len(),
        displays.join(", ")
    );
    let _ = writeln!(s, "NOT COVERED ({}):", skips.0.len());
    for k in &skips.0 {
        let _ = writeln!(s, "  {k}");
    }
    println!("{s}");

    assert!(
        violations.is_empty(),
        "{} delivery claim(s) contradict another surface, the filesystem, or the bridge:{}\n\n\
         Every one of these is the shape `crates/cli/src/commands/delivery.rs` warns about in \
         prose: a delivery claim computed from `delivery::Plan` alone. The plan knows the ROUTING. \
         It does not know whether a bridge is registered and it has never looked at the disk, so a \
         claim derived from it alone is one the product cannot back.",
        violations.len(),
        violations.join("\n")
    );
}

/// Rule (f), pointed at itself — two-sided, all three clauses.
///
/// Same construction as every other self-check in this file, and for the same
/// reason: the rule is enforced through the SAME function the sweep calls, so
/// it cannot rot into a no-op while still "passing". Each clause is asserted in
/// both directions, because a rule that fires on everything is as useless as
/// one that fires on nothing — and clause 3 in particular has to accept
/// `served live` where a bridge really is registered, or the correct wording
/// for a connected project would be flagged as the defect.
#[test]
fn the_guard_catches_a_delivery_claim_that_does_not_match_the_disk() {
    let mut sink = Skips::default();
    let no_bridge = |_: &str| false;
    let bridged = |h: &str| h == BRIDGED_HARNESS;
    let said =
        |surface: &str, origin: &str, harness: Option<&str>, claim: Claim, text: &str| Said {
            surface: surface.to_string(),
            origin: origin.to_string(),
            harness: harness.map(str::to_string),
            claim,
            text: text.to_string(),
        };

    // Clause 1 — the contradiction. `delivery --json` says one thing about
    // Claude Code, `delivery` says the opposite, in one project at one moment.
    let contradiction = vec![
        said(
            "delivery --json",
            "/harnesses/1/summary",
            Some(BRIDGED_HARNESS),
            Claim::ServedLive,
            "Claude Code: skills + MCP servers served live",
        ),
        said(
            "delivery",
            "Claude Code   skills + MCP servers planned live (not connected)",
            Some(BRIDGED_HARNESS),
            Claim::PlannedLive,
            "Claude Code   skills + MCP servers planned live (not connected)",
        ),
    ];
    let msgs = delivery_violations("live-no-bridge", &contradiction, &[], &no_bridge, &mut sink);
    assert!(
        msgs.iter().any(|m| m.contains("delivery --json")
            && m.contains("served live")
            && m.contains("planned live (not connected)")
            && m.contains(BRIDGED_HARNESS)),
        "clause 1 must name BOTH surfaces and BOTH values:\n{msgs:#?}"
    );
    println!(
        "guard self-check — two surfaces, one harness, opposite claims:{}",
        msgs.join("")
    );
    // …and the corrected shape — both surfaces on the honest wording — passes.
    let agreed: Vec<Said> = contradiction
        .iter()
        .map(|s| Said {
            claim: Claim::PlannedLive,
            ..s.clone()
        })
        .collect();
    assert!(
        delivery_violations("live-no-bridge", &agreed, &[], &no_bridge, &mut sink).is_empty(),
        "two surfaces using the same honest wording is the corrected shape and must pass"
    );

    // Clause 2 — THE FINDING-1 SHAPE. A surface claims nothing is on disk while
    // the walk found a config file holding the declared server.
    let claim_none = vec![said(
        "delivery",
        "· 0 project artifacts for the capabilities served live",
        None,
        Claim::NoArtifacts,
        "· 0 project artifacts for the capabilities served live",
    )];
    let on_disk = vec![PathBuf::from("/tmp/p/.mcp.json")];
    let msgs = delivery_violations("abandoned", &claim_none, &on_disk, &no_bridge, &mut sink);
    assert!(
        msgs.iter()
            .any(|m| m.contains("0 project artifacts") && m.contains("/tmp/p/.mcp.json")),
        "clause 2 must quote the false claim AND the file the walk found:\n{msgs:#?}"
    );
    println!(
        "guard self-check — a `nothing on disk` claim beside a file:{}",
        msgs.join("")
    );
    // …and the same claim over an empty project is TRUE and must pass.
    assert!(
        delivery_violations("live", &claim_none, &[], &no_bridge, &mut sink).is_empty(),
        "`0 project artifacts` over a project with none is correct and must pass"
    );
    // The other direction: a file named as on disk that is not there.
    let ghost = vec![said(
        "doctor",
        "⚠ Claude Code /tmp/p/.mcp.json is still on disk",
        Some(BRIDGED_HARNESS),
        Claim::OnDisk("/tmp/definitely/not/here.json".into()),
        "⚠ Claude Code /tmp/definitely/not/here.json is still on disk",
    )];
    assert!(
        !delivery_violations("live", &ghost, &[], &bridged, &mut sink).is_empty(),
        "a surface naming a file that is not on disk must be flagged"
    );

    // Clause 3 — `served live` with no bridge.
    let unbacked = vec![said(
        "delivery --json",
        "/harnesses/1/summary",
        Some(BRIDGED_HARNESS),
        Claim::ServedLive,
        "skills + MCP servers served live",
    )];
    let msgs = delivery_violations("live-no-bridge", &unbacked, &[], &no_bridge, &mut sink);
    assert!(
        msgs.iter().any(|m| m.contains("NOT registered")
            && m.contains("planned live (not connected)")
            && m.contains(BRIDGED_HARNESS)),
        "clause 3 must name the harness, the missing bridge, and the correct wording:\n{msgs:#?}"
    );
    println!(
        "guard self-check — `served live` with no bridge:{}",
        msgs.join("")
    );
    // …and the SAME claim with a bridge registered must pass, or the rule would
    // forbid the product ever reporting a connected project honestly.
    assert!(
        delivery_violations("live-with-bridge", &unbacked, &[], &bridged, &mut sink).is_empty(),
        "`served live` where a bridge IS registered is a true claim and must pass"
    );
}

/// The claim reader, pinned two-sided.
///
/// Rule (f) is only as strong as what [`claims_in`] recognises, and a narrowing
/// here is the one edit that could switch the rule off without touching an
/// assertion. So the qualified wording must NOT read as a live claim, the
/// unqualified one must, and the harness attribution must not confuse two tools
/// that share a prefix.
#[test]
fn the_claim_reader_separates_planned_from_served() {
    assert_eq!(
        claims_in("Claude Code   skills + MCP servers planned live (not connected)"),
        vec![Claim::PlannedLive],
        "the honest wording must never be read as a `served live` claim, or the correct product \
         behaviour would be reported as the defect"
    );
    assert_eq!(
        claims_in("Claude Code: skills + MCP servers served live"),
        vec![Claim::ServedLive],
    );
    assert_eq!(
        claims_in("· 0 project artifacts for the capabilities served live"),
        vec![Claim::ServedLive, Claim::NoArtifacts],
        "a project-wide sentence can carry two claims at once; both must be read"
    );
    assert_eq!(
        claims_in("no house rules here"),
        Vec::new(),
        "an ordinary line carries no delivery claim"
    );
    assert!(
        claims_in("VS Code /tmp/p/.vscode/mcp.json is still on disk (it holds filesystem)")
            .contains(&Claim::OnDisk("/tmp/p/.vscode/mcp.json".to_string())),
        "a path named as on disk must be lifted so clause 2 can check it exists"
    );

    let displays = vec![
        "Claude Code".to_string(),
        "Claude Desktop".to_string(),
        "VS Code".to_string(),
    ];
    assert_eq!(
        harnesses_named("Claude Desktop — planned live (not connected)", &displays),
        vec!["Claude Desktop".to_string()],
        "two harnesses sharing a prefix must not be confused, or a claim is judged against the \
         wrong tool's bridge"
    );
    assert_eq!(
        harnesses_named(
            "Claude Code, VS Code — MCP servers served live, nothing NEW rendered to compare",
            &displays
        ),
        vec!["Claude Code".to_string(), "VS Code".to_string()],
        "one sentence over several harnesses is a claim about EVERY one of them; attributing it \
         to a single tool would leave the rest unjudged"
    );
    assert_eq!(
        harnesses_named(
            "0 project artifacts for the capabilities served live",
            &displays
        ),
        Vec::<String>::new(),
        "a project-wide sentence names no harness and must not be attributed to one"
    );

    // The JSON subject rule: a claim whose harness is a SIBLING field, which is
    // the shape `delivery --json` emits and the one an isolated reading misses.
    let body = serde_json::json!({
        "harnesses": [{ "display": "Claude Code", "summary": "MCP servers served live" }]
    });
    let mut found = Vec::new();
    json_delivery_strings(&body, "", None, &mut found);
    let summary = found
        .iter()
        .find(|(ptr, _, _)| ptr == "/harnesses/0/summary")
        .expect("the walk must reach the summary");
    assert_eq!(
        summary.1.as_deref(),
        Some("Claude Code"),
        "a delivery claim whose subject is a sibling `display` field must be attributed to that \
         harness, or the most machine-readable claim the product makes is judged by nothing"
    );
}
