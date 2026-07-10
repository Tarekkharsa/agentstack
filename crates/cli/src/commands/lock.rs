//! `agentstack lock` — resolve each profile's skill + server refs
//! (library-aware) and pin them in `agentstack.lock`, WITHOUT rendering
//! configs or materializing skills.
//!
//! The lock-only counterpart of `use <profile> --write`: clean-at-rest repos
//! reference library capabilities by name and keep no generated files, so
//! pinning must not require an activate-then-deactivate dance. Resolution
//! fetches git-backed sources as needed (like `use --write`), and lock entries
//! for names outside the selected profiles are preserved.

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

pub fn run(args: &LockArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let ctx = super::load(manifest_dir)?;
    let manifest = &ctx.loaded.manifest;

    let profiles: Vec<String> = match &args.profile {
        Some(p) => {
            manifest
                .profiles
                .get(p)
                .with_context(|| format!("no profile '{p}' in manifest"))?;
            vec![p.clone()]
        }
        None => manifest.profiles.keys().cloned().collect(),
    };
    if profiles.is_empty() {
        println!("Manifest defines no profiles — nothing to lock.");
        return Ok(());
    }

    let library = Library::load_default()?;
    let lib_home = crate::util::paths::lib_home();
    let store = crate::store::Store::default_store();

    let (skills, servers) =
        resolve_profiles(manifest, &ctx.dir, &library, &lib_home, &store, &profiles)?;
    record_lock(&ctx.dir, &skills, &servers, manifest, &library)?;

    println!(
        "{} pinned {} skill(s) + {} server(s) from {} profile(s) in {}",
        "✓".green(),
        skills.len(),
        servers.len(),
        profiles.len(),
        Lock::path(&ctx.dir).display()
    );
    println!(
        "  no configs rendered, no skills materialized — that stays `agentstack use <profile> --write`."
    );
    Ok(())
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
            pname,
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
        assert_eq!(skill.checksum.len(), 64);
        let server = lock.get_server("kibana").expect("server pinned");
        assert_eq!(server.source, "library");
        // Lock-only: nothing was rendered or materialized in the project.
        assert!(!proj.child(".mcp.json").path().exists());
        assert!(!proj.child(".claude").path().exists());
        // And never a secret value — the definition digest only.
        let text = std::fs::read_to_string(Lock::path(proj.path())).unwrap();
        assert!(!text.contains("Bearer"));
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
