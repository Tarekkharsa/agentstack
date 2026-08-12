//! Name resolution — the single seam that maps a `skills = ["name"]` or
//! `servers = ["name"]` reference to a concrete definition (see
//! `docs/reference.md#the-library-linked-source-folders`).
//!
//! Resolution order (first hit wins), for both skills and servers:
//!
//! 1. **Inline** — a `[skills.<name>]` / `[servers.<name>]` entry in the project
//!    manifest. An inline definition always wins (a project that wants to override
//!    a library item defines it inline).
//! 2. **The linked library sources, in order** — a `[[skill]]` / `[[server]]`
//!    entry in a source's `library.toml`, whose body lives under that source's
//!    `skills/` or `servers/<name>.toml`. First source holding the name wins;
//!    `docs/design/linked-library-sources.md` is the contract.
//!
//! A reference may be **qualified** as `<source>:<name>`, which resolves only in
//! the source it names and skips step 1 entirely — inline is not a source, and
//! letting it win would make the explicit spelling weaker than the bare one.
//! The qualifier is a selector: the resolved capability's `name` is always the
//! bare one, so the lock key, the rendered directory and the gateway's name for
//! it are unchanged by how a project chose to spell the reference.
//!
//! An unresolved name is a hard, structured error. Resolvers return the
//! definition plus metadata (checksum/provenance) for later lock + drift steps;
//! they never resolve secrets — server `${REF}` values stay intact and are
//! resolved per-machine only at render/gateway time.

use std::path::{Path, PathBuf};

use crate::library::Library;
use crate::lock::Lock;
use crate::manifest::{Manifest, Server, Skill};
use crate::store::Store;

/// Where a resolved skill came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillOrigin {
    /// Defined inline in the project manifest (`[skills.<name>]`).
    Inline,
    /// Resolved from the central library (`library.toml`).
    Library,
}

/// A skill name resolved to a concrete source, with the metadata needed to
/// materialize it and to record a reproducible lock entry.
#[derive(Debug, Clone)]
pub struct ResolvedSkill {
    /// The name the project referenced.
    pub name: String,
    /// Which source satisfied the reference.
    pub origin: SkillOrigin,
    /// Local directory holding the skill body.
    pub path: PathBuf,
    /// `"path"` or `"git"`.
    pub source_kind: &'static str,
    /// Resolved git revision (git sources only).
    pub rev: Option<String>,
    /// SHA-256 of the content. Empty if a path source does not exist on disk
    /// yet, or when resolved with [`ResolveMode::PathOnly`] (which skips
    /// digesting entirely).
    pub checksum: String,
    /// Provenance recorded in the library index (library origin only).
    pub provenance: Option<String>,
}

/// Whether resolution may touch the network, and how much content work it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveMode {
    /// Fetch git sources as needed (the materializing path).
    Fetch,
    /// Never fetch: git sources resolve only from an existing store clone, and
    /// an un-cached git source is reported as [`ResolveError::NotAvailableOffline`].
    /// Path/library-path sources resolve identically in both modes.
    NoFetch,
    /// Locate only: no network and **no content digest** — the returned
    /// `checksum` is empty. For read-only surfaces that just need the skill's
    /// path (list/load), where digesting would read+hash the whole body for
    /// nothing. Never use for anything that records a lock entry.
    PathOnly,
}

/// A structured resolution failure. `Unresolved` is the hard error for a name
/// that matches neither an inline manifest skill nor a library entry;
/// `NotAvailableOffline` is a non-fatal `NoFetch` outcome for a git source that
/// is not cached locally; `Source` wraps an underlying fetch/IO failure while
/// resolving a matched entry.
#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("skill '{name}' is not defined in the project manifest or the central library")]
    Unresolved { name: String },
    #[error("skill '{name}' (git {url}) is not available offline — run `agentstack install`")]
    NotAvailableOffline { name: String, url: String },
    /// P19: an inline `[skills.<name>]` block carries no `path`/`git` source, but
    /// a library skill of the same name exists. Rather than the low-level
    /// "neither `path` nor `git`" error, teach the exact fix — the block should
    /// be dropped so the by-name reference resolves the library copy.
    #[error(
        "skill '{name}' is in your central library — drop the `[skills.{name}]` block and list it \
         in the profile's `skills = [...]` to use the library copy"
    )]
    InlineNoSourceShadowsLibrary { name: String },
    #[error(transparent)]
    Source(#[from] anyhow::Error),
}

/// Resolve a single skill name through the resolution order above.
///
/// - `manifest` / `manifest_dir`: the project manifest and the directory its
///   relative skill paths are resolved against.
/// - `library` / `lib_home`: the loaded central index and its home directory
///   (skill bodies live under `<lib_home>/skills/`).
/// - `store`: reused to resolve both origins to a local path + checksum.
/// - `mode`: whether git sources may be fetched ([`ResolveMode`]).
pub fn resolve_skill(
    manifest: &Manifest,
    manifest_dir: &Path,
    library: &Library,
    lib_home: &Path,
    store: &Store,
    name: &str,
    mode: ResolveMode,
) -> Result<ResolvedSkill, ResolveError> {
    resolve_skill_with_pin(
        manifest,
        manifest_dir,
        library,
        lib_home,
        store,
        name,
        mode,
        None,
    )
}

/// Resolve a skill while honoring an authoritative lock commit when one is
/// available. The pin wins over a manifest branch/tag/empty `rev` in both the
/// fetching and offline paths; callers without a lock use [`resolve_skill`].
#[allow(clippy::too_many_arguments)]
pub fn resolve_skill_with_pin(
    manifest: &Manifest,
    manifest_dir: &Path,
    library: &Library,
    lib_home: &Path,
    store: &Store,
    name: &str,
    mode: ResolveMode,
    pinned_rev: Option<&str>,
) -> Result<ResolvedSkill, ResolveError> {
    // A `<source>:<name>` reference names one linked library source, so it
    // never consults the manifest: inline entries are not a source, and letting
    // one win would make the explicit spelling weaker than the bare one. The
    // capability's identity is the bare name in either case — the qualifier is
    // a selector (docs/design/linked-library-sources.md §"Being explicit").
    let qualifier = crate::sources::split_reference(name);
    if let Some((source, _)) = qualifier {
        if !library.linked.has_source(source) {
            return Err(unknown_source(library, name, source, "skill").into());
        }
    }
    let bare = crate::sources::capability_name(name);

    // Locate the source (inline wins over the central library) and the base dir
    // its relative paths resolve against.
    let (skill, base, origin, provenance) =
        if let Some(skill) = manifest.skills.get(name).filter(|_| qualifier.is_none()) {
            // P19: an inline block with no source, when a library skill of the same
            // name exists, is almost always someone who meant to reference the
            // library copy but left an empty `[skills.<name>]` block behind. Teach
            // the fix here — where both the manifest and the library are in hand —
            // instead of letting the store surface the low-level source error.
            // `return` diverges (type `!`), so the `if let` arm still yields the
            // tuple on the normal path.
            if skill.path.is_none() && skill.git.is_none() && library.get(name).is_some() {
                return Err(ResolveError::InlineNoSourceShadowsLibrary {
                    name: name.to_string(),
                });
            }
            (
                skill.clone(),
                manifest_dir.to_path_buf(),
                SkillOrigin::Inline,
                None,
            )
        } else if let Some(entry) = library.get(name) {
            let skill = Skill {
                path: entry.path.clone(),
                git: entry.git.clone(),
                rev: entry.rev.clone(),
                subpath: entry.subpath.clone(),
            };
            (
                skill,
                lib_home.join("skills"),
                SkillOrigin::Library,
                entry.provenance.clone(),
            )
        } else {
            return Err(ResolveError::Unresolved {
                name: name.to_string(),
            });
        };

    let resolved = resolve_source(store, &skill, &base, mode, name, pinned_rev)?;
    Ok(ResolvedSkill {
        name: bare.to_string(),
        origin,
        path: resolved.path,
        source_kind: resolved.source_kind,
        rev: resolved.rev,
        checksum: resolved.checksum,
        provenance,
    })
}

/// The error for a qualified reference naming a library source that is not
/// linked. "No such source" and "no such capability" are different mistakes
/// with different fixes, so they get different messages — and this one lists
/// the sources that *are* linked rather than making the user go look.
fn unknown_source(library: &Library, reference: &str, source: &str, noun: &str) -> anyhow::Error {
    let linked = library.linked.source_names().join(", ");
    let linked = if linked.is_empty() {
        "none".to_string()
    } else {
        linked
    };
    anyhow::anyhow!(
        "{noun} '{}' names library source '{}', which is not linked — linked sources: {linked} \
         (see `agentstack lib sources`)",
        crate::text::sanitize_line(reference),
        crate::text::sanitize_line(source),
    )
}

/// Resolve a located source through the store, honoring the fetch mode. A
/// `NoFetch`/`PathOnly` miss on an un-cached git source becomes
/// `NotAvailableOffline`.
fn resolve_source(
    store: &Store,
    skill: &Skill,
    base: &Path,
    mode: ResolveMode,
    name: &str,
    pinned_rev: Option<&str>,
) -> Result<crate::store::Resolved, ResolveError> {
    let local = match mode {
        ResolveMode::Fetch => return Ok(store.resolve(skill, base, pinned_rev)?),
        ResolveMode::NoFetch => store.resolve_local(skill, base, pinned_rev)?,
        ResolveMode::PathOnly => store.resolve_path_only(skill, base, pinned_rev)?,
    };
    match local {
        Some(r) => Ok(r),
        None => Err(ResolveError::NotAvailableOffline {
            name: name.to_string(),
            url: skill.git.clone().unwrap_or_default(),
        }),
    }
}

// ---------- server resolution (Phase 1b) ----------

/// Where a resolved server came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerOrigin {
    /// Defined inline in the project manifest (`[servers.<name>]`).
    Inline,
    /// Resolved from the central library (`[[server]]` + `servers/<name>.toml`).
    Library,
    /// Carried by a package a toolset selected, read back from the content
    /// store by the digest the lock pins (`[[package.member]]`).
    ///
    /// A third variant rather than a reuse of [`ServerOrigin::Inline`]: a
    /// package member is content from OUTSIDE this project, and every surface
    /// that answers "where did this definition come from?" must be able to say
    /// so. Reusing `Inline` made `explain` print "inline (this project)" for a
    /// package's bytes and made the frozen grant record them under a binding
    /// with no provenance field at all. The compiler now forces each of those
    /// sites to answer for this case.
    Package,
}

/// A server name resolved to its **definition** — the `manifest::Server` with
/// `${REF}` secrets left intact. Secret values are never resolved here; that is
/// exclusively a render/gateway concern.
#[derive(Debug, Clone)]
pub struct ResolvedServer {
    pub name: String,
    pub origin: ServerOrigin,
    /// The server definition, `${REF}` placeholders preserved verbatim.
    pub server: Server,
    /// SHA-256 of the definition (the `servers/<name>.toml` file content for a
    /// library server; the serialized inline table otherwise).
    pub checksum: String,
    /// Provenance recorded in the library index (library origin only).
    pub provenance: Option<String>,
}

