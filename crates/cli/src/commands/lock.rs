//! `agentstack lock` — resolve each profile's skill + server refs
//! (library-aware) and pin them in `agentstack.lock`, WITHOUT rendering
//! configs or materializing skills.
//!
//! The lock-only counterpart of `use <profile> --write`: clean-at-rest repos
//! reference library capabilities by name and keep no generated files, so
//! pinning must not require an activate-then-deactivate dance. Resolution
//! fetches git-backed sources as needed (like `use --write`), and lock entries
//! for names outside the selected profiles are preserved.

use agentstack_core::digest::Sha256Hex;
use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Context, Result};
use owo_colors::OwoColorize;

use crate::cli::LockArgs;
use crate::library::Library;
use crate::lock::Lock;
use crate::manifest::Manifest;
use crate::render::{resolve_active_servers, Selection};
use crate::resolve::{ResolveMode, ResolvedServer, ResolvedSkill};

use super::use_profile::{record_lock, resolve_active_skills};

/// The one lockfile verb: plain `lock` pins, `--update` re-resolves git skills
/// first (the old `update` command), `--upgrade` re-resolves an installed
/// vendor pack (the old `upgrade` command). The absorbed implementations are
/// unchanged — this only routes.
pub fn dispatch(args: &LockArgs, manifest_dir: Option<&Path>) -> Result<()> {
    if let Some(name) = &args.update {
        return super::install::run_update(
            &crate::cli::UpdateArgs { name: name.clone() },
            manifest_dir,
        );
    }
    if args.upgrade.is_some() {
        let name = args.upgrade.clone().flatten();
        return super::upgrade::run(
            &crate::cli::UpgradeArgs {
                name,
                all: args.all,
                with_instructions: args.with_instructions,
                yes: args.yes,
                write: args.write,
            },
            manifest_dir,
        );
    }
    run(args, manifest_dir)
}

pub fn run(args: &LockArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let ctx = super::load(manifest_dir)?;
    let manifest = &ctx.loaded.manifest;

    // Instructions are manifest-global, not profile-scoped: pin them
    // regardless of the profile selection (and even with zero profiles). The
    // lock command is strict — an unreadable fragment errors, stale pins for
    // undeclared names are pruned.
    let instructions = record_instruction_pins(&ctx.dir, manifest, true)?;

    let library = Library::load_default()?;
    let lib_home = crate::util::paths::lib_home();
    let store = crate::store::Store::default_store();

    // The D3 executable surface is manifest-global too: it derives from the
    // EFFECTIVE runtime server set (inline `[servers.*]` fan-out included),
    // not from profiles — a profile-less manifest still declares runnable
    // local code, and the trust gate blocks unpinned executables, so `lock`
    // must be able to pin them or a profile-less project could never be
    // trusted at all.
    let executables = record_executable_pins(&ctx.dir, manifest, &library, &lib_home)?;

    // Native extensions (D6) are manifest-global like instructions: pin them
    // regardless of the profile selection. Strict — an undigestable or
    // unresolvable source is an error, stale pins for undeclared names are
    // pruned. Resolution is library-aware and fetches git sources.
    let extensions = record_extension_pins(&ctx.dir, manifest, &library, &lib_home, &store)?;

    // Profile selection mirrors activation: named → that one; default → every
    // declared profile; none declared → the implicit default (the full inline
    // set), so a profile-less manifest is fully pinnable.
    let profiles: Option<Vec<String>> = match &args.profile {
        Some(p) => {
            manifest
                .profiles
                .get(p)
                .with_context(|| format!("no profile '{p}' in manifest"))?;
            Some(vec![p.clone()])
        }
        None if manifest.profiles.is_empty() => None,
        None => Some(manifest.profiles.keys().cloned().collect()),
    };

    let (skills, servers) = match &profiles {
        Some(profiles) => {
            resolve_profiles(manifest, &ctx.dir, &library, &lib_home, &store, profiles)?
        }
        None => resolve_implicit_default(manifest, &ctx.dir, &library, &lib_home, &store)?,
    };
    record_lock(&ctx.dir, &skills, &servers, manifest, &library)?;

    let from = match &profiles {
        Some(p) => format!("{} profile(s)", p.len()),
        None => "the implicit default (no profiles declared)".to_string(),
    };
    println!(
        "{} pinned {} skill(s) + {} server(s) from {from} + {} instruction(s) + {} executable pin(s) + {} extension(s) in {}",
        "✓".green(),
        skills.len(),
        servers.len(),
        instructions,
        executables,
        extensions,
        Lock::path(&ctx.dir).display()
    );
    println!(
        "  no configs rendered, no skills materialized — that stays `agentstack use --write`."
    );
    Ok(())
}

