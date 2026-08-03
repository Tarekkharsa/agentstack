// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! W5, the package layer: a compact package *reference* in a toolset, and the
//! exact pinned member set it compiles into.
//!
//! `docs/design/package-layer.md` is the contract, and every test here is one
//! clause of it. The claim under test is the one that makes the compact
//! reference legitimate at all (`automatic-delivery.md` §"Copy versus live
//! reference, settled"): a reference is as safe as vendoring **iff the lock
//! pins the expanded member set**. So these witness the expansion, that runtime
//! reads it rather than the library, that an override is visible instead of
//! silent, and the two refusals — package-carried executable kinds, and any
//! uncertainty at all.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use agentstack::cli::LockArgs;
use agentstack::commands::lock as lock_cmd;
use agentstack::library::{Library, LibraryPackage, LibrarySkill};
use agentstack::lock::{Lock, PackageMemberKind, PackageMemberOrigin};

// These tests mutate the process-global HOME/AGENTSTACK_HOME; serialize them.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// A temp machine: `HOME` and `AGENTSTACK_HOME` pointed inside `tmp`, with a
/// project dir returned. Mirrors `content_pinning.rs`'s setup.
fn machine(tmp: &assert_fs::TempDir) -> (PathBuf, PathBuf) {
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));
    let lib_home = home.join(".agentstack/lib");
    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    (proj, lib_home)
}

/// Install a path-sourced package body under `<lib_home>/packages/<name>/` and
/// index it. `members` are `(relative path, contents)` pairs written verbatim,
/// so a test controls exactly what each member's digest covers.
fn install_package(
    lib_home: &Path,
    name: &str,
    version: &str,
    pack_toml: &str,
    members: &[(&str, &str)],
) {
    let root = lib_home.join("packages").join(name);
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("pack.toml"), pack_toml).unwrap();
    for (rel, contents) in members {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, contents).unwrap();
    }
    let mut lib = Library::load(lib_home).unwrap();
    lib.upsert_package(LibraryPackage {
        name: name.into(),
        source: "path".into(),
        path: Some(name.into()),
        git: None,
        rev: None,
        subpath: None,
        checksum: None,
        version: Some(version.into()),
        description: None,
        provenance: Some("manual".into()),
    });
    lib.save(lib_home).unwrap();
}

/// The `pack.toml` used by most tests: a server, a skill, and an instruction —
/// one member of every v1 kind, so lane and kind assertions are meaningful.
const FULL_PACK: &str = r#"
name = "rust-backend"
description = "Rust backend house rules"

[server]
type = "http"
url = "https://backend.example/mcp"
secret_headers = ["Authorization"]

[[skill]]
name = "sql-review"
path = "skills/sql-review"

[[instruction]]
name = "house-rules"
path = "instructions/house.md"
"#;

fn full_pack_members() -> Vec<(&'static str, &'static str)> {
    vec![
        ("skills/sql-review/SKILL.md", "# sql review v1\n"),
        ("instructions/house.md", "Prefer boring Rust.\n"),
    ]
}

/// The full anyhow chain of a refused lock, flattened. `to_string()` alone
/// shows only the outermost context, which for a package is "expanding package
/// 'x'" — true, and never the reason.
fn lock_failure(proj: &Path) -> String {
    let err = lock_cmd::run(&LockArgs::default(), Some(proj)).expect_err("lock must refuse");
    format!("{err:#}")
}

fn write_manifest(proj: &Path, body: &str) {
    fs::write(proj.join("agentstack.toml"), body).unwrap();
}

/// W5 acceptance, first half: *a package selected in a toolset expands in the
/// lock to exact members with per-member digests and provenance.* Asserted
/// field by field, because "there is a package entry" is not the claim — the
/// claim is that every member is individually pinned and individually
/// attributed.
#[test]
fn a_package_selected_in_a_toolset_expands_in_the_lock_to_exact_members() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let (proj, lib_home) = machine(&tmp);
    install_package(
        &lib_home,
        "rust-backend",
        "1.4.0",
        FULL_PACK,
        &full_pack_members(),
    );
    write_manifest(
        &proj,
        "version = 1\n[profiles.backend]\npackages = [\"rust-backend\"]\n",
    );

    lock_cmd::run(&LockArgs::default(), Some(&proj)).unwrap();

    let lock = Lock::load(&proj).unwrap();
    let pkg = lock.get_package("rust-backend").expect("package pinned");

    // Package identity: the exact version, the source, and WHY it is here.
    assert_eq!(pkg.version, "1.4.0");
    assert_eq!(pkg.source, "library:rust-backend");
    assert_eq!(pkg.toolsets, vec!["backend".to_string()]);
    assert!(pkg.removed.is_empty());

    // The exact member list — all three, nothing more, sorted by name.
    let names: Vec<&str> = pkg.members.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, vec!["house-rules", "rust-backend", "sql-review"]);

    // Per-member kind, lane, origin, digest and provenance, field by field.
    let member = |n: &str| {
        pkg.members
            .iter()
            .find(|m| m.name == n)
            .unwrap_or_else(|| panic!("member '{n}' pinned"))
    };

    let skill = member("sql-review");
    assert_eq!(skill.kind, PackageMemberKind::Skill);
    assert_eq!(skill.kind.lane(), "dynamic");
    assert_eq!(skill.origin, PackageMemberOrigin::Package);
    assert_eq!(skill.checksum.hex().len(), 64);
    assert_eq!(
        skill.provenance,
        "package:rust-backend@1.4.0#skills/sql-review"
    );

    let instruction = member("house-rules");
    assert_eq!(instruction.kind, PackageMemberKind::Instruction);
    assert_eq!(
        instruction.kind.lane(),
        "rendered",
        "an instruction member is never described as going live via the gateway"
    );
    assert_eq!(instruction.origin, PackageMemberOrigin::Package);
    assert_eq!(instruction.checksum.hex().len(), 64);
    assert_eq!(
        instruction.provenance,
        "package:rust-backend@1.4.0#instructions/house.md"
    );

    let server = member("rust-backend");
    assert_eq!(server.kind, PackageMemberKind::Server);
    assert_eq!(server.kind.lane(), "dynamic");
    assert_eq!(server.checksum.hex().len(), 64);
    assert_eq!(server.provenance, "package:rust-backend@1.4.0#[server]");

    // Every member digest is distinct: one roll-up digest wearing three hats
    // would let a member's bytes move inside an "unchanged" package.
    let digests: BTreeSet<&str> = pkg.members.iter().map(|m| m.checksum.hex()).collect();
    assert_eq!(digests.len(), 3);

    // Invariant 5: the server member pins its ${REF}-only definition, and no
    // resolved secret value can reach the lock.
    let text = fs::read_to_string(Lock::path(&proj)).unwrap();
    assert!(text.contains("[[package]]"), "{text}");
    assert!(text.contains("[[package.member]]"), "{text}");
    assert!(!text.to_lowercase().contains("bearer "), "{text}");

    // Invariant 4: one byte inside one member re-pins that member and only
    // that member, so the lock bytes move and the trust digest flips.
    let before_skill = skill.checksum.clone();
    let before_instruction = instruction.checksum.clone();
    fs::write(
        lib_home.join("packages/rust-backend/skills/sql-review/SKILL.md"),
        "# sql review v2\n",
    )
    .unwrap();
    lock_cmd::run(&LockArgs::default(), Some(&proj)).unwrap();
    let relocked = Lock::load(&proj).unwrap();
    let repinned = relocked.get_package("rust-backend").unwrap();
    let new_skill = repinned
        .members
        .iter()
        .find(|m| m.name == "sql-review")
        .unwrap();
    let new_instruction = repinned
        .members
        .iter()
        .find(|m| m.name == "house-rules")
        .unwrap();
    assert_ne!(
        new_skill.checksum, before_skill,
        "the edited member re-pins"
    );
    assert_eq!(
        new_instruction.checksum, before_instruction,
        "an untouched member's pin is unchanged"
    );
}

