// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! P3.7 — does a machine-readable status surface say only what the evidence
//! supports?
//!
//! Three symptoms, one question. Each gets a witness here, plus the two
//! properties that make the fix a *contract revision* rather than an edit:
//!
//! 1. `state` said `ready` over an untrusted, never-activated project. Zero
//!    findings was true; "ready" was not, because nothing the project declares
//!    was live. Fixed additively — `readiness` is the honest field and `state`
//!    keeps its `status-v1` meaning, so the panel rendering "Ready" today does
//!    not silently change meaning under its users.
//! 2. `snapshot` emitted a plural `nextActions` where `doctor` and `status`
//!    both settle on one. A list where a decision belongs.
//! 3. `doctor` printed a green `✓ <REF> resolved from env` for a ref that
//!    `[policy.secrets]` refuses for every server referencing it — the same
//!    vacuous-green shape P3.1 removed, one section over, and directly
//!    contradicted by the Error the Policy section already raises.
//!
//! The two contract properties are the load-bearing ones: **`status-v1` is
//! untouched** and **the new name is advertised**. Without them this is three
//! bug fixes; with them it is a surface a consumer can migrate onto.

use std::fs;
use std::sync::Mutex;

use agentstack::commands::doctor;

// doctor mutates the process-global HOME; serialize these tests.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Point HOME/AGENTSTACK_HOME at the sandbox and write `manifest` into a fresh
/// project. Nothing is trusted and nothing is locked — which is exactly the
/// state symptom 1 is about.
fn project(tmp: &std::path::Path, manifest: &str) -> std::path::PathBuf {
    let home = tmp.join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    let proj = tmp.join("proj");
    fs::create_dir_all(proj.join(".agentstack")).unwrap();
    fs::write(proj.join(".agentstack/agentstack.toml"), manifest).unwrap();
    proj
}

// ---------------------------------------------------------------- symptom 1

/// The bug, stated as a property: a project nobody has reviewed and nothing
/// has activated is not ready, and the surface must not say it is.
#[test]
fn an_untrusted_never_activated_project_is_not_reported_ready() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project(
        tmp.path(),
        "version = 1\n[servers.demo]\ntype = \"stdio\"\ncommand = \"echo\"\n",
    );

    let report = doctor::collect(Some(&proj)).unwrap();
    let readiness = report["readiness"]
        .as_str()
        .unwrap_or_else(|| panic!("readiness must be present on every report: {report}"));

    assert_ne!(
        readiness, "ready",
        "an untrusted / never-activated project must not be reported ready — \
         this is the whole of P3.7 symptom 1: {report}"
    );
    // And it must say WHICH, so the reader knows what stands between here and
    // live. "not ready" with no reason is the dead end P3.1 removed.
    assert!(
        matches!(
            readiness,
            "untrusted" | "drifted" | "never_activated" | "needs_attention"
        ),
        "readiness must name what is missing, got {readiness:?}: {report}"
    );
}

/// The other half of symptom 1, and the reason this is a *revision*: `state`
/// still answers its own, narrower `status-v1` question. If this test ever
/// fails, an existing consumer's "Ready" chip changed meaning without them
/// opting in — the exact failure the versioned approach exists to prevent.
#[test]
fn status_v1_state_semantics_are_unchanged() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project(tmp.path(), "version = 1\n");

    let report = doctor::collect(Some(&proj)).unwrap();

    assert_eq!(
        report["state"].as_str(),
        Some("ready"),
        "`state` must keep meaning 'no check found anything to repair' — it is \
         status-v1 and has external consumers: {report}"
    );
    // The P3.1 seam must survive — but "survive" is about the FIELD, which is
    // what this test was written to protect, and the old assertion overstated
    // it into "and it is never null here".
    //
    // That extra clause encoded an accident of this one fixture, not the
    // contract. `next_action` is `string | null` and always has been: the Group
    // and Verified rungs — the largest healthy states on either surface — answer
    // `null` from this same field, and `guidance_is_executable.rs` documents
    // `null` as "the terminal answer, and a driver that reads it stops". A
    // consumer that cannot take a null here was already broken on projects far
    // commoner than an empty one.
    //
    // It also forced a wrong answer. The only command an empty project could be
    // given was `agentstack search`, a read-only browse that changes nothing —
    // so a driver polling this field ran it and was handed it again, forever.
    // The honest answer in a state whose one missing input is a human decision
    // is that there is no command, and that is what `null` says.
    //
    // So: the key is present, and its value is a non-empty command or an
    // explicit null. Never a missing key, never an empty string.
    let next = &report["next_action"];
    assert!(
        next.is_null() || next.as_str().is_some_and(|s| !s.is_empty()),
        "the P3.1 seam must stay present and typed — a command, or an explicit \
         null meaning there is none: {report}"
    );
}

