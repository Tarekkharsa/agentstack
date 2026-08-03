//! The package layer (W5): turning a compact package *reference* in a toolset
//! into an exact, digest-pinned member set in the lock.
//!
//! `docs/design/package-layer.md` is the contract. The one sentence that
//! explains every choice in this file comes from
//! `docs/design/automatic-delivery.md` §"Copy versus live reference, settled":
//!
//! > A compact central package reference in the manifest is exactly as safe as
//! > vendored copying **iff the lock pins the expanded member set.**
//!
//! So this module has exactly two jobs. [`expand_selected`] does the *iff* —
//! it reads the package boundary, applies the project's per-member overrides,
//! digests every surviving member through the **existing** pinning acts, and
//! hands back [`LockedPackage`] entries. [`effective_members`] is the read
//! seam: every surface that reports members reads it, and it reads the LOCK —
//! never the library, whose whole point is that it may move ahead freely.
//!
//! Nothing here introduces a digest path or a pinning path. Skills go through
//! [`Store::pin`], instruction fragments through [`Store::pin_instruction`],
//! and a server member is the same definition digest [`resolve_server`] has
//! always produced. That is deliberate: a second pinning path is a second place
//! for content binding to be weakened (invariant 4).
//!
//! [`Store::pin`]: crate::store::Store::pin
//! [`Store::pin_instruction`]: crate::store::Store::pin_instruction
//! [`resolve_server`]: crate::resolve::resolve_server

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use agentstack_core::digest::Sha256Hex;
use agentstack_core::lock::{
    Lock, LockedPackage, LockedPackageMember, PackageMemberKind, PackageMemberOrigin,
};
use anyhow::{Context, Result};

use crate::library::{Library, LibraryPackage, PACKAGES_DIR, PACK_FILE};
use crate::manifest::Manifest;
use crate::provider::gitpack::{self, PackToml};
use crate::resolve::ResolveMode;
use crate::store::Store;

/// A package body located on disk, with its parsed and gated `pack.toml`.
///
/// "Gated" is load-bearing: by the time this exists the name contract has
/// passed and the executable-kind fence has refused hooks and extensions, so
/// no caller has to remember either check.
#[derive(Debug)]
pub struct LoadedPackage {
    /// The name the toolset referenced, which is also the index key.
    pub name: String,
    /// The exact version, recorded verbatim in the pin.
    pub version: String,
    /// `library:<name>` or `git:<url>@<tag>[#subdir]`.
    pub source: String,
    /// The exact revision, for a git-sourced package.
    pub rev: Option<String>,
    /// The package root — every member path resolves under it, and never
    /// outside it.
    pub root: PathBuf,
    pub toml: PackToml,
}

/// Which packages the given toolsets select, and which toolsets selected each.
///
/// A `BTreeMap<package, BTreeSet<toolset>>` rather than a flat list because the
/// lock records *why* a package is pinned: two toolsets naming one package
/// produce one pin carrying both names, and dropping one of them must leave the
/// pin (and change its `toolsets` list), not orphan it.
pub fn selected_packages(
    manifest: &Manifest,
    toolsets: &[String],
) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for toolset in toolsets {
        let Some(profile) = manifest.profiles.get(toolset) else {
            continue;
        };
        for package in &profile.packages {
            out.entry(package.clone())
                .or_default()
                .insert(toolset.clone());
        }
    }
    out
}

