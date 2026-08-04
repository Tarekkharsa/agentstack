// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Item 8 — the panel surfaces, over the existing ui-contract.
//!
//! The queue names four: **lease status**, **the grouped review card**,
//! **library sources**, and **workflow control**. Items 1–7 shipped the reads
//! behind all four; this file is the witness that a panel can actually consume
//! them, that they agree with one another, and — the part that matters more
//! than completeness — that exposing them adds no authority.
//!
//! The binding constraint, from `STRATEGY.md` §"Carried forward" and
//! `docs/archive/design/ui-control-plane.md`: **the CLI is the sole
//! authority.** The panel is a graphical companion over the same fixed action
//! contract, never a second enforcement boundary. Three properties carry that
//! here:
//!
//! 1. every state-changing thing a panel can do is a declared entry in
//!    [`agentstack::ui_contract::PANEL_ACTIONS`] — no generic command string,
//!    and no per-item consent answer anywhere in the set;
//! 2. an apply whose consent digest went stale is refused before a byte moves;
//! 3. nothing outside `ui_contract.rs` reads the negotiated feature list, so
//!    what a caller negotiated can never change what the CLI enforces.
//!
//! Reads are driven the way a panel really drives them — the fixed argv
//! through the real binary, stdout decoded as JSON — so a change to clap, to
//! dispatch, or to the payload fails here before the panel ships the drift.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use serde_json::Value;

use agentstack::ui_contract::{self, Consent};

// These tests mutate the process-global HOME/AGENTSTACK_HOME (children inherit
// them, and the in-process trust grant reads them directly); serialize them.
static ENV_LOCK: Mutex<()> = Mutex::new(());

const BIN: &str = env!("CARGO_BIN_EXE_agentstack");

/// Point HOME/AGENTSTACK_HOME at a sandbox and return the agentstack home.
fn sandbox_home(tmp: &Path) -> PathBuf {
    let home = tmp.join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    let as_home = home.join(".agentstack");
    std::env::set_var("AGENTSTACK_HOME", &as_home);
    as_home
}

fn cleanup() {
    std::env::remove_var("HOME");
    std::env::remove_var("AGENTSTACK_HOME");
}

/// A project exercising every one of the four surfaces at once: a server and a
/// skill to review, a workflow whose two roles bind DIFFERENT harnesses, and
/// per-role model/effort so the payload has both stories to tell — codex
/// carries both dimensions as launch args, Claude Code has `effortLevel` in
/// its catalog but no confirmed way to select it for a single headless launch.
fn fixture_project(tmp: &Path) -> PathBuf {
    let proj = tmp.join("proj");
    std::fs::create_dir_all(proj.join("skills/demo")).unwrap();
    std::fs::create_dir_all(proj.join("workflows")).unwrap();
    std::fs::write(
        proj.join("skills/demo/SKILL.md"),
        "---\ndescription: demo skill\n---\n# demo\n",
    )
    .unwrap();
    std::fs::write(
        proj.join("workflows/ship.js"),
        "export const meta = { roles: ['builder', 'reviewer'] };\nreturn 1;\n",
    )
    .unwrap();
    std::fs::write(
        proj.join("agentstack.toml"),
        r#"version = 1
[targets]
default = ["claude-code"]

[servers.search]
type = "http"
url = "https://search.example/mcp"

[skills.demo]
path = "./skills/demo"

[profiles.builder]
harness = "codex"
model = "gpt-5.5"
effort = "high"
servers = ["search"]
skills = ["demo"]

[profiles.reviewer]
harness = "claude-code"
model = "claude-opus-4-5"
effort = "high"
servers = []
skills = []

[workflows.ship]
path = "./workflows/ship.js"
roles = ["builder", "reviewer"]
"#,
    )
    .unwrap();
    proj
}