/// A structured server-resolution failure. Mirrors the skill resolver's shape;
/// servers are local definitions (no fetch), so there is no offline variant.
#[derive(Debug, thiserror::Error)]
pub enum ServerResolveError {
    #[error("server '{name}' is not defined in the project manifest or the central library")]
    Unresolved { name: String },
    #[error(transparent)]
    Source(#[from] anyhow::Error),
}

/// Where a central-library server's definition file lives. One place spells the
/// layout out, so a caller that needs the raw definition BYTES (rather than the
/// parsed table) cannot disagree with [`resolve_server`] about which file it
/// hashed.
pub fn library_server_path(lib_home: &Path, name: &str) -> PathBuf {
    lib_home
        .join("servers")
        .join(format!("{}.toml", library_file_stem(name)))
}

/// The file-name stem a library server definition is stored under.
///
/// A server's name is whatever the user's other CLIs call it, and those names
/// are not path components: `upstash/context7` is a real MCP server name in six
/// native config formats. The library still keeps one definition per file, so
/// the name is mapped to a file name — here, in one place, so the writer and
/// the reader can never disagree.
///
/// A plain name maps to itself, so every library written before this function
/// existed still resolves. Any other name is percent-encoded byte-wise:
/// everything outside `[A-Za-z0-9_-]` becomes `%XX`. In that branch `.` is
/// encoded too, so `.` and `..` cannot survive as a component, and `%` is
/// encoded so the mapping stays injective — `a/b` and the literal name `a%2Fb`
/// must not claim the same file. That is why a name containing `%` always takes
/// the encoding branch even when it is otherwise plain.
pub fn library_file_stem(name: &str) -> String {
    let plain =
        !name.is_empty() && !name.contains(['/', '\\', '\0', '%']) && name != "." && name != "..";
    if plain {
        return name.to_string();
    }
    let mut out = String::with_capacity(name.len());
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || b == b'_' || b == b'-' {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Resolve a single server name: inline `[servers.<name>]` wins, else the central
/// library's `[[server]]` entry (definition at `<lib_home>/servers/<name>.toml`).
/// Returns the definition with `${REF}`s intact — no secret resolution.
pub fn resolve_server(
    manifest: &Manifest,
    library: &Library,
    lib_home: &Path,
    name: &str,
) -> Result<ResolvedServer, ServerResolveError> {
    // Same qualification rule as skills: `<source>:<name>` addresses one linked
    // library source, bypasses inline entirely, and keeps the bare name as the
    // capability's identity.
    let qualifier = crate::sources::split_reference(name);
    if let Some((source, _)) = qualifier {
        if !library.linked.has_source(source) {
            return Err(unknown_source(library, name, source, "server").into());
        }
    }
    let bare = crate::sources::capability_name(name);

    // 1. Inline manifest server wins.
    if let Some(server) = manifest.servers.get(name).filter(|_| qualifier.is_none()) {
        let text = toml::to_string(server)
            .map_err(|e| anyhow::anyhow!("serializing inline server '{name}': {e}"))?;
        return Ok(ResolvedServer {
            name: name.to_string(),
            origin: ServerOrigin::Inline,
            server: server.clone(),
            checksum: sha256_hex(text.as_bytes()),
            provenance: None,
        });
    }

    // 2. Linked library sources, in precedence order.
    if let Some(entry) = library.get_server(name) {
        // A server definition is a bare file with no `path` field, so the
        // owning source's root is what decides where it lives.
        let root = library.source_root(crate::library::Kind::Server, lib_home, name);
        let path = library_server_path(&root, bare);
        let content = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))?;
        let server: Server = toml::from_str(&content)
            .map_err(|e| anyhow::anyhow!("parsing {}: {e}", path.display()))?;
        return Ok(ResolvedServer {
            name: bare.to_string(),
            origin: ServerOrigin::Library,
            server,
            checksum: sha256_hex(content.as_bytes()),
            provenance: entry.provenance.clone(),
        });
    }

    Err(ServerResolveError::Unresolved {
        name: name.to_string(),
    })
}

/// The servers a runtime surface (the gateway) serves, resolved through the
/// same inline-first/central-library path as rendering — so a server declared
/// only as a name ref in a profile reaches the gateway exactly like an inline
/// one (docs/reference.md: name refs resolve at render/gateway time).
///
/// With `profile` set (an active session whose profile exists), the set is
/// exactly that profile's `servers` list. Otherwise it is everything the
/// manifest declares anywhere: inline `[servers.*]` entries plus every
/// profile-referenced name, deduped in first-seen order.
///
/// Results are per-name so a best-effort caller can skip (and report) a broken
/// ref individually, where rendering hard-fails the whole run. Each success is
/// the full [`ResolvedServer`] — origin and definition checksum included — so
/// runtime surfaces can verify library definitions against `agentstack.lock`
/// before serving them.
pub fn effective_runtime_servers(
    manifest: &Manifest,
    library: &Library,
    lib_home: &Path,
    profile: Option<&str>,
) -> Vec<(String, Result<ResolvedServer, ServerResolveError>)> {
    runtime_server_names(manifest, profile)
        .into_iter()
        .map(|n| {
            let r = resolve_server(manifest, library, lib_home, &n);
            (n, r)
        })
        .collect()
}

/// One entry of a [`frozen_runtime_servers`] set: a resolved, pin-verified
/// server definition, or a fail-closed skip reason. The SAME frozen entries
/// feed both D4 classification and gateway dispatch — never a second, possibly
/// different resolution.
pub type FrozenServer = (String, Result<ResolvedServer, String>);

/// Verify a resolved server's library pin against the lock. Inline servers pass
/// (their definition is inside the trust digest). A library server must match
/// its `agentstack.lock` pin; drift, a missing pin, or an unreadable lock is a
/// fail-closed reason. Extracted so the frozen resolution and the ordinary
/// gateway path apply the identical check.
pub fn verify_library_pin(
    resolved: &ResolvedServer,
    lock: Option<&Lock>,
    name: &str,
) -> Result<(), String> {
    if resolved.origin != ServerOrigin::Library {
        return Ok(());
    }
    let Some(lock) = lock else {
        return Err(
            "library-referenced and the lockfile is unreadable — its pin can't be verified"
                .to_string(),
        );
    };
    match lock.get_server(name) {
        Some(entry) if entry.checksum.hex() != resolved.checksum => Err(format!(
            "library definition drifted from agentstack.lock (locked {}, current {}) — \
             review it, then re-pin with `agentstack lock --write`",
            entry.checksum, resolved.checksum
        )),
        Some(_) => Ok(()),
        None => Err(
            "library server is not pinned in agentstack.lock — pin it with `agentstack lock --write`"
                .to_string(),
        ),
    }
}

/// The **package-carried** runtime servers a toolset fence admits, as the same
/// [`FrozenServer`] entries [`effective_runtime_servers`] produces — so a
/// package's server joins the gateway's upstream set through one shape and one
/// dispatch path, never a parallel one (`CLAUDE.md` invariant 6).
///
/// Two properties do all the work here.
///
/// **Fenced by toolset.** A member is in scope only under a toolset that
/// selected its package; `crate::package::members_of_kind` applies exactly the
/// fence the loadable index applies to skill members. The unfenced read
/// (`profile == None`) contributes **nothing**: an unfenced gateway already
/// serves every manifest server, and unioning every package's server on top of
/// that would turn package membership into a way to widen the surface rather
/// than something a toolset selects. A fence naming a toolset that is gone
/// never reaches here at all — the caller has already resolved that to "no
/// servers", which is the rule this must not regress.
///
/// **Resolved from the lock and the store, never from the library.** The
/// definition is read out of the content-addressed deposit named by the
/// member's pinned checksum and re-verified against it
/// ([`Store::pinned_definition`]). The package's current `pack.toml` is never
/// opened: it is precisely the mutable thing the pin exists to stop reading
/// (`docs/design/pinned-serving-and-library-drift.md`). Anything unresolvable
/// or unverifiable becomes a per-member `Err` — the same fail-closed skip
/// reason a drifted library pin produces — so the gateway refuses it out loud
/// through the existing seatbelt path instead of the member quietly vanishing.
///
/// `already_served` are the names the manifest/library resolution has already
/// claimed. Inline-first precedence is the resolver's rule everywhere else, so
/// it holds here too; a collision is reported rather than dropped.
///
/// [`Store::pinned_definition`]: crate::store::Store::pinned_definition
pub fn package_runtime_servers(
    lock: &Lock,
    store: &Store,
    profile: Option<&str>,
    already_served: &[String],
) -> Vec<FrozenServer> {
    let Some(profile) = profile else {
        return Vec::new();
    };
    crate::package::members_of_kind(lock, crate::lock::PackageMemberKind::Server, Some(profile))
        .into_iter()
        .map(|(pkg, member)| {
            let name = member.name.clone();
            if already_served.iter().any(|n| n == &name) {
                return (
                    name.clone(),
                    // Reasons stay short on purpose: the gateway bounds them to
                    // 200 characters before they reach a terminal or the audit
                    // log, and a diagnosis that gets truncated mid-sentence is
                    // worse than a brief one.
                    Err(format!(
                        "package '{}' carries a server of this name and the project already \
                         serves one — two upstreams under one name have no unambiguous dispatch",
                        crate::text::sanitize_line(&pkg.name)
                    )),
                );
            }
            (name, resolve_package_server(store, pkg, member))
        })
        .collect()
}

/// One pinned package server member, read back from the content store.
fn resolve_package_server(
    store: &Store,
    pkg: &agentstack_core::lock::LockedPackage,
    member: &agentstack_core::lock::LockedPackageMember,
) -> Result<ResolvedServer, String> {
    let hex = member.checksum.hex();
    let text = store.pinned_definition(hex).map_err(|_| {
        format!(
            "its pinned definition is not in the content store, or no longer verifies against \
             the digest package '{}' pins — re-pin with `agentstack lock --write`",
            crate::text::sanitize_line(&pkg.name)
        )
    })?;
    // The bytes just verified against the pin; parsing them is the same
    // round trip `resolve_server` performs on a library definition file.
    let server: Server = toml::from_str(&text).map_err(|_| {
        format!(
            "its pinned definition is not a readable server table — re-pin package '{}' with \
             `agentstack lock --write`",
            crate::text::sanitize_line(&pkg.name)
        )
    })?;
    Ok(ResolvedServer {
        // `Package`, not `Inline`. The integrity question these two share —
        // "does this need `verify_library_pin`'s extra lock check?" — has the
        // same answer for both (no: the digest that binds it is in the lock,
        // the lock is inside the trust digest, and the bytes were re-verified
        // against that digest a few lines above). But integrity is not the only
        // question `origin` is asked. Display and evidence surfaces ask where
        // the bytes came from, and for a package member "this project" is a
        // false answer.
        name: member.name.clone(),
        origin: ServerOrigin::Package,
        server,
        checksum: hex.to_string(),
        provenance: Some(member.provenance.clone()),
    })
}

/// Resolve the profile-fenced runtime server set ONE time for a sandbox/lockdown
/// run, freezing the exact definitions that will feed BOTH D4 classification and
/// the gateway's dispatch — eliminating the classification/dispatch mismatch of
/// a second, independent resolution ([`Gateway::from_frozen`]).
///
/// Guarantees the D4 contract requires of the frozen input:
/// - **Strict profile fencing.** A pinned profile absent from the manifest is a
///   hard error — it must NEVER broaden to every server.
/// - **Library-pin verification up front.** A drift / missing pin / unreadable
///   lock becomes a per-server `Err` here (via [`verify_library_pin`]), so both
///   consumers see the identical accepted set.
///
/// Per-server errors are preserved rather than dropped: the gateway skips them
/// (host-proxy semantics), while a lockdown classification fails the whole run
/// rather than leave a selected endpoint reachable.
///
/// **Package members are part of the frozen set** (review finding 1). They used
/// to be added by the gateway and the image builder only, which meant a
/// Protected run — the default for a plain `agentstack run <cli>` — silently
/// omitted every server a toolset's package carried, while `status --json`
/// listed it as an effective member. Adding them HERE, rather than at each
/// consumer, is what makes "one frozen set feeds classification, dispatch, the
/// grant and the image" true by construction instead of by care (invariant 6).
///
/// The toolset fence is unchanged and is the reason this cannot widen anything:
/// `package_runtime_servers` contributes nothing when `profile` is `None`,
/// because an unfenced set already carries every manifest server and unioning
/// every package's servers on top of that would make package membership a way
/// to widen rather than something a toolset selects.
pub fn frozen_runtime_servers(
    manifest: &Manifest,
    library: &Library,
    lib_home: &Path,
    dir: &Path,
    profile: Option<&str>,
) -> anyhow::Result<Vec<FrozenServer>> {
    if let Some(p) = profile {
        if !manifest.profiles.contains_key(p) {
            anyhow::bail!(
                "toolset '{p}' is not defined in the manifest — refusing to serve any \
                 servers (a missing pinned toolset must never broaden to all servers)"
            );
        }
    }
    let lock = Lock::load(dir).ok();
    let mut out: Vec<FrozenServer> =
        effective_runtime_servers(manifest, library, lib_home, profile)
            .into_iter()
            .map(|(name, resolved)| {
                let out = match resolved {
                    Ok(rs) => verify_library_pin(&rs, lock.as_ref(), &name).map(|()| rs),
                    Err(e) => Err(e.to_string()),
                };
                (name, out)
            })
            .collect();
    // Appended AFTER `verify_library_pin`, for the reason the gateway states:
    // a package member is not library-referenced, and its bytes were already
    // re-verified against the digest the lock pins — a stricter check than
    // `verify_library_pin`'s, applied to the definition itself rather than to a
    // name. A name collision with an already-resolved server becomes a per-member
    // `Err` inside `package_runtime_servers`, never a silent replacement.
    if let Some(lock) = lock.as_ref() {
        let taken: Vec<String> = out.iter().map(|(n, _)| n.clone()).collect();
        out.extend(package_runtime_servers(
            lock,
            &crate::store::Store::default_store(),
            profile,
            &taken,
        ));
    }
    Ok(out)
}

/// Derive the D4 gateway-only host set for a **lockdown** run from a
/// [`frozen_runtime_servers`] set: the normalized host of every HTTP-transport
/// server. Under lockdown a container may reach these hosts only through the
/// gateway relay, never by direct egress.
///
/// Fails the whole lockdown run — rather than silently omitting a server — if:
/// - a selected server is unavailable (unresolved, or a pin failure): its
///   endpoint might still be reachable directly, so dropping it would leave a
///   hole in the fence; or
/// - a selected HTTP server has no classifiable host (empty/malformed URL, or an
///   unresolved `${REF}` in the host portion).
///
/// stdio servers contribute nothing: they are host-side subprocesses with no
/// network endpoint an internal-network container could reach, so they are
/// inherently gateway-only.
pub fn gateway_only_hosts(
    servers: &[FrozenServer],
) -> anyhow::Result<std::collections::BTreeSet<String>> {
    use agentstack_core::manifest::{host_from_url, normalize_host, ServerType};
    let mut hosts = std::collections::BTreeSet::new();
    for (name, resolved) in servers {
        let server = match resolved {
            Ok(r) => &r.server,
            Err(reason) => anyhow::bail!(
                "lockdown: server '{name}' is not available ({reason}) — refusing to \
                 start; a selected server that might still be reachable directly must \
                 not be silently omitted from the gateway-only fence"
            ),
        };
        if server.server_type != ServerType::Http {
            continue; // stdio: host-side subprocess, inherently gateway-only
        }
        let url = server.url.as_deref().unwrap_or("");
        // The ONE shared extractor (`core::manifest::host_from_url`) — the same
        // one the write-time egress check uses — so the fence and every other
        // reader of a declared URL can never disagree on a host.
        let host = host_from_url(url).ok_or_else(|| {
            anyhow::anyhow!(
                "lockdown: HTTP server '{name}' has no classifiable host in its URL \
                 {url:?} (empty, malformed, or an unresolved ${{REF}} in the host) — \
                 refusing to start"
            )
        })?;
        hosts.insert(normalize_host(&host));
    }
    Ok(hosts)
}

/// Every `${REF}` this project actually references, with library-backed
/// servers resolved.
///
/// [`Manifest::referenced_secrets`] reads the inline `[servers]` table, which
/// is all `core` can see — it has no library. That was right while every server
/// was inline; since the import writes definitions to the library and
/// references them by name, the inline reading reports "no secrets referenced"
/// for a project whose lifted `${TOKEN}` the same run just wrote to `.env`.
///
/// **Display surfaces only.** This is what `status`, `doctor`, and the setup
/// wizard should count. It is deliberately not wired into the enforcement paths
/// (`grant`, `locked`, the render-time scoping check): those already hold a
/// *resolved* server and ask it directly, which is the stronger reading — they
/// must never depend on a manifest-shaped guess.
///
/// A name that fails to resolve is skipped rather than fatal: a broken
/// reference is `doctor`'s finding to report, not a reason for a status line to
/// fail.
pub fn effective_referenced_secrets(
    manifest: &Manifest,
    library: &crate::library::Library,
    lib_home: &Path,
) -> Vec<String> {
    let mut refs: Vec<String> = Vec::new();
    let push = |r: String, refs: &mut Vec<String>| {
        if !refs.contains(&r) {
            refs.push(r);
        }
    };
    for name in runtime_server_names(manifest, None) {
        let Ok(resolved) = resolve_server(manifest, library, lib_home, &name) else {
            continue;
        };
        for r in resolved.server.referenced_secrets() {
            push(r, &mut refs);
        }
    }
    // Hooks are inline by definition, so the manifest's own answer already
    // covers them; unioning it in also keeps this a superset of the old
    // behaviour, which is what makes it safe to swap in at a display site.
    for r in manifest.referenced_secrets() {
        push(r, &mut refs);
    }
    refs.sort();
    refs
}

/// The names of [`effective_runtime_servers`] without resolving them — for
/// surfaces (doctor, say) that only need to know whether a runtime surface is
/// declared, without touching the library on disk.
pub fn runtime_server_names(manifest: &Manifest, profile: Option<&str>) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut push = |n: &str, names: &mut Vec<String>| {
        if seen.insert(n.to_string()) {
            names.push(n.to_string());
        }
    };
    match profile.and_then(|p| manifest.profiles.get(p)) {
        Some(p) => {
            for n in &p.servers {
                push(n, &mut names);
            }
        }
        // Delegated rather than repeated: the render selection, the counters,
        // and this runtime surface must name the same set, and they drifted
        // once already — `apply` read `[servers]` alone while this function
        // read the union, so a library-first manifest served six servers and
        // rendered none.
        None => {
            for n in manifest.declared_server_names() {
                push(&n, &mut names);
            }
        }
    }
    names
}