/// Read one package's boundary from the central library, fail-closed at every
/// step where the answer is uncertain.
///
/// Uncertainty refuses rather than serving a partial set — that is W5's
/// fail-closed clause, and it is why each of these is an error and not a skip:
/// an unknown name, a body that is not on this machine, a `pack.toml` that
/// cannot be read or parsed, an index checksum the bytes no longer match, a
/// `pack.toml` whose own name disagrees with the index key, a missing version,
/// a hostile member name, and an executable member.
pub fn load_from_library(library: &Library, lib_home: &Path, name: &str) -> Result<LoadedPackage> {
    let entry = library.get_package(name).with_context(|| {
        format!(
            "no package '{}' in the central library — `agentstack lib list` shows what is \
             installed here",
            crate::text::sanitize_line(name)
        )
    })?;
    let root = entry.body_dir(lib_home).with_context(|| {
        format!(
            "package '{}' is indexed but its body is not available on this machine \
             (a git-sourced package must be fetched before it can be pinned)",
            crate::text::sanitize_line(name)
        )
    })?;
    let toml = read_and_gate(&root, name, entry)?;
    let version = entry.version.clone().with_context(|| {
        format!(
            "package '{}' has no version recorded in the library index — the lock pins an \
             exact version, so an unversioned package cannot be selected by a toolset",
            crate::text::sanitize_line(name)
        )
    })?;
    let source = match (&entry.git, &entry.rev) {
        (Some(url), _) => {
            let mut s = format!("git:{url}@{version}");
            if let Some(sub) = &entry.subpath {
                s.push('#');
                s.push_str(sub);
            }
            s
        }
        // The bare name, never the `<source>:<name>` reference: a qualifier is
        // a selector, and the lock records the capability's own identity.
        _ => format!("library:{}", crate::sources::capability_name(name)),
    };
    Ok(LoadedPackage {
        name: crate::sources::capability_name(name).to_string(),
        version: crate::text::sanitize_line(&version),
        source: crate::text::sanitize_line(&source),
        rev: entry.rev.as_deref().map(crate::text::sanitize_line),
        root,
        toml,
    })
}

/// Read `pack.toml` (bounded — it is remote content, rule 7), verify it against
/// the index pin, and run both intake gates.
fn read_and_gate(root: &Path, name: &str, entry: &LibraryPackage) -> Result<PackToml> {
    let manifest_path = root.join(PACK_FILE);
    let text = crate::util::read_to_string_bounded(&manifest_path, crate::util::MAX_CONFIG_BYTES)
        .with_context(|| format!("reading {}", manifest_path.display()))?;
    // The index pin covers the package BOUNDARY. A divergence here is not the
    // benign "library moved ahead" of `pinned-serving-and-library-drift.md` —
    // that exemption is scoped to the MCP serve path and to skills that already
    // carry a project pin. This is the intake gate, and an intake gate that
    // accepted unverified bytes would be pinning a member set nobody reviewed.
    if let Some(pinned) = &entry.checksum {
        let live = Sha256Hex::of(text.as_bytes());
        anyhow::ensure!(
            &live == pinned,
            "package '{}' no longer matches its library index pin — its {PACK_FILE} has \
             changed since it was installed. Re-install or re-sync the package; nothing was \
             pinned.",
            crate::text::sanitize_line(name)
        );
    }
    let parsed: PackToml =
        toml::from_str(&text).with_context(|| format!("parsing {}", manifest_path.display()))?;
    gitpack::validate_pack_names(&parsed)?;
    gitpack::refuse_executable_members(&parsed)?;
    // The index key is what a toolset writes; the body's own name is what the
    // provenance strings and member coordinates say. If they disagree, one of
    // the two is lying about which artifact this is, and there is no safe way
    // to pick a winner.
    anyhow::ensure!(
        parsed.name == entry.name,
        "package '{}' declares the name '{}' in its {PACK_FILE} — the library index key and \
         the package's own name must agree",
        crate::text::sanitize_line(&entry.name),
        crate::text::sanitize_line(&parsed.name)
    );
    Ok(parsed)
}

/// One member as the package publishes it, before any override is applied.
struct DeclaredMember {
    name: String,
    kind: PackageMemberKind,
    /// Path relative to the package root. `None` for the package's server,
    /// which is declared inline in `pack.toml` rather than as a file.
    rel_path: Option<String>,
}