/// Run one fixed argv through the real binary the way the panel bridge does,
/// and decode stdout as JSON. `argv` excludes the program name and the
/// `--manifest-dir` pair, which every panel read carries.
fn read_json(proj: &Path, home: &Path, argv: &[&str]) -> Value {
    let out = Command::new(BIN)
        .args(["--manifest-dir", proj.to_str().unwrap()])
        .args(argv)
        .env("HOME", home.parent().unwrap())
        .env("AGENTSTACK_HOME", home)
        .current_dir(proj)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "`agentstack {}` failed: {}",
        argv.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "`agentstack {}` did not emit JSON ({e}): {}",
            argv.join(" "),
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// Run a fixed argv expecting a refusal, returning stderr.
fn expect_refusal(proj: &Path, home: &Path, argv: &[&str]) -> String {
    let out = Command::new(BIN)
        .args(["--manifest-dir", proj.to_str().unwrap()])
        .args(argv)
        .env("HOME", home.parent().unwrap())
        .env("AGENTSTACK_HOME", home)
        .current_dir(proj)
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "`agentstack {}` was supposed to refuse but succeeded",
        argv.join(" ")
    );
    String::from_utf8_lossy(&out.stderr).to_string()
}

/// A negotiated payload's feature list, as a panel reads it.
fn features(value: &Value) -> Vec<&str> {
    value["features"]
        .as_array()
        .unwrap_or_else(|| panic!("payload carries no `features`: {value}"))
        .iter()
        .map(|f| f.as_str().unwrap())
        .collect()
}

fn assert_negotiable(value: &Value, feature: &str) {
    assert_eq!(
        value["schema_version"],
        ui_contract::SCHEMA_VERSION,
        "payload carries the envelope: {value}"
    );
    assert!(
        features(value).contains(&feature),
        "payload advertises `{feature}` so a panel can gate on it instead of \
         sniffing a field: {value}"
    );
}