/// The reproducibility rule, for packages: *runtime resolves from the project
/// lock, never from the mutable current state of the library.* The library is
/// moved ahead after locking — a new member, and new bytes for an existing one
/// — and the effective member set is still the locked one.
#[test]
fn runtime_resolves_members_from_the_lock_not_the_current_library() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let (proj, lib_home) = machine(&tmp);
    install_package(
        &lib_home,
        "rust-backend",
        "1.4.0",
        FULL_PACK,
        &full_pack_members(),
    );
    write_manifest(
        &proj,
        "version = 1\n[profiles.backend]\npackages = [\"rust-backend\"]\n",
    );
    lock_cmd::run(&LockArgs::default(), Some(&proj)).unwrap();

    let locked = Lock::load(&proj).unwrap();
    let pinned = locked.get_package("rust-backend").unwrap().clone();

    // The library moves ahead: a fourth member appears, an existing member's
    // bytes change, and the index declares a new version.
    let root = lib_home.join("packages/rust-backend");
    fs::write(
        root.join("pack.toml"),
        format!("{FULL_PACK}\n[[skill]]\nname = \"newcomer\"\npath = \"skills/newcomer\"\n"),
    )
    .unwrap();
    fs::create_dir_all(root.join("skills/newcomer")).unwrap();
    fs::write(root.join("skills/newcomer/SKILL.md"), "# newcomer\n").unwrap();
    fs::write(root.join("skills/sql-review/SKILL.md"), "# moved ahead\n").unwrap();
    let mut lib = Library::load(&lib_home).unwrap();
    let mut entry = lib.get_package("rust-backend").unwrap().clone();
    entry.version = Some("2.0.0".into());
    lib.upsert_package(entry);
    lib.save(&lib_home).unwrap();

    // The read seam resolves from the LOCK. Nothing about the library's new
    // state reaches it: not the new member, not the new bytes, not the new
    // version. Taking them requires an explicit `agentstack lock`.
    let reread = Lock::load(&proj).unwrap();
    let effective = agentstack::package::effective_members(&reread);
    assert_eq!(effective.len(), 1);
    assert_eq!(effective[0], pinned, "the locked expansion is unchanged");
    assert_eq!(effective[0].version, "1.4.0", "not the library's 2.0.0");
    assert!(
        !effective[0].members.iter().any(|m| m.name == "newcomer"),
        "a member the library gained after locking is not in the effective set"
    );
    assert_eq!(
        agentstack::package::effective_members_of(&reread, "rust-backend"),
        Some(&pinned)
    );

    // And the pinned bytes are actually retrievable by digest — the deposit
    // that makes serving-from-the-lock possible rather than merely claimed.
    let store = agentstack::store::Store::default_store();
    let skill_pin = pinned
        .members
        .iter()
        .find(|m| m.name == "sql-review")
        .unwrap();
    assert!(
        store.has_pinned_content(skill_pin.checksum.hex()),
        "the pinned member's bytes are in the content store"
    );
}

/// W5 acceptance, second half: *a per-member override is visible as an
/// effective member set rather than silently diverging from the package.*
/// Both origins are named, the removal is named, and nothing about the
/// package's own vocabulary is lost.
#[test]
fn a_per_member_override_is_visible_as_an_effective_member_set() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let (proj, lib_home) = machine(&tmp);
    install_package(
        &lib_home,
        "rust-backend",
        "1.4.0",
        FULL_PACK,
        &full_pack_members(),
    );

    // The project declares its own skill and takes the package with one member
    // replaced by it and one member dropped.
    fs::create_dir_all(proj.join("skills/house-sql-review")).unwrap();
    fs::write(
        proj.join("skills/house-sql-review/SKILL.md"),
        "# our own sql review\n",
    )
    .unwrap();
    write_manifest(
        &proj,
        "version = 1\n\
         [skills.house-sql-review]\npath = \"./skills/house-sql-review\"\n\
         [profiles.backend]\npackages = [\"rust-backend\"]\n\
         [package_overrides.rust-backend]\nremove = [\"house-rules\"]\n\
         [package_overrides.rust-backend.replace]\nsql-review = \"house-sql-review\"\n",
    );
    lock_cmd::run(&LockArgs::default(), Some(&proj)).unwrap();

    let lock = Lock::load(&proj).unwrap();
    let pkg = lock.get_package("rust-backend").expect("package pinned");

    // The effective set: two members, and the removal is stated rather than
    // inferable only by diffing against the package.
    let names: Vec<&str> = pkg.members.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, vec!["rust-backend", "sql-review"]);
    assert_eq!(pkg.removed, vec!["house-rules".to_string()]);

    // Both origins are named on the members that have them.
    let overridden = pkg.members.iter().find(|m| m.name == "sql-review").unwrap();
    assert_eq!(overridden.origin, PackageMemberOrigin::ProjectOverride);
    assert_eq!(overridden.provenance, "project:skills.house-sql-review");
    assert_eq!(
        overridden.kind,
        PackageMemberKind::Skill,
        "the member keeps the package's kind and name — only its bytes changed"
    );
    let untouched = pkg
        .members
        .iter()
        .find(|m| m.name == "rust-backend")
        .unwrap();
    assert_eq!(untouched.origin, PackageMemberOrigin::Package);
    assert!(untouched
        .provenance
        .starts_with("package:rust-backend@1.4.0"));

    // Nothing silently diverges: the overriding member's digest is the
    // PROJECT's bytes, not the package's, and the two are distinguishable.
    let package_skill_digest = {
        let dir = lib_home.join("packages/rust-backend/skills/sql-review");
        agentstack::store::dir_digest(&dir).unwrap()
    };
    assert_ne!(
        overridden.checksum, package_skill_digest,
        "the effective member is the project's content"
    );
    let project_skill_digest =
        agentstack::store::dir_digest(&proj.join("skills/house-sql-review")).unwrap();
    assert_eq!(overridden.checksum, project_skill_digest);
}

/// The v1 fence, and it is permanent rather than pending: a package carrying a
/// hook or an extension is refused BY NAME. These are executable kinds whose
/// full consent ceremony can never be compressed, and a package reference is a
/// compression by construction.
#[test]
fn a_package_carrying_hooks_or_extensions_is_refused_by_name() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let (proj, lib_home) = machine(&tmp);
    install_package(
        &lib_home,
        "hooky",
        "1.0.0",
        "name = \"hooky\"\n\n[[hook]]\nname = \"pre-commit\"\npath = \"hooks/pre.sh\"\n",
        &[("hooks/pre.sh", "#!/bin/sh\necho hi\n")],
    );
    install_package(
        &lib_home,
        "extendy",
        "1.0.0",
        "name = \"extendy\"\n\n[[extension]]\nname = \"checkpoint\"\npath = \"ext/checkpoint\"\n",
        &[("ext/checkpoint/index.ts", "export default () => {}\n")],
    );

    for (package, member) in [("hooky", "pre-commit"), ("extendy", "checkpoint")] {
        write_manifest(
            &proj,
            &format!("version = 1\n[profiles.t]\npackages = [\"{package}\"]\n"),
        );
        let err = lock_failure(&proj);
        assert!(err.contains(member), "the refusal names the member: {err}");
        assert!(
            err.contains("not supported in v1"),
            "the refusal says what is unsupported: {err}"
        );
        assert!(
            err.contains("consent ceremony"),
            "the refusal says WHY, not just that: {err}"
        );
        assert!(
            !Lock::path(&proj).exists() || Lock::load(&proj).unwrap().packages.is_empty(),
            "a refused package pins nothing"
        );
    }
}

