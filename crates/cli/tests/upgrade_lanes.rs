// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! W3 — update semantics: the mixed-lane upgrade transaction, the per-lane
//! report, and `status`'s update offer.
//!
//! Its own binary rather than more of `vendor_packs.rs` for one structural
//! reason: every property here needs a **real version bump**, and the embedded
//! catalog carries one version per id (a catalog upgrade always reports
//! "already current"). Only the git rail can move a pack from v0.1.0 to
//! v0.2.0, so these tests need a `file://` pack repo — the fixture
//! `vendor_packs.rs` deliberately does not have. Two of them also need the
//! command's real **stdout**, which means driving the built binary instead of
//! the library entry point.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn git(args: &[&str], cwd: &Path) {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A mixed-lane pack: one skill (dynamic lane) and one house-rule instruction
/// (rendered lane) — the exact composition the contract's transaction rule is
/// written about.
const PACK_TOML: &str = r#"name = "acme"
description = "Acme's agent setup."

[[skill]]
name = "sql-review"
path = "skills/sql-review"

[[instruction]]
name = "acme-rules"
path = "rules.md"
"#;

/// A pack repo tagged v0.1.0, with v0.2.0 published on top when `two_versions`
/// — the clone `add` makes then carries BOTH tags, which is what lets the
/// offline update check in `status` see the newer one.
fn make_pack_repo(dir: &Path, two_versions: bool) -> String {
    std::fs::create_dir_all(dir.join("skills/sql-review")).unwrap();
    std::fs::write(dir.join("pack.toml"), PACK_TOML).unwrap();
    std::fs::write(
        dir.join("skills/sql-review/SKILL.md"),
        "---\nname: sql-review\ndescription: Review SQL.\n---\nBody v1.\n",
    )
    .unwrap();
    std::fs::write(dir.join("rules.md"), "Always use transactions.\n").unwrap();
    git(&["init", "-q"], dir);
    git(&["add", "."], dir);
    git(&["commit", "-qm", "v0.1.0"], dir);
    git(&["tag", "v0.1.0"], dir);
    if two_versions {
        publish_v2(dir);
    }
    format!("file://{}", dir.display())
}

/// Publish v0.2.0: both lanes move — the skill body and the house rules.
fn publish_v2(dir: &Path) {
    std::fs::write(
        dir.join("skills/sql-review/SKILL.md"),
        "---\nname: sql-review\ndescription: Review SQL.\n---\nBody v2.\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("rules.md"),
        "Always use transactions. And EXPLAIN.\n",
    )
    .unwrap();
    git(&["add", "."], dir);
    git(&["commit", "-qm", "v0.2.0"], dir);
    git(&["tag", "v0.2.0"], dir);
}

struct Sandbox {
    _tmp: assert_fs::TempDir,
    home: PathBuf,
    proj: PathBuf,
}

/// A project plus its own agentstack home. `targets` is written verbatim so a
/// test can pick which instruction files exist.
fn sandbox(targets: &str) -> Sandbox {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(
        proj.join("agentstack.toml"),
        format!("version = 1\n[targets]\ndefault = [{targets}]\n"),
    )
    .unwrap();
    Sandbox {
        _tmp: tmp,
        home,
        proj,
    }
}

impl Sandbox {
    /// Run the real binary against this project. The agentstack home is passed
    /// per-child, so these tests never mutate the process environment and need
    /// no cross-test lock.
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_agentstack"))
            .arg("--manifest-dir")
            .arg(&self.proj)
            .args(args)
            .env("HOME", &self.home)
            .env("AGENTSTACK_HOME", self.home.join(".agentstack"))
            .output()
            .expect("agentstack runs")
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "agentstack {args:?} failed:\n{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.proj.join(rel)).unwrap_or_default()
    }
}

/// The result report is the block after the `✓ upgraded …` headline; the
/// preview above it describes a plan that had not been written yet.
fn result_report(stdout: &str) -> Vec<String> {
    let lines: Vec<&str> = stdout.lines().collect();
    let head = lines
        .iter()
        .position(|l| l.contains("upgraded acme"))
        .unwrap_or_else(|| panic!("no upgrade headline in:\n{stdout}"));
    lines[head + 1..].iter().map(|l| l.to_string()).collect()
}