/// Pin the project and grant trust through the SAME digest-bound pair the
/// panel's `trust-grant` action uses — never a test-only back door.
fn lock_and_trust(proj: &Path, home: &Path) {
    let out = Command::new(BIN)
        .args(["--manifest-dir", proj.to_str().unwrap(), "lock", "--write"])
        .env("HOME", home.parent().unwrap())
        .env("AGENTSTACK_HOME", home)
        .current_dir(proj)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "lock failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let preview = read_json(proj, home, &["trust", "--preview"]);
    let digest = preview["surface_digest"].as_str().unwrap().to_string();
    let out = Command::new(BIN)
        .args([
            "--manifest-dir",
            proj.to_str().unwrap(),
            "trust",
            proj.to_str().unwrap(),
            "--yes",
            "--consented-digest",
            &digest,
        ])
        .env("HOME", home.parent().unwrap())
        .env("AGENTSTACK_HOME", home)
        .current_dir(proj)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "the digest-bound grant failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ── 1. all four surfaces, end to end ─────────────────────────────────────────

/// The queue's four surfaces, each read through the fixed argv a panel emits,
/// each negotiable by name, and each coherent with the others.
///
/// "Coherent" is the part worth stating: these payloads are read side by side
/// in one view, so they must not contradict each other about the same project.
/// The lease read and the workflow read name the same toolsets the trust card
/// reviews, and none of them claims an activation the others deny.
#[test]
fn every_panel_surface_the_queue_names_is_readable() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = sandbox_home(tmp.path());
    let proj = fixture_project(tmp.path());

    // ── Surface 1: lease status. Machine-level, project-independent, and the
    // authoritative read — a lease used to be visible only inside the MCP
    // process that owned it.
    let leases = read_json(&proj, &home, &["lease", "status", "--json"]);
    assert_negotiable(&leases, "lease-status-v1");
    assert!(
        leases["leases"].as_array().unwrap().is_empty(),
        "no lease has been opened here: {leases}"
    );
    assert!(
        leases["note"].as_str().unwrap().contains("process-scoped"),
        "the scope note travels with the payload, so a panel cannot present a \
         record as a durable activation: {leases}"
    );

    // ── Surface 2: the grouped review card, on an untrusted project — the
    // state a panel meets it in.
    let card = read_json(&proj, &home, &["trust", "--preview"]);
    assert_negotiable(&card, "trust-card-groups-v1");
    assert_eq!(card["state"], "untrusted");
    assert!(card["surface_digest"].as_str().is_some());
    let groups = card["review"]["groups"].as_array().unwrap();
    assert!(
        groups.iter().any(|g| g["kind"] == "server"),
        "the declared server is grouped: {card}"
    );
    assert!(
        groups.iter().any(|g| g["kind"] == "skill"),
        "the declared skill is grouped: {card}"
    );
    // A group points at items; it never carries copies, which is what keeps
    // grouping presentation rather than a second description of the review.
    let items = card["review"]["items"].as_array().unwrap();
    for group in groups {
        for ix in group["items"].as_array().unwrap() {
            let ix = ix.as_u64().expect("a group holds INDICES, never copies") as usize;
            assert!(ix < items.len(), "index out of range: {card}");
        }
    }

    // ── Surface 3: library sources. The collision reading rides on `status`,
    // and the array is always present — "checked, nothing shadowed" must be
    // distinguishable from a binary that has no such key.
    let status = read_json(&proj, &home, &["status", "--json"]);
    assert_negotiable(&status, "library-sources-v1");
    assert!(
        status["project"]["shadowed_names"].is_array(),
        "shadowed_names is always present, `[]` when nothing collides: {status}"
    );
    // It rides beside the other per-project readings, so a panel drawing one
    // project card reads them from one place.
    assert!(
        status["project"]["instruction_channels"].is_array(),
        "instruction_channels sits under the same `project` object: {status}"
    );

    // ── Surface 4: workflow control — observation plus the per-role selection
    // facts item 6 shipped. `list` is the refusal-free surface, so it answers
    // before the project is trusted.
    let list = read_json(&proj, &home, &["workflow", "list", "--json"]);
    assert_negotiable(&list, "workflow-role-selection-v1");
    let ship = list["workflows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["name"] == "ship")
        .expect("the declared workflow is listed even untrusted");
    assert_eq!(ship["trusted"], false, "listed, not gated: {list}");
    let details = ship["role_details"].as_array().unwrap();
    let builder = details
        .iter()
        .find(|r| r["role"] == "builder")
        .expect("builder's selection facts");
    assert_eq!(builder["harness"], "codex");
    assert_eq!(builder["model"], "gpt-5.5");
    assert_eq!(builder["effort"], "high");
    assert!(
        builder["undeliverable"].as_array().unwrap().is_empty(),
        "codex carries both dimensions as launch args: {builder}"
    );
    let reviewer = details
        .iter()
        .find(|r| r["role"] == "reviewer")
        .expect("reviewer's selection facts");
    assert_eq!(reviewer["harness"], "claude-code");
    let undeliverable = reviewer["undeliverable"].as_array().unwrap();
    assert!(
        undeliverable.iter().any(|u| u["dimension"] == "effort"),
        "Claude Code's effort has no confirmed per-launch selector, and the \
         payload says so rather than implying the value arrives: {reviewer}"
    );
    for entry in undeliverable {
        assert!(
            entry["reason"].as_str().is_some_and(|r| !r.is_empty()),
            "every undeliverable value carries the sentence saying why: {entry}"
        );
    }

    // The four surfaces agree about the same project: every role the workflow
    // read names is a toolset, and the trust card reviews that toolset's
    // content. Nothing here claims the workflow is running.
    assert!(
        ship["roles"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| details.iter().any(|d| d["role"] == *r)),
        "every declared role resolved to a toolset in this fixture: {ship}"
    );

    // `explain` is the deeper per-workflow read, and it re-gates on its own:
    // reading an untrusted bundle's script is what rule 3 forbids, so the
    // panel meets a refusal here, not a payload.
    let refusal = expect_refusal(&proj, &home, &["workflow", "explain", "ship", "--json"]);
    assert!(
        refusal.contains("not trusted"),
        "explain refuses an untrusted bundle rather than parsing it: {refusal}"
    );

    // With the yes given through the panel's own digest-bound action, the same
    // read answers — and now carries the envelope it never used to.
    lock_and_trust(&proj, &home);
    let explain = read_json(&proj, &home, &["workflow", "explain", "ship", "--json"]);
    assert_negotiable(&explain, "workflow-role-selection-v1");
    assert_eq!(explain["workflow"], "ship");
    assert!(
        explain["roles"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["role"] == "builder" && r["model"] == "gpt-5.5"),
        "explain and list tell the same model story: {explain}"
    );

    cleanup();
}

// ── 2. a stale lease never renders as live ───────────────────────────────────

/// The decisive lease property, asserted on the PANEL payload rather than on
/// the registry: whatever a record on disk says, the row a panel renders may
/// never read `live` unless the recorded process is really the one running.
///
/// `lease_registry` already witnesses the derivation. What this adds is the
/// payload's honesty — every row carries `liveness` AND the `why` sentence, so
/// there is no field a panel could reasonably substitute (a bare `pid` plus
/// `started_unix` would let a UI infer liveness itself, which is exactly the
/// inference the start token exists to refuse).
#[test]
fn a_stale_lease_never_renders_as_live_in_a_panel_payload() {
    use agentstack::lease_registry::{liveness, LeaseRecord};

    // A record whose PID is this very process — so a naive "does the PID
    // exist?" check would answer live — but whose start token belongs to some
    // other process that once held the number.
    let reused = LeaseRecord {
        instance: "reused".into(),
        project: "/tmp/proj".into(),
        toolset: "backend".into(),
        pid: std::process::id() as i32,
        start_token: Some("a-start-time-this-process-does-not-have".into()),
        started_unix: 1_700_000_000,
    };
    // And one whose start time was never recorded at all: unknown, which is a
    // real answer and must never fold into live.
    let unknown = LeaseRecord {
        instance: "unknown".into(),
        project: "/tmp/proj".into(),
        toolset: "frontend".into(),
        pid: std::process::id() as i32,
        start_token: None,
        started_unix: 1_700_000_001,
    };

    let derived: Vec<_> = [reused, unknown]
        .into_iter()
        .map(|r| {
            let state = liveness(&r);
            (r, state)
        })
        .collect();
    let payload = agentstack::commands::lease::status_value(&derived);
    assert_negotiable(&payload, "lease-status-v1");

    let rows = payload["leases"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "both records are reported: {payload}");
    for row in rows {
        assert_ne!(
            row["liveness"], "live",
            "a record the OS cannot confirm must never render as live: {payload}"
        );
        assert!(
            row["why"].as_str().is_some_and(|w| !w.is_empty()),
            "every row carries the sentence behind its liveness: {row}"
        );
    }
    let unknown_row = rows.iter().find(|r| r["instance"] == "unknown").unwrap();
    assert_eq!(
        unknown_row["liveness"], "unknown",
        "no start time means not established, never live: {payload}"
    );
    assert!(
        unknown_row["why"]
            .as_str()
            .unwrap()
            .contains("never as live"),
        "the sentence tells the panel which way to fail: {unknown_row}"
    );
}

// ── 3. one card, one question ────────────────────────────────────────────────

/// The grouped card reaches the panel with EXACTLY ONE question, and no answer
/// affordance anywhere in it.
///
/// Grouping the detail body was item 5's change, and the hazard it created is
/// precisely this: a body split into per-capability groups invites a per-group
/// yes. The payload structurally cannot grow one — a group holds indices, so
/// it has nowhere to put a decision — but "there is only one question" is the
/// kind of property that stops being true quietly, so it is asserted by
/// walking every key path in the whole payload rather than by reading the one
/// key we expect.
#[test]
fn the_grouped_card_reaches_the_panel_with_exactly_one_question() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = sandbox_home(tmp.path());
    let proj = fixture_project(tmp.path());

    let card = read_json(&proj, &home, &["trust", "--preview"]);
    assert_negotiable(&card, "trust-card-groups-v1");

    /// Collect every key name appearing anywhere in the payload, with its path.
    fn walk(value: &Value, path: &str, out: &mut Vec<(String, String)>) {
        match value {
            Value::Object(map) => {
                for (key, child) in map {
                    let child_path = format!("{path}.{key}");
                    out.push((key.clone(), child_path.clone()));
                    walk(child, &child_path, out);
                }
            }
            Value::Array(items) => {
                for (i, child) in items.iter().enumerate() {
                    walk(child, &format!("{path}[{i}]"), out);
                }
            }
            _ => {}
        }
    }
    let mut keys = Vec::new();
    walk(&card, "", &mut keys);

    let questions: Vec<&String> = keys
        .iter()
        .filter(|(key, _)| key == "question")
        .map(|(_, path)| path)
        .collect();
    assert_eq!(
        questions,
        vec![".review.question"],
        "exactly one closing question, and it belongs to the whole review — a \
         per-group or per-item question would multiply the moments a human \
         commits to something"
    );
    assert!(
        card["review"]["question"]
            .as_str()
            .is_some_and(|q| !q.is_empty()),
        "the one question carries text: {card}"
    );

    // And no answer affordance: the three interactive-review answers exist
    // only in the terminal, where one closing yes commits them.
    for forbidden in ["accept", "keep_pinned", "keep-pinned", "block", "answer"] {
        assert!(
            !keys.iter().any(|(key, _)| key == forbidden),
            "the payload must carry no `{forbidden}` affordance — a panel may \
             render what changed, never collect the answer: {card}"
        );
    }

    // The group whose kind is a server exists AND carries no fact of its own
    // beyond presentation: its item list is indices into `review.items`.
    let groups = card["review"]["groups"].as_array().unwrap();
    let items = card["review"]["items"].as_array().unwrap();
    let mut referenced = std::collections::BTreeSet::new();
    for group in groups {
        for ix in group["items"].as_array().unwrap() {
            referenced.insert(ix.as_u64().expect("indices, never copies"));
        }
        assert!(
            group["counts"]["total"].is_number(),
            "a group's counts are folded from the same markers: {group}"
        );
    }
    assert_eq!(
        referenced.len(),
        items.len(),
        "every item appears in exactly one group — a kind this binary has no \
         label for is grouped under its own name, never dropped: {card}"
    );

    cleanup();
}