/// Every member `pack.toml` declares, in a stable order. The optional server is
/// named after the package itself — the same convention the vendored install
/// rail uses when it writes `[servers.<pack>]`, so one package means one server
/// name on both rails.
fn declared_members(loaded: &LoadedPackage) -> Vec<DeclaredMember> {
    let mut out = Vec::new();
    if loaded.toml.server.is_some() {
        out.push(DeclaredMember {
            name: loaded.toml.name.clone(),
            kind: PackageMemberKind::Server,
            rel_path: None,
        });
    }
    for m in &loaded.toml.skills {
        out.push(DeclaredMember {
            name: m.name.clone(),
            kind: PackageMemberKind::Skill,
            rel_path: Some(m.path.clone()),
        });
    }
    for m in &loaded.toml.instructions {
        out.push(DeclaredMember {
            name: m.name.clone(),
            kind: PackageMemberKind::Instruction,
            rel_path: Some(m.path.clone()),
        });
    }
    out
}

/// Every package any declared toolset selects. The denominator for "is this
/// override live?" and for the lock's stale-pin pruning — deliberately NOT the
/// subset a single `lock --profile <p>` run expands, or locking one toolset
/// would condemn another toolset's override and prune its pin.
pub fn all_selected_packages(manifest: &Manifest) -> BTreeMap<String, BTreeSet<String>> {
    let declared: Vec<String> = manifest.profiles.keys().cloned().collect();
    selected_packages(manifest, &declared)
}

/// Expand every selected package into a pinned [`LockedPackage`].
///
/// Strict throughout, and strict *before* anything is written: the caller gets
/// either the complete set of expansions or an error. A half-expanded package
/// in the lock would be a member set that reads as authoritative and is not.
pub fn expand_selected(
    manifest: &Manifest,
    dir: &Path,
    library: &Library,
    lib_home: &Path,
    store: &Store,
    selections: &BTreeMap<String, BTreeSet<String>>,
) -> Result<Vec<LockedPackage>> {
    // An override for a package NO declared toolset names is refused for the
    // same reason a stale `remove` key is: the project believes it changed
    // something, and nothing did. Measured against every declared toolset, not
    // against this run's `selections`, so `lock --profile backend` does not
    // condemn an override belonging to `ops`.
    let live = all_selected_packages(manifest);
    for name in manifest.package_overrides.keys() {
        anyhow::ensure!(
            live.contains_key(name),
            "[package_overrides.{}] overrides a package no toolset selects — add '{}' to a \
             toolset's `packages`, or remove the override",
            crate::text::sanitize_line(name),
            crate::text::sanitize_line(name)
        );
    }
    let mut out = Vec::new();
    for (name, toolsets) in selections {
        let loaded = load_from_library(library, lib_home, name)
            .with_context(|| format!("expanding package '{}'", crate::text::sanitize_line(name)))?;
        out.push(
            expand_one(manifest, dir, library, lib_home, store, &loaded, toolsets).with_context(
                || format!("expanding package '{}'", crate::text::sanitize_line(name)),
            )?,
        );
    }
    Ok(out)
}

