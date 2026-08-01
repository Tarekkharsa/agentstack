// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Phase 2 CONSENT WITNESS: **answers stage; the single final yes commits.**
//!
//! The re-gate collects a per-item accept / keep-pinned / block answer while it
//! walks. The contract (`docs/design/consent-card.md`, "Answers stage; the
//! single final yes commits") is that NOTHING acts on those answers — no
//! re-lock, no recorded decision, no pinned copy, no exclusion — until the one
//! closing confirmation. Acting on each answer as it is given is the obvious
//! implementation, and it quietly creates three or four moments where a human
//! commits to something; there is exactly one.
//!
//! So this witness gives all three answer kinds, declines at the end, and
//! asserts nothing moved: manifest, lock, trust store **including its recorded
//! decisions**, delivered artifacts, and the event log.
//!
//! Scoping note, from an adversarial review: the review walk DOES write to disk
//! — `Store::ensure_worktree` materializes git worktrees under
//! `~/.agentstack/store/`. Those writes are content-addressed, idempotent,
//! outside the project, and carry no consent meaning, so this asserts on the
//! things consent actually covers (as `declining_leaves_nothing_behind`
//! already does) rather than on all of `~/.agentstack` being frozen.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use agentstack::commands::trust::{Answer, ReGateProbe};

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn isolate_home(home: &Path) {
    std::env::set_var("AGENTSTACK_HOME", home.join("agentstack-home"));
    std::env::set_var("HOME", home);
}

fn skill(proj: &Path, name: &str, body: &str) {
    let dir = proj.join(".agentstack/skills").join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("SKILL.md"), body).unwrap();
}

/// A project with three path skills, locked and trusted — the state a re-gate
/// starts from.
fn trusted_project(root: &Path) -> PathBuf {
    trusted_project_at(root, "proj")
}

/// The same fixture under a chosen directory name, so two byte-identical
/// projects can be built side by side — parity compares their digests, which
/// are over content and must therefore agree.
fn trusted_project_at(root: &Path, name: &str) -> PathBuf {
    let proj = root.join(name);
    fs::create_dir_all(proj.join(".agentstack")).unwrap();
    skill(&proj, "alpha", "---\ndescription: a\n---\n# Alpha\nfirst\n");
    skill(&proj, "beta", "---\ndescription: b\n---\n# Beta\nfirst\n");
    skill(&proj, "gamma", "---\ndescription: g\n---\n# Gamma\nfirst\n");
    fs::write(
        proj.join(".agentstack/agentstack.toml"),
        "version = 1\n\n\
         [skills.alpha]\npath = \"./skills/alpha\"\n\n\
         [skills.beta]\npath = \"./skills/beta\"\n\n\
         [skills.gamma]\npath = \"./skills/gamma\"\n\n\
         [profiles.default]\nskills = [\"alpha\", \"beta\", \"gamma\"]\n",
    )
    .unwrap();
    agentstack::commands::lock::run(&Default::default(), Some(&proj)).unwrap();
    let digest = agentstack::trust::digest_for(&proj).unwrap();
    agentstack::commands::trust::grant_with_answers(&proj, true, Some(&digest), false, None)
        .unwrap();
    proj
}

/// Run the real binary against the isolated home this test set up. `doctor`
/// exits nonzero when it finds problems, which is the normal case here, so the
/// caller decides whether success matters.
fn cli(proj: &Path, args: &[&str]) -> (String, bool) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(args)
        .current_dir(proj)
        .env_clear()
        .env("HOME", std::env::var("HOME").unwrap())
        .env("AGENTSTACK_HOME", std::env::var("AGENTSTACK_HOME").unwrap())
        .env("PATH", "/usr/bin:/bin")
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

fn events(proj: &Path) -> Vec<String> {
    let key = agentstack::trust::key_for(proj);
    agentstack_recorder::read_trust_all()
        .into_iter()
        .filter(|e| e.project == key)
        .map(|e| format!("{:?}:{}", e.action, e.digest))
        .collect()
}

/// Every file under `root`, as (relpath, bytes) — for byte-identical compares.
fn tree(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            let Ok(meta) = p.symlink_metadata() else {
                continue;
            };
            if meta.is_dir() {
                stack.push(p);
            } else if let Ok(bytes) = fs::read(&p) {
                out.push((
                    p.strip_prefix(root).unwrap().to_string_lossy().to_string(),
                    bytes,
                ));
            }
        }
    }
    out.sort();
    out
}