/// Any uncertainty refuses rather than serving a partial or unpinned set. Five
/// shapes of uncertainty, each one leaving the lock's package pins untouched.
#[test]
fn an_unresolvable_or_drifted_package_member_fails_closed() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let (proj, lib_home) = machine(&tmp);

    // 1 · A toolset naming a package the library does not have.
    write_manifest(
        &proj,
        "version = 1\n[profiles.t]\npackages = [\"absent\"]\n",
    );
    let err = lock_failure(&proj);
    assert!(err.contains("absent"), "{err}");
    assert!(
        !Lock::path(&proj).exists(),
        "a refused expansion writes no lock at all"
    );

    // 2 · A member whose declared path is not in the package.
    install_package(
        &lib_home,
        "gappy",
        "1.0.0",
        "name = \"gappy\"\n\n[[skill]]\nname = \"missing\"\npath = \"skills/missing\"\n",
        &[],
    );
    write_manifest(&proj, "version = 1\n[profiles.t]\npackages = [\"gappy\"]\n");
    let err = lock_failure(&proj);
    assert!(err.contains("missing"), "{err}");

    // 3 · A member path that escapes the package (rule 7 containment).
    install_package(
        &lib_home,
        "escapey",
        "1.0.0",
        "name = \"escapey\"\n\n[[skill]]\nname = \"out\"\npath = \"../../elsewhere\"\n",
        &[],
    );
    write_manifest(
        &proj,
        "version = 1\n[profiles.t]\npackages = [\"escapey\"]\n",
    );
    let err = lock_failure(&proj);
    assert!(err.contains("stay inside the pack"), "{err}");

    // 4 · A pack.toml whose bytes no longer match the library index pin. This
    //     is the intake gate, so it refuses — the "library moved ahead" reading
    //     is scoped to the serve path and to already-pinned skills, never here.
    install_package(&lib_home, "drifty", "1.0.0", "name = \"drifty\"\n", &[]);
    let mut lib = Library::load(&lib_home).unwrap();
    let mut entry = lib.get_package("drifty").unwrap().clone();
    entry.checksum = Some(agentstack_core::digest::Sha256Hex::of(b"different bytes"));
    lib.upsert_package(entry);
    lib.save(&lib_home).unwrap();
    write_manifest(
        &proj,
        "version = 1\n[profiles.t]\npackages = [\"drifty\"]\n",
    );
    let err = lock_failure(&proj);
    assert!(err.contains("index pin"), "{err}");

    // 5 · An override that would diverge from something the package does not
    //     carry. Silently matching nothing is how a project comes to believe it
    //     dropped a member it still has.
    install_package(
        &lib_home,
        "fine",
        "1.0.0",
        "name = \"fine\"\n\n[[skill]]\nname = \"real\"\npath = \"skills/real\"\n",
        &[("skills/real/SKILL.md", "# real\n")],
    );
    write_manifest(
        &proj,
        "version = 1\n[profiles.t]\npackages = [\"fine\"]\n\
         [package_overrides.fine]\nremove = [\"ghost\"]\n",
    );
    let err = lock_failure(&proj);
    assert!(err.contains("ghost"), "{err}");
    assert!(
        err.contains("does not carry"),
        "the refusal says what is wrong: {err}"
    );

    // Through all five, no package was ever pinned.
    let packages = Lock::load(&proj).map(|l| l.packages.len()).unwrap_or(0);
    assert_eq!(packages, 0, "nothing partial was ever written");

    // And a well-formed replacement of the wrong KIND is refused too: swapping
    // a skill for a server is a different composition, not an override.
    fs::write(proj.join("skills-placeholder"), "x").unwrap();
    write_manifest(
        &proj,
        "version = 1\n\
         [servers.db]\ntype = \"http\"\nurl = \"https://db.example/mcp\"\n\
         [profiles.t]\npackages = [\"fine\"]\n\
         [package_overrides.fine.replace]\nreal = \"db\"\n",
    );
    let err = lock_failure(&proj);
    assert!(err.contains("same kind"), "{err}");
}

/// A package no toolset selects any more loses its pin, while a package another
/// toolset still selects keeps one — the stale-pin rule, which matters more for
/// packages than for other kinds because a stale member set is a set runtime
/// would resolve from.
#[test]
fn dropping_a_package_reference_prunes_its_expansion() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let (proj, lib_home) = machine(&tmp);
    install_package(
        &lib_home,
        "rust-backend",
        "1.4.0",
        FULL_PACK,
        &full_pack_members(),
    );
    install_package(
        &lib_home,
        "observability",
        "0.2.0",
        "name = \"observability\"\n\n[[skill]]\nname = \"traces\"\npath = \"skills/traces\"\n",
        &[("skills/traces/SKILL.md", "# traces\n")],
    );

    write_manifest(
        &proj,
        "version = 1\n\
         [profiles.backend]\npackages = [\"rust-backend\"]\n\
         [profiles.ops]\npackages = [\"rust-backend\", \"observability\"]\n",
    );
    lock_cmd::run(&LockArgs::default(), Some(&proj)).unwrap();
    let lock = Lock::load(&proj).unwrap();
    assert_eq!(lock.packages.len(), 2);
    assert_eq!(
        lock.get_package("rust-backend").unwrap().toolsets,
        vec!["backend".to_string(), "ops".to_string()],
        "both selecting toolsets are recorded"
    );

    // Drop one reference; its expansion goes, the other's stays.
    write_manifest(
        &proj,
        "version = 1\n\
         [profiles.backend]\npackages = [\"rust-backend\"]\n\
         [profiles.ops]\npackages = [\"rust-backend\"]\n",
    );
    lock_cmd::run(&LockArgs::default(), Some(&proj)).unwrap();
    let lock = Lock::load(&proj).unwrap();
    assert!(lock.get_package("observability").is_none());
    assert!(lock.get_package("rust-backend").is_some());

    // Drop the rest; nothing is left behind.
    write_manifest(&proj, "version = 1\n[profiles.backend]\nskills = []\n");
    lock_cmd::run(&LockArgs::default(), Some(&proj)).unwrap();
    assert!(Lock::load(&proj).unwrap().packages.is_empty());
}

/// A library holding packages changes nothing for a project that references
/// none: the lock stays byte-identical, so no project is re-gated by the mere
/// existence of the package layer.
#[test]
fn a_project_referencing_no_package_is_untouched() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let (proj, lib_home) = machine(&tmp);
    install_package(
        &lib_home,
        "rust-backend",
        "1.4.0",
        FULL_PACK,
        &full_pack_members(),
    );

    let mut lib = Library::load(&lib_home).unwrap();
    fs::create_dir_all(lib_home.join("skills/solo")).unwrap();
    fs::write(lib_home.join("skills/solo/SKILL.md"), "# solo\n").unwrap();
    lib.upsert(LibrarySkill {
        name: "solo".into(),
        source: "path".into(),
        path: Some("solo".into()),
        git: None,
        rev: None,
        subpath: None,
        checksum: None,
        version: None,
        provenance: Some("manual".into()),
    });
    lib.save(&lib_home).unwrap();

    write_manifest(&proj, "version = 1\n[profiles.p]\nskills = [\"solo\"]\n");
    lock_cmd::run(&LockArgs::default(), Some(&proj)).unwrap();
    let first = fs::read(Lock::path(&proj)).unwrap();
    assert!(Lock::load(&proj).unwrap().packages.is_empty());

    // Re-locking is byte-identical — no empty `[[package]]` churn.
    lock_cmd::run(&LockArgs::default(), Some(&proj)).unwrap();
    assert_eq!(fs::read(Lock::path(&proj)).unwrap(), first);
}

