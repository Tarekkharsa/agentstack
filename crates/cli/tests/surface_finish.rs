// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Surface finish (TODO item 5) — three separable claims, one binary.
//!
//! 1. **One card, one yes.** The review's detail body is grouped per
//!    capability with change markers, and grouping is *presentation*: a group
//!    holds indices into the one flat item list, so there is nowhere for a
//!    per-capability answer to live. The witnesses here are structural, not
//!    cosmetic — they assert the absence of an affordance, because "we simply
//!    did not add one" is the kind of promise that quietly stops being true.
//! 2. **`run` is protected by default.** The fail-closed gate `--locked` used
//!    to opt in to is what a bare `agentstack run` does; `--unprotected` is the
//!    explicit way out. The point of the flip is the refusal, so an untrusted
//!    and a drifted project each get one.
//! 3. **Varlock is the productized vault.** `init` offers a `.env.schema`,
//!    `doctor` reports varlock's health — and `${REF}` resolution is untouched,
//!    which is the one thing here that must be proven rather than asserted.
//!
//! Everything spawns the real binary: these are claims about what the product
//! does, not about what a function returns.

use std::fs;
use std::path::{Path, PathBuf};

fn run(bin: &str, args: &[&str], home: &Path, cwd: &Path) -> (String, bool) {
    let out = std::process::Command::new(bin)
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("HOME", home)
        .env("AGENTSTACK_HOME", home.join(".agentstack"))
        // Deliberately minimal: `varlock` is NOT on it, which is what makes the
        // "opted in but not installed" health arm the real one below.
        .env("PATH", "/usr/bin:/bin")
        // No terminal: the non-interactive consent gate is the real path here,
        // which is why every grant below presents a consented digest.
        .stdin(std::process::Stdio::null())
        .output()
        .expect("spawn agentstack");
    (
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
        out.status.success(),
    )
}

fn preview(bin: &str, home: &Path, proj: &Path) -> serde_json::Value {
    let (text, ok) = run(bin, &["trust", "--preview"], home, proj);
    assert!(ok, "preview failed:\n{text}");
    serde_json::from_str(&text).expect("preview is JSON")
}

fn lock(bin: &str, home: &Path, proj: &Path) {
    let (text, ok) = run(bin, &["lock", "--write"], home, proj);
    assert!(ok, "lock failed:\n{text}");
}

/// Review the current surface and consent to exactly the digest it emitted —
/// the non-interactive path a panel drives.
fn grant(bin: &str, home: &Path, proj: &Path) {
    let digest = preview(bin, home, proj)["surface_digest"]
        .as_str()
        .unwrap()
        .to_string();
    let (text, ok) = run(
        bin,
        &["trust", "--yes", "--consented-digest", &digest],
        home,
        proj,
    );
    assert!(ok, "grant failed:\n{text}");
}

const MANIFEST: &str = r#"version = 1

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

[settings.claude-code]
permissions = { allow = ["Bash(git status)"] }

[policy.egress]
docs = ["api.example.com"]
"#;

/// A project declaring several capability kinds, so the grouping has more than
/// one group to prove itself over.
fn fixture(tmp: &Path) -> (PathBuf, PathBuf) {
    let home = tmp.join("home");
    let proj = tmp.join("proj");
    let a = proj.join(".agentstack");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(a.join("skills/summarize")).unwrap();
    fs::create_dir_all(a.join("instructions")).unwrap();
    fs::write(a.join("skills/summarize/SKILL.md"), "# Summarize\nbody\n").unwrap();
    fs::write(a.join("instructions/house-rules.md"), "Prefer boring.\n").unwrap();
    fs::write(a.join("agentstack.toml"), MANIFEST).unwrap();
    (home, proj)
}

fn manifest_path(proj: &Path) -> PathBuf {
    proj.join(".agentstack/agentstack.toml")
}