// TODO(phase-1): shim — migrate callers to agentstack_core::digest and drop.
pub(crate) use agentstack_core::digest::sha256_hex;

/// How a server's currently-resolved **definition** compares to its
/// `agentstack.lock` pin. No rev/offline variants — servers are local
/// definitions, not fetched sources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerLockStatus {
    /// The resolved definition digest matches the locked one.
    Matches,
    /// The server resolved but has no entry in the lockfile yet.
    MissingLockEntry,
    /// The resolved definition digest differs from the locked one.
    ChecksumDrift { locked: String, current: String },
    /// The reference could not be resolved (unknown, or a broken/missing
    /// library definition file).
    ResolveFailed { error: String },
}

/// A neutral, render-agnostic lock/drift status for one server. `doctor` maps it
/// to severity; `explain` renders it as origin/provenance/lock detail.
#[derive(Debug, Clone)]
pub struct ServerLockReport {
    pub name: String,
    /// `None` when resolution failed.
    pub origin: Option<ServerOrigin>,
    pub provenance: Option<String>,
    pub status: ServerLockStatus,
}

/// Resolve one server by name and compare its definition digest to the lockfile.
/// Inline-first, then central library — the same order as activation.
pub fn server_lock_status(
    name: &str,
    manifest: &Manifest,
    library: &Library,
    lib_home: &Path,
    lock: &Lock,
) -> ServerLockReport {
    match resolve_server(manifest, library, lib_home, name) {
        Err(e) => ServerLockReport {
            name: name.to_string(),
            origin: None,
            provenance: None,
            status: ServerLockStatus::ResolveFailed {
                error: e.to_string(),
            },
        },
        Ok(resolved) => ServerLockReport {
            name: name.to_string(),
            origin: Some(resolved.origin),
            provenance: resolved.provenance.clone(),
            status: classify_server(name, &resolved.checksum, lock),
        },
    }
}

/// How an instruction fragment's current file bytes compare to its
/// `agentstack.lock` pin. A strict subset of [`SkillLockStatus`]: instructions
/// are always single local files, so the git-only variants don't apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstructionLockStatus {
    /// Current bytes match the locked checksum.
    Matches,
    /// The fragment has no entry in the lockfile yet.
    MissingLockEntry,
    /// Current bytes differ from the locked checksum.
    ChecksumDrift { locked: String, current: String },
    /// The fragment file could not be read (missing/unreadable).
    ResolveFailed { error: String },
}

/// Compare an **already-computed** instruction checksum to its lockfile pin.
/// Pure — same contract as [`classify_skill`]/[`classify_server`].
pub fn classify_instruction(
    name: &str,
    current_checksum: &str,
    lock: &Lock,
) -> InstructionLockStatus {
    match lock.get_instruction(name) {
        None => InstructionLockStatus::MissingLockEntry,
        Some(entry) if entry.checksum.hex() != current_checksum => {
            InstructionLockStatus::ChecksumDrift {
                locked: entry.checksum.hex().to_string(),
                current: current_checksum.to_string(),
            }
        }
        Some(_) => InstructionLockStatus::Matches,
    }
}

/// Read one instruction fragment's bytes and compare them to the lock pin.
/// The path anchors exactly like compilation does
/// ([`crate::render::instructions::fragment_source`]) so verification and the
/// compiler always read the same file. Machine-layer fragments
/// (`from_user_layer`) are the caller's job to filter — they are never pinned.
pub fn instruction_lock_status(
    name: &str,
    instr: &crate::manifest::Instruction,
    manifest_dir: &Path,
    lock: &Lock,
) -> InstructionLockStatus {
    instruction_lock_status_with(
        name,
        instr,
        manifest_dir,
        lock,
        &crate::library::Library::default(),
    )
}

/// [`instruction_lock_status`] with the linked library view in hand — needed to
/// resolve a **sourceless** fragment's bodies, and to see its variants.
///
/// Every declared body is compared, base and variants alike: a variant nothing
/// currently selects is still content the consent digest covers, so its bytes
/// moving must read as drift rather than pass unnoticed until the day some
/// toolset selects it. The first mismatch wins — one drifted body is enough to
/// mark the fragment changed, and naming more than one would not change the
/// single fix (`agentstack lock`).
pub fn instruction_lock_status_with(
    name: &str,
    instr: &crate::manifest::Instruction,
    manifest_dir: &Path,
    lock: &Lock,
    library: &crate::library::Library,
) -> InstructionLockStatus {
    let bodies = match crate::instructions::bodies(name, instr, manifest_dir, library) {
        Ok(b) => b,
        Err(e) => {
            return InstructionLockStatus::ResolveFailed {
                error: format!("{e:#}"),
            };
        }
    };

    let entry = lock.get_instruction(name);
    for (variant, declared, src) in bodies.all() {
        let bytes = match std::fs::read(&src) {
            Ok(b) => b,
            Err(e) => {
                return InstructionLockStatus::ResolveFailed {
                    error: format!("reading {}: {e}", src.display()),
                };
            }
        };
        let current = agentstack_core::digest::sha256_hex(&bytes);
        match variant {
            None => match entry {
                None => return InstructionLockStatus::MissingLockEntry,
                Some(e) if e.checksum.hex() != current => {
                    return InstructionLockStatus::ChecksumDrift {
                        locked: e.checksum.hex().to_string(),
                        current,
                    };
                }
                Some(_) => {}
            },
            Some(v) => {
                // A variant with no pin is the same state an unpinned fragment
                // is in — `agentstack lock` is the fix, and the caller's
                // existing MissingLockEntry handling already says so.
                let Some(pin) = entry.and_then(|e| {
                    e.variants
                        .iter()
                        .find(|p| p.cli == v.cli && p.model == v.model && p.path == declared)
                }) else {
                    return InstructionLockStatus::MissingLockEntry;
                };
                if pin.checksum.hex() != current {
                    return InstructionLockStatus::ChecksumDrift {
                        locked: pin.checksum.hex().to_string(),
                        current,
                    };
                }
            }
        }
    }
    InstructionLockStatus::Matches
}

/// Where a resolved native extension came from — mirrors [`SkillOrigin`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionOrigin {
    /// The manifest `[extensions.<name>]` entry declares its own source
    /// (`path` or `git`).
    Inline,
    /// The manifest entry is sourceless and the body resolves from the central
    /// library (`[[extension]]` in `library.toml`).
    Library,
}

/// A native extension resolved to a concrete source plus the strict
/// integrity-root digest of its content — the E3 analogue of [`ResolvedSkill`].
#[derive(Debug, Clone)]
pub struct ResolvedExtension {
    pub name: String,
    pub origin: ExtensionOrigin,
    /// The one adapter this extension targets (the manifest entry's `target`).
    pub target: String,
    /// `"path"` or `"git"`.
    pub source_kind: &'static str,
    /// The declared path a `path` source pinned (lock provenance).
    pub path: Option<String>,
    /// The git URL a `git` source pinned (lock provenance).
    pub git: Option<String>,
    /// Resolved git commit (git sources only).
    pub rev: Option<String>,
    /// Strict integrity-root digest of the content (never the lenient skill
    /// digest — extensions are executable code).
    pub checksum: String,
    /// Library provenance, when the extension resolved from the central library.
    pub provenance: Option<String>,
    /// The exact `(root, declared)` pair the digest pinned — a copy-render
    /// must deliver from this same anchor (manifest dir, git checkout root, or
    /// the library body dir) so the copied bytes are the pinned bytes.
    pub anchor: std::path::PathBuf,
    /// The declared path under `anchor` the digest walked.
    pub declared: String,
}