// ---------------------------------------------------------------------------
// W5 acceptance, the runtime half: the boundary is exposed without loading
// bodies, and a server starts on first tool use rather than on activation.
// ---------------------------------------------------------------------------

/// Drive one real `agentstack mcp` process against `proj`, in eager mode, and
/// return one parsed JSON-RPC response per request.
///
/// A subprocess rather than an in-process call because the loadable index and
/// the load path are only reachable through the protocol — and because the
/// machine-global load stream this asserts on is written by the process that
/// actually served the load.
fn mcp_exchange(
    proj: &Path,
    home: &Path,
    requests: &[serde_json::Value],
) -> Vec<serde_json::Value> {
    use std::io::Write;
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(["mcp", "--manifest-dir"])
        .arg(proj)
        .env("HOME", home)
        .env("AGENTSTACK_HOME", home.join(".agentstack"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for request in requests {
        writeln!(stdin, "{request}").unwrap();
    }
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "the mcp server exited cleanly");
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// The JSON payload an `agentstack_*` tool call answers with.
fn tool_json(response: &serde_json::Value) -> serde_json::Value {
    serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
}

/// Every line of the machine-global on-demand load stream — the record of what
/// actually entered agent context.
fn recorded_loads(home: &Path) -> Vec<serde_json::Value> {
    let path = home.join(".agentstack/audit/loads.jsonl");
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// A package whose skill member carries a real frontmatter description, so the
/// boundary has something to expose that is not the body.
const BOUNDARY_PACK: &str = r#"
name = "rust-backend"
description = "Rust backend house rules"

[[skill]]
name = "sql-review"
path = "skills/sql-review"

[[instruction]]
name = "house-rules"
path = "instructions/house.md"
"#;

const SQL_REVIEW_BODY: &str = "---\nname: sql-review\ndescription: Reviews SQL migrations for locks.\n---\n\nZZBODYMARKER — the full instructions.\n";

/// *The boundary is exposed without loading bodies.* Activating a toolset that
/// selects a package makes each member skill discoverable by name and
/// description — and nothing about any member's BODY reaches the agent until
/// that one member is loaded on purpose.
///
/// The negative half is the one that matters: eagerly injecting twenty skill
/// bodies because a package was selected would recreate, under a new name, the
/// context bloat this whole lane exists to remove
/// (`automatic-delivery.md` §"Boundary, not bodies").
#[test]
fn activating_a_package_exposes_the_boundary_without_loading_any_body() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let (proj, lib_home) = machine(&tmp);
    let home = tmp.path().join("home");
    install_package(
        &lib_home,
        "rust-backend",
        "1.4.0",
        BOUNDARY_PACK,
        &[
            ("skills/sql-review/SKILL.md", SQL_REVIEW_BODY),
            ("instructions/house.md", "Prefer boring Rust.\n"),
        ],
    );
    write_manifest(
        &proj,
        "version = 1\n[profiles.backend]\npackages = [\"rust-backend\"]\n",
    );
    lock_cmd::run(&LockArgs::default(), Some(&proj)).unwrap();

    // Activate the toolset, then list. Two calls, no load.
    let responses = mcp_exchange(
        &proj,
        &home,
        &[
            serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }),
            serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "agentstack_lease_open", "arguments": { "profile": "backend" } } }),
            serde_json::json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "agentstack_list_loadable", "arguments": {} } }),
        ],
    );
    let catalog = tool_json(&responses[2]);
    let entries = catalog["loadable"].as_array().unwrap();
    let member = entries
        .iter()
        .find(|e| e["name"] == "sql-review")
        .expect("the package member is discoverable");

    // The boundary: name, one-line description, and where it came from.
    assert_eq!(member["description"], "Reviews SQL migrations for locks.");
    assert_eq!(member["origin"], "package");
    assert_eq!(member["package"], "rust-backend");
    assert_eq!(
        member["provenance"],
        "package:rust-backend@1.4.0#skills/sql-review"
    );
    assert_eq!(member["loaded"], false);

    // An INSTRUCTION member is not agent-loadable context — it is the rendered
    // lane, and the loadable index is skill-only.
    assert!(
        !entries.iter().any(|e| e["name"] == "house-rules"),
        "an instruction member never appears as loadable: {entries:?}"
    );

    // Not one byte of any member's body is in the listing…
    let serialized = serde_json::to_string(&catalog).unwrap();
    assert!(
        !serialized.contains("ZZBODYMARKER"),
        "no member body is in the boundary listing: {serialized}"
    );
    // …and nothing was loaded. The load stream is the record of what entered
    // agent context, and after activating a package and listing it, it is empty.
    assert!(
        recorded_loads(&home).is_empty(),
        "activation + listing loads no body"
    );

    // The body arrives only on an explicit, one-at-a-time load — served from
    // the PINNED bytes, with the package's provenance on it.
    let responses = mcp_exchange(
        &proj,
        &home,
        &[
            serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }),
            serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "agentstack_lease_open", "arguments": { "profile": "backend" } } }),
            serde_json::json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "agentstack_load", "arguments": { "name": "sql-review", "reason": "reviewing a migration" } } }),
        ],
    );
    let loaded = tool_json(&responses[2]);
    assert_eq!(loaded["origin"], "package");
    assert_eq!(loaded["package"], "rust-backend");
    assert!(loaded["instructions"]
        .as_str()
        .unwrap()
        .contains("ZZBODYMARKER"));
    let stream = recorded_loads(&home);
    assert_eq!(stream.len(), 1, "exactly one body, on purpose: {stream:?}");
    assert_eq!(stream[0]["name"], "sql-review");
}

/// The honesty fix from `pinned-serving-and-library-drift.md` §"Debt": the
/// listed one-line description comes from the bytes this project PINNED, not
/// from whatever the central library now holds.
///
/// Before this, the catalog resolved every skill `PathOnly` and read the live
/// `SKILL.md`, so after a `lib sync` an agent could read the library's newer
/// description for a skill whose body would load at the pinned version — one
/// skill, two stories, and the last place a library that moved ahead was
/// visible to an agent with no re-gate behind it.
#[test]
fn a_listed_description_comes_from_the_pinned_bytes_not_the_live_library() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let (proj, lib_home) = machine(&tmp);
    let home = tmp.path().join("home");

    // A central-library skill, pinned by this project.
    fs::create_dir_all(lib_home.join("skills/quokka-lint")).unwrap();
    fs::write(
        lib_home.join("skills/quokka-lint/SKILL.md"),
        "---\nname: quokka-lint\ndescription: PINNEDDESC at the reviewed version.\n---\nbody\n",
    )
    .unwrap();
    let mut lib = Library::load(&lib_home).unwrap();
    lib.upsert(LibrarySkill {
        name: "quokka-lint".into(),
        source: "path".into(),
        path: Some("quokka-lint".into()),
        git: None,
        rev: None,
        subpath: None,
        checksum: None,
        version: None,
        provenance: Some("manual".into()),
    });
    lib.save(&lib_home).unwrap();

    write_manifest(
        &proj,
        "version = 1\n[profiles.p]\nskills = [\"quokka-lint\"]\n",
    );
    lock_cmd::run(&LockArgs::default(), Some(&proj)).unwrap();

    // The library moves ahead — exactly what `lib sync` does. Per the update
    // model this must change nothing in any project.
    fs::write(
        lib_home.join("skills/quokka-lint/SKILL.md"),
        "---\nname: quokka-lint\ndescription: LIVEDESC from the library's newer bytes.\n---\nnew body\n",
    )
    .unwrap();

    let responses = mcp_exchange(
        &proj,
        &home,
        &[
            serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }),
            serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "agentstack_list_loadable", "arguments": {} } }),
        ],
    );
    let catalog = tool_json(&responses[1]);
    let entry = catalog["loadable"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "quokka-lint")
        .expect("the pinned skill is listed");

    assert_eq!(
        entry["description"], "PINNEDDESC at the reviewed version.",
        "the description comes from the pin, not the live library"
    );
    assert_eq!(entry["pinned"], true);
    assert_eq!(
        entry["origin"], "library",
        "origin still says where the reference was satisfied"
    );
}