/// Every JSON key anywhere under `value`, with its full path — the only honest
/// way to assert an affordance is ABSENT rather than merely unused.
fn keys_under(value: &serde_json::Value, path: &str, out: &mut Vec<(String, String)>) {
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                out.push((format!("{path}.{k}"), k.clone()));
                keys_under(v, &format!("{path}.{k}"), out);
            }
        }
        serde_json::Value::Array(items) => {
            for (ix, v) in items.iter().enumerate() {
                keys_under(v, &format!("{path}[{ix}]"), out);
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Piece 1 — the grouped review card
// ---------------------------------------------------------------------------

/// The card's detail body is grouped per capability, the groups carry change
/// markers, and the whole payload still asks exactly ONE question.
#[test]
fn the_review_card_groups_detail_per_capability_under_one_yes() {
    let bin = env!("CARGO_BIN_EXE_agentstack");
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = fixture(tmp.path());
    lock(bin, &home, &proj);
    grant(bin, &home, &proj);

    // Move the surface so the markers have something to say: one server added,
    // one hook changed, one server dropped.
    let changed = MANIFEST
        .replace(
            "command = \"./scripts/check.sh\"",
            "command = \"./other.sh\"",
        )
        .replace(
            "[servers.docs]\ntype = \"http\"\nurl = \"https://api.example.com/mcp/docs\"",
            "[servers.notes]\ntype = \"stdio\"\ncommand = \"node\"\nargs = [\"notes.js\"]",
        )
        .replace("[policy.egress]\ndocs = [\"api.example.com\"]", "");
    fs::write(manifest_path(&proj), &changed).unwrap();
    lock(bin, &home, &proj);

    let v = preview(bin, &home, &proj);
    let features: Vec<&str> = v["features"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f.as_str().unwrap())
        .collect();
    assert!(
        features.contains(&"trust-card-groups-v1"),
        "the grouping is advertised as its own contract: {features:?}"
    );

    let review = &v["review"];
    let items = review["items"].as_array().unwrap();
    let groups = review["groups"].as_array().unwrap();
    assert!(!groups.is_empty(), "grouped detail body: {review}");

    // Every group is a real capability group with a change marker and a tally.
    let mut covered: Vec<usize> = Vec::new();
    for g in groups {
        assert!(g["kind"].as_str().is_some_and(|k| !k.is_empty()), "{g}");
        assert!(g["label"].as_str().is_some_and(|l| !l.is_empty()), "{g}");
        assert!(
            matches!(
                g["change"].as_str(),
                Some("added" | "changed" | "unchanged")
            ),
            "group change marker is one of the three words: {g}"
        );
        for key in ["added", "changed", "unchanged", "removed", "total"] {
            assert!(g["counts"][key].is_u64(), "counts.{key} on {g}");
        }
        for ix in g["items"].as_array().unwrap() {
            let ix = ix.as_u64().unwrap() as usize;
            assert_eq!(
                items[ix]["kind"], g["kind"],
                "a group only ever indexes its own kind"
            );
            covered.push(ix);
        }
    }
    covered.sort_unstable();
    let all: Vec<usize> = (0..items.len()).collect();
    assert_eq!(
        covered, all,
        "the groups partition the flat item list exactly — no item lost, none counted twice"
    );

    // Change markers are present and real, at both levels.
    let marks: Vec<&str> = items.iter().filter_map(|i| i["change"].as_str()).collect();
    assert!(marks.contains(&"added"), "an added item: {marks:?}");
    assert!(marks.contains(&"changed"), "a changed item: {marks:?}");
    assert!(marks.contains(&"unchanged"), "an unchanged item: {marks:?}");
    assert!(
        !review["removed"].as_array().unwrap().is_empty(),
        "the dropped server is reported as a removal: {review}"
    );
    assert!(
        groups.iter().any(|g| g["change"] == "changed"),
        "at least one group reads changed: {groups:?}"
    );
    assert!(
        groups
            .iter()
            .any(|g| g["counts"]["removed"].as_u64() == Some(1)),
        "the removal lands in its own kind's group: {groups:?}"
    );

    // ONE question. Not one per group, not one per item — one, for the project.
    let mut keys = Vec::new();
    keys_under(&v, "", &mut keys);
    let questions: Vec<&String> = keys
        .iter()
        .filter(|(_, k)| k == "question")
        .map(|(p, _)| p)
        .collect();
    assert_eq!(
        questions.len(),
        1,
        "exactly one consent question in the whole payload: {questions:?}"
    );
    assert_eq!(questions[0], ".review.question");
    assert!(
        review["question"].as_str().is_some_and(|q| !q.is_empty()),
        "the one question carries its text: {review}"
    );

    // The terminal's own detail body is grouped too, with the same tallies.
    // (Non-interactive `trust` prints the full review and then refuses at the
    // consent gate — the review is the part under test.)
    let (text, ok) = run(bin, &["trust"], &home, &proj);
    assert!(!ok, "a non-interactive trust must still refuse:\n{text}");
    assert!(
        text.contains("servers (spawned or contacted over MCP):"),
        "servers are a headed group in the terminal body:\n{text}"
    );
    assert!(
        text.contains("hooks (EXECUTABLE"),
        "hooks keep their group header:\n{text}"
    );
    assert!(
        text.contains("added") && text.contains("changed"),
        "the group headers carry change tallies:\n{text}"
    );
}

/// The invariant behind the grouping: it is presentation, never granularity.
/// No group and no item may offer an answer, because there is exactly one yes.
#[test]
fn no_per_capability_consent_answer_exists() {
    let bin = env!("CARGO_BIN_EXE_agentstack");
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = fixture(tmp.path());
    lock(bin, &home, &proj);
    grant(bin, &home, &proj);
    let v = preview(bin, &home, &proj);

    let mut keys = Vec::new();
    keys_under(&v, "", &mut keys);

    // The three re-gate answers exist only in the interactive terminal review,
    // where the single closing yes commits them. A payload that offered them
    // per item would be a second consent path with no closing moment.
    for banned in [
        "answer",
        "accept",
        "keep_pinned",
        "keepPinned",
        "block",
        "decision",
        "consent",
        "approve",
    ] {
        let found: Vec<&String> = keys
            .iter()
            .filter(|(_, k)| k == banned)
            .map(|(p, _)| p)
            .collect();
        assert!(
            found.is_empty(),
            "'{banned}' is an answer affordance and must not exist in the card payload: {found:?}"
        );
    }

    // And no question below the top of the review — not on a group, not on an
    // item. Grouping the body must never multiply the moments a human commits.
    let questions: Vec<&String> = keys
        .iter()
        .filter(|(_, k)| k == "question")
        .map(|(p, _)| p)
        .collect();
    assert_eq!(questions, vec![".review.question"], "{questions:?}");

    for g in v["review"]["groups"].as_array().unwrap() {
        let g_keys: Vec<&str> = g.as_object().unwrap().keys().map(|k| k.as_str()).collect();
        let mut sorted = g_keys.clone();
        sorted.sort_unstable();
        assert_eq!(
            sorted,
            vec!["change", "counts", "items", "kind", "label", "removed"],
            "a group carries indices and a tally and NOTHING answerable: {g}"
        );
        // Indices, not copies: a group cannot hold a second description of an
        // item, so it cannot hold a second decision about one either.
        assert!(
            g["items"].as_array().unwrap().iter().all(|i| i.is_u64()),
            "group items are indices into review.items: {g}"
        );
    }
}

/// No consumer breakage: `trust-card-diff-v1` still serves the exact shape it
/// shipped with. Grouping rides alongside it, never through it.
#[test]
fn a_shipped_card_payload_still_serves_its_old_shape() {
    let bin = env!("CARGO_BIN_EXE_agentstack");
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = fixture(tmp.path());
    lock(bin, &home, &proj);
    grant(bin, &home, &proj);
    let v = preview(bin, &home, &proj);

    let features: Vec<&str> = v["features"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f.as_str().unwrap())
        .collect();
    for shipped in [
        "trust-preview",
        "trust-review-card-v1",
        "trust-card-diff-v1",
    ] {
        assert!(
            features.contains(&shipped),
            "a UI gating on '{shipped}' keeps its working loop: {features:?}"
        );
    }

    // Every field `trust-card-diff-v1` shipped with is still served: removing
    // one would break a panel that renders it today.
    let shipped_fields = vec![
        "change",
        "contacts",
        "diff",
        "identity",
        "kind",
        "may_read",
        "name",
        "pin",
        "prior_pin",
        "recognized_other_projects",
        "runs",
    ];
    // A field may be ADDED only when a feature name announces it, which is the
    // flag a strict decoder gates on (`ui_contract.rs`: adding a read or a
    // feature is backward-compatible and does not bump `SCHEMA_VERSION`). So
    // this is not a free-for-all: every extra key must be listed here, and
    // adding one to this list is the reviewable moment.
    //
    // `drifted` / `fix` — announced by `trust-content-drift-v1`: whether this
    // item's approved bytes moved, and the one command that re-pins them.
    let announced_additions = ["drifted", "fix"];
    let mut expected = shipped_fields.clone();
    expected.extend(announced_additions.iter().copied());
    expected.sort_unstable();
    assert!(
        features.contains(&"trust-content-drift-v1"),
        "the added item fields must be announced by a feature name a decoder can gate on: \
         {features:?}"
    );
    let items = v["review"]["items"].as_array().unwrap();
    assert!(!items.is_empty());
    for item in items {
        let mut keys: Vec<&str> = item
            .as_object()
            .unwrap()
            .keys()
            .map(|k| k.as_str())
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, expected, "trust-card-diff-v1 item shape: {item}");
    }

    // And the fields the review object shipped with still mean what they meant.
    for field in ["re_review", "prior_recorded", "items", "removed"] {
        assert!(
            v["review"].get(field).is_some(),
            "review.{field} still served: {}",
            v["review"]
        );
    }
    assert!(v["review"]["removed"].is_array());
    // Every item reads `unchanged` right after its own grant — the identity
    // parity `trust-card-diff-v1` promises, unmoved by the grouping.
    assert!(
        items.iter().all(|i| i["change"] == "unchanged"),
        "grouping did not disturb the marker computation: {items:?}"
    );
}

// ---------------------------------------------------------------------------
// Piece 2 — `run` locked by default
// ---------------------------------------------------------------------------

/// The default flipped to the Protected tier; plain host mode is now something
/// you ask for by name. The isolation opt-ins and their honest posture labels
/// are exactly as they were — this changed the gate, not the isolation story.
#[test]
fn run_is_locked_by_default_and_host_mode_is_explicit() {
    let bin = env!("CARGO_BIN_EXE_agentstack");
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = fixture(tmp.path());
    lock(bin, &home, &proj);
    grant(bin, &home, &proj);

    // The opt-out exists and says what it costs.
    let (help, ok) = run(bin, &["run", "--help"], &home, &proj);
    assert!(ok, "{help}");
    assert!(help.contains("--unprotected"), "{help}");
    assert!(
        help.contains("--sandbox") && help.contains("--lockdown"),
        "the isolation opt-ins are still offered: {help}"
    );

    // A bare `run` takes the protected path: it reaches the gates. The harness
    // binary is absent from this PATH either way, so the discriminator is WHICH
    // failure arrives — the protected run resolves and freezes before launch,
    // the unprotected one goes straight at the binary.
    let (protected, ok) = run(bin, &["run", "claude-code"], &home, &proj);
    assert!(!ok, "no claude-code on this PATH:\n{protected}");
    let (unprotected, ok) = run(bin, &["run", "claude-code", "--unprotected"], &home, &proj);
    assert!(!ok, "no claude-code on this PATH:\n{unprotected}");
    assert_ne!(
        protected, unprotected,
        "the default and the opt-out are not the same run"
    );

    // The contradiction refuses rather than letting flag order decide.
    let (both, ok) = run(
        bin,
        &["run", "claude-code", "--locked", "--unprotected"],
        &home,
        &proj,
    );
    assert!(!ok);
    assert!(both.contains("contradict each other"), "{both}");

    // `--locked` still routes exactly where it did, including its own named
    // limitation. Nothing an existing script types behaves differently.
    let (combo, ok) = run(
        bin,
        &["run", "claude-code", "--locked", "--sandbox"],
        &home,
        &proj,
    );
    assert!(!ok);
    assert!(
        combo.contains("--locked --sandbox/--lockdown is not wired yet"),
        "{combo}"
    );

    // `--plan` is the cheapest proof of the new default: it used to refuse
    // ("--plan needs a run mode") because no mode flag was given, and now it
    // prints the protected plan, because the protected run IS the mode.
    let (plan, _) = run(bin, &["run", "claude-code", "--plan"], &home, &proj);
    assert!(
        !plan.contains("--plan needs a gated run mode"),
        "a bare --plan is the protected plan now:\n{plan}"
    );
    assert!(
        plan.to_lowercase().contains("trust") || plan.contains("PROTECTED"),
        "and it is the PROTECTED plan, gates and all:\n{plan}"
    );

    // The posture labels are untouched — a plan prints the same honest words
    // about what each tier does and does not enforce.
    let (sandbox_plan, _) = run(
        bin,
        &["run", "claude-code", "--sandbox", "--plan"],
        &home,
        &proj,
    );
    assert!(
        sandbox_plan.contains("SANDBOX / PROXIED · DIRECT ROUTE OPEN"),
        "sandbox posture label unchanged:\n{sandbox_plan}"
    );
    let (lockdown_plan, _) = run(
        bin,
        &["run", "claude-code", "--lockdown", "--plan"],
        &home,
        &proj,
    );
    assert!(
        lockdown_plan.contains("LOCKDOWN / ENFORCED · NO DIRECT ROUTE"),
        "lockdown posture label unchanged:\n{lockdown_plan}"
    );
}

/// The point of the flip. A project nobody trusted, and a project whose bytes
/// moved since the yes, are both refused by the plain `run` — and the refusal
/// names the way out rather than leaving the user guessing.
#[test]
fn an_untrusted_or_drifted_project_fails_closed_under_the_new_default() {
    let bin = env!("CARGO_BIN_EXE_agentstack");
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = fixture(tmp.path());
    lock(bin, &home, &proj);
    // A harness binary that exists and does nothing. Without it the launcher
    // refuses on "not on your PATH" before any gate runs, which would make this
    // test pass for the wrong reason — the run must be stopped BY THE GATE.
    let stub = tmp.path().join("bin");
    fs::create_dir_all(&stub).unwrap();
    let claude = stub.join("claude");
    fs::write(&claude, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&claude, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let with_stub = |args: &[&str]| -> (String, bool) {
        let out = std::process::Command::new(bin)
            .args(args)
            .current_dir(&proj)
            .env_clear()
            .env("HOME", &home)
            .env("AGENTSTACK_HOME", home.join(".agentstack"))
            .env("PATH", format!("{}:/usr/bin:/bin", stub.display()))
            .stdin(std::process::Stdio::null())
            .output()
            .expect("spawn agentstack");
        (
            format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ),
            out.status.success(),
        )
    };

    // Never trusted: the default run refuses at the trust gate, with the
    // harness sitting right there, runnable, unrun.
    let (untrusted, ok) = with_stub(&["run", "claude-code"]);
    assert!(!ok, "an untrusted project must not launch:\n{untrusted}");
    assert!(
        untrusted.contains("trust"),
        "the refusal names the gate and its fix:\n{untrusted}"
    );

    // The contrast that makes it a gate rather than an accident: the same
    // untrusted project, the same binary, the explicit opt-out — and it runs.
    // That is exactly what `--unprotected` promises, and exactly why it is
    // spelled that way.
    let (opted_out, ok) = with_stub(&["run", "claude-code", "--unprotected"]);
    assert!(
        ok,
        "the opt-out skips the gate by design — that is the whole flag:\n{opted_out}"
    );
    assert!(
        opted_out.contains("HOST / ADVISORY"),
        "and it says so, in the unchanged posture label:\n{opted_out}"
    );
    assert!(
        opted_out.contains("--unprotected"),
        "the banner names what was turned off:\n{opted_out}"
    );

    // Trusted, then the manifest moves: the yes no longer covers these bytes.
    grant(bin, &home, &proj);
    fs::write(
        manifest_path(&proj),
        format!("{MANIFEST}\n[servers.evil]\ntype = \"stdio\"\ncommand = \"curl\"\n"),
    )
    .unwrap();
    let (drifted, ok) = with_stub(&["run", "claude-code"]);
    assert!(!ok, "a drifted project must not launch:\n{drifted}");
    assert!(drifted.contains("trust"), "{drifted}");
}

// ---------------------------------------------------------------------------
// Piece 3 — Varlock productization
// ---------------------------------------------------------------------------

/// `init` offers the vault's opt-in, and `doctor` answers for its health in the
/// section that already owns secrets.
#[test]
fn init_offers_an_env_schema_and_doctor_reports_varlock_health() {
    let bin = env!("CARGO_BIN_EXE_agentstack");
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let proj = tmp.path().join("proj");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&proj).unwrap();
    // One detected CLI carrying an inline token, so init has a name to lift and
    // therefore a schema worth offering.
    fs::write(
        home.join(".claude.json"),
        r#"{"mcpServers":{"search":{"command":"npx","args":["search-mcp"],"env":{"SEARCH_TOKEN":"sk-live-x"}}}}"#,
    )
    .unwrap();

    let (out, ok) = run(bin, &["init", "--secrets", "skip", "--yes"], &home, &proj);
    assert!(ok, "init failed:\n{out}");
    assert!(
        out.contains("varlock") && out.contains(".env.schema"),
        "init offers the vault's opt-in by name:\n{out}"
    );
    // Non-interactive, so the offer is declined without prompting — the CI and
    // t3code contract. A scripted init writes exactly what it wrote before.
    assert!(
        !proj.join(".env.schema").exists(),
        "an unanswered offer writes nothing"
    );

    // Doctor's turn, over a project that actually references a secret — a
    // project with nothing to resolve has nothing to say about a vault.
    let secretful = tmp.path().join("secretful/.agentstack");
    fs::create_dir_all(&secretful).unwrap();
    fs::write(
        secretful.join("agentstack.toml"),
        "version = 1\n\n[servers.search]\ntype = \"stdio\"\ncommand = \"npx\"\n\
         args = [\"search-mcp\"]\nenv = { SEARCH_TOKEN = \"${SEARCH_TOKEN}\" }\n",
    )
    .unwrap();
    let proj = secretful.parent().unwrap().to_path_buf();

    // Not opted in: doctor still names varlock as the recommended vault. A
    // recommendation, never a defect, so it must not become a next action.
    let (before, _) = run(bin, &["doctor"], &home, &proj);
    assert!(
        before.contains("varlock"),
        "the Secrets section teaches the vault:\n{before}"
    );
    assert!(
        !before.contains("next: ") || !before.contains("varlock ↳"),
        "a recommendation never becomes the one next action:\n{before}"
    );

    // Opting in with no varlock binary installed is the silent-degradation case
    // doctor exists to break: every ref quietly falls through to the next store.
    // Next to the manifest, which is where the resolution chain looks for it
    // (`Chain::default_for_dir` is handed the manifest dir) — the same place
    // `init` offers to write one.
    fs::write(secretful.join(".env.schema"), "# ---\nSEARCH_TOKEN=\n").unwrap();
    let (after, _) = run(bin, &["doctor"], &home, &proj);
    assert!(
        after.contains("varlock") && after.contains(".env.schema"),
        "varlock health is reported from the schema:\n{after}"
    );
    assert!(
        after.contains("varlock.dev") || after.contains("install varlock"),
        "the finding names its fix:\n{after}"
    );
    // In the Secrets section, not a check family of its own.
    let secrets_section = after.split("Secrets").nth(1).unwrap_or("");
    assert!(
        secrets_section.contains("varlock"),
        "varlock health sits under Secrets:\n{after}"
    );
}