/// A structured extension-resolution failure — mirrors [`ResolveError`].
#[derive(Debug, thiserror::Error)]
pub enum ExtensionResolveError {
    #[error("extension '{name}' resolves to no source (no inline `path`/`git`, and not in the central library)")]
    Unresolved { name: String },
    #[error("extension '{name}' (git {url}) is not available offline — run `agentstack install`")]
    NotAvailableOffline { name: String, url: String },
    #[error(transparent)]
    Source(#[from] anyhow::Error),
}

/// Resolve one extension entry to its source location + strict content digest,
/// inline-first then central library (the same order as skills/servers):
///
/// 1. the manifest entry's own `path` (Inline, anchored at `manifest_dir`);
/// 2. the manifest entry's own `git` (Inline, fetched/cached through the store);
/// 3. a sourceless entry falls to the central library's `[[extension]]` body
///    (Library — a `path` under `<lib_home>/extensions/`, or a git source).
///
/// Git sources are digested at their `subpath` anchored at the **checkout
/// root** with the strict [`agentstack_core::digest::integrity_root_digest`] —
/// the checkout lives outside the manifest dir, and a checkout's `.git` can
/// never be part of a reproducible pin, so a git extension MUST declare a
/// `subpath` pointing at its own directory. Symlinks are rejected exactly as
/// the digest already rejects them; the digest is never weakened to the skill
/// digest.
pub fn resolve_extension_entry(
    name: &str,
    ext: &crate::manifest::Extension,
    manifest_dir: &Path,
    library: &Library,
    lib_home: &Path,
    store: &Store,
    mode: ResolveMode,
) -> Result<ResolvedExtension, ExtensionResolveError> {
    // 1. Inline path source.
    if let Some(path) = ext.path.as_deref() {
        let checksum = agentstack_core::digest::integrity_root_digest(manifest_dir, path)
            .map_err(ExtensionResolveError::Source)?
            .hex()
            .to_string();
        return Ok(ResolvedExtension {
            name: name.to_string(),
            origin: ExtensionOrigin::Inline,
            target: ext.target.clone(),
            source_kind: "path",
            path: Some(path.to_string()),
            git: None,
            rev: None,
            checksum,
            provenance: None,
            anchor: manifest_dir.to_path_buf(),
            declared: path.to_string(),
        });
    }
    // 2. Inline git source.
    if let Some(url) = ext.git.as_deref() {
        let (checksum, rev, anchor, declared) = resolve_git_extension(
            store,
            name,
            url,
            ext.rev.as_deref(),
            ext.subpath.as_deref(),
            mode,
        )?;
        return Ok(ResolvedExtension {
            name: name.to_string(),
            origin: ExtensionOrigin::Inline,
            target: ext.target.clone(),
            source_kind: "git",
            path: None,
            git: Some(url.to_string()),
            rev,
            checksum,
            provenance: None,
            anchor,
            declared,
        });
    }
    // 3. Central library (sourceless manifest entry).
    let Some(entry) = library.get_extension(name) else {
        return Err(ExtensionResolveError::Unresolved {
            name: name.to_string(),
        });
    };
    if let Some(path) = entry.path.as_deref() {
        let anchor = lib_home.join("extensions");
        let checksum = agentstack_core::digest::integrity_root_digest(&anchor, path)
            .map_err(ExtensionResolveError::Source)?
            .hex()
            .to_string();
        return Ok(ResolvedExtension {
            name: name.to_string(),
            origin: ExtensionOrigin::Library,
            target: ext.target.clone(),
            source_kind: "path",
            path: Some(path.to_string()),
            git: None,
            rev: None,
            checksum,
            provenance: entry.provenance.clone(),
            anchor,
            declared: path.to_string(),
        });
    }
    if let Some(url) = entry.git.as_deref() {
        let (checksum, rev, anchor, declared) = resolve_git_extension(
            store,
            name,
            url,
            entry.rev.as_deref(),
            entry.subpath.as_deref(),
            mode,
        )?;
        return Ok(ResolvedExtension {
            name: name.to_string(),
            origin: ExtensionOrigin::Library,
            target: ext.target.clone(),
            source_kind: "git",
            path: None,
            git: Some(url.to_string()),
            rev,
            checksum,
            provenance: entry.provenance.clone(),
            anchor,
            declared,
        });
    }
    Err(ExtensionResolveError::Source(anyhow::anyhow!(
        "library extension '{name}' has neither a `path` nor a `git` source"
    )))
}

/// Fetch (or locate offline) a git extension source and digest its `subpath`
/// directory at the checkout root with the strict integrity-root digest.
/// Returns `(checksum, resolved_rev, checkout_root, subpath)` — the last two
/// are the digest's exact anchor pair, so a copy-render delivers the pinned
/// bytes.
fn resolve_git_extension(
    store: &Store,
    name: &str,
    url: &str,
    rev: Option<&str>,
    subpath: Option<&str>,
    mode: ResolveMode,
) -> Result<(String, Option<String>, std::path::PathBuf, String), ExtensionResolveError> {
    // A git extension is always digested at a subpath: anchoring at the checkout
    // root and declaring the subpath keeps the clone's own `.git` (a sibling of
    // the subpath, not under it) out of the pin, so it stays reproducible.
    let Some(sub) = subpath.map(str::trim).filter(|s| !s.is_empty()) else {
        return Err(ExtensionResolveError::Source(anyhow::anyhow!(
            "git-source extension '{name}' requires a `subpath` pointing at the extension's \
             directory — a checkout's `.git` cannot be part of a reproducible extension pin"
        )));
    };
    let (clone_root, resolved_rev) = match mode {
        ResolveMode::Fetch => {
            let (root, head) =
                crate::store::checkout(store, url, rev).map_err(ExtensionResolveError::Source)?;
            (root, Some(head))
        }
        // NoFetch and PathOnly never touch the network; an un-cached clone is
        // reported offline. (PathOnly still digests here — an extension pin is
        // only meaningful with its content digest, unlike a skill listing.)
        ResolveMode::NoFetch | ResolveMode::PathOnly => match store.local_git_clone(url) {
            Some((root, head)) => (root, head),
            None => {
                return Err(ExtensionResolveError::NotAvailableOffline {
                    name: name.to_string(),
                    url: url.to_string(),
                })
            }
        },
    };
    let checksum = agentstack_core::digest::integrity_root_digest(&clone_root, sub)
        .map_err(ExtensionResolveError::Source)?
        .hex()
        .to_string();
    Ok((checksum, resolved_rev, clone_root, sub.to_string()))
}

/// How a native extension's current source bytes compare to its
/// `agentstack.lock` pin (D6). Mirrors [`SkillLockStatus`] (git rev-drift +
/// offline) plus a target-drift case: the pin records which adapter the
/// reviewed code was destined for, so a retargeted extension must re-lock even
/// when its bytes are unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionLockStatus {
    /// Current digest, target, and rev all match the locked pin.
    Matches,
    /// The extension has no entry in the lockfile yet.
    MissingLockEntry,
    /// Current source digest differs from the locked checksum.
    ChecksumDrift { locked: String, current: String },
    /// The manifest's `target` differs from the one the pin was reviewed for.
    TargetDrift { locked: String, current: String },
    /// Git rev differs from the locked rev (both sides carry one).
    RevDrift { locked: String, current: String },
    /// A git-backed source that is not cached locally, checked under `NoFetch`
    /// (offline) — reproducibility just can't be verified offline. Not a failure.
    NotAvailableOffline { source: String },
    /// The source could not be digested (missing, symlink, traversal, broken
    /// library ref — anything the strict digest or the resolver refuses). Never
    /// proceed.
    ResolveFailed { error: String },
}

/// A neutral, render-agnostic lock/drift status for one extension — mirrors
/// [`SkillLockReport`]. `origin` is `None` only when resolution failed.
#[derive(Debug, Clone)]
pub struct ExtensionLockReport {
    pub name: String,
    pub origin: Option<ExtensionOrigin>,
    pub provenance: Option<String>,
    pub status: ExtensionLockStatus,
}

/// Resolve one extension (inline-first, then central library) and compare its
/// strict content digest + target + rev to the lockfile pin. `mode` controls
/// whether git sources may be fetched; read commands pass `NoFetch` so an
/// un-cached git source surfaces as [`ExtensionLockStatus::NotAvailableOffline`]
/// rather than a failure — the same offline handling as skills.
// Mirrors `skill_lock_status`'s parameter cluster.
#[allow(clippy::too_many_arguments)]
pub fn extension_lock_status(
    name: &str,
    ext: &crate::manifest::Extension,
    manifest_dir: &Path,
    library: &Library,
    lib_home: &Path,
    store: &Store,
    lock: &Lock,
    mode: ResolveMode,
) -> ExtensionLockReport {
    match resolve_extension_entry(name, ext, manifest_dir, library, lib_home, store, mode) {
        Err(ExtensionResolveError::NotAvailableOffline { url, .. }) => ExtensionLockReport {
            name: name.to_string(),
            origin: None,
            provenance: None,
            status: ExtensionLockStatus::NotAvailableOffline { source: url },
        },
        Err(e) => ExtensionLockReport {
            name: name.to_string(),
            origin: None,
            provenance: None,
            status: ExtensionLockStatus::ResolveFailed {
                error: format!("{e:#}"),
            },
        },
        Ok(resolved) => ExtensionLockReport {
            name: name.to_string(),
            origin: Some(resolved.origin),
            provenance: resolved.provenance.clone(),
            status: classify_extension(
                name,
                &resolved.checksum,
                &resolved.target,
                resolved.rev.as_deref(),
                lock,
            ),
        },
    }
}

/// Compare an **already-resolved** extension (content checksum + target +
/// optional git rev) to its lockfile pin. Pure — no filesystem, no
/// re-resolution. Checksum drift takes precedence over target drift, which
/// takes precedence over rev drift.
pub fn classify_extension(
    name: &str,
    current_checksum: &str,
    current_target: &str,
    current_rev: Option<&str>,
    lock: &Lock,
) -> ExtensionLockStatus {
    match lock.get_extension(name) {
        None => ExtensionLockStatus::MissingLockEntry,
        Some(entry) if entry.checksum != current_checksum => ExtensionLockStatus::ChecksumDrift {
            locked: entry.checksum.clone(),
            current: current_checksum.to_string(),
        },
        Some(entry) if entry.target != current_target => ExtensionLockStatus::TargetDrift {
            locked: entry.target.clone(),
            current: current_target.to_string(),
        },
        Some(entry) => match (entry.rev.as_deref(), current_rev) {
            (Some(l), Some(c)) if l != c => ExtensionLockStatus::RevDrift {
                locked: l.to_string(),
                current: c.to_string(),
            },
            _ => ExtensionLockStatus::Matches,
        },
    }
}

/// A governed workflow resolved to a concrete source plus the strict
/// integrity-root digest of its content — the D7 analogue of
/// [`ResolvedExtension`], minus the library origin: workflow sources are
/// inline-only in W1 (`path` or `git`); the central-library kind is W4.
#[derive(Debug, Clone)]
pub struct ResolvedWorkflow {
    pub name: String,
    /// The declared roles, canonicalized sorted-unique — the form the lock
    /// pin stores and every drift comparison uses.
    pub roles: Vec<String>,
    /// `"path"` or `"git"`.
    pub source_kind: &'static str,
    /// The declared path a `path` source pinned (lock provenance).
    pub path: Option<String>,
    /// The git URL a `git` source pinned (lock provenance).
    pub git: Option<String>,
    /// Resolved git commit (git sources only).
    pub rev: Option<String>,
    /// Strict integrity-root digest of the content (never the lenient skill
    /// digest — workflow source is executable code).
    pub checksum: String,
    /// The exact `(root, declared)` pair the digest pinned, so anything that
    /// later reads the script reads the pinned bytes from the same anchor.
    pub anchor: std::path::PathBuf,
    /// The declared path under `anchor` the digest walked.
    pub declared: String,
}