/// `readiness` is a decision, not free text: every value a consumer may see is
/// one of the documented set. A typo'd or invented state is worse than a
/// missing one, because a UI will render it verbatim.
#[test]
fn readiness_is_drawn_from_the_documented_set() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    const KNOWN: &[&str] = &[
        "ready",
        "needs_attention",
        "untrusted",
        "drifted",
        "never_activated",
        "empty",
        "unknown",
        "needs_setup",
    ];

    for manifest in [
        "version = 1\n",
        "version = 1\n[servers.demo]\ntype = \"stdio\"\ncommand = \"echo\"\n",
        "version = 1\n\n[targets]\nclaude = true\n",
        "version = 1\n[servers.demo]\ntype = \"stdio\"\ncommand = \"echo\"\n\
         env = { TOKEN = \"${NOWHERE_TOKEN}\" }\n",
    ] {
        let tmp = assert_fs::TempDir::new().unwrap();
        let proj = project(tmp.path(), manifest);
        let report = doctor::collect(Some(&proj)).unwrap();
        let readiness = report["readiness"].as_str().unwrap_or_default();
        assert!(
            KNOWN.contains(&readiness),
            "undocumented readiness {readiness:?} for manifest {manifest:?}"
        );
    }

    // The pre-manifest payload is hand-written rather than built from
    // `Report::to_json`, so it is the one place the key can silently go
    // missing. Assert it is there, in the source, with a documented value.
    let src = include_str!("../src/commands/doctor.rs");
    assert!(
        src.contains(r#""readiness": "needs_setup""#),
        "the no-manifest JSON must carry readiness too — a key that vanishes on \
         the least-informed path is worse than one that never existed"
    );
}

// ---------------------------------------------------------------- symptom 2