/// Digest every project-declared instruction fragment and pin it in the lock.
/// Machine-layer (`from_user_layer`) fragments never pin — they are the user's
/// own machine content, not repo bytes under review. Returns how many pinned.
///
/// `strict` (the `agentstack lock` command): an unreadable fragment is an
/// error (can't pin what can't be read), and pins for names no longer declared
/// are pruned. Non-strict (`apply --write` / `instructions --write` first-pin
/// recording): unreadable fragments are skipped — the compile machinery
/// already reports and blocks them per target — and nothing is pruned.
pub(crate) fn record_instruction_pins(
    dir: &Path,
    manifest: &Manifest,
    strict: bool,
) -> Result<usize> {
    let mut lock = Lock::load(dir)?;
    let before = lock.clone();
    let mut declared: Vec<String> = Vec::new();
    let mut pinned = 0usize;
    for (name, instr) in &manifest.instructions {
        if instr.from_user_layer {
            continue;
        }
        declared.push(name.clone());
        let src = crate::render::instructions::fragment_source(dir, &instr.path);
        match std::fs::read(&src) {
            Ok(bytes) => {
                lock.upsert_instruction(agentstack_core::lock::LockedInstruction {
                    name: name.clone(),
                    path: instr.path.clone(),
                    checksum: Sha256Hex::of(&bytes),
                });
                pinned += 1;
            }
            Err(e) if strict => {
                return Err(e).with_context(|| {
                    format!("pinning instruction '{name}': reading {}", src.display())
                });
            }
            Err(_) => {}
        }
    }
    if strict {
        lock.retain_instruction_names(&declared);
    }
    // Don't churn the lockfile (or the trust digest) for a byte-identical pin.
    if lock != before {
        lock.save(dir)?;
    }
    Ok(pinned)
}

/// Pin the D3 executable surface of the EFFECTIVE runtime server set (inline
/// fan-out + every profile-referenced name; the same set the trust preview,
/// doctor, and a locked run verify). Strict like the instruction pins: an
/// unverifiable local candidate (symlink, traversal, broken declared root) is
/// an error, and stale pins are PRUNED — a removed server or un-declared
/// integrity root must not leave a dead pin masking the surface (mirror of
/// `retain_instruction_names`; the profile-scoped `record_lock` first-pin path
/// never prunes, since it only sees a subset of servers). Unresolvable server
/// refs are skipped here — the profile resolution below (or the use path)
/// reports those; their existing pins are retained, never pruned on a broken
/// resolution. Returns how many pinned.
pub(crate) fn record_executable_pins(
    dir: &Path,
    manifest: &Manifest,
    library: &Library,
    lib_home: &Path,
) -> Result<usize> {
    let mut lock = Lock::load(dir)?;
    let before = lock.clone();
    let mut pinned = 0usize;
    let mut keep: Vec<(String, agentstack_core::lock::ExecutableKind)> = Vec::new();
    let mut all_resolved = true;
    for (name, resolved) in
        crate::resolve::effective_runtime_servers(manifest, library, lib_home, None)
    {
        let Ok(r) = resolved else {
            all_resolved = false;
            continue;
        };
        for pin in crate::executable::derive_executable_pins(dir, &name, &r.server)? {
            keep.push((pin.path.clone(), pin.kind));
            lock.upsert_executable(pin);
            pinned += 1;
        }
    }
    // Prune only from a complete picture: if any server failed to resolve, its
    // executable surface is unknown, and pruning would drop live pins.
    if all_resolved {
        lock.retain_executables(&keep);
    }
    // Don't churn the lockfile (or the trust digest) for byte-identical pins.
    if lock != before {
        lock.save(dir)?;
    }
    Ok(pinned)
}