/// The rendered lane for a package instruction member, under W3's conservative
/// scoping: the managed region is refreshed where one already exists, and
/// locking a package is NEVER the reason an instruction file (or a managed
/// region inside one) first appears in a repo.
#[test]
fn a_package_instruction_member_renders_into_an_existing_managed_region_only() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let (proj, lib_home) = machine(&tmp);
    install_package(
        &lib_home,
        "rust-backend",
        "1.4.0",
        BOUNDARY_PACK,
        &[
            ("skills/sql-review/SKILL.md", SQL_REVIEW_BODY),
            ("instructions/house.md", "Prefer boring Rust.\n"),
        ],
    );
    write_manifest(
        &proj,
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [profiles.backend]\npackages = [\"rust-backend\"]\n",
    );

    // No instruction file at all: locking pins the member and writes nothing.
    lock_cmd::run(&LockArgs::default(), Some(&proj)).unwrap();
    assert!(
        !proj.join("CLAUDE.md").exists(),
        "locking a package never creates an instruction file"
    );

    // A CLAUDE.md that exists but carries no managed region: still untouched.
    let unmanaged = "# House\n\nOur own prose, written by a human.\n";
    fs::write(proj.join("CLAUDE.md"), unmanaged).unwrap();
    lock_cmd::run(&LockArgs::default(), Some(&proj)).unwrap();
    assert_eq!(
        fs::read_to_string(proj.join("CLAUDE.md")).unwrap(),
        unmanaged,
        "locking never adds a managed region to a file that had none"
    );

    // And the report says so, naming the command that WOULD render it.
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(["lock", "--manifest-dir"])
        .arg(&proj)
        .env("HOME", tmp.path().join("home"))
        .env("AGENTSTACK_HOME", tmp.path().join("home/.agentstack"))
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        text.contains("rendered lane:"),
        "the two lanes are reported on separate lines: {text}"
    );
    assert!(
        text.contains("no file was written"),
        "the honest negative is stated: {text}"
    );
    assert!(
        text.contains("agentstack instructions --write"),
        "and the command that would render it is named: {text}"
    );
    assert!(
        !text.contains("via gateway"),
        "an instruction is never described as going live via gateway: {text}"
    );

    // Now the file carries agentstack's managed region — the human accepted one.
    fs::write(
        proj.join("CLAUDE.md"),
        "# House\n\nOur own prose.\n\n<!-- agentstack:start -->\nstale\n<!-- agentstack:end -->\n\nTrailing prose.\n",
    )
    .unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(["lock", "--manifest-dir"])
        .arg(&proj)
        .env("HOME", tmp.path().join("home"))
        .env("AGENTSTACK_HOME", tmp.path().join("home/.agentstack"))
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let rendered = fs::read_to_string(proj.join("CLAUDE.md")).unwrap();
    assert!(
        rendered.contains("Prefer boring Rust."),
        "the member's PINNED prose is in the region: {rendered}"
    );
    assert!(
        rendered.contains("Our own prose.") && rendered.contains("Trailing prose."),
        "prose outside the markers survives untouched: {rendered}"
    );
    assert!(
        text.contains("managed region updated in CLAUDE.md"),
        "the rendered lane names what was written and where: {text}"
    );

    // The bytes rendered are the PINNED ones: the library moving ahead changes
    // no rendered file until an explicit re-lock takes the new version.
    fs::write(
        lib_home.join("packages/rust-backend/instructions/house.md"),
        "Prefer exciting Rust.\n",
    )
    .unwrap();
    let plan_only = fs::read_to_string(proj.join("CLAUDE.md")).unwrap();
    assert!(!plan_only.contains("Prefer exciting Rust."));
}

/// A minimal MCP stdio server in POSIX sh that records the fact that it ran:
/// it touches `$SENTINEL` the moment it starts, before reading a line. So the
/// sentinel's existence is exactly "this server was started", independent of
/// whether anything was ever asked of it.
#[cfg(unix)]
const SENTINEL_FIXTURE: &str = r#"#!/bin/sh
[ -n "$SENTINEL" ] && : > "$SENTINEL"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-06-18","capabilities":{},"serverInfo":{"name":"fix","version":"0"}}}\n' "$id"
      ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"ping","description":"Ping.","inputSchema":{"type":"object"}}]}}\n' "$id"
      ;;
    *'"method":"tools/call"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"pong"}]}}\n' "$id"
      ;;
  esac
done
"#;

/// *A server starts on first tool use, not on activation.*
///
/// Three phases, and the middle one is the claim: selecting a toolset — the
/// activation a package reference rides on — must not start anything, and
/// reading the served boundary must not either. Only calling a tool starts the
/// server that owns it, and only that one.
///
/// Covered for both transports, because they fail differently: a stdio server
/// is a child process (the sentinel file), an HTTP server is a socket (the
/// accept count). Neither is contacted at construction.
#[cfg(unix)]
#[test]
fn a_server_is_not_started_until_one_of_its_tools_is_called() {
    use std::io::Read;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let (proj, _lib_home) = machine(&tmp);

    // An HTTP "upstream" that only counts connections. Nothing MCP-shaped is
    // needed: the question is whether the socket is ever dialled at all.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let dialled = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&dialled);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            seen.fetch_add(1, Ordering::SeqCst);
            // Read what arrived, then hang up: the gateway treats an empty
            // body as "no result", which is a call failure, not a dial failure
            // — and the dial is all this asserts on.
            let mut buf = [0u8; 512];
            let _ = stream.read(&mut buf);
        }
    });

    let script = proj.join("srv.sh");
    fs::write(&script, SENTINEL_FIXTURE).unwrap();
    let called = proj.join("called.started");
    let idle = proj.join("idle.started");
    write_manifest(
        &proj,
        &format!(
            "version = 1\n\
             [servers.called]\ntype = \"stdio\"\ncommand = \"/bin/sh\"\nargs = [\"{script}\"]\n\
             env = {{ SENTINEL = \"{called}\" }}\n\
             [servers.idle]\ntype = \"stdio\"\ncommand = \"/bin/sh\"\nargs = [\"{script}\"]\n\
             env = {{ SENTINEL = \"{idle}\" }}\n\
             [servers.web]\ntype = \"http\"\nurl = \"http://127.0.0.1:{port}/mcp\"\n\
             [profiles.backend]\nservers = [\"called\", \"idle\", \"web\"]\n",
            script = script.display(),
            called = called.display(),
            idle = idle.display(),
        ),
    );

    // 1 · ACTIVATION. Selecting the toolset builds the served surface and
    //     starts nothing: no child, no socket.
    let gw = agentstack::gateway::Gateway::from_manifest_lease(Some(&proj), "backend");
    assert!(!called.exists() && !idle.exists(), "no child at activation");
    assert_eq!(dialled.load(Ordering::SeqCst), 0, "no dial at activation");

    // 2 · READING THE BOUNDARY. The served server set — which surfaces name
    //     the toolset admits — is answered from the resolved definitions
    //     alone, so listing WHAT is proxied still starts nothing.
    let names: Vec<String> = gw.proxied_servers().into_iter().map(|(n, _)| n).collect();
    assert_eq!(names, ["called", "idle", "web"]);
    assert!(!called.exists() && !idle.exists(), "no child after listing");
    assert_eq!(dialled.load(Ordering::SeqCst), 0, "no dial after listing");

    // 3 · FIRST TOOL USE. Exactly the called server starts. Its neighbour in
    //     the same toolset never does, and the HTTP upstream is never dialled
    //     — laziness is per server, not per gateway.
    let res = gw.try_call("called__ping", &serde_json::json!({}));
    assert!(res.is_some(), "the call routed to the upstream");
    assert!(called.exists(), "the called server started");
    assert!(
        !idle.exists(),
        "a server nobody called is still not started"
    );
    assert_eq!(
        dialled.load(Ordering::SeqCst),
        0,
        "an HTTP upstream nobody called is never dialled"
    );

    // And the HTTP transport is dialled on ITS first call, not before.
    let _ = gw.try_call("web__anything", &serde_json::json!({}));
    assert!(
        dialled.load(Ordering::SeqCst) >= 1,
        "the HTTP upstream is dialled on its first call"
    );

    // The named counter-fact, deliberately witnessed rather than left to be
    // rediscovered: building the *aggregated tool list* — transparent mode's
    // `tools/list`, `tools_search`, code-mode bindings — asks every upstream
    // for its tools, which starts every one of them. That is inherent to
    // enumerating tools nobody has cached, not a lapse in the lazy path above;
    // the default compact surface never does it (see
    // `mcp_server::tests::transparent_tools_list_advertises_upstream_tools`).
    // This assertion is here so the day that changes, it changes on purpose.
    let _ = gw.namespaced_tools();
    assert!(
        idle.exists(),
        "aggregated tool discovery contacts every upstream — the one eager path"
    );
}