/// A structured workflow-resolution failure — mirrors [`ExtensionResolveError`].
#[derive(Debug, thiserror::Error)]
pub enum WorkflowResolveError {
    #[error("workflow '{name}' declares no source — a `[workflows.*]` entry needs an inline `path` or `git` source (the central-library workflow kind is not implemented yet)")]
    Sourceless { name: String },
    #[error("workflow '{name}' (git {url}) is not available offline — run `agentstack install`")]
    NotAvailableOffline { name: String, url: String },
    #[error(transparent)]
    Source(#[from] anyhow::Error),
}

/// Resolve one workflow entry to its source location + strict content digest.
/// Inline sources only (W1): the manifest entry's own `path` (anchored at
/// `manifest_dir`) or its own `git` (fetched/cached through the store). A
/// sourceless entry is an error — there is deliberately NO central-library
/// fallback here; that is the W4 `kind: workflow`, and resolution must not
/// invent it early.
///
/// Git sources follow [`resolve_extension_entry`]'s rules exactly: digested at
/// their `subpath` anchored at the **checkout root** with the strict
/// [`agentstack_core::digest::integrity_root_digest`] (a checkout's `.git` can
/// never be part of a reproducible pin, so `subpath` is required; symlinks are
/// rejected by the digest itself).
pub fn resolve_workflow_entry(
    name: &str,
    wf: &crate::manifest::Workflow,
    manifest_dir: &Path,
    store: &Store,
    mode: ResolveMode,
) -> Result<ResolvedWorkflow, WorkflowResolveError> {
    let roles = wf.roles_sorted_unique();
    if let Some(path) = wf.path.as_deref() {
        let checksum = agentstack_core::digest::integrity_root_digest(manifest_dir, path)
            .map_err(WorkflowResolveError::Source)?
            .hex()
            .to_string();
        return Ok(ResolvedWorkflow {
            name: name.to_string(),
            roles,
            source_kind: "path",
            path: Some(path.to_string()),
            git: None,
            rev: None,
            checksum,
            anchor: manifest_dir.to_path_buf(),
            declared: path.to_string(),
        });
    }
    if let Some(url) = wf.git.as_deref() {
        // A git workflow is always digested at a subpath: anchoring at the
        // checkout root and declaring the subpath keeps the clone's own `.git`
        // (a sibling of the subpath, not under it) out of the pin.
        let Some(sub) = wf
            .subpath
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            return Err(WorkflowResolveError::Source(anyhow::anyhow!(
                "git-source workflow '{name}' requires a `subpath` pointing at the workflow's \
                 directory — a checkout's `.git` cannot be part of a reproducible workflow pin"
            )));
        };
        let (clone_root, resolved_rev) = match mode {
            ResolveMode::Fetch => {
                let (root, head) = crate::store::checkout(store, url, wf.rev.as_deref())
                    .map_err(WorkflowResolveError::Source)?;
                (root, Some(head))
            }
            // NoFetch and PathOnly never touch the network; an un-cached clone
            // is reported offline — same as extensions.
            ResolveMode::NoFetch | ResolveMode::PathOnly => match store.local_git_clone(url) {
                Some((root, head)) => (root, head),
                None => {
                    return Err(WorkflowResolveError::NotAvailableOffline {
                        name: name.to_string(),
                        url: url.to_string(),
                    })
                }
            },
        };
        let checksum = agentstack_core::digest::integrity_root_digest(&clone_root, sub)
            .map_err(WorkflowResolveError::Source)?
            .hex()
            .to_string();
        return Ok(ResolvedWorkflow {
            name: name.to_string(),
            roles,
            source_kind: "git",
            path: None,
            git: Some(url.to_string()),
            rev: resolved_rev,
            checksum,
            anchor: clone_root,
            declared: sub.to_string(),
        });
    }
    Err(WorkflowResolveError::Sourceless {
        name: name.to_string(),
    })
}

/// How a workflow's current source + declared roles compare to its
/// `agentstack.lock` pin (D7 W1). Mirrors [`ExtensionLockStatus`], with
/// `RolesDrift` in the place of `TargetDrift`: the pin records which role
/// profiles the reviewed script may spawn under, so widening (or otherwise
/// changing) `roles` must re-lock even when the bytes are unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowLockStatus {
    /// Current digest, roles, and rev all match the locked pin.
    Matches,
    /// The workflow has no entry in the lockfile yet.
    MissingLockEntry,
    /// Current source digest differs from the locked checksum.
    ChecksumDrift { locked: String, current: String },
    /// The manifest's role set differs from the one the pin was reviewed for
    /// (compared sorted-unique, so declaration order can't fake or mask it).
    RolesDrift {
        locked: Vec<String>,
        current: Vec<String>,
    },
    /// Git rev differs from the locked rev (both sides carry one).
    RevDrift { locked: String, current: String },
    /// A git-backed source that is not cached locally, checked under `NoFetch`
    /// (offline) — reproducibility just can't be verified offline. Not a
    /// failure for read paths; admission still requires a verified Matches.
    NotAvailableOffline { source: String },
    /// The source could not be digested (missing, symlink, sourceless —
    /// anything the strict digest or the resolver refuses). Never proceed.
    ResolveFailed { error: String },
}

/// Resolve one workflow and compare its strict content digest + roles + rev to
/// the lockfile pin. `mode` controls whether git sources may be fetched; read
/// commands pass `NoFetch` so an un-cached git source surfaces as
/// [`WorkflowLockStatus::NotAvailableOffline`] rather than a failure.
pub fn workflow_lock_status(
    name: &str,
    wf: &crate::manifest::Workflow,
    manifest_dir: &Path,
    store: &Store,
    lock: &Lock,
    mode: ResolveMode,
) -> WorkflowLockStatus {
    match resolve_workflow_entry(name, wf, manifest_dir, store, mode) {
        Err(WorkflowResolveError::NotAvailableOffline { url, .. }) => {
            WorkflowLockStatus::NotAvailableOffline { source: url }
        }
        Err(e) => WorkflowLockStatus::ResolveFailed {
            error: format!("{e:#}"),
        },
        Ok(resolved) => classify_workflow(
            name,
            &resolved.checksum,
            &resolved.roles,
            resolved.rev.as_deref(),
            lock,
        ),
    }
}

/// Compare an **already-resolved** workflow (content checksum + sorted-unique
/// roles + optional git rev) to its lockfile pin. Pure — no filesystem, no
/// re-resolution. Checksum drift takes precedence over roles drift, which
/// takes precedence over rev drift (same ordering as [`classify_extension`]).
pub fn classify_workflow(
    name: &str,
    current_checksum: &str,
    current_roles: &[String],
    current_rev: Option<&str>,
    lock: &Lock,
) -> WorkflowLockStatus {
    match lock.get_workflow(name) {
        None => WorkflowLockStatus::MissingLockEntry,
        Some(entry) if entry.checksum.hex() != current_checksum => {
            WorkflowLockStatus::ChecksumDrift {
                locked: entry.checksum.hex().to_string(),
                current: current_checksum.to_string(),
            }
        }
        Some(entry) if entry.roles != current_roles => WorkflowLockStatus::RolesDrift {
            locked: entry.roles.clone(),
            current: current_roles.to_vec(),
        },
        Some(entry) => match (entry.rev.as_deref(), current_rev) {
            (Some(l), Some(c)) if l != c => WorkflowLockStatus::RevDrift {
                locked: l.to_string(),
                current: c.to_string(),
            },
            _ => WorkflowLockStatus::Matches,
        },
    }
}

/// Compare an **already-resolved** server definition digest to its lockfile
/// pin. Pure — no filesystem, no re-resolution — so use-time gates can verify
/// the exact resolved set they are about to act on (no re-resolve between
/// check and use).
pub fn classify_server(name: &str, current_checksum: &str, lock: &Lock) -> ServerLockStatus {
    match lock.get_server(name) {
        None => ServerLockStatus::MissingLockEntry,
        Some(entry) if entry.checksum.hex() != current_checksum => {
            ServerLockStatus::ChecksumDrift {
                locked: entry.checksum.hex().to_string(),
                current: current_checksum.to_string(),
            }
        }
        Some(_) => ServerLockStatus::Matches,
    }
}

/// Expand a profile's skill refs to active skill names, applying the same
/// wildcard rule as activation (`use_profile`): `"*"` means the manifest's inline
/// skills only — it does not pull in central-library skills.
pub fn active_skill_names(manifest: &Manifest, profile_name: &str) -> Vec<String> {
    match manifest.profiles.get(profile_name) {
        None => Vec::new(),
        Some(p) if p.loads_all_skills() => manifest.skills.keys().cloned().collect(),
        Some(p) => p.skills.clone(),
    }
}

/// How an active skill's currently-resolved content compares to its
/// `agentstack.lock` pin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillLockStatus {
    /// Resolved content matches the locked checksum (and rev, when applicable).
    Matches,
    /// The skill resolved but has no entry in the lockfile yet.
    MissingLockEntry,
    /// Resolved checksum differs from the locked checksum.
    ChecksumDrift { locked: String, current: String },
    /// Git rev differs from the locked rev (both sides carry one).
    RevDrift { locked: String, current: String },
    /// A git-backed source that is not cached locally, checked under `NoFetch`
    /// (offline). Not a failure — reproducibility just can't be verified offline.
    NotAvailableOffline { source: String },
    /// The reference could not be resolved (broken/missing source).
    ResolveFailed { error: String },
}

/// A neutral, render-agnostic lock/drift status for one skill. `doctor` maps it
/// to warning/error severity; `explain` renders it as provenance/detail.
#[derive(Debug, Clone)]
pub struct SkillLockReport {
    pub name: String,
    /// `None` when resolution failed.
    pub origin: Option<SkillOrigin>,
    /// Library provenance, when the skill resolved from the central library.
    pub provenance: Option<String>,
    pub status: SkillLockStatus,
}

/// Resolve one skill by name and compare it to its lockfile pin, through the
/// same resolution seam as activation ([`resolve_skill`]). Checksum drift takes
/// precedence over rev drift. `mode` controls whether git sources may be fetched;
/// read commands pass `NoFetch` so an un-cached git source surfaces as
/// [`SkillLockStatus::NotAvailableOffline`] rather than a failure.
// Mirrors `resolve_skill`'s parameter cluster plus lock + mode; a shared
// resolve-context struct is a worthwhile follow-up but out of scope here.
#[allow(clippy::too_many_arguments)]
pub fn skill_lock_status(
    name: &str,
    manifest: &Manifest,
    manifest_dir: &Path,
    library: &Library,
    lib_home: &Path,
    store: &Store,
    lock: &Lock,
    mode: ResolveMode,
) -> SkillLockReport {
    // Offline readers reproduce the lock's intended commit rather than the
    // mutable clone checkout. Fetch-mode status checks intentionally resolve
    // the manifest source itself: their job is to detect that it has moved
    // away from the lock, not to force it back to the locked revision.
    let pinned_rev = match mode {
        ResolveMode::Fetch => None,
        ResolveMode::NoFetch | ResolveMode::PathOnly => lock
            .get(crate::sources::capability_name(name))
            .and_then(|entry| entry.rev.as_deref()),
    };
    match resolve_skill_with_pin(
        manifest,
        manifest_dir,
        library,
        lib_home,
        store,
        name,
        mode,
        pinned_rev,
    ) {
        Err(ResolveError::NotAvailableOffline { url, .. }) => SkillLockReport {
            name: name.to_string(),
            origin: None,
            provenance: None,
            status: SkillLockStatus::NotAvailableOffline { source: url },
        },
        Err(e) => SkillLockReport {
            name: name.to_string(),
            origin: None,
            provenance: None,
            status: SkillLockStatus::ResolveFailed {
                error: e.to_string(),
            },
        },
        Ok(resolved) => SkillLockReport {
            name: name.to_string(),
            origin: Some(resolved.origin),
            provenance: resolved.provenance.clone(),
            status: classify_skill(name, &resolved.checksum, resolved.rev.as_deref(), lock),
        },
    }
}