/// Pin every declared native extension (D6) by the STRICT integrity-root
/// digest — the executable-content family (symlink anywhere = hard error,
/// `.git` included), never the lenient skill digest. Resolution is inline-first
/// then central library, and git sources are fetched through the shared store
/// (`ResolveMode::Fetch`), exactly like `agentstack lock` resolves skills.
///
/// Always strict, like the lock command's other manifest-global pins: an
/// undigestable or unresolvable source errors, and pins for undeclared names
/// are pruned. Records the full source provenance (`source`/`path`/`git`/`rev`)
/// so the pin is self-describing and a git rev-drift is detectable. Returns how
/// many pinned.
pub(crate) fn record_extension_pins(
    dir: &Path,
    manifest: &Manifest,
    library: &Library,
    lib_home: &Path,
    store: &crate::store::Store,
) -> Result<usize> {
    let mut lock = Lock::load(dir)?;
    let before = lock.clone();
    let mut declared: Vec<String> = Vec::new();
    let mut pinned = 0usize;
    for (name, ext) in &manifest.extensions {
        declared.push(name.clone());
        let resolved = crate::resolve::resolve_extension_entry(
            name,
            ext,
            dir,
            library,
            lib_home,
            store,
            ResolveMode::Fetch,
        )
        .with_context(|| format!("pinning extension '{name}'"))?;
        lock.upsert_extension(agentstack_core::lock::LockedExtension {
            name: name.clone(),
            target: resolved.target.clone(),
            source: resolved.source_kind.to_string(),
            path: resolved.path.clone(),
            git: resolved.git.clone(),
            rev: resolved.rev.clone(),
            checksum: resolved.checksum,
        });
        pinned += 1;
    }
    lock.retain_extension_names(&declared);
    // Don't churn the lockfile (or the trust digest) for byte-identical pins.
    if lock != before {
        lock.save(dir)?;
    }
    Ok(pinned)
}

/// Resolve the named profiles' skill + server refs through the library-aware
/// resolvers (inline-first, then central library), deduplicated by name across
/// profiles. Fails before any lock write if a ref resolves nowhere.
fn resolve_profiles(
    manifest: &Manifest,
    dir: &Path,
    library: &Library,
    lib_home: &Path,
    store: &crate::store::Store,
    profiles: &[String],
) -> Result<(Vec<ResolvedSkill>, Vec<ResolvedServer>)> {
    let mut seen_skills = BTreeSet::new();
    let mut seen_servers = BTreeSet::new();
    let mut skills = Vec::new();
    let mut servers = Vec::new();
    for pname in profiles {
        for r in resolve_active_skills(
            manifest,
            Some(pname),
            dir,
            library,
            lib_home,
            store,
            ResolveMode::Fetch,
        )? {
            if seen_skills.insert(r.name.clone()) {
                skills.push(r);
            }
        }
        let selection = Selection::Profile(pname.clone());
        for r in resolve_active_servers(manifest, library, lib_home, &selection)? {
            if seen_servers.insert(r.name.clone()) {
                servers.push(r);
            }
        }
    }
    Ok((skills, servers))
}