/// `snapshot` now answers with one action as well as the list. The list stays
/// (its consumers are external and unversioned-in-this-repo); the singular is
/// what a panel should render.
#[test]
fn the_snapshot_offers_one_next_action_beside_the_list() {
    let src = include_str!("../src/snapshot.rs");
    assert!(
        src.contains(r#""nextActions": next_actions"#),
        "the plural array must survive — removing it is a breaking change to a \
         contract this item is explicitly not allowed to mutate"
    );
    assert!(
        src.contains(r#""nextAction": one_next_action(&next_actions)"#),
        "the singular decision must be emitted beside it"
    );
}

/// The singular is the *most severe* action, not merely the first — otherwise
/// it is a different arbitrary pick rather than a decision.
#[test]
fn the_one_next_action_prefers_severity_over_position() {
    let src = include_str!("../src/snapshot.rs");
    let body = src
        .split("fn one_next_action")
        .nth(1)
        .expect("one_next_action must exist");
    let head = &body[..body.len().min(600)];
    let error_at = head.find(r#"by_level("error")"#);
    let warn_at = head.find(r#"by_level("warn")"#);
    assert!(
        error_at.is_some() && warn_at.is_some() && error_at < warn_at,
        "errors must be preferred to warnings, and both to list position"
    );
}

// ---------------------------------------------------------------- symptom 3

/// A ref that resolves but that `[policy.secrets]` refuses for every server
/// referencing it must not be reported as a green pass. It resolves; nothing
/// can read it; saying "✓ resolved from env" tells the user the opposite of
/// what will happen at apply/gateway time.
#[test]
fn a_policy_refused_ref_is_not_a_green_resolved_line() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project(
        tmp.path(),
        "version = 1\n\
         [servers.demo]\n\
         type = \"stdio\"\n\
         command = \"echo\"\n\
         env = { TOKEN = \"${DEMO_TOKEN}\" }\n\
         \n\
         [policy.secrets]\n\
         demo = [\"SOMETHING_ELSE\"]\n",
    );
    // Make it genuinely resolvable, so the only reason not to print green is
    // the policy refusal — this is the exact state the bug reported as ✓.
    std::env::set_var("DEMO_TOKEN", "real-value");

    let report = doctor::collect(Some(&proj)).unwrap();
    std::env::remove_var("DEMO_TOKEN");

    let mut found = false;
    for section in report["sections"].as_array().into_iter().flatten() {
        if section["title"].as_str() != Some("Secrets") {
            continue;
        }
        for line in section["lines"].as_array().into_iter().flatten() {
            let msg = line["msg"].as_str().unwrap_or_default();
            if !msg.contains("DEMO_TOKEN") {
                continue;
            }
            found = true;
            assert_ne!(
                line["level"].as_str(),
                Some("ok"),
                "a ref refused by [policy.secrets] for every referencing server \
                 must not be a green pass: {msg}"
            );
            assert!(
                msg.contains("policy.secrets"),
                "the line must say WHY it is not a pass, in the reader's terms: {msg}"
            );
        }
    }
    assert!(
        found,
        "the Secrets section must still report the ref: {report}"
    );
}

/// The counterpart, so the fix is a discrimination rather than a blanket
/// downgrade: a ref the policy allows still reports the plain green line.
/// Without this, "never say green" would pass the test above and be useless.
#[test]
fn an_allowed_ref_still_reports_a_plain_green_line() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project(
        tmp.path(),
        "version = 1\n\
         [servers.demo]\n\
         type = \"stdio\"\n\
         command = \"echo\"\n\
         env = { TOKEN = \"${DEMO_TOKEN}\" }\n",
    );
    std::env::set_var("DEMO_TOKEN", "real-value");
    let report = doctor::collect(Some(&proj)).unwrap();
    std::env::remove_var("DEMO_TOKEN");

    let mut saw_green = false;
    for section in report["sections"].as_array().into_iter().flatten() {
        if section["title"].as_str() != Some("Secrets") {
            continue;
        }
        for line in section["lines"].as_array().into_iter().flatten() {
            let msg = line["msg"].as_str().unwrap_or_default();
            if msg.contains("DEMO_TOKEN") && line["level"].as_str() == Some("ok") {
                saw_green = true;
                assert!(
                    !msg.contains("policy.secrets"),
                    "an unrefused ref should get the plain line, not a caveat: {msg}"
                );
            }
        }
    }
    assert!(
        saw_green,
        "a resolvable ref no policy refuses must still read as a pass: {report}"
    );
}

// ------------------------------------------------------- the contract itself

/// The revision is only usable if a consumer can detect it. `status-honesty-v1`
/// must be advertised, and every name shipped before it must still be — a
/// feature list that loses a name breaks gating for every UI that reads it.
#[test]
fn the_new_contract_is_advertised_and_nothing_older_was_dropped() {
    let features = agentstack::ui_contract::FEATURES;
    assert!(
        features.contains(&"status-honesty-v1"),
        "the new contract must be advertised or no consumer can opt in"
    );
    assert!(
        features.contains(&"status-v1"),
        "status-v1 must remain advertised: it is untouched, and withdrawing it \
         would tell consumers a breaking change happened when none did"
    );
    assert_eq!(
        agentstack::ui_contract::SCHEMA_VERSION,
        1,
        "this change is additive — bumping the schema version would tell every \
         panel to disable itself over fields none of them read yet"
    );
}

// ------------------------------------------- the two trust readings, paired

/// The same honesty question, one field over: does `status --json` say
/// something the trust gate contradicts?
///
/// It did. `trust_relevant` is a DELIVERY-POSTURE hint — true when a bridge is
/// registered or the derived mode is one the gate itself produces — but the
/// trust gate reaches all six declared kinds in every mode. So a static,
/// no-gateway project reported `trust_relevant: false` while `apply --write`
/// refused its servers and its instructions, and a consumer deciding "do I
/// need to mention trust to this user?" was told no exactly when the answer
/// was yes.
///
/// The fix is the one symptom 1 above already established as this file's
/// pattern: **fixed additively.** `trust_relevant` keeps its shipped value
/// byte for byte, because changing what an existing name MEANS is a
/// schema-version bump under `ui_contract`'s own rule — and this file already
/// pins `SCHEMA_VERSION == 1` on the grounds that bumping it tells every panel
/// to disable itself. `trust_blocks_delivery` is the honest field beside it.
///
/// This is the end-to-end half of the witness: the pure truth table lives in
/// `commands::overview::tests::the_trust_relevance_truth_table`, and what is
/// pinned HERE is the pairing that table cannot reach — the two field values
/// standing next to a real refusal from a real `apply --write`.
#[test]
fn a_static_untrusted_project_admits_the_gate_that_refuses_its_writes() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();

    // Deliberately NOT a servers-only project: the emitted counts are
    // `servers` and `skills`, so an instruction fragment is content a consumer
    // provably cannot see in the payload. It is also the kind that makes
    // "derive it yourself from the counts" impossible rather than merely
    // awkward.
    let proj = project(
        tmp.path(),
        "version = 1\n\
         [delivery]\nrender_locally = true\n\
         [targets]\ndefault = [\"claude-code\"]\n\
         [servers.demo]\ntype = \"stdio\"\ncommand = \"/bin/echo\"\n\
         [instructions.house]\npath = \"./house.md\"\n",
    );
    fs::write(proj.join(".agentstack/house.md"), "House rule one.\n").unwrap();

    let body = agentstack::commands::overview::status_body(Some(&proj)).unwrap();
    let p = &body["project"];

    // Nothing is rendered and nothing is locked, so the derived mode is
    // `static` and no bridge exists — the exact row the finding names.
    assert_eq!(p["mode"], "static", "the fixture is the static row: {p}");
    assert_eq!(p["gateway_connected"], false);
    assert_eq!(p["trust"], "untrusted");

    // The posture hint says false. That is its shipped answer and it stays.
    assert_eq!(
        p["trust_relevant"], false,
        "trust_relevant keeps its delivery-posture meaning; changing it under \
         the same name is the schema bump this file refuses: {p}"
    );

    // And a consumer cannot recover the truth from the rest of the payload:
    // the instruction fragment the gate refuses is not counted anywhere in it.
    assert_eq!(p["skills"], 0);
    assert!(
        p.get("instructions").is_none(),
        "if this key ever appears, revisit whether the new field is still the \
         only way to see declared-but-uncounted content: {p}"
    );

    // The honest field says what the gate is about to do.
    assert_eq!(
        p["trust_blocks_delivery"], true,
        "the gate stands between this project's content and every harness: {p}"
    );

    // Now prove it, rather than asserting a claim about a claim.
    let err = agentstack::commands::apply::run(
        &agentstack::cli::ApplyArgs {
            verbose: false,
            targets: vec![],
            profile: None,
            dry_run: false,
            write: true,
            scope: Some(agentstack::scope::Scope::Project),
            allow_unresolved: false,
            prune_foreign: false,
            no_gitignore: true,
        },
        Some(&proj),
    )
    .expect_err("an untrusted project's render is refused, not written");
    let err = err.to_string();
    assert!(
        err.contains("blocked"),
        "the refusal is a blocked write, not a partial success: {err}"
    );

    // The refusal reached the whole declared surface, not just the servers —
    // which is why a mode-shaped hint could never have answered for it.
    assert!(
        !proj.join(".mcp.json").exists(),
        "a refused render writes no server config"
    );

    std::env::remove_var("HOME");
    std::env::remove_var("AGENTSTACK_HOME");
}

/// The counterpart, so the field is a discrimination rather than a constant.
///
/// Same project, same `static` mode, same `trust_relevant: false` — trusted.
/// Without this, `trust_blocks_delivery: true` everywhere would pass the test
/// above and mean nothing.
#[test]
fn the_same_project_stops_claiming_a_gate_once_it_is_trusted() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project(
        tmp.path(),
        "version = 1\n\
         [delivery]\nrender_locally = true\n\
         [targets]\ndefault = [\"claude-code\"]\n\
         [servers.demo]\ntype = \"stdio\"\ncommand = \"/bin/echo\"\n",
    );

    agentstack::trust::trust_unreviewed(&proj).unwrap();
    let p = &agentstack::commands::overview::status_body(Some(&proj)).unwrap()["project"];
    assert_eq!(p["trust"], "trusted");
    assert_eq!(
        p["trust_blocks_delivery"], false,
        "a trusted project has no gate in the way: {p}"
    );

    // And an EMPTY project is false for the other reason: the gate is up, but
    // there is nothing declared for it to block. Both halves of the predicate
    // are load-bearing, so both are witnessed.
    // Its own sandbox: `project` always writes `<tmp>/proj`, so reusing this
    // one would overwrite the manifest just trusted above and read `drifted`.
    let tmp2 = assert_fs::TempDir::new().unwrap();
    let empty = project(tmp2.path(), "version = 1\n");
    let p = &agentstack::commands::overview::status_body(Some(&empty)).unwrap()["project"];
    assert_eq!(p["trust"], "untrusted");
    assert_eq!(
        p["trust_blocks_delivery"], false,
        "an untrusted project that declares nothing has nothing blocked: {p}"
    );

    std::env::remove_var("HOME");
    std::env::remove_var("AGENTSTACK_HOME");
}