fn expand_one(
    manifest: &Manifest,
    dir: &Path,
    library: &Library,
    lib_home: &Path,
    store: &Store,
    loaded: &LoadedPackage,
    toolsets: &BTreeSet<String>,
) -> Result<LockedPackage> {
    let declared = declared_members(loaded);
    let empty = agentstack_core::manifest::PackageOverride::default();
    let overrides = manifest
        .package_overrides
        .get(&loaded.name)
        .unwrap_or(&empty);

    // A `remove` or `replace` key naming nothing is refused: a stale override
    // that silently matches nothing is how a project comes to believe it
    // dropped a member it still has.
    for key in overrides.remove.iter().chain(overrides.replace.keys()) {
        anyhow::ensure!(
            declared.iter().any(|m| &m.name == key),
            "override names '{}', which package '{}' does not carry — its members are: {}",
            crate::text::sanitize_line(key),
            crate::text::sanitize_line(&loaded.name),
            declared
                .iter()
                .map(|m| crate::text::sanitize_line(&m.name))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    let mut members = Vec::new();
    for member in &declared {
        if overrides.remove.iter().any(|r| r == &member.name) {
            continue;
        }
        let pinned = match overrides.replace.get(&member.name) {
            Some(replacement) => {
                pin_replacement(manifest, dir, library, lib_home, store, member, replacement)?
            }
            None => pin_package_member(loaded, store, member)?,
        };
        members.push(pinned);
    }

    Ok(LockedPackage {
        name: crate::text::sanitize_line(&loaded.name),
        version: loaded.version.clone(),
        source: loaded.source.clone(),
        rev: loaded.rev.clone(),
        toolsets: toolsets
            .iter()
            .map(|t| crate::text::sanitize_line(t))
            .collect(),
        removed: overrides
            .remove
            .iter()
            .map(|r| crate::text::sanitize_line(r))
            .collect(),
        members,
    })
}

/// Digest one member as the package publishes it, through the existing pinning
/// acts. Each of the three kinds pins the thing that kind has always pinned:
/// a skill's tree digest, an instruction's file digest, a server's definition
/// digest over the `${REF}`-only table (invariant 5 — no resolved value ever
/// reaches here).
fn pin_package_member(
    loaded: &LoadedPackage,
    store: &Store,
    member: &DeclaredMember,
) -> Result<LockedPackageMember> {
    let coordinate = member.rel_path.as_deref().unwrap_or("[server]");
    let provenance = crate::text::sanitize_line(&format!(
        "package:{}@{}#{coordinate}",
        loaded.name, loaded.version
    ));
    let checksum = match member.kind {
        PackageMemberKind::Server => {
            // `to_server` lifts secret headers/env to `${REF}`s; the digest is
            // over that table, exactly as `resolve_server` digests an inline
            // server. Same bytes, same meaning, one definition of "a server
            // pin".
            let server = loaded
                .toml
                .server
                .as_ref()
                .expect("a server member exists only when pack.toml declares one")
                .to_install()?
                .to_server_named(&member.name);
            let text = toml::to_string(&server).with_context(|| {
                format!(
                    "serializing the server definition of package '{}'",
                    loaded.name
                )
            })?;
            // `Store::pin_server_definition` returns the same digest this line
            // always produced (`Sha256Hex::of(text)`) and additionally deposits
            // the bytes it covers — which is what lets the gateway serve this
            // member from the lock and the store, without ever re-reading the
            // package's current `pack.toml`.
            store.pin_server_definition(&member.name, &text)
        }
        PackageMemberKind::Skill => {
            // Containment first: a member path is remote content and must stay
            // inside the package (rule 7). `contained` is the same check the
            // git pack rail applies at fetch time.
            let body = gitpack::contained(&loaded.root, coordinate, &member.name)?;
            // The digest excludes symlinks, so following one would serve bytes
            // no pin covers — refuse before digesting, as every other skill
            // path does.
            crate::scan::reject_symlinks(&body)?;
            let checksum = crate::store::dir_digest(&body)
                .with_context(|| format!("digesting package member '{}'", member.name))?
                .hex()
                .to_string();
            // `Store::pin` deposits the bytes it pins into the content store,
            // which is what lets runtime serve this member by digest instead of
            // re-reading a library that may have moved on.
            store.pin(&crate::store::Resolved {
                path: body,
                rev: None,
                checksum,
                fetched: false,
                source_kind: "path",
            })?
        }
        PackageMemberKind::Instruction => {
            let file = gitpack::contained(&loaded.root, coordinate, &member.name)?;
            store
                .pin_instruction(&file)
                .with_context(|| format!("pinning package member '{}'", member.name))?
        }
    };
    Ok(LockedPackageMember {
        name: crate::text::sanitize_line(&member.name),
        kind: member.kind,
        origin: PackageMemberOrigin::Package,
        checksum,
        provenance,
    })
}

/// Digest the project's stand-in for one member.
///
/// The replacement is an ordinary project declaration, so it pins through the
/// path that already pins that kind — no new resolution and no new digest. The
/// member keeps the package's NAME (so `remove`/`replace` keys keep matching
/// the package's vocabulary) and gains `origin = project-override`, which is
/// what makes the divergence visible instead of silent.
fn pin_replacement(
    manifest: &Manifest,
    dir: &Path,
    library: &Library,
    lib_home: &Path,
    store: &Store,
    member: &DeclaredMember,
    replacement: &str,
) -> Result<LockedPackageMember> {
    let kind_word = member.kind.as_str();
    let checksum = match member.kind {
        PackageMemberKind::Skill => {
            anyhow::ensure!(
                manifest.skills.contains_key(replacement),
                "override replaces {kind_word} '{}' with '{}', which this project does not \
                 declare as a [skills.{}] — a replacement must be a capability of the same \
                 kind the project already declares",
                crate::text::sanitize_line(&member.name),
                crate::text::sanitize_line(replacement),
                crate::text::sanitize_line(replacement)
            );
            let resolved = crate::resolve::resolve_skill(
                manifest,
                dir,
                library,
                lib_home,
                store,
                replacement,
                ResolveMode::Fetch,
            )
            .with_context(|| {
                format!(
                    "resolving the replacement skill '{}'",
                    crate::text::sanitize_line(replacement)
                )
            })?;
            store.pin(&crate::store::Resolved {
                path: resolved.path,
                rev: resolved.rev,
                checksum: resolved.checksum,
                fetched: false,
                source_kind: resolved.source_kind,
            })?
        }
        PackageMemberKind::Server => {
            anyhow::ensure!(
                manifest.servers.contains_key(replacement),
                "override replaces {kind_word} '{}' with '{}', which this project does not \
                 declare as a [servers.{}] — a replacement must be a capability of the same \
                 kind the project already declares",
                crate::text::sanitize_line(&member.name),
                crate::text::sanitize_line(replacement),
                crate::text::sanitize_line(replacement)
            );
            let resolved = crate::resolve::resolve_server(manifest, library, lib_home, replacement)
                .map_err(anyhow::Error::from)?;
            // Deposit the bytes this checksum covers, for the same reason the
            // package arm does — a replaced member is still a served member, so
            // runtime must be able to read its definition from the store.
            //
            // The two origins hash different bytes (`resolve_server`: the
            // serialized table for an inline server, the definition FILE for a
            // library one), so each is deposited from the source its own
            // checksum was taken over. Deposit is content-addressed, so a text
            // that no longer hashes to `resolved.checksum` simply lands under
            // its own address and leaves the pin's address empty — a
            // fail-closed miss at serve time, never a wrong definition served.
            let definition = match resolved.origin {
                crate::resolve::ServerOrigin::Inline => toml::to_string(&resolved.server).ok(),
                // The definition file lives under whichever linked source
                // satisfied the name, which is what `source_root` answers.
                crate::resolve::ServerOrigin::Library => {
                    std::fs::read_to_string(crate::resolve::library_server_path(
                        &library.source_root(crate::library::Kind::Server, lib_home, replacement),
                        crate::sources::capability_name(replacement),
                    ))
                    .ok()
                }
            };
            if let Some(text) = definition {
                store.pin_server_definition(&member.name, &text);
            }
            Sha256Hex::parse(&resolved.checksum)?
        }
        PackageMemberKind::Instruction => {
            let instr = manifest.instructions.get(replacement).with_context(|| {
                format!(
                    "override replaces {kind_word} '{}' with '{}', which this project does not \
                     declare as an [instructions.{}] — a replacement must be a capability of \
                     the same kind the project already declares",
                    crate::text::sanitize_line(&member.name),
                    crate::text::sanitize_line(replacement),
                    crate::text::sanitize_line(replacement)
                )
            })?;
            // The replacement's BASE body: a package member is one fragment,
            // and the override swaps which fragment it is, not which variant of
            // it a harness gets. The replacement's own variants pin through the
            // ordinary instruction path.
            let src = crate::instructions::base_source(replacement, instr, dir, library)
                .with_context(|| {
                    format!(
                        "resolving the replacement fragment '{}'",
                        crate::text::sanitize_line(replacement)
                    )
                })?;
            store
                .pin_instruction(&src)
                .with_context(|| format!("pinning the replacement fragment at {}", src.display()))?
        }
    };
    Ok(LockedPackageMember {
        name: crate::text::sanitize_line(&member.name),
        kind: member.kind,
        origin: PackageMemberOrigin::ProjectOverride,
        checksum,
        provenance: crate::text::sanitize_line(&format!("project:{kind_word}s.{replacement}")),
    })
}

/// **The read seam.** The effective member set of every package this project
/// pinned, straight from the LOCK.
///
/// Every surface that lists or reports package members goes through here, and
/// none of them may reach for the library instead. That is the reproducibility
/// rule (`docs/design/automatic-delivery.md`): the library can move
/// arbitrarily far ahead without changing what any project resolves, precisely
/// because no project reads it at resolve time. A function that took a
/// `&Library` would make that property depend on every caller remembering not
/// to use it; this one cannot.
pub fn effective_members(lock: &Lock) -> &[LockedPackage] {
    &lock.packages
}

/// The effective member set of one pinned package, or `None` when this project
/// pinned no such package. Same rule as [`effective_members`]: the lock is the
/// only input.
pub fn effective_members_of<'a>(lock: &'a Lock, name: &str) -> Option<&'a LockedPackage> {
    lock.get_package(name)
}

