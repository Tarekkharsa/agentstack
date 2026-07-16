//! Name resolution — the single seam that maps a `skills = ["name"]` or
//! `servers = ["name"]` reference to a concrete definition (see
//! `docs/reference.md#the-central-library`).
//!
//! Resolution order (first hit wins), for both skills and servers:
//!
//! 1. **Inline** — a `[skills.<name>]` / `[servers.<name>]` entry in the project
//!    manifest. An inline definition always wins (a project that wants to override
//!    a central item defines it inline).
//! 2. **Central library** — a `[[skill]]` / `[[server]]` entry in
//!    `<lib_home>/library.toml`, whose body lives under `<lib_home>/skills/` or
//!    `<lib_home>/servers/<name>.toml`.
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
    // Locate the source (inline wins over the central library) and the base dir
    // its relative paths resolve against.
    let (skill, base, origin, provenance) = if let Some(skill) = manifest.skills.get(name) {
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

    let resolved = resolve_source(store, &skill, &base, mode, name)?;
    Ok(ResolvedSkill {
        name: name.to_string(),
        origin,
        path: resolved.path,
        source_kind: resolved.source_kind,
        rev: resolved.rev,
        checksum: resolved.checksum,
        provenance,
    })
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
) -> Result<crate::store::Resolved, ResolveError> {
    let local = match mode {
        ResolveMode::Fetch => return Ok(store.resolve(skill, base, skill.rev.as_deref())?),
        ResolveMode::NoFetch => store.resolve_local(skill, base)?,
        ResolveMode::PathOnly => store.resolve_path_only(skill, base)?,
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

/// Resolve a single server name: inline `[servers.<name>]` wins, else the central
/// library's `[[server]]` entry (definition at `<lib_home>/servers/<name>.toml`).
/// Returns the definition with `${REF}`s intact — no secret resolution.
pub fn resolve_server(
    manifest: &Manifest,
    library: &Library,
    lib_home: &Path,
    name: &str,
) -> Result<ResolvedServer, ServerResolveError> {
    // 1. Inline manifest server wins.
    if let Some(server) = manifest.servers.get(name) {
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

    // 2. Central library.
    if let Some(entry) = library.get_server(name) {
        let path = lib_home.join("servers").join(format!("{name}.toml"));
        let content = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))?;
        let server: Server = toml::from_str(&content)
            .map_err(|e| anyhow::anyhow!("parsing {}: {e}", path.display()))?;
        return Ok(ResolvedServer {
            name: name.to_string(),
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
        Some(entry) if entry.checksum != resolved.checksum => Err(format!(
            "library definition drifted from agentstack.lock (locked {}, current {}) — \
             review it and re-run `agentstack lock`",
            entry.checksum, resolved.checksum
        )),
        Some(_) => Ok(()),
        None => Err(
            "library server is not pinned in agentstack.lock — pin it with `agentstack lock`"
                .to_string(),
        ),
    }
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
                "profile '{p}' is not defined in the manifest — refusing to serve any \
                 servers (a missing pinned profile must never broaden to all servers)"
            );
        }
    }
    let lock = Lock::load(dir).ok();
    Ok(
        effective_runtime_servers(manifest, library, lib_home, profile)
            .into_iter()
            .map(|(name, resolved)| {
                let out = match resolved {
                    Ok(rs) => verify_library_pin(&rs, lock.as_ref(), &name).map(|()| rs),
                    Err(e) => Err(e.to_string()),
                };
                (name, out)
            })
            .collect(),
    )
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
        None => {
            for n in manifest.servers.keys() {
                push(n, &mut names);
            }
            for p in manifest.profiles.values() {
                for n in &p.servers {
                    push(n, &mut names);
                }
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
        Some(entry) if entry.checksum != current_checksum => InstructionLockStatus::ChecksumDrift {
            locked: entry.checksum.clone(),
            current: current_checksum.to_string(),
        },
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
    let src = crate::render::instructions::fragment_source(manifest_dir, &instr.path);
    match std::fs::read(&src) {
        Ok(bytes) => classify_instruction(name, &agentstack_core::digest::sha256_hex(&bytes), lock),
        Err(e) => InstructionLockStatus::ResolveFailed {
            error: format!("reading {}: {e}", src.display()),
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
        Some(entry) if entry.checksum != current_checksum => ServerLockStatus::ChecksumDrift {
            locked: entry.checksum.clone(),
            current: current_checksum.to_string(),
        },
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
    match resolve_skill(manifest, manifest_dir, library, lib_home, store, name, mode) {
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
    match lock.get(name) {
        None => SkillLockStatus::MissingLockEntry,
        Some(entry) if entry.checksum != current_checksum => SkillLockStatus::ChecksumDrift {
            locked: entry.checksum.clone(),
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
            checksum: "aaaa".into(),
        });
        assert_eq!(
            classify_instruction("house", "aaaa", &lock),
            InstructionLockStatus::Matches
        );
        assert_eq!(
            classify_instruction("house", "bbbb", &lock),
            InstructionLockStatus::ChecksumDrift {
                locked: "aaaa".into(),
                current: "bbbb".into()
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
        let checksum = agentstack_core::digest::sha256_hex(b"be kind\n");
        let mut lock = Lock::default();
        lock.upsert_instruction(agentstack_core::lock::LockedInstruction {
            name: "house".into(),
            path: "./instructions/house.md".into(),
            checksum,
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
            checksum: resolved.checksum.clone(),
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
            checksum: "staledigest".into(),
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
            SkillLockStatus::ChecksumDrift { locked, .. } => assert_eq!(locked, "staledigest"),
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
            checksum: resolved.checksum.clone(),
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
            checksum: checksum.into(),
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
        let lock = server_lock("kibana", "staledigest");
        lib_home
            .child("servers/kibana.toml")
            .write_str("type = \"http\"\nurl = \"https://changed/mcp\"\n")
            .unwrap();

        let r = server_lock_status("kibana", &manifest, &library, lib_home.path(), &lock);
        match r.status {
            ServerLockStatus::ChecksumDrift { locked, .. } => assert_eq!(locked, "staledigest"),
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
}