// --------------------------------------------------- symptom 4 (G27): routing

/// A manifest that declares one server and renders locally — the `static` lane.
const ONE_SERVER: &str = "version = 1\n\
     [delivery]\nrender_locally = true\n\
     [targets]\ndefault = [\"claude-code\"]\n\
     [servers.demo]\ntype = \"stdio\"\ncommand = \"/bin/echo\"\n";

/// The one next action as BOTH surfaces of `status --json` state it: the
/// machine command a driver may run, and the human sentence the screen prints.
///
/// Returned as one tuple on purpose. The G27 defect was precisely a divergence
/// between the two — the sentence named `agentstack toolset create <name>
/// --server <server>` while the command was `null` — so a witness that reads
/// only one of them cannot see the class of bug at all.
fn next_action(proj: &std::path::Path) -> (Option<String>, String, String) {
    let body = agentstack::commands::overview::status_body(Some(proj)).unwrap();
    let n = &body["next_action"];
    (
        n["command"].as_str().map(str::to_string),
        n["sentence"].as_str().unwrap().to_string(),
        n["why"].as_str().unwrap().to_string(),
    )
}

/// G27, stated as a property: when the trust gate is what blocks delivery, the
/// one next action names the review — as a command a driver can run verbatim.
///
/// The defect: a static, rendered, UNTRUSTED project emitted `next_action:
/// null`. Two things combined. `next_step`'s trust arm was guarded on
/// `trust_relevant`, a DELIVERY-POSTURE hint that is false for a static project
/// with no bridge, so routing fell through to the setup ladder and landed on
/// the Group rung; that rung's honest human answer is a shape (`toolset create
/// <name> --server <server>`), and `machine_command` drops shapes on purpose,
/// because a driver cannot resolve `<name>`. So the one field a panel reads for
/// "what should this person do" said nothing, in the exact state where the
/// answer is a single concrete command — and `doctor`, whose ladder has carried
/// this rung all along, answered `agentstack trust .` for the same project.
///
/// The fix is a rung, not a special case: `trust_blocks_delivery` — the reading
/// the sibling field above already publishes — gates the whole setup ladder,
/// because the gate refuses every write those rungs would ask for. It sits
/// BELOW `adopt`, which still makes progress under the gate and rewrites the
/// very manifest a grant binds itself to.
///
/// This table is the ladder across trust states and delivery lanes. Both fields
/// are asserted on every row so they cannot drift apart again.
#[test]
fn the_gate_rung_names_the_review_and_leaves_every_other_rung_alone() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    // ── static lane, untrusted, content declared, and NOT YET LOCKED: the gate
    // is what blocks delivery, and the pin is what has to happen before the
    // grant that lifts it.
    //
    // This row expected `agentstack trust .` until G29. That expectation was
    // wrong, not merely superseded: a `trust` grant binds to the manifest
    // layers AND the lockfile, and this fixture has no `agentstack.lock`. A
    // grant bought here is void the moment `use --write` mints the lock the
    // grant was bound to — measured as `locked · trust stale (content
    // changed)`, i.e. the ladder sent the user to review, then asked them to
    // review again. `lock --write` first is the order the product already
    // states in `agentstack yes` and in `lock --write`'s own footer.
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project(tmp.path(), ONE_SERVER);
    assert!(
        !proj.join(".agentstack/agentstack.lock").exists(),
        "fixture: unlocked is what makes the review one rung too early"
    );
    let body = agentstack::commands::overview::status_body(Some(&proj)).unwrap();
    assert_eq!(body["project"]["mode"], "static");
    assert_eq!(
        body["project"]["trust_relevant"], false,
        "the posture hint is false here — routing must not depend on it"
    );
    assert_eq!(body["project"]["trust_blocks_delivery"], true);
    let (cmd, sentence, why) = next_action(&proj);
    assert_eq!(
        cmd.as_deref(),
        Some("agentstack lock --write"),
        "a driver gets a command it can run verbatim, not null — and the one \
         that actually terminates: `trust .` here buys a grant `use --write` voids"
    );
    assert_eq!(
        sentence, "agentstack lock --write",
        "and the screen says the same thing: {why}"
    );
    assert!(
        !sentence.contains('<'),
        "a rung added to fix a dropped command must not name a placeholder: {sentence}"
    );

    // ── same project, TRUSTED: the gate is down and the setup ladder resumes,
    // byte for byte. Without this row the new rung could answer everywhere.
    agentstack::trust::trust_unreviewed(&proj).unwrap();
    assert_eq!(
        next_action(&proj),
        (
            Some("agentstack apply --write".to_string()),
            "agentstack apply --write".to_string(),
            "render this setup into your CLIs".to_string()
        ),
        "a project the gate does not block keeps today's ladder"
    );

    // ── DRIFTED: content already approved has moved. Its own arm, above the
    // gate rung, with its own re-review wording — pinned so the new rung cannot
    // quietly take this state over.
    fs::write(
        proj.join(".agentstack/agentstack.toml"),
        format!("{ONE_SERVER}[servers.second]\ntype = \"stdio\"\ncommand = \"/bin/cat\"\n"),
    )
    .unwrap();
    // …but this project is STILL unlocked, and the pin comes before the review
    // whichever arm names it. G29's rewrite is applied to the ANSWER, not to one
    // arm, exactly so the six places that say `trust .` cannot disagree about
    // the order; drift is one of the six. The old expectation of `trust .` here
    // was wrong for the same reason as the row above — a re-review bought while
    // no lockfile exists is void once `use --write` mints one.
    let (cmd, sentence, _) = next_action(&proj);
    assert_eq!(cmd.as_deref(), Some("agentstack lock --write"));
    assert_eq!(sentence, "agentstack lock --write");

    // Pin it, and drift's OWN arm becomes observable — with its own re-review
    // wording, which is what this row has always been here to protect. This is
    // also the locked half of the rewrite: once the pins exist, the review is
    // exactly right and `correct_trust_rung` must leave it alone.
    agentstack::lock::Lock::default()
        .save(&proj.join(".agentstack"))
        .unwrap();
    let (cmd, sentence, why) = next_action(&proj);
    assert_eq!(
        cmd.as_deref(),
        Some("agentstack trust ."),
        "locked: the review is the answer again, untouched"
    );
    assert_eq!(sentence, "agentstack trust .");
    assert!(
        why.contains("changed"),
        "drift keeps the re-review wording, not the gate rung's: {why}"
    );

    // ── clean-at-rest lane, untrusted: a lockfile and nothing rendered. Trust
    // IS posture-relevant here, so the arm ABOVE the gate rung owns this state
    // and keeps its own wording. Pinned because both conditions now hold, and
    // the order between them is the thing that decides the sentence.
    let tmp2 = assert_fs::TempDir::new().unwrap();
    let car = project(tmp2.path(), ONE_SERVER);
    agentstack::lock::Lock::default()
        .save(&car.join(".agentstack"))
        .unwrap();
    let body = agentstack::commands::overview::status_body(Some(&car)).unwrap();
    assert_eq!(body["project"]["mode"], "clean-at-rest");
    assert_eq!(body["project"]["trust_relevant"], true);
    let (cmd, sentence, why) = next_action(&car);
    assert_eq!(cmd.as_deref(), Some("agentstack trust ."));
    assert_eq!(sentence, "agentstack trust .");
    assert!(
        why.contains("unlock"),
        "the posture arm keeps its shipped reason: {why}"
    );

    // ── untrusted and declaring NOTHING: the gate is up but holds nothing, so
    // this is a project the gate does not block and today's ladder stands —
    // including its `null` command, which is the honest answer for a shape.
    let tmp3 = assert_fs::TempDir::new().unwrap();
    let empty = project(tmp3.path(), "version = 1\n");
    assert_eq!(
        agentstack::commands::overview::status_body(Some(&empty)).unwrap()["project"]
            ["trust_blocks_delivery"],
        false
    );
    let (cmd, sentence, _) = next_action(&empty);
    // The sentence is prose. It used to read `agentstack search <query>`, which
    // looks runnable and is not (G28), so the screen promised a command the
    // reader could not type.
    assert_eq!(
        sentence,
        "find a server or skill to add — only you know what this project needs"
    );
    // The machine field beside it is null. `agentstack search` used to sit here
    // because it runs without a query — but running is not the bar for a next
    // ACTION, moving the project is, and a read-only browse never does. The
    // pair is pinned in full by
    // `a_rung_with_no_runnable_command_says_why_the_machine_field_is_empty`.
    assert_eq!(
        cmd, None,
        "a browse that leaves every field identical is not the one next action"
    );

    std::env::remove_var("HOME");
    std::env::remove_var("AGENTSTACK_HOME");
}