/// Every pinned member of one kind, paired with the package that carries it,
/// narrowed to what a toolset fence admits.
///
/// This is the **boundary** read: it answers "which members are in scope right
/// now" from names, kinds and digests alone — no member body is opened here,
/// which is what lets a package expose its boundary without loading twenty
/// skill bodies into context (`automatic-delivery.md` §"Boundary, not bodies").
///
/// `profile` is the runtime fence: `Some(toolset)` keeps only packages that
/// toolset selected (the lock records which ones did, in `toolsets`), `None`
/// is the unfenced, development-open read. The fence is applied here rather
/// than by each caller so the loadable index and the load path cannot disagree
/// about what is in scope.
pub fn members_of_kind<'a>(
    lock: &'a Lock,
    kind: PackageMemberKind,
    profile: Option<&str>,
) -> Vec<(&'a LockedPackage, &'a LockedPackageMember)> {
    lock.packages
        .iter()
        .filter(|pkg| match profile {
            Some(p) => pkg.toolsets.iter().any(|t| t == p),
            None => true,
        })
        .flat_map(|pkg| {
            pkg.members
                .iter()
                .filter(move |m| m.kind == kind)
                .map(move |m| (pkg, m))
        })
        .collect()
}

/// One named member of one kind, under the same fence as [`members_of_kind`].
///
/// Member names are unique per package but not across packages; the first
/// match in lock order wins, which is deterministic because
/// [`Lock::upsert_package`] keeps packages sorted by name.
///
/// [`Lock::upsert_package`]: agentstack_core::lock::Lock::upsert_package
pub fn member_of_kind<'a>(
    lock: &'a Lock,
    kind: PackageMemberKind,
    profile: Option<&str>,
    name: &str,
) -> Option<(&'a LockedPackage, &'a LockedPackageMember)> {
    members_of_kind(lock, kind, profile)
        .into_iter()
        .find(|(_, m)| m.name == name)
}