/// CONSENT WITNESS (Phase 2, staging). NEVER weaken this: it is the only thing
/// standing between "one commit moment" and "one commit moment per item".
#[test]
fn all_three_answers_given_then_declined_leave_no_residue_anywhere() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    isolate_home(tmp.path());
    let proj = trusted_project(tmp.path());

    // Drift all three, so all three stage a question.
    skill(
        &proj,
        "alpha",
        "---\ndescription: a\n---\n# Alpha\nCHANGED\n",
    );
    skill(&proj, "beta", "---\ndescription: b\n---\n# Beta\nCHANGED\n");
    skill(
        &proj,
        "gamma",
        "---\ndescription: g\n---\n# Gamma\nCHANGED\n",
    );

    let dir = proj.join(".agentstack");
    let manifest_before = fs::read(dir.join("agentstack.toml")).unwrap();
    let lock_before = fs::read(agentstack::lock::Lock::path(&dir)).unwrap();
    let store_before = fs::read(agentstack::trust::store_path()).unwrap();
    let decisions_before = agentstack::trust::decisions_for(&proj);
    let events_before = events(&proj);
    let project_before = tree(&proj);

    // All three answer kinds given — then the closing gate DECLINED.
    let err = agentstack::commands::trust::grant_with_answers(
        &proj,
        false,
        None,
        true,
        Some(&ReGateProbe {
            answers: vec![
                ("alpha".to_string(), Answer::Accept),
                ("beta".to_string(), Answer::KeepPinned),
                ("gamma".to_string(), Answer::Block),
            ],
            confirm: false,
        }),
    )
    .expect_err("declining is an error exit, not a silent success");
    assert!(
        err.to_string().contains("cancelled"),
        "the refusal says so plainly: {err:#}"
    );

    // (a) the manifest and the lock did not move — accept did NOT re-pin.
    assert_eq!(
        fs::read(dir.join("agentstack.toml")).unwrap(),
        manifest_before,
        "the manifest moved after a declined re-gate"
    );
    assert_eq!(
        fs::read(agentstack::lock::Lock::path(&dir)).unwrap(),
        lock_before,
        "the lock was re-pinned before the human confirmed — accept leaked"
    );

    // (b) the trust store is byte-identical, INCLUDING its decisions — the new
    //     state this witness exists to cover.
    assert_eq!(
        fs::read(agentstack::trust::store_path()).unwrap(),
        store_before,
        "the trust store moved after a declined re-gate"
    );
    assert_eq!(
        agentstack::trust::decisions_for(&proj),
        decisions_before,
        "a keep-pinned or blocked answer was recorded despite the decline"
    );

    // (c) no new events: a declined re-gate is not a trust mutation.
    assert_eq!(
        events(&proj),
        events_before,
        "a declined re-gate recorded an event"
    );

    // (d) nothing in the project changed at all — no delivered artifacts, no
    //     stray files.
    assert_eq!(
        tree(&proj),
        project_before,
        "a declined re-gate left something behind in the project"
    );
}

/// Accept must actually land, and this is the sharpest thing in the wiring:
/// accepting re-locks, the consent digest covers the lock bytes, so accept
/// moves the very digest the review rendered from. Both naive commit paths are
/// broken — with `--consented-digest` the grant fails AFTER the lock was
/// rewritten, and without one it records the pre-accept digest and the project
/// immediately reads `Changed`, i.e. the user accepts and silently gets an
/// untrusted project. The fix recomputes from the snapshot's manifest/local
/// plus the lock bytes this process just wrote. This is the witness for that.
#[test]
fn accepting_repins_and_leaves_the_project_actually_trusted() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    isolate_home(tmp.path());
    let proj = trusted_project(tmp.path());
    let dir = proj.join(".agentstack");
    let lock_before = fs::read(agentstack::lock::Lock::path(&dir)).unwrap();

    skill(
        &proj,
        "alpha",
        "---\ndescription: a\n---\n# Alpha\nCHANGED\n",
    );
    agentstack::commands::trust::grant_with_answers(
        &proj,
        false,
        None,
        true,
        Some(&ReGateProbe {
            answers: vec![("alpha".to_string(), Answer::Accept)],
            confirm: true,
        }),
    )
    .unwrap();

    // The lock moved to the new bytes…
    assert_ne!(
        fs::read(agentstack::lock::Lock::path(&dir)).unwrap(),
        lock_before,
        "accept did not re-pin"
    );
    // …and — the whole point — the project is TRUSTED, not Changed. A grant
    // that records a digest not matching the bytes on disk leaves the user
    // having said yes to something that immediately reads as untrusted.
    assert_eq!(
        agentstack::trust::check(&proj),
        agentstack::trust::TrustState::Trusted,
        "accepting left the project untrusted — the recorded digest does not \
         match the lock that accept just wrote"
    );
    // Accept leaves no standing decision: the new bytes ARE the approved ones.
    assert!(
        agentstack::trust::decision_for(&proj, "skill", "alpha").is_none(),
        "accept recorded a standing decision it should have cleared"
    );
}

/// Keep-pinned records its answer against the pin that was approved, and does
/// NOT move the lock — the approved bytes stay the pinned bytes.
#[test]
fn keeping_the_pin_records_the_answer_and_never_moves_the_lock() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    isolate_home(tmp.path());
    let proj = trusted_project(tmp.path());
    let dir = proj.join(".agentstack");
    let lock_before = fs::read(agentstack::lock::Lock::path(&dir)).unwrap();

    skill(&proj, "beta", "---\ndescription: b\n---\n# Beta\nCHANGED\n");
    agentstack::commands::trust::grant_with_answers(
        &proj,
        false,
        None,
        true,
        Some(&ReGateProbe {
            answers: vec![("beta".to_string(), Answer::KeepPinned)],
            confirm: true,
        }),
    )
    .unwrap();

    assert_eq!(
        fs::read(agentstack::lock::Lock::path(&dir)).unwrap(),
        lock_before,
        "keep-pinned moved the lock; the approved bytes are no longer the pin"
    );
    match agentstack::trust::decision_for(&proj, "skill", "beta") {
        Some(agentstack::trust::Decision::KeepPinned { pin }) => {
            assert!(!pin.is_empty(), "keep-pinned recorded an empty pin");
        }
        other => panic!("expected a KeepPinned decision, got {other:?}"),
    }
}