// ── 4. the closed set ────────────────────────────────────────────────────────

/// **The decisive invariant witness.** Enumerate everything a panel can do
/// that changes state, and assert the shape of each one.
///
/// The closed set is declared in the CLI ([`ui_contract::PANEL_ACTIONS`]) so
/// that "the panel is never a second authority" is a property this repository
/// can check, not a promise made in a frontend. Four things are asserted:
///
/// 1. every declared action names a real clap subcommand — a fixed verb, never
///    a generated command string;
/// 2. every action whose apply introduces reviewable content is bound to a
///    consent digest, and the flag it claims really exists on that verb;
/// 3. the four surfaces this item exposes contribute NO action at all;
/// 4. no per-item consent answer is anywhere in the set.
#[test]
fn no_panel_action_exists_outside_the_closed_digest_bound_set() {
    use clap::CommandFactory;

    let root = agentstack::cli::Cli::command();

    // (1) + (2): every entry resolves to a real verb carrying the binding it
    // claims.
    for action in ui_contract::PANEL_ACTIONS {
        let mut cmd = &root;
        for segment in action.verb {
            cmd = cmd
                .get_subcommands()
                .find(|c| c.get_name() == *segment)
                .unwrap_or_else(|| {
                    panic!(
                        "panel action `{}` names `{}`, which is not a subcommand — \
                         an action must map to a fixed verb",
                        action.name,
                        action.verb.join(" ")
                    )
                });
        }
        // Match on the LONG flag, which is what the panel's fixed argv spells,
        // not clap's internal id (which is the Rust field name).
        let has_arg = |name: &str| cmd.get_arguments().any(|a| a.get_long() == Some(name));
        match action.consent {
            Consent::Digest(flag) => {
                assert!(
                    has_arg(flag),
                    "panel action `{}` claims to bind `--{flag}`, but `{}` has no \
                     such argument",
                    action.name,
                    action.verb.join(" ")
                );
                assert!(
                    has_arg("yes"),
                    "a digest-bound apply is non-interactive: `{}` must take --yes",
                    action.verb.join(" ")
                );
            }
            Consent::Preconditions => {
                // Nothing to assert about a flag here; what makes these safe is
                // the CLI's own gates, witnessed elsewhere (`session start`
                // refuses an untrusted or unpinned surface, `restore` is bound
                // to an id from its own inventory, `apply` renders only
                // already-declared content).
            }
        }
    }

    // (3) The four surfaces this item exposes are READS. Leases are opened and
    // closed by the MCP connection that owns them; linked library sources are
    // personal-layer machine state; workflow authoring and supervision stay
    // deferred. None of them may acquire an action here by accident.
    for forbidden in [
        "lease",
        "lease-open",
        "lease-close",
        "lib",
        "lib-link",
        "lib-unlink",
        "lib-reorder",
        "workflow",
        "workflow-run",
        "workflow-resume",
        "workflow-declare",
    ] {
        assert!(
            !ui_contract::PANEL_ACTIONS
                .iter()
                .any(|a| a.name == forbidden || a.verb.first() == Some(&forbidden)),
            "`{forbidden}` must not be a panel action: each would need an \
             authority path the read surface deliberately does not have"
        );
    }

    // (4) No per-item consent answer, in any spelling.
    for forbidden in ["accept-item", "block-item", "keep-pinned", "trust-item"] {
        assert!(
            !ui_contract::PANEL_ACTIONS
                .iter()
                .any(|a| a.name == forbidden),
            "the review has exactly one answer, over the whole project: \
             `{forbidden}` may never join the set"
        );
    }

    // The two consent digests that bind a reviewed preview are both present —
    // setup and the yes. Losing either would leave a reviewable write unbound.
    for (name, flag) in [
        ("setup-apply", "consented-plan"),
        ("trust-grant", "consented-digest"),
    ] {
        assert!(
            ui_contract::PANEL_ACTIONS
                .iter()
                .any(|a| a.name == name && a.consent == Consent::Digest(flag)),
            "`{name}` must stay bound to --{flag}"
        );
    }

    // Names are unique: two entries sharing one means two contracts think they
    // own the same action.
    let mut names: Vec<&str> = ui_contract::PANEL_ACTIONS.iter().map(|a| a.name).collect();
    names.sort_unstable();
    let before = names.len();
    names.dedup();
    assert_eq!(before, names.len(), "duplicate panel action name");
}