// ---------------------------------------------------------------------------
// W5 acceptance, the last clause: *the boundary is exposed … only the selected
// servers' tools are exposed* — for a package's server member too. A member of
// kind `server` becomes a gateway upstream on exactly the same terms as a
// manifest-declared one: fenced by the toolset that selected the package,
// resolved from the lock, lazy, and behind the same policy and trust gates.
// ---------------------------------------------------------------------------

/// Write a minimal stdio MCP server in POSIX sh whose sentinel path and tool
/// name are baked in.
///
/// Baked in rather than passed through `$SENTINEL` (as [`SENTINEL_FIXTURE`]
/// does) because a package's `[server]` table can only declare `secret_env`
/// names, which become unresolved `${REF}`s — the fixture has to be
/// self-contained to say anything about a package-carried server.
#[cfg(unix)]
fn write_pinned_server(script: &Path, sentinel: &Path, tool: &str) {
    fs::write(
        script,
        format!(
            r#"#!/bin/sh
: > '{sentinel}'
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"protocolVersion":"2025-06-18","capabilities":{{}},"serverInfo":{{"name":"pkg","version":"0"}}}}}}\n' "$id"
      ;;
    *'"method":"tools/list"'*)
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"tools":[{{"name":"{tool}","description":"Ping.","inputSchema":{{"type":"object"}}}}]}}}}\n' "$id"
      ;;
    *'"method":"tools/call"'*)
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"content":[{{"type":"text","text":"{tool}-pong"}}]}}}}\n' "$id"
      ;;
  esac
done
"#,
            sentinel = sentinel.display(),
            tool = tool,
        ),
    )
    .unwrap();
}

/// A `pack.toml` carrying one stdio server member (named after the package, as
/// the schema requires) that runs `script`.
#[cfg(unix)]
fn server_pack(script: &Path) -> String {
    format!(
        "name = \"rust-backend\"\n\
         description = \"Rust backend house rules\"\n\n\
         [server]\ntype = \"stdio\"\ncommand = \"/bin/sh\"\nargs = [\"{}\"]\n",
        script.display()
    )
}

/// The command line a gateway upstream would actually run, per served server.
#[cfg(unix)]
fn served_names(gw: &agentstack::gateway::Gateway) -> Vec<String> {
    gw.proxied_servers().into_iter().map(|(n, _)| n).collect()
}

/// *Only the selected servers' tools are exposed*, with a package's server
/// member inside "selected". The fence is the toolset, and it is the whole of
/// the admission decision: the package's server is reachable under the toolset
/// that selected the package, and under nothing else.
///
/// The two negatives are the claim. A different toolset must not reach it —
/// that is the ordinary fence. And an UNFENCED gateway must not reach it
/// either: unfenced already means every manifest-declared server, so unioning
/// every package's server on top would turn package membership into a way to
/// widen the surface rather than something a toolset selects.
#[cfg(unix)]
#[test]
fn a_package_carried_server_is_exposed_only_under_a_toolset_that_selects_it() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let (proj, lib_home) = machine(&tmp);

    let script = proj.join("pkg-srv.sh");
    let sentinel = proj.join("pkg.started");
    write_pinned_server(&script, &sentinel, "ping");
    install_package(
        &lib_home,
        "rust-backend",
        "1.4.0",
        &server_pack(&script),
        &[],
    );

    // Two toolsets: one selects the package, one does not.
    write_manifest(
        &proj,
        "version = 1\n\
         [profiles.backend]\npackages = [\"rust-backend\"]\n\
         [profiles.frontend]\n",
    );
    lock_cmd::run(&LockArgs::default(), Some(&proj)).unwrap();

    // Under the selecting toolset: served, and its tools actually dispatch.
    let gw = agentstack::gateway::Gateway::from_manifest_lease(Some(&proj), "backend");
    assert_eq!(served_names(&gw), ["rust-backend"]);
    assert!(
        gw.skipped_servers().is_empty(),
        "nothing was skipped: {:?}",
        gw.skipped_servers()
    );
    let answer = gw
        .try_call("rust-backend__ping", &serde_json::json!({}))
        .expect("the package server's tool routes to an upstream")
        .expect("and the upstream answers");
    assert!(
        serde_json::to_string(&answer)
            .unwrap()
            .contains("ping-pong"),
        "the member server answered: {answer:?}"
    );

    // Under a toolset that does not select the package: nothing.
    let other = agentstack::gateway::Gateway::from_manifest_lease(Some(&proj), "frontend");
    assert!(
        served_names(&other).is_empty(),
        "a toolset that selects no package serves none of its servers: {:?}",
        served_names(&other)
    );
    assert!(other
        .try_call("rust-backend__ping", &serde_json::json!({}))
        .is_none());

    // Unfenced: still nothing. Package membership is selected, never unioned.
    let unfenced = agentstack::gateway::Gateway::from_manifest(Some(&proj));
    assert!(
        served_names(&unfenced).is_empty(),
        "an unfenced gateway does not union in every package's server: {:?}",
        served_names(&unfenced)
    );
    assert!(unfenced
        .try_call("rust-backend__ping", &serde_json::json!({}))
        .is_none());
}