/// Blocking records a standing refusal — a state, not a question to re-ask.
#[test]
fn blocking_records_a_standing_refusal() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    isolate_home(tmp.path());
    let proj = trusted_project(tmp.path());
    skill(
        &proj,
        "gamma",
        "---\ndescription: g\n---\n# Gamma\nCHANGED\n",
    );
    agentstack::commands::trust::grant_with_answers(
        &proj,
        false,
        None,
        true,
        Some(&ReGateProbe {
            answers: vec![("gamma".to_string(), Answer::Block)],
            confirm: true,
        }),
    )
    .unwrap();
    assert_eq!(
        agentstack::trust::decision_for(&proj, "skill", "gamma"),
        Some(agentstack::trust::Decision::Blocked)
    );
}

/// Item 2 CONSENT WITNESS: a standing answer reshapes DELIVERY.
///
/// Three properties in one activation, because they are one behaviour:
///   - a blocked skill is not delivered at all (fails closed, like drift);
///   - a keep-pinned skill IS delivered, from the content-store snapshot, as a
///     copy — so the approved bytes are what an agent loads;
///   - an unanswered, undrifted skill is untouched.
#[test]
fn standing_answers_reshape_delivery() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    isolate_home(tmp.path());
    let proj = trusted_project(tmp.path());

    // beta is kept at its approved bytes; gamma is blocked.
    skill(&proj, "beta", "---\ndescription: b\n---\n# Beta\nCHANGED\n");
    skill(
        &proj,
        "gamma",
        "---\ndescription: g\n---\n# Gamma\nCHANGED\n",
    );
    agentstack::commands::trust::grant_with_answers(
        &proj,
        false,
        None,
        true,
        Some(&ReGateProbe {
            answers: vec![
                ("beta".to_string(), Answer::KeepPinned),
                ("gamma".to_string(), Answer::Block),
            ],
            confirm: true,
        }),
    )
    .unwrap();

    let (out, ok) = cli(&proj, &["use", "--write"]);
    assert!(ok, "use --write failed:\n{out}");

    // Find whichever harness skills dir this machine's adapters produced.
    let delivered = ["\u{2e}claude/skills", ".agents/skills", ".pi/skills"]
        .iter()
        .map(|d| proj.join(d))
        .find(|d| d.is_dir())
        .expect("something was materialized");

    // Blocked: absent entirely.
    assert!(
        !delivered.join("gamma").exists(),
        "a blocked skill was delivered anyway"
    );
    // Keep-pinned: present, and holding the APPROVED bytes, not the live edit.
    let beta = delivered.join("beta").join("SKILL.md");
    let body = fs::read_to_string(&beta).expect("keep-pinned skill was delivered");
    assert!(
        body.contains("first") && !body.contains("CHANGED"),
        "keep-pinned delivered the declined content: {body}"
    );
    // …and it is a real file, not a link into the project.
    assert!(
        !delivered
            .join("beta")
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink(),
        "keep-pinned was symlinked; it would follow the declined change"
    );
    // Unanswered and unchanged: delivered normally.
    assert!(delivered.join("alpha").exists());
}

/// Item 2 CONSENT WITNESS: keep-pinned resolves ONE consent moment. It must
/// never silence the drift itself — the live file and the delivered version
/// really have diverged, and a status that hid that would be lying by omission.
#[test]
fn keep_pinned_never_silences_the_drift_report() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    isolate_home(tmp.path());
    let proj = trusted_project(tmp.path());
    skill(&proj, "beta", "---\ndescription: b\n---\n# Beta\nCHANGED\n");
    agentstack::commands::trust::grant_with_answers(
        &proj,
        false,
        None,
        true,
        Some(&ReGateProbe {
            answers: vec![("beta".to_string(), Answer::KeepPinned)],
            confirm: true,
        }),
    )
    .unwrap();

    let (text, _) = cli(&proj, &["doctor"]);
    // The standing state is named, with the way out…
    assert!(
        text.contains("using the version you approved"),
        "the standing answer is not reported: {text}"
    );
    assert!(
        text.contains("agentstack trust"),
        "no way out named: {text}"
    );
    // …AND the drift is still reported. Both lines, for the same skill.
    assert!(
        text.contains("content drifted from lock"),
        "keep-pinned silenced the drift report: {text}"
    );
}

