// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Phase 1, slice B — the single-action path's witnesses.
//!
//! The claim slice B makes is that the collapse is **presentation, not
//! semantics**. Three properties have to hold for that to be true rather than
//! merely asserted:
//!
//! - **Event parity.** The compressed path leaves the identical trust-mutation
//!   audit trail as the explicit `lock` → `trust` → `use` sequence: same
//!   actions, same digests, same order. Compressing the presentation may never
//!   compress the evidence.
//! - **Structural exclusion.** Content that failed provenance, or whose name
//!   collides with a declaration, cannot reach the compressed path — not
//!   "is routed around it", but cannot be selected by it.
//! - **Disclosure.** The provenance line the user is shown travels into the
//!   combined preview with the item it describes.

use std::fs;
use std::path::Path;

use agentstack_recorder::{TrustAction, TrustMutation};

// These tests set HOME/AGENTSTACK_HOME, which are process-global.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn isolate_home(tmp: &Path) -> std::path::PathBuf {
    let home = tmp.join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));
    home.join(".agentstack")
}

fn project(tmp: &Path, name: &str) -> std::path::PathBuf {
    let proj = tmp.join(name);
    fs::create_dir_all(proj.join(".agentstack/skills")).unwrap();
    fs::write(
        proj.join(".agentstack/agentstack.toml"),
        "version = 1\n[delivery]\nrender_locally = true\n",
    )
    .unwrap();
    proj
}

fn drop_skill(proj: &Path, name: &str, body: &str) {
    let d = proj.join(".agentstack/skills").join(name);
    fs::create_dir_all(&d).unwrap();
    fs::write(d.join("SKILL.md"), body).unwrap();
}

fn git(proj: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(proj)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

/// Every trust-mutation event recorded for one project, oldest first.
fn events_for(proj: &Path) -> Vec<TrustMutation> {
    let key = agentstack::trust::key_for(proj);
    agentstack_recorder::read_trust_all()
        .into_iter()
        .filter(|e| e.project == key)
        .collect()
}

fn yes_args() -> agentstack::cli::YesArgs {
    agentstack::cli::YesArgs { yes: true }
}

/// Rider 1 — the compressed path and the explicit path leave the SAME trail.
///
/// Two projects, byte-identical content, one driven through
/// `agentstack yes` and the other through `adopt` → `lock` → `trust`. The
/// recorded actions and their order must match, and so must the consent digest
/// — because both paths consent to the same bytes through the same grant.
#[test]
fn the_compressed_path_records_the_same_events_as_the_explicit_one() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    isolate_home(tmp.path());

    let body = "---\ndescription: Same bytes both ways\n---\n# Review\n";

    // The compressed path.
    let fast = project(tmp.path(), "fast");
    git(&fast, &["init", "-q"]);
    drop_skill(&fast, "review", body);
    agentstack::commands::yes::run_gated(&yes_args(), Some(&fast), true).unwrap();

    // The explicit path, over identical content.
    let slow = project(tmp.path(), "slow");
    git(&slow, &["init", "-q"]);
    drop_skill(&slow, "review", body);
    agentstack::commands::adopt::run(
        &agentstack::cli::AdoptArgs {
            targets: Vec::new(),
            scope: None,
            write: true,
            no_keychain: true,
            to_library: false,
        },
        Some(&slow),
    )
    .unwrap();
    agentstack::commands::lock::run(
        &agentstack::cli::LockArgs::default(),
        Some(&slow.join(".agentstack")),
    )
    .unwrap();
    // The explicit path's own headless form, exactly as a scripted user runs
    // it: preview the surface, then grant bound to the previewed digest.
    let preview = agentstack::commands::trust::preview_value(&slow).unwrap();
    let digest = preview["surface_digest"]
        .as_str()
        .expect("the preview names the digest it rendered")
        .to_string();
    agentstack::commands::trust::run(
        &agentstack::cli::TrustArgs {
            path: Some(slow.clone()),
            list: false,
            revoke: false,
            yes: true,
            consented: Some(digest),
            ..Default::default()
        },
        // This call names its target with an explicit `path`, which outranks
        // `--manifest-dir`; passing None keeps it resolving exactly as before.
        None,
    )
    .unwrap();

    let fast_events = events_for(&fast);
    let slow_events = events_for(&slow);

    assert!(
        !fast_events.is_empty(),
        "the compressed path records its grant — an unrecorded yes is an unfalsifiable one"
    );
    let actions = |es: &[TrustMutation]| es.iter().map(|e| e.action).collect::<Vec<TrustAction>>();
    assert_eq!(
        actions(&fast_events),
        actions(&slow_events),
        "same actions, same order:\nfast {fast_events:?}\nslow {slow_events:?}"
    );
    let digests = |es: &[TrustMutation]| es.iter().map(|e| e.digest.clone()).collect::<Vec<_>>();
    assert_eq!(
        digests(&fast_events),
        digests(&slow_events),
        "the same content consented to through the same grant yields the same digest"
    );

    // And the end state is the same: both projects trusted over the same
    // manifest, with the skill declared identically.
    assert_eq!(
        agentstack::trust::check(&fast),
        agentstack::trust::TrustState::Trusted
    );
    assert_eq!(
        agentstack::trust::check(&slow),
        agentstack::trust::TrustState::Trusted
    );
    assert_eq!(
        fs::read_to_string(fast.join(".agentstack/agentstack.toml")).unwrap(),
        fs::read_to_string(slow.join(".agentstack/agentstack.toml")).unwrap(),
        "both paths declare the content the same way"
    );
}