/// The reproducibility rule at the serving point: what is proxied is the
/// definition the LOCK pins, never whatever the package's `pack.toml` currently
/// says.
///
/// The library package is edited to point its server at a different program
/// after locking — the shape a `lib sync` takes. Nothing re-locks, so nothing
/// in the project may change: the pinned program runs and the library's newer
/// one is never executed. A serving path that re-read `pack.toml` would run an
/// arbitrary command nobody reviewed, which is the whole reason the pin exists.
#[cfg(unix)]
#[test]
fn a_package_carried_server_is_resolved_from_the_lock_not_the_current_package() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let (proj, lib_home) = machine(&tmp);

    let pinned = proj.join("pinned-srv.sh");
    let pinned_ran = proj.join("pinned.started");
    write_pinned_server(&pinned, &pinned_ran, "ping");

    let newer = proj.join("newer-srv.sh");
    let newer_ran = proj.join("newer.started");
    write_pinned_server(&newer, &newer_ran, "ping");

    install_package(
        &lib_home,
        "rust-backend",
        "1.4.0",
        &server_pack(&pinned),
        &[],
    );
    write_manifest(
        &proj,
        "version = 1\n[profiles.backend]\npackages = [\"rust-backend\"]\n",
    );
    lock_cmd::run(&LockArgs::default(), Some(&proj)).unwrap();

    // The library moves ahead: same package, same version, different server.
    fs::write(
        lib_home.join("packages/rust-backend/pack.toml"),
        server_pack(&newer),
    )
    .unwrap();

    let gw = agentstack::gateway::Gateway::from_manifest_lease(Some(&proj), "backend");
    assert_eq!(served_names(&gw), ["rust-backend"]);
    gw.try_call("rust-backend__ping", &serde_json::json!({}))
        .expect("routed")
        .expect("answered");

    assert!(
        pinned_ran.exists(),
        "the PINNED server definition is what was served"
    );
    assert!(
        !newer_ran.exists(),
        "the library's newer definition is never read at serving time"
    );
}

/// *A server starts on first tool use, not on activation* — for a package's
/// server member too. Same sentinel technique as the manifest-server witness
/// above, because the failure it guards against is the same one: an upstream
/// that is dialled or spawned because a toolset was selected turns activation
/// into execution.
#[cfg(unix)]
#[test]
fn a_package_carried_server_is_not_started_until_one_of_its_tools_is_called() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let (proj, lib_home) = machine(&tmp);

    let script = proj.join("pkg-srv.sh");
    let sentinel = proj.join("pkg.started");
    write_pinned_server(&script, &sentinel, "ping");
    install_package(
        &lib_home,
        "rust-backend",
        "1.4.0",
        &server_pack(&script),
        &[],
    );
    write_manifest(
        &proj,
        "version = 1\n[profiles.backend]\npackages = [\"rust-backend\"]\n",
    );
    lock_cmd::run(&LockArgs::default(), Some(&proj)).unwrap();
    assert!(
        !sentinel.exists(),
        "locking a package never starts its server"
    );

    // 1 · ACTIVATION builds the served surface and starts nothing.
    let gw = agentstack::gateway::Gateway::from_manifest_lease(Some(&proj), "backend");
    assert!(!sentinel.exists(), "no child at activation");

    // 2 · READING THE BOUNDARY names the server without contacting it.
    assert_eq!(served_names(&gw), ["rust-backend"]);
    assert!(!sentinel.exists(), "no child after listing what is proxied");

    // 3 · FIRST TOOL USE, and only then.
    gw.try_call("rust-backend__ping", &serde_json::json!({}))
        .expect("routed")
        .expect("answered");
    assert!(sentinel.exists(), "the server started on its first call");
}

/// Fail closed, out loud. A pinned member whose definition bytes cannot be
/// produced and verified is NOT served — and the user is told which capability
/// was refused and why, through the same seatbelt refusal a drifted library pin
/// takes. A member that silently vanished would leave a toolset quietly
/// narrower than the lock says it is.
#[cfg(unix)]
#[test]
fn a_package_carried_server_that_cannot_be_verified_fails_closed_with_a_reason() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let (proj, lib_home) = machine(&tmp);
    let home = tmp.path().join("home");

    let script = proj.join("pkg-srv.sh");
    let sentinel = proj.join("pkg.started");
    write_pinned_server(&script, &sentinel, "ping");
    install_package(
        &lib_home,
        "rust-backend",
        "1.4.0",
        &server_pack(&script),
        &[],
    );
    write_manifest(
        &proj,
        "version = 1\n[profiles.backend]\npackages = [\"rust-backend\"]\n",
    );
    lock_cmd::run(&LockArgs::default(), Some(&proj)).unwrap();

    // Tamper with the store: the deposit under the member's pinned digest no
    // longer holds the bytes that digest names. (Deleting it exercises the same
    // branch — a store that was pruned, or that never received the best-effort
    // deposit.)
    let member_digest = Lock::load(&proj)
        .unwrap()
        .get_package("rust-backend")
        .unwrap()
        .members
        .iter()
        .find(|m| m.kind == PackageMemberKind::Server)
        .unwrap()
        .checksum
        .hex()
        .to_string();
    let deposit = home.join(".agentstack/store/content").join(&member_digest);
    assert!(deposit.is_dir(), "locking deposited the pinned definition");
    fs::write(
        deposit.join("rust-backend.toml"),
        "type = \"stdio\"\ncommand = \"/bin/echo\"\n",
    )
    .unwrap();

    let gw = agentstack::gateway::Gateway::from_manifest_lease(Some(&proj), "backend");
    assert!(
        served_names(&gw).is_empty(),
        "tampered pinned bytes are never served: {:?}",
        served_names(&gw)
    );
    assert!(
        !sentinel.exists(),
        "and nothing was started from them either"
    );
    // Named, not silently dropped: the refusal is on the same skipped list the
    // executor's fail-closed check reads.
    assert_eq!(gw.skipped_servers(), ["rust-backend"]);
    assert!(gw
        .try_call("rust-backend__ping", &serde_json::json!({}))
        .is_none());

    // And it left evidence a user can look up afterwards, with the reason and
    // the next step in it — the whole point of routing this through `seatbelt`
    // rather than a bare stderr line.
    let audit = fs::read_to_string(home.join(".agentstack/audit/calls.jsonl")).unwrap_or_default();
    let denial = audit
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .find(|r| r["server"] == "rust-backend" && r["outcome"] == "denied")
        .expect("the refusal was recorded");
    let detail = denial["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains("agentstack lock"),
        "the reason names the command that fixes it: {detail}"
    );
    assert!(
        detail.contains("rust-backend"),
        "and names the package: {detail}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Review finding 1 + 6: a package's server reaches EVERY run, and it never
// reaches one wearing this project's name.
//
// The defect these witness: `package_runtime_servers` had exactly two callers —
// the live host gateway and the image builder. `locked.rs` never saw packages,
// so a Protected run (the DEFAULT for a plain `agentstack run <cli>`) silently
// omitted every server a toolset's package carried, while `status --json`
// listed it as an effective member. The fix adds them to
// `frozen_runtime_servers`, which is the ONE set a Protected run freezes into
// its authority grant, the sandbox classifies from, and the image is built
// from.
// ─────────────────────────────────────────────────────────────────────────────

/// **The load-bearing witness for finding 1.** `frozen_runtime_servers` is the
/// set `locked.rs::resolve_inputs` builds `LockedInputs::frozen` from, and
/// `freeze_grant` binds one `GrantedServer` per entry. So a package server
/// present here — with `ServerOrigin::Package` and its provenance intact — is a
/// package server in the Protected run's frozen grant.
///
/// Reverting the fix (dropping the `package_runtime_servers` extension from
/// `frozen_runtime_servers`) fails this on the first assertion: the toolset's
/// server set is empty.
#[test]
fn a_toolset_package_server_reaches_the_frozen_set_a_protected_run_freezes() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let (proj, lib_home) = machine(&tmp);
    install_package(
        &lib_home,
        "rust-backend",
        "1.4.0",
        FULL_PACK,
        &full_pack_members(),
    );
    write_manifest(
        &proj,
        "version = 1\n[profiles.backend]\npackages = [\"rust-backend\"]\n",
    );
    lock_cmd::run(&LockArgs::default(), Some(&proj)).unwrap();

    let loaded = agentstack::manifest::load_from_dir(&proj).unwrap();
    let library = Library::load_default_or_warn();
    let frozen = agentstack::resolve::frozen_runtime_servers(
        &loaded.manifest,
        &library,
        &lib_home,
        &proj,
        Some("backend"),
    )
    .unwrap();

    let (name, resolved) = frozen
        .iter()
        .find(|(n, _)| n == "rust-backend")
        .expect("the toolset's package server is in the frozen set a Protected run freezes");
    let r = resolved.as_ref().expect("and it resolved");
    assert_eq!(name, "rust-backend");

    // Finding 6: NOT `Inline`. `GrantedServer::from_resolved` maps this origin
    // to `GrantedServerBinding::Package`, which is the only server binding that
    // carries provenance out of the grant and into the handoff artifact.
    assert_eq!(
        r.origin,
        agentstack::resolve::ServerOrigin::Package,
        "a package member must never bind as this project's own inline server"
    );
    assert_eq!(
        r.provenance.as_deref(),
        Some("package:rust-backend@1.4.0#[server]"),
        "provenance names the package, its version and the member — this is what \
         the `Inline` binding had nowhere to put"
    );

    // The definition is the pinned one, and it still carries `${REF}` rather
    // than a value (invariant 5).
    assert_eq!(r.server.server_type, agentstack::manifest::ServerType::Http);
    assert_eq!(r.server.url.as_deref(), Some("https://backend.example/mcp"));
    let text = toml::to_string(&r.server).unwrap();
    assert!(
        text.contains("${"),
        "the secret header stays a reference in the frozen definition: {text}"
    );

    // The digest the grant binds is the one the lock pins for that member —
    // not a re-derivation.
    let lock = Lock::load(&proj).unwrap();
    let member = lock
        .get_package("rust-backend")
        .unwrap()
        .members
        .iter()
        .find(|m| m.name == "rust-backend" && m.kind == PackageMemberKind::Server)
        .unwrap();
    assert_eq!(r.checksum, member.checksum.hex());
}

