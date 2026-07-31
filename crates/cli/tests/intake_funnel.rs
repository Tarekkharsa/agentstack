// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Phase 1, slice A — the intake funnel's witnesses (`STRATEGY.md` Phase 1).
//!
//! Three properties, each with a test:
//!
//! - **Inertness.** Staged content does nothing before any yes. This is the
//!   property the funnel must not weaken as it gets more convenient.
//! - **The provenance split.** One directory, two skills differing only in
//!   provenance, two different paths — here with git as the discriminator,
//!   which is the signal a real dropped file actually hits.
//! - **Adoption is a declaration, not an activation.** `adopt --write` adds a
//!   manifest entry and nothing else: no pin, no trust, no rendered config.

use std::fs;
use std::path::Path;

use agentstack::intake;

// These tests set HOME/AGENTSTACK_HOME, which are process-global.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn isolate_home(tmp: &Path) {
    let home = tmp.join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));
}

/// A project with a manifest and an `.agentstack/` tree, ready for drops.
fn project(tmp: &Path) -> std::path::PathBuf {
    let proj = tmp.join("proj");
    fs::create_dir_all(proj.join(".agentstack/skills")).unwrap();
    fs::create_dir_all(proj.join(".agentstack/instructions")).unwrap();
    fs::write(
        proj.join(".agentstack/agentstack.toml"),
        "version = 1\n\n# a comment the merger must preserve\n",
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
    assert!(out.status.success(), "git {args:?}: {:?}", out);
}

fn scan(proj: &Path) -> Vec<intake::Item> {
    let dir = proj.join(".agentstack");
    let loaded = agentstack::manifest::load_from_dir(&dir).unwrap();
    intake::scan(&dir, proj, &loaded.manifest).items
}

/// Witness 1 — staged content is provably inert before any yes.
///
/// The dropped skill is visible to intake detection and to nothing else: it is
/// not in the manifest, not resolvable, and not materialized to any CLI. The
/// funnel's whole safety story is that noticing content and delivering it are
/// separate events.
#[test]
fn staged_content_is_inert_until_it_is_declared() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    isolate_home(tmp.path());
    let proj = project(tmp.path());
    drop_skill(
        &proj,
        "dropped",
        "---\ndescription: Not yours yet\n---\n# Dropped\n",
    );

    // Seen by intake…
    let items = scan(&proj);
    assert_eq!(items.len(), 1, "the drop is noticed: {items:?}");
    assert_eq!(items[0].name, "dropped");

    // …and by nothing else.
    let dir = proj.join(".agentstack");
    let loaded = agentstack::manifest::load_from_dir(&dir).unwrap();
    assert!(
        loaded.manifest.skills.is_empty(),
        "noticing content must not declare it"
    );
    assert!(
        !agentstack::lock::Lock::path(&dir).exists(),
        "noticing content must not pin it"
    );
    assert_eq!(
        agentstack::trust::check(&proj),
        agentstack::trust::TrustState::Untrusted,
        "noticing content must not grant anything"
    );
    // No CLI has a copy of it: nothing rendered, nothing materialized.
    assert!(
        !proj.join(".claude/skills/dropped").exists(),
        "inert content is not delivered to any CLI"
    );
}

/// Witness 2 — the provenance split, with git as the discriminator.
///
/// Same directory, two skills, identical in every way except that one is
/// committed and one is not. The committed one came with the project; the
/// untracked one is the user's own work. They must take different paths.
#[test]
fn same_directory_two_provenances_two_paths() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    isolate_home(tmp.path());
    let proj = project(tmp.path());

    git(&proj, &["init", "-q"]);
    drop_skill(&proj, "from-clone", "# Same body\n");
    git(&proj, &["add", "-A"]);
    git(&proj, &["commit", "-qm", "initial"]);

    // Byte-identical content, dropped after the commit.
    drop_skill(&proj, "my-own", "# Same body\n");

    let items = scan(&proj);
    let by = |n: &str| {
        items
            .iter()
            .find(|i| i.name == n)
            .unwrap_or_else(|| panic!("{n} in {items:?}"))
    };
    assert!(
        !by("from-clone").provenance.is_local(),
        "committed content took the full-review path"
    );
    assert!(
        by("my-own").provenance.is_local(),
        "untracked content is recognized as local work"
    );
    assert_ne!(
        by("from-clone").provenance,
        by("my-own").provenance,
        "identical content, different provenance, different paths"
    );
}

/// Review follow-up — git's own checkout timestamps must not read as authorship.
///
/// `git pull`/`checkout` rewrites the mtime of every file it lands, so tracked
/// content is routinely "newer than the last review" while being entirely
/// remote-authored. Inside a work tree, tracking is the only signal that may
/// decide, or the split inverts exactly where it matters.
#[test]
fn a_freshly_checked_out_tracked_skill_is_not_local_work() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    isolate_home(tmp.path());
    let proj = project(tmp.path());

    git(&proj, &["init", "-q"]);
    drop_skill(&proj, "from-clone", "# From the repo\n");
    git(&proj, &["add", "-A"]);
    git(&proj, &["commit", "-qm", "initial"]);
    // What a checkout does: the tracked file's mtime becomes "now", newer than
    // any plausible past review.
    let f = proj.join(".agentstack/skills/from-clone/SKILL.md");
    std::fs::File::options()
        .write(true)
        .open(&f)
        .unwrap()
        .set_modified(std::time::SystemTime::now())
        .unwrap();

    let items = scan(&proj);
    let item = items.iter().find(|i| i.name == "from-clone").unwrap();
    assert!(
        !item.provenance.is_local(),
        "a fresh mtime on tracked content is git's, not the user's: {:?}",
        item.provenance
    );
}