/// Where a library package's body lives for a `path` source — the one place
/// this layout is spelled out for writers (a future `lib add-package`).
pub fn body_dir_for(lib_home: &Path, name: &str) -> PathBuf {
    lib_home.join(PACKAGES_DIR).join(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::prelude::*;

    /// Build a library holding one path-sourced package with the given
    /// `pack.toml` body, returning the library and its home.
    fn library_with_package(lib_home: &assert_fs::TempDir, name: &str, pack_toml: &str) -> Library {
        lib_home
            .child(format!("{PACKAGES_DIR}/{name}/{PACK_FILE}"))
            .write_str(pack_toml)
            .unwrap();
        let mut lib = Library::default();
        lib.upsert_package(LibraryPackage {
            name: name.into(),
            source: "path".into(),
            path: Some(name.into()),
            git: None,
            rev: None,
            subpath: None,
            checksum: None,
            version: Some("1.4.0".into()),
            description: None,
            provenance: Some("manual".into()),
        });
        lib
    }

    #[test]
    fn selection_records_every_toolset_that_named_the_package() {
        let manifest: Manifest = toml::from_str(
            r#"
            version = 1
            [profiles.backend]
            packages = ["rust-backend"]
            [profiles.ops]
            packages = ["rust-backend", "observability"]
            "#,
        )
        .unwrap();
        let all: Vec<String> = manifest.profiles.keys().cloned().collect();
        let selected = selected_packages(&manifest, &all);
        assert_eq!(
            selected["rust-backend"],
            BTreeSet::from(["backend".to_string(), "ops".to_string()])
        );
        assert_eq!(
            selected["observability"],
            BTreeSet::from(["ops".to_string()])
        );
        // A toolset selection that names only one of them narrows accordingly.
        let narrow = selected_packages(&manifest, &["backend".to_string()]);
        assert!(!narrow.contains_key("observability"));
    }

    #[test]
    fn a_package_carrying_hooks_is_refused_by_name() {
        let lib_home = assert_fs::TempDir::new().unwrap();
        let library = library_with_package(
            &lib_home,
            "rust-backend",
            "name = \"rust-backend\"\n\n[[hook]]\nname = \"pre-commit\"\npath = \"hooks/pre.sh\"\n",
        );
        let err = load_from_library(&library, lib_home.path(), "rust-backend")
            .unwrap_err()
            .to_string();
        assert!(err.contains("pre-commit"), "names the member: {err}");
        assert!(err.contains("not supported in v1"), "{err}");
    }

    #[test]
    fn an_index_key_disagreeing_with_the_body_refuses() {
        let lib_home = assert_fs::TempDir::new().unwrap();
        let library =
            library_with_package(&lib_home, "rust-backend", "name = \"something-else\"\n");
        let err = load_from_library(&library, lib_home.path(), "rust-backend")
            .unwrap_err()
            .to_string();
        assert!(err.contains("must agree"), "{err}");
    }

    #[test]
    fn an_unversioned_package_cannot_be_selected() {
        let lib_home = assert_fs::TempDir::new().unwrap();
        let mut library = library_with_package(&lib_home, "p", "name = \"p\"\n");
        let mut entry = library.get_package("p").unwrap().clone();
        entry.version = None;
        library.upsert_package(entry);
        let err = load_from_library(&library, lib_home.path(), "p")
            .unwrap_err()
            .to_string();
        assert!(err.contains("no version recorded"), "{err}");
    }

    /// The boundary pin is an intake gate, not the serve path: `pack.toml`
    /// bytes that no longer match the index refuse rather than being read as
    /// "the library moved ahead".
    #[test]
    fn a_pack_toml_that_drifted_from_its_index_pin_refuses() {
        let lib_home = assert_fs::TempDir::new().unwrap();
        let mut library = library_with_package(&lib_home, "p", "name = \"p\"\n");
        let mut entry = library.get_package("p").unwrap().clone();
        entry.checksum = Some(Sha256Hex::of(b"some other bytes"));
        library.upsert_package(entry);
        let err = load_from_library(&library, lib_home.path(), "p")
            .unwrap_err()
            .to_string();
        assert!(err.contains("index pin"), "{err}");
    }

    #[test]
    fn a_stale_override_key_refuses() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());
        let library = library_with_package(&lib_home, "p", "name = \"p\"\n");
        let manifest: Manifest = toml::from_str(
            r#"
            version = 1
            [profiles.t]
            packages = ["p"]
            [package_overrides.p]
            remove = ["ghost"]
            "#,
        )
        .unwrap();
        let selected = selected_packages(&manifest, &["t".to_string()]);
        // `{:#}` — the outermost context is only "expanding package 'p'"; the
        // reason lives further down the chain.
        let err = format!(
            "{:#}",
            expand_selected(
                &manifest,
                proj.path(),
                &library,
                lib_home.path(),
                &store,
                &selected,
            )
            .unwrap_err()
        );
        assert!(err.contains("ghost"), "{err}");
    }

    #[test]
    fn an_override_for_an_unselected_package_refuses() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());
        let library = library_with_package(&lib_home, "p", "name = \"p\"\n");
        let manifest: Manifest = toml::from_str(
            r#"
            version = 1
            [package_overrides.p]
            remove = ["anything"]
            "#,
        )
        .unwrap();
        let err = expand_selected(
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            &BTreeMap::new(),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("no toolset selects"), "{err}");
    }
}