/// The fence is what keeps the fix from being a widening: a package's server is
/// in the frozen set of the toolset that selected it, and of no other. An
/// unfenced set contributes none of them — it already carries every manifest
/// server, so unioning packages in would make membership a way to widen rather
/// than something a toolset selects.
#[test]
fn a_package_server_is_fenced_to_the_toolset_that_selected_it() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let (proj, lib_home) = machine(&tmp);
    install_package(
        &lib_home,
        "rust-backend",
        "1.4.0",
        FULL_PACK,
        &full_pack_members(),
    );
    write_manifest(
        &proj,
        "version = 1\n\
         [profiles.backend]\npackages = [\"rust-backend\"]\n\
         [profiles.frontend]\n",
    );
    lock_cmd::run(&LockArgs::default(), Some(&proj)).unwrap();

    let loaded = agentstack::manifest::load_from_dir(&proj).unwrap();
    let library = Library::load_default_or_warn();
    let names = |profile: Option<&str>| -> Vec<String> {
        agentstack::resolve::frozen_runtime_servers(
            &loaded.manifest,
            &library,
            &lib_home,
            &proj,
            profile,
        )
        .unwrap()
        .into_iter()
        .map(|(n, _)| n)
        .collect()
    };

    assert_eq!(names(Some("backend")), vec!["rust-backend".to_string()]);
    assert!(
        names(Some("frontend")).is_empty(),
        "a toolset that selected no package gets none of its servers"
    );
    assert!(
        names(None).is_empty(),
        "and the unfenced set contributes no package server at all"
    );
}

/// Finding 1's third acceptance clause: the two rails agree on what reaches a
/// run. The git-pack rail vendors a `pack.toml`'s `[server]` into
/// `[servers.<pack>]`; the library-package rail references it. Both must put
/// the SAME definition in the frozen set — otherwise "reference is as safe as
/// vendoring" (`automatic-delivery.md`) is false at the only place it matters.
#[test]
fn the_vendored_and_referenced_rails_freeze_the_same_server_definition() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let (proj, lib_home) = machine(&tmp);
    install_package(
        &lib_home,
        "rust-backend",
        "1.4.0",
        FULL_PACK,
        &full_pack_members(),
    );

    // The referenced rail.
    write_manifest(
        &proj,
        "version = 1\n[profiles.backend]\npackages = [\"rust-backend\"]\n",
    );
    lock_cmd::run(&LockArgs::default(), Some(&proj)).unwrap();
    let loaded = agentstack::manifest::load_from_dir(&proj).unwrap();
    let library = Library::load_default_or_warn();
    let referenced = agentstack::resolve::frozen_runtime_servers(
        &loaded.manifest,
        &library,
        &lib_home,
        &proj,
        Some("backend"),
    )
    .unwrap();
    let referenced = referenced
        .into_iter()
        .find(|(n, _)| n == "rust-backend")
        .and_then(|(_, r)| r.ok())
        .expect("referenced rail freezes the package's server");

    // The vendored rail: the same `[server]` table, written inline as the
    // install rail writes it.
    let vendored_dir = tmp.path().join("vendored");
    fs::create_dir_all(&vendored_dir).unwrap();
    // Built through the vendored rail's own `Candidate::to_server`, not
    // hand-copied — a hand-written table would prove only that I can type the
    // same TOML twice.
    let parsed: agentstack::provider::gitpack::PackToml = toml::from_str(FULL_PACK).unwrap();
    let candidate = agentstack::provider::Candidate {
        id: "rust-backend".into(),
        name: "rust-backend".into(),
        description: String::new(),
        source: "catalog",
        kind: agentstack::provider::CandidateKind::Pack(agentstack::provider::PackSpec {
            server: Some(parsed.server.as_ref().unwrap().to_install().unwrap()),
            skills: Vec::new(),
            instructions: Vec::new(),
            targets: Vec::new(),
        }),
    };
    let server = candidate.to_server();
    // Nested through `toml::Value` rather than string concatenation: a server
    // with a `[headers]` sub-table pasted under `[servers.<name>]` by hand
    // re-parses as a TOP-LEVEL `[headers]`, which would silently drop the
    // secret header and make this comparison pass for the wrong reason.
    let mut servers = toml::value::Table::new();
    servers.insert(
        "rust-backend".to_string(),
        toml::Value::try_from(&server).unwrap(),
    );
    let mut root = toml::value::Table::new();
    root.insert("version".to_string(), toml::Value::Integer(1));
    root.insert("servers".to_string(), toml::Value::Table(servers));
    write_manifest(
        &vendored_dir,
        &toml::to_string(&toml::Value::Table(root)).unwrap(),
    );
    let vendored_loaded = agentstack::manifest::load_from_dir(&vendored_dir).unwrap();
    let vendored = agentstack::resolve::frozen_runtime_servers(
        &vendored_loaded.manifest,
        &library,
        &lib_home,
        &vendored_dir,
        None,
    )
    .unwrap();
    let vendored = vendored
        .into_iter()
        .find(|(n, _)| n == "rust-backend")
        .and_then(|(_, r)| r.ok())
        .expect("vendored rail freezes the same server");

    // Same definition, and the same content digest over it — one `pack.toml`
    // grammar, one server definition, whichever rail installed it.
    assert_eq!(
        toml::to_string(&referenced.server).unwrap(),
        toml::to_string(&vendored.server).unwrap()
    );
    assert_eq!(referenced.checksum, vendored.checksum);

    // What legitimately differs is the ORIGIN, and that difference is the
    // point of finding 6: the referenced rail says "a package", the vendored
    // rail says "this project", and neither is allowed to say the other.
    assert_eq!(
        referenced.origin,
        agentstack::resolve::ServerOrigin::Package
    );
    assert_eq!(vendored.origin, agentstack::resolve::ServerOrigin::Inline);
}