/// "Live everywhere" covers BOTH inert kinds.
///
/// Skills materialize through `use`; instruction fragments compile into the
/// managed regions of `CLAUDE.md`/`AGENTS.md` through their own command. A
/// funnel that ran only the first would declare an instruction, pin it, take
/// the user's yes for it, and then never deliver it — the quiet one-step-short
/// failure this witness exists to catch.
#[test]
fn one_yes_delivers_skills_and_instructions_alike() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    isolate_home(tmp.path());
    let proj = project(tmp.path(), "both");
    git(&proj, &["init", "-q"]);
    drop_skill(&proj, "pdf-review", "# PDF review\n");
    fs::create_dir_all(proj.join(".agentstack/instructions")).unwrap();
    fs::write(
        proj.join(".agentstack/instructions/house.md"),
        "Always run fmt before handing off.\n",
    )
    .unwrap();

    agentstack::commands::yes::run_gated(&yes_args(), Some(&proj), true).unwrap();

    let compiled = fs::read_to_string(proj.join("CLAUDE.md"))
        .expect("the instruction fragment compiled into CLAUDE.md");
    assert!(
        compiled.contains("Always run fmt before handing off."),
        "the fragment's text is in the managed region:\n{compiled}"
    );
    assert!(
        proj.join(".claude/skills/pdf-review").exists(),
        "and the skill is materialized for the CLIs that take one"
    );
}