fn lane<'a>(report: &'a [String], label: &str) -> Vec<&'a String> {
    report
        .iter()
        .filter(|l| l.trim_start().starts_with(label))
        .collect()
}

// ------------------------------------------------------------- the transaction

/// The contract, in one test: "it updates the lock **and** re-renders the
/// managed instruction region, or it does neither."
///
/// First half — a mixed-lane upgrade moves both: the skill pin, the
/// instruction pin, and the managed region in the rendered file. Second half —
/// a failure injected *after* the manifest is written leaves every artifact
/// byte-identical to its pre-upgrade state.
#[test]
fn an_upgrade_updates_the_lock_and_the_rendered_region_or_neither() {
    let sb = sandbox("\"claude-code\", \"junie\"");
    let repo = sb.proj.parent().unwrap().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let url = make_pack_repo(&repo, false);

    sb.ok(&[
        "add",
        "from",
        &format!("git:{url}@v0.1.0"),
        "--with-instructions",
        "--write",
    ]);
    // Pin the skill, then render the managed region once — deliberately, the
    // way a user would. The upgrade is only ever allowed to refresh a region
    // that already exists.
    sb.ok(&["install"]);
    sb.ok(&["instructions", "--write"]);
    assert!(sb.read("CLAUDE.md").contains("Always use transactions."));
    assert!(sb
        .read(".junie/AGENTS.md")
        .contains("Always use transactions."));

    publish_v2(&repo);

    let lock_path = sb.proj.join("agentstack.lock");
    let lock_before = agentstack::lock::Lock::load(&sb.proj).unwrap();
    let skill_pin_before = lock_before.get("sql-review").unwrap().checksum.clone();
    let instr_pin_before = lock_before
        .instructions
        .iter()
        .find(|i| i.name == "acme-rules")
        .expect("instruction pinned by `instructions --write`")
        .checksum
        .clone();

    sb.ok(&["lock", "--upgrade", "acme", "--yes", "--write"]);

    // Both lanes moved, in one transaction.
    let lock_after = agentstack::lock::Lock::load(&sb.proj).unwrap();
    assert_ne!(
        lock_after.get("sql-review").unwrap().checksum,
        skill_pin_before,
        "the skill pin must move with the upgrade"
    );
    assert_ne!(
        lock_after
            .instructions
            .iter()
            .find(|i| i.name == "acme-rules")
            .expect("instruction pin survives the upgrade")
            .checksum,
        instr_pin_before,
        "the instruction pin must move with the upgrade"
    );
    assert!(
        sb.read("CLAUDE.md").contains("And EXPLAIN."),
        "the managed region must be re-rendered: {}",
        sb.read("CLAUDE.md")
    );

    // ---- "or it does neither" -------------------------------------------
    //
    // Publish v0.3.0, then make the SECOND rendered target's directory
    // read-only. The upgrade gets through the manifest write, the asset swap,
    // the fragment write, the lock re-pin and the first region — and then
    // cannot write `.junie/AGENTS.md`. Everything must come back.
    std::fs::write(repo.join("rules.md"), "Rules v3.\n").unwrap();
    std::fs::write(
        repo.join("skills/sql-review/SKILL.md"),
        "---\nname: sql-review\ndescription: Review SQL.\n---\nBody v3.\n",
    )
    .unwrap();
    git(&["add", "."], &repo);
    git(&["commit", "-qm", "v0.3.0"], &repo);
    git(&["tag", "v0.3.0"], &repo);

    let before = Snapshot::take(&sb, &lock_path);
    let junie_dir = sb.proj.join(".junie");
    set_mode(&junie_dir, 0o555);
    let out = sb.run(&["lock", "--upgrade", "acme", "--yes", "--write"]);
    set_mode(&junie_dir, 0o755);

    assert!(
        !out.status.success(),
        "an unwritable rendered target must fail the upgrade, not half-apply it:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("rolled back"),
        "the failure must name the rollback: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    before.assert_restored(&sb, &lock_path);
}

/// Every artifact the transaction can touch, as bytes.
struct Snapshot {
    manifest: String,
    fragment: String,
    lock: String,
    claude: String,
    junie: String,
}

impl Snapshot {
    fn take(sb: &Sandbox, lock: &Path) -> Snapshot {
        Snapshot {
            manifest: sb.read("agentstack.toml"),
            fragment: sb.read("instructions/acme-rules.md"),
            lock: std::fs::read_to_string(lock).unwrap(),
            claude: sb.read("CLAUDE.md"),
            junie: sb.read(".junie/AGENTS.md"),
        }
    }

    fn assert_restored(&self, sb: &Sandbox, lock: &Path) {
        assert_eq!(sb.read("agentstack.toml"), self.manifest, "manifest");
        assert_eq!(
            sb.read("instructions/acme-rules.md"),
            self.fragment,
            "instruction fragment"
        );
        assert_eq!(
            std::fs::read_to_string(lock).unwrap(),
            self.lock,
            "lockfile — a rolled-back upgrade must not leave a moved pin"
        );
        assert_eq!(sb.read("CLAUDE.md"), self.claude, "rendered region");
        assert_eq!(sb.read(".junie/AGENTS.md"), self.junie, "rendered region");
    }
}

fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

// ------------------------------------------------------------ the lane report

/// The binding honesty rules of §"Mixed-lane upgrades are transactional, and
/// report per lane": two separate lane lines, the rendered one naming the file
/// it actually wrote, and never an instruction described as going live "via
/// gateway" — it went to a file, and the sentence has to say so.
#[test]
fn the_report_names_each_lane_separately_and_never_claims_a_file_went_live_via_gateway() {
    let sb = sandbox("\"claude-code\"");
    let repo = sb.proj.parent().unwrap().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let url = make_pack_repo(&repo, false);

    sb.ok(&[
        "add",
        "from",
        &format!("git:{url}@v0.1.0"),
        "--with-instructions",
        "--write",
    ]);
    sb.ok(&["instructions", "--write"]);
    publish_v2(&repo);

    let stdout = sb.ok(&["lock", "--upgrade", "acme", "--yes", "--write"]);
    let report = result_report(&stdout);

    let dynamic = lane(&report, "dynamic lane:");
    let rendered = lane(&report, "rendered lane:");
    assert_eq!(dynamic.len(), 1, "one dynamic-lane line:\n{stdout}");
    assert_eq!(rendered.len(), 1, "one rendered-lane line:\n{stdout}");
    assert!(
        dynamic[0].contains("skill"),
        "the dynamic lane names its members: {}",
        dynamic[0]
    );
    assert!(
        rendered[0].contains("CLAUDE.md"),
        "the rendered lane must name the file it wrote: {}",
        rendered[0]
    );
    // Never one blended sentence: neither line may carry the other's lane.
    assert!(!dynamic[0].contains("rendered lane"));
    assert!(!rendered[0].contains("dynamic lane"));

    // The binding copy rule, as a property over the whole run.
    for line in stdout.lines() {
        if line.contains("acme-rules") {
            assert!(
                !line.contains("gateway"),
                "an instruction never goes live via gateway: {line}"
            );
        }
    }
}

// ------------------------------------------------------- the conservative rule

/// A package upgrade must never be the reason an instruction file appears in a
/// project. With no managed region on disk, the fragment still moves — and the
/// report says plainly that nothing was rendered, naming the command that
/// would do it.
#[test]
fn an_upgrade_never_creates_an_instruction_file_that_did_not_exist() {
    let sb = sandbox("\"claude-code\", \"junie\"");
    let repo = sb.proj.parent().unwrap().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let url = make_pack_repo(&repo, false);

    // Installed WITH house rules, but never rendered — no `instructions
    // --write`, so no file carries the managed region.
    sb.ok(&[
        "add",
        "from",
        &format!("git:{url}@v0.1.0"),
        "--with-instructions",
        "--write",
    ]);
    assert!(!sb.proj.join("CLAUDE.md").exists());
    publish_v2(&repo);

    let stdout = sb.ok(&["lock", "--upgrade", "acme", "--yes", "--write"]);

    assert!(
        !sb.proj.join("CLAUDE.md").exists(),
        "the upgrade must not create an instruction file"
    );
    assert!(
        !sb.proj.join(".junie/AGENTS.md").exists(),
        "…nor a nested one"
    );
    // The fragment itself did move — and the report names where it went.
    assert!(sb
        .read("instructions/acme-rules.md")
        .contains("And EXPLAIN."));

    let report = result_report(&stdout);
    let rendered = lane(&report, "rendered lane:");
    assert_eq!(rendered.len(), 1, "{stdout}");
    assert!(
        rendered[0].contains("acme-rules.md"),
        "names what WAS written: {}",
        rendered[0]
    );
    assert!(
        rendered[0].contains("no file was rendered"),
        "and says plainly that nothing was rendered: {}",
        rendered[0]
    );
    assert!(
        stdout.contains("agentstack instructions --write"),
        "…naming the command that would render it:\n{stdout}"
    );
}

// -------------------------------------------------------------- status offers

/// Update model rule 2 — `status` offers: it names that updates are available
/// and the one shipped command that takes them. Plus the honest negatives: no
/// packs, and a pack whose check simply cannot be made, both emit **no**
/// `updates` object, because absence must never read as "up to date".
#[test]
fn status_offers_an_available_update_and_names_the_one_command() {
    let sb = sandbox("\"claude-code\"");
    let repo = sb.proj.parent().unwrap().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    // Both tags exist before the install, so the clone `add` makes carries
    // v0.2.0 — the offline check reads tags git already fetched, nothing more.
    let url = make_pack_repo(&repo, true);

    // No packs at all: no offer, and the key is absent rather than empty.
    let bare: serde_json::Value =
        serde_json::from_str(&sb.ok(&["status", "--json"])).expect("status --json parses");
    assert!(
        bare["project"].get("updates").is_none(),
        "a project with no packs offers nothing: {}",
        bare["project"]
    );

    sb.ok(&["add", "from", &format!("git:{url}@v0.1.0"), "--write"]);

    let json: serde_json::Value =
        serde_json::from_str(&sb.ok(&["status", "--json"])).expect("status --json parses");
    let updates = &json["project"]["updates"];
    assert_eq!(updates["packs"][0]["name"], "acme");
    assert_eq!(updates["packs"][0]["current"], "v0.1.0");
    assert_eq!(updates["packs"][0]["available"], "v0.2.0");
    assert_eq!(
        updates["fix"], "agentstack lock --upgrade acme",
        "the SHIPPED spelling, not the working name `upgrade`"
    );
    assert!(
        json["features"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f == "update-offer-v1"),
        "the contract name is advertised"
    );

    // The human screen offers the same thing, in one line, with the command.
    let screen = sb.ok(&["status"]);
    assert!(screen.contains("v0.1.0 → v0.2.0"), "{screen}");
    assert!(
        screen.contains("agentstack lock --upgrade acme"),
        "{screen}"
    );

    // The check that cannot be made: the same ledger, on a machine whose store
    // has no clone of that repo. Nothing is offered — and nothing anywhere
    // says "up to date", because this surface cannot know that.
    let elsewhere = sandbox("\"claude-code\"");
    std::fs::write(
        elsewhere.proj.join("agentstack.toml"),
        format!(
            "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\n\
             [packs.acme]\nversion = \"v0.1.0\"\ndescription = \"Acme\"\n\
             source = \"git:{url}@v0.1.0\"\n"
        ),
    )
    .unwrap();
    let uncheckable: serde_json::Value =
        serde_json::from_str(&elsewhere.ok(&["status", "--json"])).expect("status --json parses");
    assert!(
        uncheckable["project"].get("updates").is_none(),
        "an unanswerable check offers nothing: {}",
        uncheckable["project"]
    );
    let screen = elsewhere.ok(&["status"]);
    assert!(
        !screen.to_lowercase().contains("up to date"),
        "silence must never be rendered as currency: {screen}"
    );
}