// ── 5. stale consent is refused ──────────────────────────────────────────────

/// A panel action carrying a consent digest that no longer matches is refused
/// before anything moves — the property that makes a previewed action safe to
/// render in a browser at all.
///
/// Driven end to end through the real binary: preview, then let the manifest
/// move underneath (exactly what a concurrent terminal edit does), then apply
/// the reviewed digest.
#[test]
fn a_panel_action_with_a_stale_consent_digest_is_refused() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = sandbox_home(tmp.path());
    let proj = fixture_project(tmp.path());

    let preview = read_json(
        &proj,
        &home,
        &[
            "create-profile",
            "--name",
            "web",
            "--skill",
            "demo",
            "--preview",
        ],
    );
    let digest = preview["consent_digest"].as_str().unwrap().to_string();
    assert!(digest.starts_with("sha256:"), "{preview}");

    // The manifest moves after the review.
    let manifest_path = proj.join("agentstack.toml");
    let before = std::fs::read_to_string(&manifest_path).unwrap();
    std::fs::write(&manifest_path, format!("{before}# edited elsewhere\n")).unwrap();
    let after_edit = std::fs::read(&manifest_path).unwrap();

    let stderr = expect_refusal(
        &proj,
        &home,
        &[
            "create-profile",
            "--name",
            "web",
            "--skill",
            "demo",
            "--yes",
            "--consented",
            &digest,
        ],
    );
    assert!(
        stderr.contains("consent digest mismatch"),
        "the refusal names what went stale: {stderr}"
    );
    assert_eq!(
        after_edit,
        std::fs::read(&manifest_path).unwrap(),
        "a refused apply writes nothing"
    );
    assert!(
        !proj.join("agentstack.lock").exists(),
        "a refused apply does not re-lock"
    );

    cleanup();
}