/// Item 5 PARITY WITNESS: **the card compresses presentation, never evidence.**
///
/// Accepting a changed skill through the re-gate card must leave exactly the
/// event trail its explicit-path equivalent leaves — re-lock, then grant bound
/// to the previewed digest. Same actions, same digests, same order. A consent
/// surface that recorded less than the scripted sequence would make the
/// recorded history depend on which UI the human happened to use, and the
/// trust-mutation log is what Phase 1's countable gate rests on.
#[test]
fn accepting_leaves_the_same_event_trail_as_relock_then_trust() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    isolate_home(tmp.path());

    // Two projects, byte-identical content, so their digests are comparable.
    let card = trusted_project_at(tmp.path(), "card");
    let script = trusted_project_at(tmp.path(), "script");
    let edited = "---\ndescription: a\n---\n# Alpha\nEDITED\n";
    skill(&card, "alpha", edited);
    skill(&script, "alpha", edited);

    let card_before = events(&card).len();
    let script_before = events(&script).len();

    // Through the card: accept.
    agentstack::commands::trust::grant_with_answers(
        &card,
        false,
        None,
        true,
        Some(&ReGateProbe {
            answers: vec![("alpha".to_string(), Answer::Accept)],
            confirm: true,
        }),
    )
    .unwrap();

    // The scripted equivalent: re-pin, preview, grant bound to that digest.
    agentstack::commands::lock::run(&Default::default(), Some(&script)).unwrap();
    let digest = agentstack::commands::trust::preview_value(&script).unwrap()["surface_digest"]
        .as_str()
        .unwrap()
        .to_string();
    agentstack::commands::trust::grant_with_answers(&script, true, Some(&digest), false, None)
        .unwrap();

    // Compare only the events each flow ADDED — both projects already have a
    // first grant, and parity is a claim about the incremental tail.
    let card_tail = &events(&card)[card_before..];
    let script_tail = &events(&script)[script_before..];
    assert!(
        !card_tail.is_empty(),
        "accepting through the card recorded nothing — an unrecorded yes is an \
         unfalsifiable one"
    );
    assert_eq!(
        card_tail, script_tail,
        "the card's event trail diverged from the scripted sequence's"
    );
}

/// Item 5 PARITY WITNESS, keep-pinned.
///
/// Finding, recorded because it corrects the obvious premise: keep-pinned has
/// NO pre-Phase-2 explicit equivalent. A plain re-trust of a drifted project
/// refuses — the blocker bails — so before the card the only ways out of drift
/// were to re-lock (accept) or to put the file back. Keep-pinned is a genuinely
/// new answer, not a compression of an existing sequence.
///
/// So parity is asserted against the state it produces rather than a command
/// nobody could run: restoring the approved bytes and re-trusting reaches the
/// same place — the approved content in use, the digest unchanged — and must
/// leave the same trail. This is the honest comparison, and it still catches
/// the failure that matters: keep-pinned inventing a new digest, recording
/// nothing, or recording extra events.
#[test]
fn keeping_the_pin_leaves_the_same_event_trail_as_a_plain_retrust() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    isolate_home(tmp.path());

    let card = trusted_project_at(tmp.path(), "card");
    let script = trusted_project_at(tmp.path(), "script");
    let edited = "---\ndescription: b\n---\n# Beta\nEDITED\n";
    skill(&card, "beta", edited);
    skill(&script, "beta", edited);

    let card_before = events(&card).len();
    let script_before = events(&script).len();

    agentstack::commands::trust::grant_with_answers(
        &card,
        false,
        None,
        true,
        Some(&ReGateProbe {
            answers: vec![("beta".to_string(), Answer::KeepPinned)],
            confirm: true,
        }),
    )
    .unwrap();

    // The scripted route to the same state: put the approved bytes back, then
    // re-trust. (Re-trusting the DRIFTED project simply refuses, which is the
    // finding above.)
    skill(&script, "beta", "---\ndescription: b\n---\n# Beta\nfirst\n");
    let digest = agentstack::commands::trust::preview_value(&script).unwrap()["surface_digest"]
        .as_str()
        .unwrap()
        .to_string();
    agentstack::commands::trust::grant_with_answers(&script, true, Some(&digest), false, None)
        .unwrap();

    let card_tail = &events(&card)[card_before..];
    let script_tail = &events(&script)[script_before..];
    assert!(!card_tail.is_empty(), "keep-pinned recorded nothing");
    assert_eq!(
        card_tail, script_tail,
        "keep-pinned's event trail diverged from a plain re-trust's"
    );
}

/// Item 5: the closing gate appears EXACTLY when answers were collected, and
/// not otherwise. Scripts and CI must be byte-for-byte unaffected by the new
/// prompt — typing the command remains the consent when nothing was asked.
#[test]
fn a_review_with_nothing_to_answer_is_unchanged_for_scripts() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    isolate_home(tmp.path());
    let proj = trusted_project(tmp.path());

    // Nothing drifted, so nothing is staged and nothing can be asked. The
    // probe supplies `confirm: false` — which WOULD cancel if the closing gate
    // ran. It must not run.
    agentstack::commands::trust::grant_with_answers(
        &proj,
        false,
        None,
        true,
        Some(&ReGateProbe {
            answers: Vec::new(),
            confirm: false,
        }),
    )
    .expect("a clean re-review must not be gated by a question nobody was asked");
    assert_eq!(
        agentstack::trust::check(&proj),
        agentstack::trust::TrustState::Trusted
    );

    // And the non-interactive scripted form still works untouched.
    let digest = agentstack::trust::digest_for(&proj).unwrap();
    agentstack::commands::trust::grant_with_answers(&proj, true, Some(&digest), false, None)
        .expect("the scripted path is unchanged");
}