/// The end-to-end half: the exact shape the finding describes — static,
/// RENDERED, untrusted — with the refusal it suffers measured in the same test.
///
/// The table above reaches the routing; it cannot reach this state, because
/// "rendered" is a fact in the state ledger that only a real `apply --write`
/// can write. That is what makes this row worth its cost: rendered is precisely
/// what pushed the ladder past the Apply rung and onto the Group rung, whose
/// placeholder `machine_command` then dropped.
#[test]
fn a_rendered_untrusted_project_names_the_review_its_refused_apply_needs() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project(tmp.path(), ONE_SERVER);

    let apply = |proj: &std::path::Path| {
        agentstack::commands::apply::run(
            &agentstack::cli::ApplyArgs {
                verbose: false,
                targets: vec![],
                profile: None,
                dry_run: false,
                write: true,
                scope: Some(agentstack::scope::Scope::Project),
                allow_unresolved: false,
                prune_foreign: false,
                no_gitignore: true,
            },
            Some(proj),
        )
    };

    // Trust, render, then withdraw: the project is now rendered AND untrusted.
    agentstack::trust::trust_unreviewed(&proj).unwrap();
    apply(&proj).expect("a trusted project renders");
    assert!(proj.join(".mcp.json").exists(), "the render landed");
    assert!(agentstack::trust::revoke(&proj).unwrap(), "trust withdrawn");

    // Declare one more server, so there is a write outstanding for the gate to
    // refuse rather than an "already in sync" no-op.
    fs::write(
        proj.join(".agentstack/agentstack.toml"),
        format!("{ONE_SERVER}[servers.second]\ntype = \"stdio\"\ncommand = \"/bin/cat\"\n"),
    )
    .unwrap();

    let body = agentstack::commands::overview::status_body(Some(&proj)).unwrap();
    let p = &body["project"];
    assert_eq!(p["mode"], "static", "the fixture is the static row: {p}");
    assert_eq!(p["trust"], "untrusted");
    assert_eq!(p["gateway_connected"], false);
    assert_eq!(p["trust_relevant"], false);
    assert_eq!(p["trust_blocks_delivery"], true);
    assert_eq!(
        p["toolsets"].as_array().map(Vec::len),
        Some(0),
        "ungrouped, which is what used to select the placeholder rung: {p}"
    );

    // The refusal, measured — not asserted about.
    let err = apply(&proj)
        .expect_err("an untrusted project's render is refused")
        .to_string();
    assert!(err.contains("blocked"), "a blocked write: {err}");

    // ...and the one next action names the command that STARTS lifting it.
    //
    // Rendering does not pin: `apply --write` writes the CLIs' own config, it
    // does not write `agentstack.lock`, so this project is rendered and still
    // unlocked. That is why the answer here is the pin and not the review.
    assert!(
        !proj.join(".agentstack/agentstack.lock").exists(),
        "a render is not a pin — that is the whole reason this row moved"
    );
    let (cmd, sentence, _) = next_action(&proj);
    assert_eq!(
        cmd.as_deref(),
        Some("agentstack lock --write"),
        // This field was `null` before G27 and `agentstack trust .` between G27
        // and G29. Both were wrong for the same reason in the end: the grant
        // binds to the lockfile, and a grant made before the pins exist is void
        // the moment `use --write` writes them. The G27 fix — never answer
        // `null` in a state that has a one-command answer — still holds; only
        // WHICH command was wrong.
        "the gate is real, but the pin comes first — a grant made now is void \
         once `use --write` mints the lock it was bound to"
    );
    assert_eq!(sentence, "agentstack lock --write");

    std::env::remove_var("HOME");
    std::env::remove_var("AGENTSTACK_HOME");
}