/// Declining leaves the project exactly as it was.
///
/// The funnel writes the inert half — declarations and pins — before it can
/// render the surface being consented to, so "no" has to unwind them. If it
/// ever stops doing that, a user who read the review and refused would be left
/// holding a changed manifest they never agreed to: a consent surprise, which
/// is precisely what the Phase 2 gate counts. The `Rollback` type is correct
/// today; this witness is what keeps a later refactor honest.
#[test]
fn declining_leaves_nothing_behind() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    isolate_home(tmp.path());
    let proj = project(tmp.path(), "declined");
    git(&proj, &["init", "-q"]);
    drop_skill(&proj, "pdf-review", "# PDF review\n");
    fs::create_dir_all(proj.join(".agentstack/instructions")).unwrap();
    fs::write(proj.join(".agentstack/instructions/house.md"), "Rules.\n").unwrap();

    let dir = proj.join(".agentstack");
    let manifest_path = dir.join("agentstack.toml");
    let lock_path = agentstack::lock::Lock::path(&dir);
    let manifest_before = fs::read_to_string(&manifest_path).unwrap();
    assert!(!lock_path.exists(), "the fixture starts unpinned");
    let events_before = events_for(&proj).len();

    // Run the whole thing to the confirmation, then say no.
    let err = agentstack::commands::yes::run_answered(
        &agentstack::cli::YesArgs { yes: false },
        Some(&proj),
        true,
        Some(false),
    )
    .expect_err("declining is an error exit, not a silent success");
    assert!(
        err.to_string().contains("cancelled"),
        "the refusal says so plainly: {err:#}"
    );

    // (a) the manifest and lock are byte-identical to the pre-yes capture
    assert_eq!(
        fs::read_to_string(&manifest_path).unwrap(),
        manifest_before,
        "the manifest is byte-identical to before the run"
    );
    assert!(
        !lock_path.exists(),
        "a lockfile the project did not have is not left behind"
    );

    // (b) no consent event was recorded
    assert_eq!(
        events_for(&proj).len(),
        events_before,
        "a declined review records no trust mutation"
    );
    assert_eq!(
        agentstack::trust::check(&proj),
        agentstack::trust::TrustState::Untrusted,
        "and the project is still untrusted"
    );

    // (c) nothing was materialized for any CLI
    for path in [
        ".claude/skills",
        ".agents/skills",
        ".gemini/skills",
        ".pi/skills",
    ] {
        assert!(
            !proj.join(path).exists(),
            "{path} was not created by a declined run"
        );
    }
    assert!(
        !proj.join("CLAUDE.md").exists() && !proj.join("AGENTS.md").exists(),
        "no instruction fragment was compiled"
    );

    // And the dropped files are still just sitting there, still offered.
    let loaded = agentstack::manifest::load_from_dir(&dir).unwrap();
    let found = agentstack::intake::scan(&dir, &proj, &loaded.manifest);
    assert_eq!(
        found.items.len(),
        2,
        "both files are still waiting, undeclared and inert"
    );
}

/// Rider 2 — content that failed provenance cannot be selected by the funnel.
///
/// Not "is routed around": the plan the compressed path acts on is built by
/// filtering on provenance, so committed content is absent from it. The
/// project is left untouched and the user is pointed at the full review.
#[test]
fn content_that_arrived_with_the_project_cannot_take_the_short_path() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    isolate_home(tmp.path());
    let proj = project(tmp.path(), "cloned");

    git(&proj, &["init", "-q"]);
    drop_skill(&proj, "from-clone", "# Came with the repo\n");
    git(&proj, &["add", "-A"]);
    git(&proj, &["commit", "-qm", "initial"]);

    let before = fs::read_to_string(proj.join(".agentstack/agentstack.toml")).unwrap();
    agentstack::commands::yes::run_gated(&yes_args(), Some(&proj), true).unwrap();

    assert_eq!(
        fs::read_to_string(proj.join(".agentstack/agentstack.toml")).unwrap(),
        before,
        "nothing was declared"
    );
    assert!(
        !agentstack::lock::Lock::path(&proj.join(".agentstack")).exists(),
        "nothing was pinned"
    );
    assert_eq!(
        agentstack::trust::check(&proj),
        agentstack::trust::TrustState::Untrusted,
        "nothing was granted"
    );
    assert!(
        events_for(&proj).is_empty(),
        "and no consent event was recorded"
    );
}

/// Rider 2 — a name collision cannot take the short path either.
///
/// Slice A refuses to adopt a taken name; slice B must inherit that refusal
/// rather than re-deriving its own selection. The pinned declaration survives.
#[test]
fn a_name_collision_cannot_take_the_short_path() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    isolate_home(tmp.path());
    let proj = project(tmp.path(), "collide");
    fs::write(
        proj.join(".agentstack/agentstack.toml"),
        "version = 1\n\n[skills.review]\ngit = \"https://example.invalid/s\"\nrev = \"abc123\"\n",
    )
    .unwrap();
    git(&proj, &["init", "-q"]);
    drop_skill(&proj, "review", "# Repo-controlled\n");

    agentstack::commands::yes::run_gated(&yes_args(), Some(&proj), true).unwrap();

    let after = agentstack::manifest::load_from_dir(&proj.join(".agentstack")).unwrap();
    let review = after.manifest.skills.get("review").unwrap();
    assert_eq!(
        review.git.as_deref(),
        Some("https://example.invalid/s"),
        "the pinned source survives the funnel"
    );
    assert!(review.path.is_none(), "it was not redirected to repo bytes");
    assert_eq!(
        agentstack::trust::check(&proj),
        agentstack::trust::TrustState::Untrusted,
        "and nothing was granted over it"
    );
}