/// Item 6 CONSENT WITNESS: **recognition changes lines, and nothing else.**
///
/// The whole risk of a "you've seen this before" feature is that it starts
/// deciding things. So this drives two byte-identical projects — the second of
/// which recognizes everything the first approved — and asserts the outcome,
/// the gate, and the recorded events are identical to a run with no index at
/// all. Only the printed body may differ.
#[test]
fn recognition_changes_lines_never_the_outcome_or_the_events() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    isolate_home(tmp.path());

    // First project: nothing is recognized (the index starts empty).
    let first = trusted_project_at(tmp.path(), "first");
    let first_events = events(&first);
    assert!(!first_events.is_empty());

    // Second project, byte-identical content: everything IS recognized now.
    let second = trusted_project_at(tmp.path(), "second");
    let index = agentstack::recognition::Index::load();
    let key = agentstack::trust::key_for(&second);
    let recorded = match agentstack::trust::prior_surface(&second) {
        agentstack::trust::PriorSurface::Recorded(items) => items,
        other => panic!("expected a recorded surface, got {other:?}"),
    };
    assert!(
        agentstack::recognition::recognized_count(&index, &key, &recorded) > 0,
        "the second project should recognize the first's approvals"
    );

    // The outcome is the same…
    assert_eq!(
        agentstack::trust::check(&second),
        agentstack::trust::TrustState::Trusted
    );
    // …and so is the event trail, action-for-action and digest-for-digest.
    // Recognition contributed a line, not a decision.
    assert_eq!(
        events(&second),
        first_events,
        "recognition changed the recorded events"
    );
}

/// Item 6: recognition is derived from consent and does not outlive it.
#[test]
fn revoking_trust_stops_this_project_corroborating_others() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    isolate_home(tmp.path());
    let first = trusted_project_at(tmp.path(), "first");
    let second = trusted_project_at(tmp.path(), "second");

    let key_second = agentstack::trust::key_for(&second);
    let recorded = match agentstack::trust::prior_surface(&second) {
        agentstack::trust::PriorSurface::Recorded(items) => items,
        other => panic!("{other:?}"),
    };
    let before = agentstack::recognition::recognized_count(
        &agentstack::recognition::Index::load(),
        &key_second,
        &recorded,
    );
    assert!(before > 0);

    // The corroborating project withdraws its consent.
    agentstack::recognition::forget(&agentstack::trust::key_for(&first));
    let after = agentstack::recognition::recognized_count(
        &agentstack::recognition::Index::load(),
        &key_second,
        &recorded,
    );
    assert_eq!(
        after, 0,
        "a revoked project still corroborates another project's card"
    );
}

/// The other half of the contract: an item the human did NOT answer keeps its
/// blocker, so the review refuses exactly as it does today. Silence is not an
/// answer, and it must not become one just because a prompt now exists.
#[test]
fn an_unanswered_item_still_blocks_the_grant() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    isolate_home(tmp.path());
    let proj = trusted_project(tmp.path());
    skill(
        &proj,
        "alpha",
        "---\ndescription: a\n---\n# Alpha\nCHANGED\n",
    );
    skill(&proj, "beta", "---\ndescription: b\n---\n# Beta\nCHANGED\n");

    // Only alpha is answered; beta is left undecided.
    let err = agentstack::commands::trust::grant_with_answers(
        &proj,
        false,
        None,
        true,
        Some(&ReGateProbe {
            answers: vec![("alpha".to_string(), Answer::Accept)],
            confirm: true,
        }),
    )
    .expect_err("an unanswered drifted item must still refuse the grant");
    let msg = err.to_string();
    assert!(msg.contains("isn't fully pinned"), "{msg}");
    assert!(msg.contains("beta"), "the unanswered item is named: {msg}");
    assert!(
        !msg.contains("alpha"),
        "the answered item should no longer block: {msg}"
    );
    // And because the grant refused, the answered item did not commit either.
    assert!(
        agentstack::trust::decisions_for(&proj).is_empty(),
        "a decision was recorded on a path that refused"
    );
}

// ---------------------------------------------------------------------------
// F6 (FINDINGS.md): the instruction re-gate must correctly implement all
// three answers. Before the fix, `accept` fed the fragment FILE to
// `dir_digest` and errored out AFTER the user consented (patching the
// skills table it isn't in), and keep-pinned/block were recorded but the
// compiler never read them — it kept compiling the live file.
// ---------------------------------------------------------------------------

/// A trusted project whose consent surface includes an instruction fragment.
fn trusted_project_with_instruction(root: &Path) -> PathBuf {
    let proj = root.join("proj");
    fs::create_dir_all(proj.join(".agentstack/instructions")).unwrap();
    skill(&proj, "alpha", "---\ndescription: a\n---\n# Alpha\nfirst\n");
    fs::write(
        proj.join(".agentstack/instructions/house.md"),
        "House rule one.\n",
    )
    .unwrap();
    fs::write(
        proj.join(".agentstack/agentstack.toml"),
        "version = 1\n\n\
         [targets]\ndefault = [\"claude-code\"]\n\n\
         [skills.alpha]\npath = \"./skills/alpha\"\n\n\
         [instructions.house]\npath = \"./instructions/house.md\"\n",
    )
    .unwrap();
    agentstack::commands::lock::run(&Default::default(), Some(&proj)).unwrap();
    let digest = agentstack::trust::digest_for(&proj).unwrap();
    agentstack::commands::trust::grant_with_answers(&proj, true, Some(&digest), false, None)
        .unwrap();
    proj
}

/// The in-process compile, project scope, one capable target.
fn compile_instructions(proj: &Path) {
    agentstack::commands::instructions::run(
        &agentstack::cli::InstructionsArgs {
            targets: vec!["claude-code".into()],
            scope: Some(agentstack::scope::Scope::Project),
            write: true,
        },
        Some(proj),
    )
    .unwrap();
}

