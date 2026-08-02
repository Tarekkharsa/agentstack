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