// ------------------------------------------------ symptom 5 (G29): the order

/// **G29.** The ladder must never send anyone to `trust` while the project is
/// unlocked — on either surface.
///
/// The trap, measured before the fix: `add server --write` left the manifest
/// declaring a server and no `agentstack.lock`, `status` printed
/// `not locked (never activated)` and `Next: agentstack trust .` on consecutive
/// lines, and obeying it bought a grant that the next `use --write` destroyed
/// by minting the lockfile the grant was bound to. `agentstack trust --help`
/// states the mechanism itself: the grant pins "the manifest layers AND the
/// lockfile". The product already knew the order — `agentstack yes` prints
/// `adopt --write` → `lock --write` → `trust .`, and `lock --write`'s own
/// footer ends with "Next: `agentstack trust .`". Only the ladder disagreed.
///
/// The sentence and the machine command are pinned TOGETHER, and both surfaces
/// are read in one test, because a fix that moved one of the four could leave
/// the other three trapping users exactly as before.
#[test]
fn no_surface_names_the_review_before_anything_is_pinned() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project(tmp.path(), ONE_SERVER);

    // Unlocked, with content a lock would pin: the state the trap lives in.
    assert!(
        !proj.join(".agentstack/agentstack.lock").exists(),
        "fixture: this state is defined by having no lockfile"
    );

    let (cmd, sentence, why) = next_action(&proj);
    assert_eq!(
        cmd.as_deref(),
        Some("agentstack lock --write"),
        "an unlocked project's one next action is the pin, not the review"
    );
    assert_eq!(
        sentence, "agentstack lock --write",
        "sentence and command move together or they drift apart"
    );
    assert!(
        !sentence.starts_with("agentstack trust"),
        "the review is one rung too early here: {sentence}"
    );
    assert!(
        why.contains("trust") || why.contains("grant"),
        "the reason must say what the pin is FOR, or the user reads it as busywork: {why}"
    );

    // `doctor` answers the same state with the same command. Two surfaces, one
    // ceremony — the disagreement class this whole file exists to catch.
    let report = doctor::collect(Some(&proj)).unwrap();
    assert_eq!(
        report["next_action"].as_str(),
        Some("agentstack lock --write"),
        "doctor must not send the user to the review either: {report}"
    );

    std::env::remove_var("HOME");
    std::env::remove_var("AGENTSTACK_HOME");
}