/// The mechanism-unchanged guard. Surfacing varlock must not have touched
/// `${REF}` resolution: an unresolved reference still blocks the write, and it
/// still blocks with a `.env.schema` present and varlock unavailable. A blocked
/// write is the feature.
#[test]
fn an_unresolved_ref_still_fails_closed() {
    let bin = env!("CARGO_BIN_EXE_agentstack");
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let proj = tmp.path().join("proj");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(proj.join(".agentstack")).unwrap();
    fs::write(
        proj.join(".agentstack/agentstack.toml"),
        r#"version = 1

[servers.search]
type = "stdio"
command = "npx"
args = ["search-mcp"]
env = { SEARCH_TOKEN = "${NOPE_TOKEN}" }
"#,
    )
    .unwrap();

    for (label, schema) in [("no schema", false), ("with .env.schema", true)] {
        if schema {
            // Opted into varlock, but varlock is not on this PATH: the layer is
            // skipped, exactly as before, and the ref stays unresolved.
            fs::write(proj.join(".agentstack/.env.schema"), "# ---\nNOPE_TOKEN=\n").unwrap();
        }
        let (text, ok) = run(bin, &["apply", "--write"], &home, &proj);
        assert!(
            !ok,
            "{label}: an unresolved ref must block the write:\n{text}"
        );
        assert!(
            text.contains("NOPE_TOKEN"),
            "{label}: the blocked write names the ref:\n{text}"
        );
        // The value never reached a native config, which is the whole point.
        assert!(
            !home.join(".claude.json").exists(),
            "{label}: nothing was written"
        );
    }
}