/// Review follow-up — a symlinked `SKILL.md` is not read.
///
/// Following it would let a hostile repo have `status` open an arbitrary file
/// (`~/.ssh/id_rsa`) and print its first line as a "summary", before any gate.
/// The refusal has to cover content, not only the directory entry.
#[cfg(unix)]
#[test]
fn a_symlinked_skill_body_is_never_read() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    isolate_home(tmp.path());
    let proj = project(tmp.path());

    let secret = tmp.path().join("id_rsa");
    fs::write(&secret, "-----BEGIN OPENSSH PRIVATE KEY-----\nsensitive\n").unwrap();
    let d = proj.join(".agentstack/skills/notes");
    fs::create_dir_all(&d).unwrap();
    std::os::unix::fs::symlink(&secret, d.join("SKILL.md")).unwrap();

    let items = scan(&proj);
    assert!(
        items.is_empty(),
        "a skill whose body is a symlink is refused outright: {items:?}"
    );
    let printed = format!("{items:?}");
    assert!(
        !printed.contains("OPENSSH") && !printed.contains("sensitive"),
        "no byte of the linked file reaches a display path"
    );
}

/// Review follow-up — a dropped file may not silently replace a pinned entry.
///
/// A git-sourced skill has no `path`, so a same-named drop is invisible to the
/// path comparison. Adopting it would swap a pinned, reviewed declaration for
/// repo-controlled bytes behind a preview that called it an addition.
#[test]
fn a_name_the_manifest_already_declares_is_reported_not_adopted() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    isolate_home(tmp.path());
    let proj = project(tmp.path());
    fs::write(
        proj.join(".agentstack/agentstack.toml"),
        "version = 1\n\n[skills.review]\ngit = \"https://example.invalid/skills\"\nrev = \"abc123\"\n",
    )
    .unwrap();
    drop_skill(&proj, "review", "# Repo-controlled\n");

    let dir = proj.join(".agentstack");
    let loaded = agentstack::manifest::load_from_dir(&dir).unwrap();
    let found = intake::scan(&dir, &proj, &loaded.manifest);
    assert!(
        found.items.is_empty(),
        "a taken name is not offered as an addition: {:?}",
        found.items
    );
    assert_eq!(
        found.collisions.len(),
        1,
        "it is reported as a collision instead"
    );
    assert_eq!(found.collisions[0].name, "review");

    // And adopting leaves the pinned declaration exactly as it was.
    let args = agentstack::cli::AdoptArgs {
        targets: Vec::new(),
        scope: None,
        write: true,
        no_keychain: true,
        to_library: false,
    };
    agentstack::commands::adopt::run(&args, Some(&proj)).unwrap();
    let after = agentstack::manifest::load_from_dir(&dir).unwrap();
    let review = after.manifest.skills.get("review").unwrap();
    assert_eq!(
        review.git.as_deref(),
        Some("https://example.invalid/skills"),
        "the pinned git source survives"
    );
    assert_eq!(review.rev.as_deref(), Some("abc123"), "the pin survives");
    assert!(review.path.is_none(), "it was not redirected to repo bytes");
}

/// Witness 3 — `adopt --write` declares, and only declares.
///
/// It writes one manifest entry through the shared insertion path (preserving
/// the file's comments), and it does not pin, trust, or render. The user still
/// owes a lock and a yes; slice B is what collapses those into one action, and
/// it may not do so by having adoption quietly perform them.
#[test]
fn adopt_declares_the_dropped_file_and_nothing_more() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    isolate_home(tmp.path());
    let proj = project(tmp.path());
    drop_skill(&proj, "dropped", "# Dropped\n");
    fs::write(
        proj.join(".agentstack/instructions/house.md"),
        "House rules.\n",
    )
    .unwrap();

    let args = agentstack::cli::AdoptArgs {
        targets: Vec::new(),
        scope: None,
        write: true,
        no_keychain: true,
        to_library: false,
    };
    agentstack::commands::adopt::run(&args, Some(&proj)).unwrap();

    let dir = proj.join(".agentstack");
    let text = fs::read_to_string(dir.join("agentstack.toml")).unwrap();
    assert!(
        text.contains("# a comment the merger must preserve"),
        "the manifest write is format-preserving:\n{text}"
    );

    let loaded = agentstack::manifest::load_from_dir(&dir).unwrap();
    assert_eq!(
        loaded
            .manifest
            .skills
            .get("dropped")
            .and_then(|s| s.path.as_deref()),
        Some("./skills/dropped"),
        "the skill is declared at its real path"
    );
    assert_eq!(
        loaded
            .manifest
            .instructions
            .get("house")
            .map(|i| i.path.as_str()),
        Some("./instructions/house.md"),
        "the instruction is declared at its real path"
    );

    // Declaring is not delivering.
    assert!(
        !agentstack::lock::Lock::path(&dir).exists(),
        "adopt does not pin"
    );
    assert_eq!(
        agentstack::trust::check(&proj),
        agentstack::trust::TrustState::Untrusted,
        "adopt does not grant"
    );

    // And a second run has nothing left to say about them.
    assert!(
        scan(&proj).is_empty(),
        "declared content is not offered again"
    );
}