// ── 6. never a second authority ──────────────────────────────────────────────

/// The panel never becomes a second authority: the CLI re-validates every
/// precondition whether or not the caller negotiated anything.
///
/// Two halves, because the claim has two halves.
///
/// **Structural.** No enforcement decision may read the negotiated feature
/// list. That is a source-level property — a `FEATURES.contains(...)` anywhere
/// outside `ui_contract.rs` would make what a caller advertised into an input
/// to what the CLI allows — so it is checked at the source, where a reviewer
/// would look. `PANEL_ACTIONS` is held to the same rule for the same reason.
///
/// **Behavioural.** A project whose payloads advertise every contract this
/// binary serves still meets each gate: an untrusted project refuses the reads
/// that would put its content in front of an agent, and refuses the actions
/// that would activate it. Negotiation bought nothing.
#[test]
fn the_panel_never_becomes_a_second_authority() {
    // ── Structural half.
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    let mut scanned = 0usize;
    fn walk(dir: &Path, offenders: &mut Vec<String>, scanned: &mut usize) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, offenders, scanned);
            } else if path.extension().is_some_and(|e| e == "rs")
                && path.file_name().is_some_and(|f| f != "ui_contract.rs")
            {
                *scanned += 1;
                let body = std::fs::read_to_string(&path).unwrap();
                for (n, line) in body.lines().enumerate() {
                    // The declarations themselves may be mentioned in prose;
                    // what may never appear is code consulting them.
                    let consults = line.contains("FEATURES") || line.contains("PANEL_ACTIONS");
                    let is_comment = line.trim_start().starts_with("//");
                    if consults && !is_comment {
                        offenders.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
                    }
                }
            }
        }
    }
    walk(&src, &mut offenders, &mut scanned);
    // Non-vacuity: a walk that found nothing to read would pass silently.
    assert!(
        scanned > 50,
        "the source scan reached only {scanned} files — it is not looking at \
         the crate"
    );
    assert!(
        offenders.is_empty(),
        "the negotiated feature list and the declared action set are \
         presentation only — no code outside ui_contract.rs may read them, or \
         what a caller advertised becomes an input to what the CLI allows:\n{}",
        offenders.join("\n")
    );

    // ── Behavioural half.
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = sandbox_home(tmp.path());
    let proj = fixture_project(tmp.path());

    // Everything this binary serves is advertised to the caller...
    let status = read_json(&proj, &home, &["status", "--json"]);
    let advertised = features(&status);
    for expected in [
        "sessions-v1",
        "trust-card-groups-v1",
        "workflow-role-selection-v1",
    ] {
        assert!(
            advertised.contains(&expected),
            "the caller negotiated `{expected}`: {status}"
        );
    }

    // ...and it buys nothing. `session start` names a real toolset and is a
    // declared panel action, and the CLI still refuses: the project is neither
    // trusted nor pinned, which is its own gate, not the panel's.
    let stderr = expect_refusal(&proj, &home, &["session", "start", "builder"]);
    assert!(
        stderr.contains("not trusted"),
        "activation is refused on the CLI's own gate, and the refusal names \
         which one: {stderr}"
    );
    assert!(
        !proj.join(".mcp.json").exists() && !proj.join(".claude").exists(),
        "a refused activation renders nothing"
    );

    // Same for the deeper workflow read: negotiating the contract does not
    // make an untrusted bundle parseable.
    let stderr = expect_refusal(&proj, &home, &["workflow", "explain", "ship", "--json"]);
    assert!(
        stderr.contains("not trusted"),
        "an untrusted bundle's script is never parsed for a panel either: {stderr}"
    );

    cleanup();
}