/// The pin set for a profile-less manifest: every inline skill and server —
/// exactly what `use --write` would activate as the implicit default.
fn resolve_implicit_default(
    manifest: &Manifest,
    dir: &Path,
    library: &Library,
    lib_home: &Path,
    store: &crate::store::Store,
) -> Result<(Vec<ResolvedSkill>, Vec<ResolvedServer>)> {
    let skills = resolve_active_skills(
        manifest,
        None,
        dir,
        library,
        lib_home,
        store,
        ResolveMode::Fetch,
    )?;
    let servers = resolve_active_servers(manifest, library, lib_home, &Selection::All)?;
    Ok((skills, servers))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{LibraryServer, LibrarySkill};
    use crate::store::Store;
    use assert_fs::prelude::*;

    /// Write a path-source skill body + a server definition under `lib_home`
    /// and index both in the returned library.
    fn library_with(lib_home: &assert_fs::TempDir) -> Library {
        lib_home
            .child("skills/sql-review/SKILL.md")
            .write_str("# lib\n")
            .unwrap();
        lib_home
            .child("servers/kibana.toml")
            .write_str(
                "type = \"http\"\nurl = \"https://x/mcp\"\n\n[headers]\nAuthorization = \"Bearer ${TOKEN}\"\n",
            )
            .unwrap();
        let mut lib = Library::default();
        lib.upsert(LibrarySkill {
            name: "sql-review".into(),
            source: "path".into(),
            path: Some("sql-review".into()),
            git: None,
            rev: None,
            subpath: None,
            checksum: None,
            version: None,
            provenance: Some("consolidated".into()),
        });
        lib.upsert_server(LibraryServer {
            name: "kibana".into(),
            checksum: None,
            version: None,
            provenance: Some("consolidated:codex".into()),
        });
        lib
    }

    #[test]
    fn resolves_and_pins_all_profiles_without_materializing() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());
        let library = library_with(&lib_home);

        // Two profiles sharing a skill: dedup keeps one entry; servers come
        // from the second profile only.
        let manifest: Manifest = toml::from_str(
            r#"
            version = 1
            [profiles.review]
            skills = ["sql-review"]
            [profiles.ops]
            skills = ["sql-review"]
            servers = ["kibana"]
            "#,
        )
        .unwrap();
        let profiles: Vec<String> = manifest.profiles.keys().cloned().collect();

        let (skills, servers) = resolve_profiles(
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            &profiles,
        )
        .unwrap();
        assert_eq!(skills.len(), 1, "shared skill deduplicated across profiles");
        assert_eq!(servers.len(), 1);

        record_lock(proj.path(), &skills, &servers, &manifest, &library).unwrap();

        let lock = Lock::load(proj.path()).unwrap();
        let skill = lock.get("sql-review").expect("skill pinned");
        assert_eq!(skill.checksum.hex().len(), 64);
        let server = lock.get_server("kibana").expect("server pinned");
        assert_eq!(server.source, agentstack_core::lock::ServerSource::Library);
        // Lock-only: nothing was rendered or materialized in the project.
        assert!(!proj.child(".mcp.json").path().exists());
        assert!(!proj.child(".claude").path().exists());
        // And never a secret value — the definition digest only.
        let text = std::fs::read_to_string(Lock::path(proj.path())).unwrap();
        assert!(!text.contains("Bearer"));
    }

    #[test]
    fn lock_pins_local_executables_and_declared_roots() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());
        let library = Library::default();

        proj.child("scripts/entry.py")
            .write_str("import x")
            .unwrap();
        proj.child("tools/lib.py").write_str("v1").unwrap();

        let manifest: Manifest = toml::from_str(
            r#"
            version = 1
            [servers.agent]
            type = "stdio"
            command = "python"
            args = ["./scripts/entry.py"]
            integrity_roots = ["tools"]
            [profiles.dev]
            servers = ["agent"]
            "#,
        )
        .unwrap();

        let (skills, servers) = resolve_profiles(
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            &["dev".to_string()],
        )
        .unwrap();
        record_lock(proj.path(), &skills, &servers, &manifest, &library).unwrap();

        let lock = Lock::load(proj.path()).unwrap();
        use agentstack_core::lock::ExecutableKind;
        let file = lock
            .get_executable("scripts/entry.py", ExecutableKind::File)
            .expect("entry script pinned");
        assert_eq!(file.checksum.hex().len(), 64);
        let root = lock
            .get_executable("tools", ExecutableKind::Root)
            .expect("declared root pinned");
        assert_eq!(root.checksum.hex().len(), 64);

        // The one-byte re-gate chain: an edit inside the declared root makes
        // a re-lock rewrite the pin (new checksum → new lock bytes → the
        // trust digest flips via the existing chain).
        proj.child("tools/lib.py").write_str("v2").unwrap();
        record_lock(proj.path(), &skills, &servers, &manifest, &library).unwrap();
        let relocked = Lock::load(proj.path()).unwrap();
        assert_ne!(
            relocked
                .get_executable("tools", ExecutableKind::Root)
                .unwrap()
                .checksum,
            root.checksum
        );
    }

    #[test]
    fn removing_a_server_or_root_prunes_its_executable_pins() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let library = Library::default();
        proj.child("tool.sh").write_str("echo").unwrap();
        proj.child("tools/lib.py").write_str("v1").unwrap();

        let with_root: Manifest = toml::from_str(
            "version = 1\n[servers.agent]\ntype = \"stdio\"\ncommand = \"./tool.sh\"\nintegrity_roots = [\"tools\"]\n",
        )
        .unwrap();
        record_executable_pins(proj.path(), &with_root, &library, lib_home.path()).unwrap();
        assert_eq!(Lock::load(proj.path()).unwrap().executables.len(), 2);

        // Un-declaring the root prunes its pin; the command pin survives.
        let without_root: Manifest = toml::from_str(
            "version = 1\n[servers.agent]\ntype = \"stdio\"\ncommand = \"./tool.sh\"\n",
        )
        .unwrap();
        record_executable_pins(proj.path(), &without_root, &library, lib_home.path()).unwrap();
        let lock = Lock::load(proj.path()).unwrap();
        assert_eq!(lock.executables.len(), 1);
        use agentstack_core::lock::ExecutableKind;
        assert!(lock
            .get_executable("tool.sh", ExecutableKind::File)
            .is_some());
        assert!(lock.get_executable("tools", ExecutableKind::Root).is_none());

        // Removing the server entirely prunes everything.
        let empty: Manifest = toml::from_str("version = 1\n").unwrap();
        record_executable_pins(proj.path(), &empty, &library, lib_home.path()).unwrap();
        assert!(Lock::load(proj.path()).unwrap().executables.is_empty());
    }

    // E1 witness (D6): a one-byte edit to an extension's source fails strict
    // locked verification before launch, and re-locking rewrites the pin —
    // new checksum → new lock bytes → the trust digest flips via the existing
    // chain, forcing re-review.
    #[test]
    fn one_byte_extension_edit_refuses_locked_and_relock_regates() {
        let proj = assert_fs::TempDir::new().unwrap();
        proj.child("extensions/checkpoint/index.ts")
            .write_str("export default function (pi) {} // v1")
            .unwrap();

        let manifest: Manifest = toml::from_str(
            r#"
            version = 1
            [extensions.checkpoint]
            path = "./extensions/checkpoint"
            target = "pi"
            "#,
        )
        .unwrap();

        let library = Library::default();
        let lib_home = proj.child("lib").path().to_path_buf();
        let store = crate::store::Store::with_root(proj.child("store").path().to_path_buf());

        record_extension_pins(proj.path(), &manifest, &library, &lib_home, &store).unwrap();
        let lock = Lock::load(proj.path()).unwrap();
        let pinned = lock.get_extension("checkpoint").expect("pinned").clone();
        assert_eq!(pinned.checksum.len(), 64);
        assert_eq!(pinned.target, "pi");
        assert_eq!(pinned.source, "path");
        assert_eq!(pinned.path.as_deref(), Some("./extensions/checkpoint"));

        let ext = &manifest.extensions["checkpoint"];
        let status = |lock: &Lock| {
            crate::resolve::extension_lock_status(
                "checkpoint",
                ext,
                proj.path(),
                &library,
                &lib_home,
                &store,
                lock,
                crate::resolve::ResolveMode::NoFetch,
            )
            .status
        };
        assert_eq!(status(&lock), crate::resolve::ExtensionLockStatus::Matches);
        let clean = vec![("checkpoint".to_string(), status(&lock))];
        assert!(crate::verify::ensure_locked_inputs("pi", &[], &[], &[], &[], &clean).is_ok());

        // One byte changes → strict verification refuses, naming the extension.
        proj.child("extensions/checkpoint/index.ts")
            .write_str("export default function (pi) {} // v2")
            .unwrap();
        let drifted = vec![("checkpoint".to_string(), status(&lock))];
        assert!(matches!(
            drifted[0].1,
            crate::resolve::ExtensionLockStatus::ChecksumDrift { .. }
        ));
        let err = crate::verify::ensure_locked_inputs("pi", &[], &[], &[], &[], &drifted)
            .unwrap_err()
            .to_string();
        assert!(err.contains("extension 'checkpoint'"), "{err}");

        // Re-locking accepts the edit by rewriting the pin: the lock bytes
        // change, which is exactly what flips the trust digest.
        let before = std::fs::read(Lock::path(proj.path())).unwrap();
        record_extension_pins(proj.path(), &manifest, &library, &lib_home, &store).unwrap();
        let after = std::fs::read(Lock::path(proj.path())).unwrap();
        assert_ne!(before, after, "accepting drift must change the lock bytes");

        // Retargeting without re-locking blocks too — the pin bound the code
        // to one harness.
        let retargeted: Manifest = toml::from_str(
            r#"
            version = 1
            [extensions.checkpoint]
            path = "./extensions/checkpoint"
            target = "opencode"
            "#,
        )
        .unwrap();
        let lock = Lock::load(proj.path()).unwrap();
        let status = crate::resolve::extension_lock_status(
            "checkpoint",
            &retargeted.extensions["checkpoint"],
            proj.path(),
            &library,
            &lib_home,
            &store,
            &lock,
            crate::resolve::ResolveMode::NoFetch,
        )
        .status;
        assert!(matches!(
            status,
            crate::resolve::ExtensionLockStatus::TargetDrift { .. }
        ));

        // Removing the declaration prunes its pin (stale-pin rule).
        let empty: Manifest = toml::from_str("version = 1\n").unwrap();
        record_extension_pins(proj.path(), &empty, &library, &lib_home, &store).unwrap();
        assert!(Lock::load(proj.path())
            .unwrap()
            .get_extension("checkpoint")
            .is_none());
    }

    #[test]
    fn single_profile_selection_locks_only_its_refs() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());
        let library = library_with(&lib_home);

        let manifest: Manifest = toml::from_str(
            r#"
            version = 1
            [profiles.review]
            skills = ["sql-review"]
            [profiles.ops]
            servers = ["kibana"]
            "#,
        )
        .unwrap();

        let (skills, servers) = resolve_profiles(
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            &["review".to_string()],
        )
        .unwrap();
        assert_eq!(skills.len(), 1);
        assert!(servers.is_empty(), "other profile's servers not resolved");
    }

    #[test]
    fn broken_ref_fails_before_any_lock_write() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());
        let library = Library::default(); // empty — nothing resolves

        let manifest: Manifest = toml::from_str(
            r#"
            version = 1
            [profiles.p]
            skills = ["nope"]
            "#,
        )
        .unwrap();

        let err = resolve_profiles(
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            &["p".to_string()],
        )
        .unwrap_err();
        assert!(err.to_string().contains("nope"));
        assert!(!Lock::path(proj.path()).exists(), "no partial lock written");
    }
}