/// The other half of the same rung: it must TERMINATE.
///
/// A hooks-only manifest declares something, so the broad "does this project
/// declare capabilities?" reading says yes — but `lock` pins skills, servers,
/// instructions, settings, extensions, workflows and packages, and hooks are in
/// none of them. Measured: `lock --write` over this manifest exits 0, prints
/// "pinned nothing new", and writes no `agentstack.lock` at all. Had the rung
/// keyed off the broad predicate it would name `lock --write` here forever —
/// the same poll-and-run dead end it was written to remove, one rung earlier.
#[test]
fn the_lock_rung_never_fires_where_locking_would_change_nothing() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = project(
        tmp.path(),
        "version = 1\n\
         [targets]\ndefault = [\"claude-code\"]\n\
         [hooks.pre]\nevent = \"PreToolUse\"\ncommand = \"/bin/echo\"\n",
    );

    let (_, sentence, _) = next_action(&proj);
    assert_ne!(
        sentence, "agentstack lock --write",
        "nothing here pins, so the lock rung would never be satisfied: {sentence}"
    );
    let report = doctor::collect(Some(&proj)).unwrap();
    assert_ne!(
        report["next_action"].as_str(),
        Some("agentstack lock --write"),
        "doctor must not loop on the lock rung either: {report}"
    );

    std::env::remove_var("HOME");
    std::env::remove_var("AGENTSTACK_HOME");
}