/// Compare an **already-resolved** skill (content checksum + optional git rev)
/// to its lockfile pin. Pure — no filesystem, no re-resolution — so use-time
/// gates can verify the exact resolved set they are about to materialize.
/// Checksum drift takes precedence over rev drift.
pub fn classify_skill(
    name: &str,
    current_checksum: &str,
    current_rev: Option<&str>,
    lock: &Lock,
) -> SkillLockStatus {
    // The lock is keyed by the capability's own name; a source qualifier
    // (`central:x`, `lib:central/x`) is a SELECTOR, not part of the identity
    // (`crate::sources::capability_name`). Looking the pin up under the
    // reference as typed reported every qualified reference as unlocked, over a
    // lockfile that had pinned it correctly all along.
    match lock.get(crate::sources::capability_name(name)) {
        None => SkillLockStatus::MissingLockEntry,
        Some(entry) if entry.checksum.hex() != current_checksum => SkillLockStatus::ChecksumDrift {
            locked: entry.checksum.hex().to_string(),
            current: current_checksum.to_string(),
        },
        Some(entry) => match (entry.rev.as_deref(), current_rev) {
            (Some(l), Some(c)) if l != c => SkillLockStatus::RevDrift {
                locked: l.to_string(),
                current: c.to_string(),
            },
            _ => SkillLockStatus::Matches,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::LibrarySkill;
    use agentstack_core::digest::Sha256Hex;
    use assert_fs::prelude::*;

    fn server_from_toml(toml_src: &str) -> agentstack_core::manifest::Server {
        toml::from_str(toml_src).unwrap()
    }

    fn resolved_ok(name: &str, server: agentstack_core::manifest::Server) -> FrozenServer {
        (
            name.to_string(),
            Ok(ResolvedServer {
                name: name.to_string(),
                origin: ServerOrigin::Inline,
                server,
                checksum: String::new(),
                provenance: None,
            }),
        )
    }

    #[test]
    fn gateway_only_hosts_are_http_only_normalized_and_deduped() {
        let servers = vec![
            resolved_ok(
                "a",
                server_from_toml("type = \"http\"\nurl = \"https://MCP.Example.Com./mcp\"\n"),
            ),
            resolved_ok(
                "b",
                server_from_toml("type = \"http\"\nurl = \"https://mcp.example.com/other\"\n"),
            ),
            resolved_ok(
                "s",
                server_from_toml("type = \"stdio\"\ncommand = \"node\"\n"),
            ),
        ];
        let hosts = gateway_only_hosts(&servers).unwrap();
        // Case/trailing-dot normalized, and the two HTTP entries dedupe to one;
        // the stdio server contributes nothing.
        assert_eq!(
            hosts.into_iter().collect::<Vec<_>>(),
            vec!["mcp.example.com".to_string()]
        );
    }

    #[test]
    fn gateway_only_fails_closed_on_unclassifiable_or_unresolved_server() {
        // An HTTP server whose host is an unresolved ${REF} must fail the run.
        let unclassifiable = vec![resolved_ok(
            "x",
            server_from_toml("type = \"http\"\nurl = \"https://${HOST}/mcp\"\n"),
        )];
        assert!(gateway_only_hosts(&unclassifiable).is_err());

        // A selected server that can't resolve at all — or whose pin failed —
        // must also fail closed, never silently omitted.
        let broken: Vec<FrozenServer> =
            vec![("y".into(), Err("server 'y' is not defined".to_string()))];
        assert!(gateway_only_hosts(&broken).is_err());
    }

    #[test]
    fn frozen_runtime_servers_strict_fences_a_missing_profile() {
        // A pinned profile that does not exist must be a hard error — never a
        // silent broadening to every declared server.
        let manifest: Manifest = toml::from_str(
            "version = 1\n\
             [servers.a]\ntype = \"http\"\nurl = \"https://a/mcp\"\n\
             [profiles.real]\nservers = [\"a\"]\n",
        )
        .unwrap();
        let lib = Library::default();
        let home = assert_fs::TempDir::new().unwrap();
        let err = frozen_runtime_servers(
            &manifest,
            &lib,
            home.path(),
            home.path(),
            Some("does-not-exist"),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("never broaden"), "{err}");

        // The real profile resolves to exactly its one server.
        let frozen =
            frozen_runtime_servers(&manifest, &lib, home.path(), home.path(), Some("real"))
                .unwrap();
        let names: Vec<&str> = frozen.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["a"]);
    }

    /// A library home with one path-source skill body written under
    /// `lib/skills/<name>/`, plus an index entry pointing at it.
    fn library_with_skill(lib_home: &assert_fs::TempDir, name: &str, body: &str) -> Library {
        lib_home
            .child(format!("skills/{name}/SKILL.md"))
            .write_str(body)
            .unwrap();
        let mut lib = Library::default();
        lib.upsert(LibrarySkill {
            name: name.into(),
            source: "path".into(),
            path: Some(name.into()),
            git: None,
            rev: None,
            subpath: None,
            checksum: None,
            version: None,
            provenance: Some("consolidated".into()),
        });
        lib
    }

    fn empty_manifest() -> Manifest {
        toml::from_str("version = 1").unwrap()
    }

    #[test]
    fn classify_instruction_covers_every_arm() {
        let mut lock = Lock::default();
        lock.upsert_instruction(agentstack_core::lock::LockedInstruction {
            name: "house".into(),
            path: "./instructions/house.md".into(),
            checksum: Sha256Hex::of(b"aaaa"),
            variants: Vec::new(),
        });
        assert_eq!(
            classify_instruction("house", Sha256Hex::of(b"aaaa").hex(), &lock),
            InstructionLockStatus::Matches
        );
        assert_eq!(
            classify_instruction("house", Sha256Hex::of(b"bbbb").hex(), &lock),
            InstructionLockStatus::ChecksumDrift {
                locked: Sha256Hex::of(b"aaaa").hex().to_string(),
                current: Sha256Hex::of(b"bbbb").hex().to_string()
            }
        );
        assert_eq!(
            classify_instruction("style", "cccc", &lock),
            InstructionLockStatus::MissingLockEntry
        );
    }

    #[test]
    fn instruction_lock_status_digests_the_anchored_file() {
        let proj = assert_fs::TempDir::new().unwrap();
        proj.child("instructions/house.md")
            .write_str("be kind\n")
            .unwrap();
        let instr: crate::manifest::Instruction =
            toml::from_str("path = \"./instructions/house.md\"").unwrap();
        let checksum = Sha256Hex::of(b"be kind\n");
        let mut lock = Lock::default();
        lock.upsert_instruction(agentstack_core::lock::LockedInstruction {
            name: "house".into(),
            path: "./instructions/house.md".into(),
            checksum,
            variants: Vec::new(),
        });

        assert_eq!(
            instruction_lock_status("house", &instr, proj.path(), &lock),
            InstructionLockStatus::Matches
        );

        // A byte flips → drift.
        proj.child("instructions/house.md")
            .write_str("be evil\n")
            .unwrap();
        assert!(matches!(
            instruction_lock_status("house", &instr, proj.path(), &lock),
            InstructionLockStatus::ChecksumDrift { .. }
        ));

        // The file vanishes → broken, never silently unpinned.
        std::fs::remove_file(proj.child("instructions/house.md").path()).unwrap();
        assert!(matches!(
            instruction_lock_status("house", &instr, proj.path(), &lock),
            InstructionLockStatus::ResolveFailed { .. }
        ));
    }

    fn manifest_with_inline_skill(dir: &assert_fs::TempDir, name: &str, body: &str) -> Manifest {
        dir.child(format!("skills/{name}/SKILL.md"))
            .write_str(body)
            .unwrap();
        let toml = format!("version = 1\n[skills.{name}]\npath = \"./skills/{name}\"\n");
        toml::from_str(&toml).unwrap()
    }

    #[test]
    fn inline_wins_over_library() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());

        // Same name defined in both places, with different content.
        let manifest = manifest_with_inline_skill(&proj, "review", "# inline\n");
        let library = library_with_skill(&lib_home, "review", "# library\n");

        let r = resolve_skill(
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            "review",
            ResolveMode::Fetch,
        )
        .unwrap();

        assert_eq!(r.origin, SkillOrigin::Inline);
        assert_eq!(r.provenance, None);
        let contents = std::fs::read_to_string(r.path.join("SKILL.md")).unwrap();
        assert_eq!(contents, "# inline\n");
    }

    #[test]
    fn resolves_from_library_when_not_inline() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());

        let manifest = empty_manifest();
        let library = library_with_skill(&lib_home, "sql-review", "# from library\n");

        let r = resolve_skill(
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            "sql-review",
            ResolveMode::Fetch,
        )
        .unwrap();

        assert_eq!(r.origin, SkillOrigin::Library);
        assert_eq!(r.source_kind, "path");
        assert_eq!(r.provenance.as_deref(), Some("consolidated"));
        assert!(r.path.join("SKILL.md").exists());
    }

    #[test]
    fn inline_no_source_shadowing_library_teaches_the_fix() {
        // P19: `[skills.greet]` with no `path`/`git`, and a library skill named
        // `greet` — the resolver names the library copy and the exact fix rather
        // than the low-level "neither `path` nor `git`" source error.
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());

        // An inline block that declares the name but no source.
        let manifest: Manifest = toml::from_str("version = 1\n[skills.greet]\n").unwrap();
        let library = library_with_skill(&lib_home, "greet", "# from library\n");

        let err = resolve_skill(
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            "greet",
            ResolveMode::Fetch,
        )
        .unwrap_err();

        match &err {
            ResolveError::InlineNoSourceShadowsLibrary { name } => assert_eq!(name, "greet"),
            other => panic!("expected InlineNoSourceShadowsLibrary, got {other:?}"),
        }
        let msg = err.to_string();
        assert!(msg.contains("central library"), "names the model: {msg}");
        assert!(
            msg.contains("drop the `[skills.greet]` block"),
            "shows the fix: {msg}"
        );
        assert!(
            msg.contains("skills = [...]"),
            "points at the by-name form: {msg}"
        );
    }

    #[test]
    fn inline_no_source_without_library_still_errors_on_source() {
        // Same empty inline block, but no library skill of that name: the P19
        // teaching path must not fire — the plain source error stands.
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());

        let manifest: Manifest = toml::from_str("version = 1\n[skills.greet]\n").unwrap();
        let library = Library::default();

        let err = resolve_skill(
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            "greet",
            ResolveMode::Fetch,
        )
        .unwrap_err();
        assert!(
            !matches!(err, ResolveError::InlineNoSourceShadowsLibrary { .. }),
            "no library shadow → no P19 teaching error, got {err:?}"
        );
    }

    #[test]
    fn returns_checksum_for_resolved_skill() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());

        let manifest = empty_manifest();
        let library = library_with_skill(&lib_home, "x", "# x\n");

        let r = resolve_skill(
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            "x",
            ResolveMode::Fetch,
        )
        .unwrap();
        assert_eq!(r.checksum.len(), 64, "sha-256 hex digest expected");
    }

    #[test]
    fn unresolved_name_is_structured_error() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());

        let manifest = empty_manifest();
        let library = Library::default();

        let err = resolve_skill(
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            "nope",
            ResolveMode::Fetch,
        )
        .unwrap_err();

        match err {
            ResolveError::Unresolved { name } => assert_eq!(name, "nope"),
            other => panic!("expected Unresolved, got {other:?}"),
        }
    }

    // ---------- drift / lock-status helpers ----------

    use crate::lock::{Lock, LockedSkill};

    fn lock_with(entry: LockedSkill) -> Lock {
        let mut lock = Lock::default();
        lock.upsert(entry);
        lock
    }

    #[test]
    fn active_skill_names_wildcard_is_inline_only() {
        let proj = assert_fs::TempDir::new().unwrap();
        let manifest = manifest_with_inline_skill(&proj, "a", "# a\n");
        // Give the manifest a wildcard profile.
        let manifest: Manifest = {
            let mut m = manifest;
            let p: crate::manifest::Profile = toml::from_str("skills = [\"*\"]").unwrap();
            m.profiles.insert("p".into(), p);
            m
        };
        assert_eq!(active_skill_names(&manifest, "p"), vec!["a".to_string()]);
    }

    #[test]
    fn stable_digest_matches_lock() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());
        let manifest = empty_manifest();
        let library = library_with_skill(&lib_home, "sql-review", "# body\n");

        // Lock the current resolved digest.
        let resolved = resolve_skill(
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            "sql-review",
            ResolveMode::Fetch,
        )
        .unwrap();
        let lock = lock_with(LockedSkill {
            name: "sql-review".into(),
            source: crate::lock::SkillLockSource::Path,
            path: Some("sql-review".into()),
            git: None,
            rev: None,
            checksum: Sha256Hex::parse(&resolved.checksum).unwrap(),
            license: None,
            origin: None,
        });

        let report = skill_lock_status(
            "sql-review",
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            &lock,
            ResolveMode::Fetch,
        );
        assert_eq!(report.status, SkillLockStatus::Matches);
        assert_eq!(report.origin, Some(SkillOrigin::Library));
        assert_eq!(report.provenance.as_deref(), Some("consolidated"));
    }

    #[test]
    fn changed_central_skill_is_checksum_drift() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());
        let manifest = empty_manifest();
        let library = library_with_skill(&lib_home, "sql-review", "# original\n");

        // Lock a stale digest, then change the library content underneath it.
        let lock = lock_with(LockedSkill {
            name: "sql-review".into(),
            source: crate::lock::SkillLockSource::Path,
            path: Some("sql-review".into()),
            git: None,
            rev: None,
            checksum: Sha256Hex::of(b"staledigest"),
            license: None,
            origin: None,
        });
        lib_home
            .child("skills/sql-review/SKILL.md")
            .write_str("# changed\n")
            .unwrap();

        let report = skill_lock_status(
            "sql-review",
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            &lock,
            ResolveMode::Fetch,
        );
        match report.status {
            SkillLockStatus::ChecksumDrift { locked, .. } => {
                assert_eq!(locked, Sha256Hex::of(b"staledigest").hex());
            }
            other => panic!("expected ChecksumDrift, got {other:?}"),
        }
    }

    #[test]
    fn active_skill_without_lock_entry_reports_missing() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());
        let manifest = empty_manifest();
        let library = library_with_skill(&lib_home, "sql-review", "# body\n");

        let report = skill_lock_status(
            "sql-review",
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            &Lock::default(),
            ResolveMode::Fetch,
        );
        assert_eq!(report.status, SkillLockStatus::MissingLockEntry);
    }

    #[test]
    fn broken_library_ref_reports_resolve_failed() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());
        let manifest = empty_manifest();
        let mut library = Library::default();
        library.upsert(LibrarySkill {
            name: "sql-review".into(),
            source: "path".into(),
            path: None,
            git: None,
            rev: None,
            subpath: None,
            checksum: None,
            version: None,
            provenance: None,
        });

        let report = skill_lock_status(
            "sql-review",
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            &Lock::default(),
            ResolveMode::Fetch,
        );
        assert!(matches!(
            report.status,
            SkillLockStatus::ResolveFailed { .. }
        ));
        assert_eq!(report.origin, None);
    }

    #[test]
    fn inline_and_library_origins_are_distinguished() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());

        // Inline "review" and library-only "sql-review".
        let manifest = manifest_with_inline_skill(&proj, "review", "# inline\n");
        let library = library_with_skill(&lib_home, "sql-review", "# lib\n");

        let inline = skill_lock_status(
            "review",
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            &Lock::default(),
            ResolveMode::Fetch,
        );
        let lib = skill_lock_status(
            "sql-review",
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            &Lock::default(),
            ResolveMode::Fetch,
        );
        assert_eq!(inline.origin, Some(SkillOrigin::Inline));
        assert_eq!(inline.provenance, None);
        assert_eq!(lib.origin, Some(SkillOrigin::Library));
        assert_eq!(lib.provenance.as_deref(), Some("consolidated"));
    }

    #[test]
    fn git_rev_drift_is_reported() {
        // A local git repo used as a library git source.
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let repo = proj.child("repo");
        repo.create_dir_all().unwrap();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(repo.path())
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?} failed");
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@e.st"]);
        git(&["config", "user.name", "t"]);
        repo.child("SKILL.md").write_str("# git skill\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "init"]);

        let store = Store::with_root(proj.child("store").path().to_path_buf());
        let url = format!("file://{}", repo.path().display());
        let manifest = empty_manifest();
        let mut library = Library::default();
        library.upsert(LibrarySkill {
            name: "gitskill".into(),
            source: "git".into(),
            path: None,
            git: Some(url),
            rev: None,
            subpath: None,
            checksum: None,
            version: None,
            provenance: None,
        });

        // Resolve to learn the real checksum + HEAD rev, then lock the same
        // checksum but a different rev → rev drift (checksum still matches).
        let resolved = resolve_skill(
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            "gitskill",
            ResolveMode::Fetch,
        )
        .unwrap();
        let lock = lock_with(LockedSkill {
            name: "gitskill".into(),
            source: crate::lock::SkillLockSource::Git,
            path: None,
            git: resolved.rev.clone().map(|_| "url".into()),
            rev: Some("0000000000000000000000000000000000000000".into()),
            checksum: Sha256Hex::parse(&resolved.checksum).unwrap(),
            license: None,
            origin: None,
        });

        let report = skill_lock_status(
            "gitskill",
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            &lock,
            ResolveMode::Fetch,
        );
        match report.status {
            SkillLockStatus::RevDrift { locked, current } => {
                assert_eq!(locked, "0000000000000000000000000000000000000000");
                assert_eq!(Some(current), resolved.rev);
            }
            other => panic!("expected RevDrift, got {other:?}"),
        }
    }

    // ---------- NoFetch mode ----------

    /// A library entry pointing at a git URL that has never been cloned.
    fn uncached_git_library(url: &str) -> Library {
        let mut lib = Library::default();
        lib.upsert(LibrarySkill {
            name: "gitskill".into(),
            source: "git".into(),
            path: None,
            git: Some(url.into()),
            rev: None,
            subpath: None,
            checksum: None,
            version: None,
            provenance: None,
        });
        lib
    }

    #[test]
    fn nofetch_uncached_git_is_not_available_offline() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());
        let manifest = empty_manifest();
        let library = uncached_git_library("https://example.com/x.git");

        let err = resolve_skill(
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            "gitskill",
            ResolveMode::NoFetch,
        )
        .unwrap_err();

        match err {
            ResolveError::NotAvailableOffline { name, url } => {
                assert_eq!(name, "gitskill");
                assert_eq!(url, "https://example.com/x.git");
            }
            other => panic!("expected NotAvailableOffline, got {other:?}"),
        }
    }

    #[test]
    fn nofetch_path_skill_resolves_normally() {
        // Path/library-path sources never fetch, so NoFetch behaves like Fetch.
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());
        let manifest = empty_manifest();
        let library = library_with_skill(&lib_home, "sql-review", "# body\n");

        let r = resolve_skill(
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            "sql-review",
            ResolveMode::NoFetch,
        )
        .unwrap();
        assert_eq!(r.origin, SkillOrigin::Library);
        assert_eq!(r.checksum.len(), 64);
    }

    #[test]
    fn path_only_locates_without_digesting() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());
        let manifest = empty_manifest();
        let library = library_with_skill(&lib_home, "sql-review", "# body\n");

        let r = resolve_skill(
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            "sql-review",
            ResolveMode::PathOnly,
        )
        .unwrap();
        assert_eq!(r.origin, SkillOrigin::Library);
        assert!(r.path.join("SKILL.md").exists());
        assert!(r.checksum.is_empty(), "PathOnly must not digest");
    }

    #[cfg(unix)]
    #[test]
    fn path_only_never_reads_skill_contents() {
        use std::os::unix::fs::PermissionsExt;
        // An unreadable file inside the skill makes any digest pass fail —
        // PathOnly must still resolve because it never opens file contents.
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());
        let manifest = empty_manifest();
        let library = library_with_skill(&lib_home, "sql-review", "# body\n");
        let locked = lib_home.child("skills/sql-review/vendored.bin");
        locked.write_str("sealed").unwrap();
        std::fs::set_permissions(locked.path(), std::fs::Permissions::from_mode(0o000)).unwrap();

        let path_only = resolve_skill(
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            "sql-review",
            ResolveMode::PathOnly,
        );
        // Restore permissions before asserting so the TempDir can clean up.
        std::fs::set_permissions(locked.path(), std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(path_only.is_ok(), "PathOnly reads no file bodies");
    }

    #[test]
    fn path_only_uncached_git_is_not_available_offline() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());
        let manifest = empty_manifest();
        let library = uncached_git_library("https://example.com/x.git");

        let err = resolve_skill(
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            "gitskill",
            ResolveMode::PathOnly,
        )
        .unwrap_err();
        assert!(matches!(err, ResolveError::NotAvailableOffline { .. }));
    }

    #[test]
    fn skill_lock_status_reports_offline_for_uncached_git() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());
        let manifest = empty_manifest();
        let library = uncached_git_library("https://example.com/x.git");

        let report = skill_lock_status(
            "gitskill",
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            &Lock::default(),
            ResolveMode::NoFetch,
        );
        match report.status {
            SkillLockStatus::NotAvailableOffline { source } => {
                assert_eq!(source, "https://example.com/x.git");
            }
            other => panic!("expected NotAvailableOffline, got {other:?}"),
        }
    }

    // ---------- server resolution (Phase 1b) ----------

    use crate::library::LibraryServer;

    /// Manifest with one inline HTTP server at `url`, carrying a `${REF}` header.
    fn manifest_with_inline_server(name: &str, url: &str) -> Manifest {
        let toml = format!(
            "version = 1\n[servers.{name}]\ntype = \"http\"\nurl = \"{url}\"\n\
             headers = {{ Authorization = \"Bearer ${{TOKEN}}\" }}\n"
        );
        toml::from_str(&toml).unwrap()
    }

    /// Write a library server definition file and index it. Returns (library,
    /// file content).
    fn library_with_server(
        lib_home: &assert_fs::TempDir,
        name: &str,
        url: &str,
    ) -> (Library, String) {
        let content = format!(
            "type = \"http\"\nurl = \"{url}\"\n\n[headers]\nAuthorization = \"Bearer ${{TOKEN}}\"\n"
        );
        lib_home
            .child(format!("servers/{name}.toml"))
            .write_str(&content)
            .unwrap();
        let mut lib = Library::default();
        lib.upsert_server(LibraryServer {
            name: name.into(),
            checksum: None,
            version: None,
            provenance: Some("consolidated:codex".into()),
        });
        (lib, content)
    }

    #[test]
    fn inline_server_wins_over_library() {
        let lib_home = assert_fs::TempDir::new().unwrap();
        let manifest = manifest_with_inline_server("kibana", "https://inline/mcp");
        let (library, _) = library_with_server(&lib_home, "kibana", "https://central/mcp");

        let r = resolve_server(&manifest, &library, lib_home.path(), "kibana").unwrap();

        assert_eq!(r.origin, ServerOrigin::Inline);
        assert_eq!(r.provenance, None);
        assert_eq!(r.server.url.as_deref(), Some("https://inline/mcp"));
    }

    #[test]
    fn library_server_resolves_from_file() {
        let lib_home = assert_fs::TempDir::new().unwrap();
        let manifest = empty_manifest();
        let (library, _) = library_with_server(&lib_home, "kibana", "https://central/mcp");

        let r = resolve_server(&manifest, &library, lib_home.path(), "kibana").unwrap();

        assert_eq!(r.origin, ServerOrigin::Library);
        assert_eq!(r.server.url.as_deref(), Some("https://central/mcp"));
        assert_eq!(r.provenance.as_deref(), Some("consolidated:codex"));
    }

    #[test]
    fn server_ref_survives_unresolved_in_definition() {
        let lib_home = assert_fs::TempDir::new().unwrap();
        let manifest = empty_manifest();
        let (library, _) = library_with_server(&lib_home, "kibana", "https://central/mcp");

        let r = resolve_server(&manifest, &library, lib_home.path(), "kibana").unwrap();

        // The resolver never touches secrets — the ${REF} is returned verbatim.
        assert_eq!(
            r.server.headers.get("Authorization").map(String::as_str),
            Some("Bearer ${TOKEN}")
        );
    }

    #[test]
    fn unresolved_server_is_structured_error() {
        let lib_home = assert_fs::TempDir::new().unwrap();
        let manifest = empty_manifest();
        let library = Library::default();

        let err = resolve_server(&manifest, &library, lib_home.path(), "nope").unwrap_err();
        match err {
            ServerResolveError::Unresolved { name } => assert_eq!(name, "nope"),
            other => panic!("expected Unresolved, got {other:?}"),
        }
    }

    #[test]
    fn runtime_servers_union_includes_profile_library_refs() {
        let lib_home = assert_fs::TempDir::new().unwrap();
        let (library, _) = library_with_server(&lib_home, "kibana", "https://central/mcp");
        let manifest: Manifest = toml::from_str(
            "version = 1\n\
             [servers.alpha]\ntype = \"http\"\nurl = \"https://a\"\n\
             [profiles.solo]\nservers = [\"kibana\", \"alpha\"]\n",
        )
        .unwrap();

        // No active profile → inline servers plus profile-referenced library
        // names, deduped, inline-first.
        let all = effective_runtime_servers(&manifest, &library, lib_home.path(), None);
        let names: Vec<&str> = all.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["alpha", "kibana"]);
        assert!(all.iter().all(|(_, r)| r.is_ok()));

        // Active profile → exactly its list, in its order.
        let fenced = effective_runtime_servers(&manifest, &library, lib_home.path(), Some("solo"));
        let names: Vec<&str> = fenced.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["kibana", "alpha"]);

        // Vanished profile → no fence, same as None.
        let ghost = effective_runtime_servers(&manifest, &library, lib_home.path(), Some("ghost"));
        assert_eq!(ghost.len(), 2);
    }

    #[test]
    fn runtime_servers_report_broken_refs_per_name() {
        let lib_home = assert_fs::TempDir::new().unwrap();
        let library = Library::default();
        let manifest: Manifest = toml::from_str(
            "version = 1\n\
             [servers.alpha]\ntype = \"http\"\nurl = \"https://a\"\n\
             [profiles.solo]\nservers = [\"nope\"]\n",
        )
        .unwrap();
        let all = effective_runtime_servers(&manifest, &library, lib_home.path(), None);
        assert!(all.iter().find(|(n, _)| n == "alpha").unwrap().1.is_ok());
        assert!(matches!(
            all.iter().find(|(n, _)| n == "nope").unwrap().1,
            Err(ServerResolveError::Unresolved { .. })
        ));
    }

    #[test]
    fn server_checksum_reflects_definition_file() {
        let lib_home = assert_fs::TempDir::new().unwrap();
        let manifest = empty_manifest();
        let (library, content) = library_with_server(&lib_home, "kibana", "https://central/mcp");

        let r = resolve_server(&manifest, &library, lib_home.path(), "kibana").unwrap();
        assert_eq!(r.checksum, sha256_hex(content.as_bytes()));
        assert_eq!(r.checksum.len(), 64);
    }

    // ---------- server lock/drift ----------

    fn server_lock(name: &str, checksum: &str) -> Lock {
        let mut lock = Lock::default();
        lock.upsert_server(crate::lock::LockedServer {
            name: name.into(),
            source: crate::lock::ServerSource::Library,
            // `checksum` is a real digest from the resolver — parse it, don't
            // re-hash it.
            checksum: Sha256Hex::parse(checksum).unwrap(),
        });
        lock
    }

    #[test]
    fn server_lock_matches_and_missing() {
        let lib_home = assert_fs::TempDir::new().unwrap();
        let manifest = empty_manifest();
        let (library, _) = library_with_server(&lib_home, "kibana", "https://x/mcp");
        let resolved = resolve_server(&manifest, &library, lib_home.path(), "kibana").unwrap();

        // Locked at the current digest → Matches.
        let lock = server_lock("kibana", &resolved.checksum);
        let r = server_lock_status("kibana", &manifest, &library, lib_home.path(), &lock);
        assert_eq!(r.status, ServerLockStatus::Matches);
        assert_eq!(r.origin, Some(ServerOrigin::Library));
        assert_eq!(r.provenance.as_deref(), Some("consolidated:codex"));

        // No entry → MissingLockEntry.
        let r2 = server_lock_status(
            "kibana",
            &manifest,
            &library,
            lib_home.path(),
            &Lock::default(),
        );
        assert_eq!(r2.status, ServerLockStatus::MissingLockEntry);
    }

    #[test]
    fn server_definition_change_is_checksum_drift() {
        let lib_home = assert_fs::TempDir::new().unwrap();
        let manifest = empty_manifest();
        let (library, _) = library_with_server(&lib_home, "kibana", "https://x/mcp");
        // Lock a stale digest, then change the definition file underneath it.
        let lock = server_lock("kibana", Sha256Hex::of(b"staledigest").hex());
        lib_home
            .child("servers/kibana.toml")
            .write_str("type = \"http\"\nurl = \"https://changed/mcp\"\n")
            .unwrap();

        let r = server_lock_status("kibana", &manifest, &library, lib_home.path(), &lock);
        match r.status {
            ServerLockStatus::ChecksumDrift { locked, .. } => {
                assert_eq!(locked, Sha256Hex::of(b"staledigest").hex());
            }
            other => panic!("expected ChecksumDrift, got {other:?}"),
        }
    }

    #[test]
    fn broken_library_server_reports_resolve_failed() {
        let lib_home = assert_fs::TempDir::new().unwrap();
        let manifest = empty_manifest();
        // Indexed by name, but the definition file is missing.
        let mut library = Library::default();
        library.upsert_server(LibraryServer {
            name: "kibana".into(),
            checksum: None,
            version: None,
            provenance: None,
        });

        let r = server_lock_status(
            "kibana",
            &manifest,
            &library,
            lib_home.path(),
            &Lock::default(),
        );
        assert!(matches!(r.status, ServerLockStatus::ResolveFailed { .. }));
        assert_eq!(r.origin, None);
    }

    #[test]
    fn inline_server_lock_status_reports_inline_origin() {
        let lib_home = assert_fs::TempDir::new().unwrap();
        // Inline "kibana" overrides a same-named library server.
        let manifest = manifest_with_inline_server("kibana", "https://inline/mcp");
        let (library, _) = library_with_server(&lib_home, "kibana", "https://central/mcp");

        let r = server_lock_status(
            "kibana",
            &manifest,
            &library,
            lib_home.path(),
            &Lock::default(),
        );
        assert_eq!(r.origin, Some(ServerOrigin::Inline));
        assert_eq!(r.provenance, None, "inline has no library provenance");
    }

    // ---------- native extensions: library origin + git sources (E3) ----------

    use crate::library::LibraryExtension;

    fn lock_ext(entry: agentstack_core::lock::LockedExtension) -> Lock {
        let mut lock = Lock::default();
        lock.upsert_extension(entry);
        lock
    }

    /// A sourceless manifest `[extensions.<name>]` (target only) resolves its
    /// body from the central library, reports Library origin + provenance, and
    /// pins with the strict integrity-root digest.
    #[test]
    fn library_extension_resolves_pins_and_reports_origin() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());
        lib_home
            .child("extensions/checkpoint/index.ts")
            .write_str("export default (pi) => {}\n")
            .unwrap();
        let mut library = Library::default();
        library.upsert_extension(LibraryExtension {
            name: "checkpoint".into(),
            source: "path".into(),
            target: "pi".into(),
            path: Some("checkpoint".into()),
            git: None,
            rev: None,
            subpath: None,
            checksum: None,
            description: Some("Checkpoint the session".into()),
            version: None,
            provenance: Some("path:/src".into()),
        });
        // Sourceless manifest entry — the library supplies the body.
        let manifest: Manifest =
            toml::from_str("version = 1\n[extensions.checkpoint]\ntarget = \"pi\"\n").unwrap();
        let ext = &manifest.extensions["checkpoint"];

        let resolved = resolve_extension_entry(
            "checkpoint",
            ext,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            ResolveMode::NoFetch,
        )
        .unwrap();
        assert_eq!(resolved.origin, ExtensionOrigin::Library);
        assert_eq!(resolved.source_kind, "path");
        assert_eq!(resolved.checksum.len(), 64);
        assert_eq!(resolved.provenance.as_deref(), Some("path:/src"));

        // Pin it and confirm the status is Matches with Library origin (the
        // trust-preview `[library, pinned]` label reads off exactly this).
        let lock = lock_ext(agentstack_core::lock::LockedExtension {
            name: "checkpoint".into(),
            target: "pi".into(),
            source: "library".into(),
            path: Some("checkpoint".into()),
            git: None,
            rev: None,
            checksum: resolved.checksum.clone(),
        });
        let report = extension_lock_status(
            "checkpoint",
            ext,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            &lock,
            ResolveMode::NoFetch,
        );
        assert_eq!(report.status, ExtensionLockStatus::Matches);
        assert_eq!(report.origin, Some(ExtensionOrigin::Library));

        // An inline `path` on the SAME name wins over the library (origin flips).
        proj.child("extensions/checkpoint/index.ts")
            .write_str("export default (pi) => {} // inline\n")
            .unwrap();
        let inline: Manifest = toml::from_str(
            "version = 1\n[extensions.checkpoint]\npath = \"./extensions/checkpoint\"\ntarget = \"pi\"\n",
        )
        .unwrap();
        let inline_resolved = resolve_extension_entry(
            "checkpoint",
            &inline.extensions["checkpoint"],
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            ResolveMode::NoFetch,
        )
        .unwrap();
        assert_eq!(inline_resolved.origin, ExtensionOrigin::Inline);
    }

    /// A git-source extension pins from a local `file://` repo (fetched through
    /// the store, digested strictly at its subpath) and rev-drifts when the
    /// locked rev differs while the content still matches — mirrors the skill
    /// git rev-drift witness.
    #[test]
    fn git_extension_pins_and_rev_drifts() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let repo = proj.child("repo");
        repo.create_dir_all().unwrap();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(repo.path())
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?} failed");
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@e.st"]);
        git(&["config", "user.name", "t"]);
        repo.child("ext/index.ts")
            .write_str("export default (pi) => {}\n")
            .unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "init"]);

        let store = Store::with_root(proj.child("store").path().to_path_buf());
        let url = format!("file://{}", repo.path().display());
        let manifest: Manifest = toml::from_str(&format!(
            "version = 1\n[extensions.gitext]\ngit = \"{url}\"\nsubpath = \"ext\"\ntarget = \"pi\"\n"
        ))
        .unwrap();
        let ext = &manifest.extensions["gitext"];

        // Fetch resolves the checksum + HEAD rev.
        let resolved = resolve_extension_entry(
            "gitext",
            ext,
            proj.path(),
            &Library::default(),
            lib_home.path(),
            &store,
            ResolveMode::Fetch,
        )
        .unwrap();
        assert_eq!(resolved.source_kind, "git");
        assert_eq!(resolved.checksum.len(), 64);
        let head = resolved.rev.clone().expect("git rev resolved");

        // Locked at the same checksum + rev → Matches.
        let lock = lock_ext(agentstack_core::lock::LockedExtension {
            name: "gitext".into(),
            target: "pi".into(),
            source: "git".into(),
            path: None,
            git: Some(url.clone()),
            rev: Some(head.clone()),
            checksum: resolved.checksum.clone(),
        });
        assert_eq!(
            extension_lock_status(
                "gitext",
                ext,
                proj.path(),
                &Library::default(),
                lib_home.path(),
                &store,
                &lock,
                ResolveMode::NoFetch,
            )
            .status,
            ExtensionLockStatus::Matches
        );

        // Same checksum, different locked rev → rev drift.
        let stale = lock_ext(agentstack_core::lock::LockedExtension {
            name: "gitext".into(),
            target: "pi".into(),
            source: "git".into(),
            path: None,
            git: Some(url.clone()),
            rev: Some("0000000000000000000000000000000000000000".into()),
            checksum: resolved.checksum.clone(),
        });
        assert!(matches!(
            extension_lock_status(
                "gitext",
                ext,
                proj.path(),
                &Library::default(),
                lib_home.path(),
                &store,
                &stale,
                ResolveMode::NoFetch,
            )
            .status,
            ExtensionLockStatus::RevDrift { .. }
        ));
    }

    /// An un-cached git extension checked offline surfaces as
    /// NotAvailableOffline (not a failure), and a git source without a subpath
    /// is a hard resolve error — a checkout's `.git` can't be reproducibly
    /// pinned.
    #[test]
    fn git_extension_offline_and_subpath_rules() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());

        let uncached: Manifest = toml::from_str(
            "version = 1\n[extensions.g]\ngit = \"https://example.com/x.git\"\nsubpath = \"ext\"\ntarget = \"pi\"\n",
        )
        .unwrap();
        let report = extension_lock_status(
            "g",
            &uncached.extensions["g"],
            proj.path(),
            &Library::default(),
            lib_home.path(),
            &store,
            &Lock::default(),
            ResolveMode::NoFetch,
        );
        assert!(matches!(
            report.status,
            ExtensionLockStatus::NotAvailableOffline { .. }
        ));

        // No subpath → resolve fails closed (surfaces as ResolveFailed).
        let no_sub: Manifest = toml::from_str(
            "version = 1\n[extensions.g]\ngit = \"https://example.com/x.git\"\ntarget = \"pi\"\n",
        )
        .unwrap();
        let report = extension_lock_status(
            "g",
            &no_sub.extensions["g"],
            proj.path(),
            &Library::default(),
            lib_home.path(),
            &store,
            &Lock::default(),
            ResolveMode::NoFetch,
        );
        assert!(matches!(
            report.status,
            ExtensionLockStatus::ResolveFailed { .. }
        ));
    }
}