/// F6 accept: succeeds (it used to error after consent), re-pins the
/// INSTRUCTION lock table to the new bytes, and leaves the project trusted
/// with no standing decision.
#[test]
fn an_instruction_regate_accept_repins_and_stays_trusted() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    isolate_home(tmp.path());
    let proj = trusted_project_with_instruction(tmp.path());
    let dir = proj.join(".agentstack");

    fs::write(dir.join("instructions/house.md"), "House rule CHANGED.\n").unwrap();
    agentstack::commands::trust::grant_with_answers(
        &proj,
        false,
        None,
        true,
        Some(&ReGateProbe {
            answers: vec![("house".to_string(), Answer::Accept)],
            confirm: true,
        }),
    )
    .expect("accepting an instruction re-gate must succeed, not error after consent");

    let lock = agentstack::lock::Lock::load(&dir).unwrap();
    let entry = lock
        .get_instruction("house")
        .expect("instruction pin survived");
    assert_eq!(
        entry.checksum.hex(),
        agentstack_core::digest::sha256_hex(b"House rule CHANGED.\n"),
        "accept did not move the INSTRUCTION pin to the accepted bytes"
    );
    assert_eq!(
        agentstack::trust::check(&proj),
        agentstack::trust::TrustState::Trusted,
        "accepting left the project untrusted"
    );
    assert!(
        agentstack::trust::decision_for(&proj, "instruction", "house").is_none(),
        "accept must clear any standing decision"
    );
}

/// F6 keep-pinned: the compiler delivers the APPROVED bytes from the content
/// store — never the live file, which holds exactly the change the human
/// declined — and neither `use`-style pin recording nor a plain
/// `agentstack lock` absorbs the declined bytes into the lock.
#[test]
fn an_instruction_keep_pinned_compiles_the_approved_bytes() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    isolate_home(tmp.path());
    let proj = trusted_project_with_instruction(tmp.path());
    let dir = proj.join(".agentstack");
    let lock_before = fs::read(agentstack::lock::Lock::path(&dir)).unwrap();

    fs::write(dir.join("instructions/house.md"), "House rule CHANGED.\n").unwrap();
    agentstack::commands::trust::grant_with_answers(
        &proj,
        false,
        None,
        true,
        Some(&ReGateProbe {
            answers: vec![("house".to_string(), Answer::KeepPinned)],
            confirm: true,
        }),
    )
    .unwrap();

    compile_instructions(&proj);
    let compiled = fs::read_to_string(proj.join("CLAUDE.md")).unwrap();
    assert!(
        compiled.contains("House rule one.") && !compiled.contains("CHANGED"),
        "keep-pinned compiled the declined content:\n{compiled}"
    );

    // A later re-lock must not absorb the declined bytes either: the pin an
    // answered item keeps is the one the answer named.
    agentstack::commands::lock::run(&Default::default(), Some(&proj)).unwrap();
    assert_eq!(
        fs::read(agentstack::lock::Lock::path(&dir)).unwrap(),
        lock_before,
        "a plain re-lock moved a decided instruction pin to the declined bytes"
    );
}

/// F6 block: the fragment reaches no managed region — neither the approved
/// bytes nor the live edit — and stays out until the human revisits.
#[test]
fn a_blocked_instruction_never_reaches_the_managed_region() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    isolate_home(tmp.path());
    let proj = trusted_project_with_instruction(tmp.path());
    let dir = proj.join(".agentstack");

    // Compile once while trusted so the region already holds the fragment —
    // blocking must then REMOVE it, not merely skip adding it.
    compile_instructions(&proj);
    assert!(fs::read_to_string(proj.join("CLAUDE.md"))
        .unwrap()
        .contains("House rule one."));

    fs::write(dir.join("instructions/house.md"), "House rule CHANGED.\n").unwrap();
    agentstack::commands::trust::grant_with_answers(
        &proj,
        false,
        None,
        true,
        Some(&ReGateProbe {
            answers: vec![("house".to_string(), Answer::Block)],
            confirm: true,
        }),
    )
    .unwrap();

    compile_instructions(&proj);
    let compiled = fs::read_to_string(proj.join("CLAUDE.md")).unwrap();
    assert!(
        !compiled.contains("House rule"),
        "a blocked instruction still reaches the managed region:\n{compiled}"
    );
}

// ---------------------------------------------------------------------------
// F4 (FINDINGS.md): keep-pinned delivery serves the approved bytes only if
// they still ARE the approved bytes. The store directory is writable; a bare
// `is_dir()` check let anything planted under the approved digest name ride
// into every harness as though the user had approved it.
// ---------------------------------------------------------------------------