// ----------------------------------------- symptom 6 (G28): honest null-ness

/// **G28.** Three rungs were written as command templates, so `machine_command`
/// dropped them for the angle brackets and `next_action.command` was `null`
/// while the sentence read like something you could type. The bracket rule is
/// right and stays; what changes is that a rung with no runnable command now
/// SAYS it needs an argument only the human can choose, so the empty machine
/// field is a statement rather than an accident.
///
/// Every state pins the sentence and the machine command together — that pair
/// is the contract, and a witness that reads one of them cannot see this class
/// of bug at all.
#[test]
fn a_rung_with_no_runnable_command_says_why_the_machine_field_is_empty() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    // ── The empty project, which now belongs to THIS test rather than beside
    //    it. The SHAPE `agentstack search <query>` has no runnable form — the
    //    query is the one thing only the person with the problem knows — so the
    //    shape lives in the `why`, where a human reads it.
    //
    //    The machine field is null, and this row used to assert the opposite.
    //    It held that `agentstack search` was complete without a query and so
    //    belonged in the field. Complete, yes — but a next ACTION has to move
    //    the project, and a read-only browse leaves every observable field of
    //    the report identical, so a driver polling this field ran it and was
    //    handed it again forever (`guidance_is_executable.rs` reproduces the
    //    loop). Nor is emptying it a schema decision: the rows below already
    //    answer null from the same `status-v1` field.
    let tmp = assert_fs::TempDir::new().unwrap();
    let empty = project(tmp.path(), "version = 1\n");
    let (cmd, sentence, why) = next_action(&empty);
    assert_eq!(
        cmd, None,
        "an empty project's one missing input is a human decision — there is no \
         command, and a browse that changes nothing is not one"
    );
    assert_eq!(
        sentence,
        "find a server or skill to add — only you know what this project needs"
    );
    assert!(
        !sentence.contains('<'),
        "the SENTENCE must stop looking like a command: {sentence}"
    );
    assert!(
        why.contains("agentstack search <query>"),
        "the fillable shape survives where a human reads it: {why}"
    );
    // The residual G28 named: `doctor` answered this identical state with
    // `agentstack trust .` while `status` answered with the search shape. A
    // review of a manifest that declares nothing grants nothing, so `status`
    // had the better answer and `doctor` now gives it too.
    let report = doctor::collect(Some(&empty)).unwrap();
    assert_eq!(
        report["next_action"].as_str(),
        cmd.as_deref(),
        "the two surfaces must agree on the empty project's machine field: {report}"
    );
    assert_eq!(
        report["next_step"].as_str(),
        Some("find a server or skill to add — only you know what this project needs"),
        "…and on its sentence: {report}"
    );

    std::env::remove_var("HOME");
    std::env::remove_var("AGENTSTACK_HOME");
}