/// Tampering with the field that actually moves: the SNAPSHOT CONTENT under
/// the approved digest name — first edited bytes, then a symlink at a
/// sensitive file. Both must fail closed (no delivery), never deliver the
/// planted bytes, and say why.
#[test]
fn keep_pinned_delivery_refuses_a_tampered_snapshot() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    isolate_home(tmp.path());
    let proj = trusted_project(tmp.path());

    skill(&proj, "beta", "---\ndescription: b\n---\n# Beta\nCHANGED\n");
    agentstack::commands::trust::grant_with_answers(
        &proj,
        false,
        None,
        true,
        Some(&ReGateProbe {
            answers: vec![("beta".to_string(), Answer::KeepPinned)],
            confirm: true,
        }),
    )
    .unwrap();
    let Some(agentstack::trust::Decision::KeepPinned { pin }) =
        agentstack::trust::decision_for(&proj, "skill", "beta")
    else {
        panic!("keep-pinned decision missing");
    };
    let hex = pin.rsplit(':').next().unwrap().to_string();
    let snapshot = PathBuf::from(std::env::var("AGENTSTACK_HOME").unwrap())
        .join("store/content")
        .join(&hex);
    assert!(snapshot.is_dir(), "fixture: approved snapshot exists");

    // Phase 1: edit the snapshot in place under the approved name.
    fs::write(
        snapshot.join("SKILL.md"),
        "---\ndescription: b\n---\n# Beta\nEVIL PAYLOAD\n",
    )
    .unwrap();
    let (out, ok) = cli(&proj, &["use", "--write"]);
    assert!(ok, "use --write failed outright:\n{out}");
    let delivered = ["\u{2e}claude/skills", ".agents/skills", ".pi/skills"]
        .iter()
        .map(|d| proj.join(d))
        .find(|d| d.is_dir())
        .expect("something was materialized");
    assert!(
        !delivered.join("beta").exists(),
        "a tampered snapshot was delivered under the approved name:\n{out}"
    );
    assert!(
        out.contains("failed verification") || out.contains("missing or failed"),
        "the exclusion does not say the approved copy failed verification:\n{out}"
    );

    // Phase 2: replace the snapshot's body with a symlink at a secret.
    let secret = tmp.path().join("secret-key");
    fs::write(&secret, "PRIVATE KEY MATERIAL\n").unwrap();
    fs::remove_file(snapshot.join("SKILL.md")).unwrap();
    std::os::unix::fs::symlink(&secret, snapshot.join("SKILL.md")).unwrap();
    let (out2, ok2) = cli(&proj, &["use", "--write"]);
    assert!(ok2, "use --write failed outright:\n{out2}");
    assert!(
        !delivered.join("beta").exists(),
        "a symlinked snapshot was delivered:\n{out2}"
    );
    // The secret's bytes must not have been copied anywhere under the project.
    for (path, bytes) in tree(&proj) {
        assert!(
            !bytes.windows(20).any(|w| w == b"PRIVATE KEY MATERIAL"),
            "secret bytes escaped into the project at {path}"
        );
    }
}

/// The absorb hazard: `use --write` after a keep-pinned answer must leave the
/// decided skill's lock pin exactly where the answer left it. Re-pinning the
/// live checksum here would make the next review read "matches" — the decline
/// quietly gone with no consent moment.
#[test]
fn use_write_never_repins_a_decided_skill() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    isolate_home(tmp.path());
    let proj = trusted_project(tmp.path());
    let dir = proj.join(".agentstack");

    skill(&proj, "beta", "---\ndescription: b\n---\n# Beta\nCHANGED\n");
    agentstack::commands::trust::grant_with_answers(
        &proj,
        false,
        None,
        true,
        Some(&ReGateProbe {
            answers: vec![("beta".to_string(), Answer::KeepPinned)],
            confirm: true,
        }),
    )
    .unwrap();
    let Some(agentstack::trust::Decision::KeepPinned { pin }) =
        agentstack::trust::decision_for(&proj, "skill", "beta")
    else {
        panic!("keep-pinned decision missing");
    };

    let (out, ok) = cli(&proj, &["use", "--write"]);
    assert!(ok, "use --write failed:\n{out}");

    let lock = agentstack::lock::Lock::load(&dir).unwrap();
    let entry = lock.get("beta").expect("beta pin survived");
    assert_eq!(
        entry.checksum.hex(),
        pin.rsplit(':').next().unwrap(),
        "use --write re-pinned a decided skill to the declined live bytes"
    );
}

/// F8 WITNESS: the standing refusal holds on the LOCKED run path. The tamper
/// is the finding's bypass: after blocking, the live bytes are restored to
/// exactly the approved ones — every drift check passes, strict verification
/// would pass — and only the human's standing refusal stands. Locked
/// execution used to construct its grant without ever reading decisions.
#[test]
fn a_standing_block_refuses_a_locked_run() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    isolate_home(tmp.path());
    let proj = trusted_project(tmp.path());

    skill(
        &proj,
        "gamma",
        "---\ndescription: g\n---\n# Gamma\nCHANGED\n",
    );
    agentstack::commands::trust::grant_with_answers(
        &proj,
        false,
        None,
        true,
        Some(&ReGateProbe {
            answers: vec![("gamma".to_string(), Answer::Block)],
            confirm: true,
        }),
    )
    .unwrap();
    // Restore the approved bytes: drift checks now pass everywhere.
    skill(&proj, "gamma", "---\ndescription: g\n---\n# Gamma\nfirst\n");
    assert_eq!(
        agentstack::trust::check(&proj),
        agentstack::trust::TrustState::Trusted,
        "fixture: the project reads trusted, so only the decision can refuse"
    );

    // A stub harness binary, so the PATH probe (which runs before the
    // locked verification) passes and the refusal under test is reachable.
    // If the run ever got past verification, the stub would run and exit 0 —
    // the assertions below would then fail loudly.
    let stub_bin = tmp.path().join("stubbin");
    fs::create_dir_all(&stub_bin).unwrap();
    fs::write(stub_bin.join("claude"), "#!/bin/sh\nexit 0\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(stub_bin.join("claude"), fs::Permissions::from_mode(0o755)).unwrap();
    }
    let out_raw = std::process::Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(["run", "claude-code", "--locked"])
        .current_dir(&proj)
        .env_clear()
        .env("HOME", std::env::var("HOME").unwrap())
        .env("AGENTSTACK_HOME", std::env::var("AGENTSTACK_HOME").unwrap())
        .env("PATH", format!("{}:/usr/bin:/bin", stub_bin.display()))
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    let out = format!(
        "{}{}",
        String::from_utf8_lossy(&out_raw.stdout),
        String::from_utf8_lossy(&out_raw.stderr)
    );
    assert!(
        !out_raw.status.success(),
        "a locked run proceeded over a standing block:\n{out}"
    );
    assert!(
        out.contains("blocked by your standing decision"),
        "the refusal does not name the standing decision:\n{out}"
    );
}

/// F11 witness (FINDINGS.md): `doctor` detects content drift against the pin
/// on the DEFAULT path. The trust digest covers manifest+lock, not the skill
/// BODY — so editing an approved skill in place used to leave `doctor` reading
/// `✓ present · SKILL.md ok` and `0 errors` over bytes that changed under the
/// approval. The tamper is the skill body; the surface must go red without
/// `--deep`, `--ci`, or `trust .`.
#[test]
fn doctor_sees_content_drift_on_the_default_path() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    isolate_home(tmp.path());
    let proj = trusted_project(tmp.path());

    // Before the edit, doctor is clean about alpha.
    let (before, _) = cli(&proj, &["doctor"]);
    assert!(
        before.contains("alpha"),
        "fixture: alpha is checked:\n{before}"
    );

    // Edit an approved skill in place — manifest and lock untouched.
    skill(
        &proj,
        "alpha",
        "---\ndescription: a\n---\n# Alpha\nEDITED AFTER APPROVAL\n",
    );

    // Plain `doctor` names the drift on the DEFAULT path (no --deep/--ci).
    let (after, _) = cli(&proj, &["doctor"]);
    assert!(
        after.contains("content changed since you approved it"),
        "doctor must name the drift on the default path:\n{after}"
    );
    assert!(
        after.contains("agentstack trust"),
        "and point at the review:\n{after}"
    );
    // …and it is an ERROR: `doctor --ci` (the trust gate) fails over it.
    let (_, ci_ok) = cli(&proj, &["doctor", "--ci"]);
    assert!(!ci_ok, "content drift must fail the doctor --ci gate");
}

/// F5 END-TO-END WITNESS (FINDINGS.md): the TOCTOU swap, performed on disk in
/// the real window between the review and the commit — not the unit gate with
/// hand-fed hashes. The verifier's exact break attempt: review benign state B,
/// then an adversarial writer replaces the bytes with M in the human-scale
/// window before the closing yes. The commit must pin nothing and leave the
/// project as it was — never bless M un-displayed.
///
/// The swap runs inside the production hook that fires after every `displayed`
/// digest is captured and before the commit re-reads to pin — the exact
/// interval `refuse_undisplayed` guards.
#[test]
fn accept_refuses_a_swap_performed_between_review_and_commit() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    isolate_home(tmp.path());
    let proj = trusted_project(tmp.path());
    let dir = proj.join(".agentstack");
    let lock_before = fs::read(agentstack::lock::Lock::path(&dir)).unwrap();

    // The reviewed drift: benign state B.
    skill(
        &proj,
        "alpha",
        "---\ndescription: a\n---\n# Alpha\nBENIGN B\n",
    );
    let alpha_md = proj.join(".agentstack/skills/alpha/SKILL.md");

    // Accept B — but an adversarial writer swaps in M during the window.
    let swapped = std::sync::atomic::AtomicBool::new(false);
    let swap = || {
        fs::write(
            &alpha_md,
            "---\ndescription: a\n---\n# Alpha\nMALICIOUS M\n",
        )
        .unwrap();
        swapped.store(true, std::sync::atomic::Ordering::SeqCst);
    };
    let err = agentstack::commands::trust::grant_with_swap_between_review_and_commit(
        &proj,
        true,
        Some(&ReGateProbe {
            answers: vec![("alpha".to_string(), Answer::Accept)],
            confirm: true,
        }),
        &swap,
    )
    .expect_err("accepting must refuse once the reviewed bytes changed under it");

    // The swap really happened, and the refusal names it.
    assert!(
        swapped.load(std::sync::atomic::Ordering::SeqCst),
        "the hook must have run"
    );
    assert!(
        err.to_string().contains("changed while you were reviewing"),
        "the refusal must name the TOCTOU, got: {err:#}"
    );

    // Fail-closed: nothing pinned, and the malicious bytes were never blessed.
    assert_eq!(
        fs::read(agentstack::lock::Lock::path(&dir)).unwrap(),
        lock_before,
        "a swapped-in version must not move the lock"
    );
    // The store holds no snapshot for M's digest — it was never deposited.
    let alpha_dir = proj.join(".agentstack/skills/alpha");
    let m_digest_hash = agentstack_core::digest::dir_digest(&alpha_dir).unwrap();
    let m_digest = m_digest_hash.hex();
    let m_snapshot = PathBuf::from(std::env::var("AGENTSTACK_HOME").unwrap())
        .join("store/content")
        .join(m_digest);
    assert!(
        !m_snapshot.exists(),
        "the un-displayed bytes must never reach the content store"
    );
}
