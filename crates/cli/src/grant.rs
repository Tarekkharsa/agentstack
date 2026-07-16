//! Machine-local commitment key and argv commitment for the locked-run
//! `AuthorityGrant` (Phase 0A). **Not wired to any runtime path yet.**
//!
//! The `AuthorityGrant`'s canonical digest (increment 3b) binds the exact
//! invocation without recording raw argv (which may carry caller tokens or
//! passwords). It does so by committing argv under a machine-local HMAC-SHA256
//! key: an evidence reader who lacks the key cannot brute-force low-entropy
//! argv. This is dictionary-attack resistance for shipped/exfiltrated evidence,
//! NOT authenticity — the harness runs as the same user who can read the key, so
//! it is a cooperative local commitment, not tamper-proof attestation (see
//! `docs/design/locked-run-contract.md` §3.1). Portable, independently
//! verifiable authenticity would require asymmetric signing and durable key
//! management, which is out of scope here.
//!
//! Because the key is machine-stable, the same complete grant on one machine
//! digests to the same value across `--plan` and the live run (the "plan matches
//! run" property) — which also means two identical *complete* grants correlate
//! on that machine. The standalone argv commitment is likewise correlatable, so
//! it is opaque, redacted, non-serializable, and exposed only to the outer grant
//! digest — never recorded or displayed on its own.

// STAGED: 3b-i lands the grant types + the sealed `GrantBuilder` unwired. 3b-ii
// (the canonical digest + evidence, which reads every field) and increment 4
// (the run flow, which calls the builder) consume this surface. Remove this
// allow once the grant is wired into the run path.
#![allow(dead_code)]

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use hmac::{Hmac, Mac};
use sha2::Sha256;

use agentstack_core::lock::ExecutableKind;
use agentstack_core::scope::Scope;
use agentstack_policy::CompiledRuleset;

use crate::adapter::{AdapterDescriptor, AdapterSource, Registry};
use crate::util::paths;

type HmacSha256 = Hmac<Sha256>;

/// Domain separator for the argv commitment (versioned).
const ARGV_COMMIT_DOMAIN: &[u8] = b"agentstack-argv-commit-v1\0";

/// A machine-local 32-byte HMAC key for argv commitments.
///
/// Sensitive: no `Debug`/`Serialize` derive — `Debug` is redacted and the key is
/// never serialized, so its bytes cannot leak through logs or evidence.
pub struct CommitmentKey([u8; 32]);

impl std::fmt::Debug for CommitmentKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CommitmentKey(<redacted>)")
    }
}

/// An opaque HMAC-SHA256 commitment over an argv sequence.
///
/// Correlatable: identical argv under the same machine key yields the same tag,
/// so it is NOT safe to record or display standalone. Redacted in `Debug`, never
/// serialized; its bytes will be exposed to the `AuthorityGrant` digest in
/// increment 3b (which adds the crate-internal accessor), never on their own.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ArgvCommitment([u8; 32]);

impl std::fmt::Debug for ArgvCommitment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ArgvCommitment(<redacted>)")
    }
}

/// `~/.agentstack/grant/commit-key` — the machine-local commitment key.
fn commit_key_path() -> PathBuf {
    paths::agentstack_home().join("grant").join("commit-key")
}

/// Load the commitment key for **read-only** use (grant construction, `--plan`).
///
/// Fail-closed: a missing, unreadable, malformed (not exactly 32 bytes), symlink,
/// or insecurely-permissioned key (including a symlinked or group/other-writable
/// parent directory) **blocks**. Never creates, heals, or replaces a key —
/// provisioning is a separate, explicitly-mutating operation
/// ([`provision_commitment_key`]).
pub fn load_commitment_key() -> Result<CommitmentKey> {
    let path = commit_key_path();

    // The parent must be a real directory (not a symlink) and not writable by
    // group/other: a 0600 key is still swappable through a permissive dir. (Unix.)
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        use std::os::unix::fs::PermissionsExt;
        let pmeta = std::fs::symlink_metadata(parent)
            .with_context(|| format!("commitment key directory {} is missing", parent.display()))?;
        if !pmeta.file_type().is_dir() {
            bail!(
                "commitment key directory {} is not a real directory (a symlink is refused) — refusing",
                parent.display()
            );
        }
        let pmode = pmeta.permissions().mode() & 0o777;
        if pmode & 0o022 != 0 {
            bail!(
                "commitment key directory {} is writable by group/other (mode {:o}) — refusing",
                parent.display(),
                pmode
            );
        }
    }

    // Portable symlink refusal: reject anything but a regular file before opening
    // (Unix additionally enforces this atomically via O_NOFOLLOW below).
    let lmeta = std::fs::symlink_metadata(&path).with_context(|| {
        format!(
            "commitment key {} is missing or unreadable — provision it first",
            path.display()
        )
    })?;
    if !lmeta.file_type().is_file() {
        bail!(
            "commitment key {} is not a regular file (a symlink or directory is refused) — refusing",
            path.display()
        );
    }

    // Open ONE handle without following a symlink, and validate that same handle
    // — no path-based re-read that could resolve to a different file.
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW); // a symlink makes open() fail; no unsafe
    }
    let file = opts.open(&path).with_context(|| {
        format!(
            "commitment key {} is missing, unreadable, or a symlink — provision it first",
            path.display()
        )
    })?;

    let meta = file
        .metadata()
        .with_context(|| format!("stat commitment key {}", path.display()))?;
    if !meta.file_type().is_file() {
        bail!(
            "commitment key {} is not a regular file — refusing",
            path.display()
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            bail!(
                "commitment key {} has insecure permissions {:o} — must be readable only by its owner (0600)",
                path.display(),
                mode
            );
        }
    }

    // Bounded read: at most 33 bytes, then require exactly 32 — never allocates
    // for an arbitrarily large file, and a larger file is malformed.
    let mut buf = Vec::with_capacity(33);
    file.take(33)
        .read_to_end(&mut buf)
        .with_context(|| format!("reading commitment key {}", path.display()))?;
    if buf.len() != 32 {
        bail!(
            "commitment key {} is malformed: expected exactly 32 bytes, found {}{}",
            path.display(),
            buf.len(),
            if buf.len() > 32 { "+" } else { "" }
        );
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&buf);
    Ok(CommitmentKey(key))
}

/// Provision the machine-local commitment key: 32 CSPRNG bytes written with
/// checked, verified `0600` permissions. **Explicitly mutating** — never called
/// by grant construction or `--plan`.
///
/// - Refuses to replace an existing key: a present *valid* key is left untouched
///   (idempotent `Ok`); a present *invalid* key is a hard error the operator must
///   resolve manually — never an automatic overwrite.
/// - Atomic + race-safe: bytes go to a per-attempt **randomly-named** temp file
///   (thread-safe — no shared PID path to unlink), fsynced, then hard-linked into
///   place (the link fails if the key exists, so the final name only appears with
///   complete content and is never clobbered).
/// - Permissions are set and **verified** with checked, handle-based operations
///   (not the umask-narrowed `OpenOptions::mode` or error-swallowing `restrict`),
///   after confirming the directory is real (not a symlink whose target would be
///   chmod'd). The final key is validated by a read-only load before success.
/// - Uses a fallible CSPRNG (`getrandom`); a CSPRNG failure is a hard error, not
///   a fallback to weaker entropy.
pub fn provision_commitment_key() -> Result<()> {
    let path = commit_key_path();
    if path.exists() {
        load_commitment_key().map(|_| ()).with_context(|| {
            format!(
                "refusing to overwrite the existing commitment key at {} — resolve it manually",
                path.display()
            )
        })?;
        return Ok(());
    }
    let dir = path
        .parent()
        .with_context(|| format!("commitment key path {} has no parent", path.display()))?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // create_dir_all accepts a pre-existing directory SYMLINK; require a real
        // directory BEFORE any chmod so we never follow a symlink and mutate its
        // target's permissions.
        let dmeta =
            std::fs::symlink_metadata(dir).with_context(|| format!("stat {}", dir.display()))?;
        if !dmeta.file_type().is_dir() {
            bail!(
                "commitment key directory {} is not a real directory (a symlink is refused) — refusing to secure it",
                dir.display()
            );
        }
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("securing {} to 0700", dir.display()))?;
        let m = std::fs::symlink_metadata(dir)?.permissions().mode() & 0o777;
        if m & 0o077 != 0 {
            bail!(
                "commitment key directory {} could not be secured (mode {:o})",
                dir.display(),
                m
            );
        }
    }

    let mut key = [0u8; 32];
    getrandom::getrandom(&mut key).map_err(|e| {
        anyhow::anyhow!("CSPRNG failure while provisioning the commitment key: {e}")
    })?;

    // Per-attempt random suffix: thread-safe, so one provisioner can never unlink
    // another's in-flight temp file.
    let mut nonce = [0u8; 12];
    getrandom::getrandom(&mut nonce)
        .map_err(|e| anyhow::anyhow!("CSPRNG failure choosing a temp name: {e}"))?;
    let suffix: String = nonce.iter().map(|b| format!("{b:02x}")).collect();
    let tmp = dir.join(format!("commit-key.tmp.{suffix}"));

    let write_tmp = || -> Result<()> {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts
            .open(&tmp)
            .with_context(|| format!("creating {}", tmp.display()))?;
        // `OpenOptions::mode` is umask-narrowed — set + verify perms on the OPEN
        // handle (not a path-based call) before writing key bytes.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(std::fs::Permissions::from_mode(0o600))
                .with_context(|| format!("securing {}", tmp.display()))?;
            let m = f.metadata()?.permissions().mode() & 0o777;
            if m & 0o077 != 0 {
                bail!(
                    "temp key {} could not be secured (mode {:o})",
                    tmp.display(),
                    m
                );
            }
        }
        f.write_all(&key)?;
        f.sync_all()?;
        Ok(())
    };
    if let Err(e) = write_tmp() {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }

    // Hard-link into place: atomic (full content before the name appears) and
    // exclusive (fails if a key exists, so it is never clobbered). The final key
    // shares the temp's inode, so it inherits the verified 0600 perms; a
    // read-only load then validates it before success is reported.
    let link = std::fs::hard_link(&tmp, &path);
    let _ = std::fs::remove_file(&tmp);
    match link {
        Ok(()) => load_commitment_key().map(|_| ()).with_context(|| {
            format!(
                "provisioned commitment key at {} did not validate",
                path.display()
            )
        }),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            // A concurrent provisioner won — validate theirs, never overwrite.
            load_commitment_key().map(|_| ()).with_context(|| {
                format!(
                    "a commitment key already exists at {} — resolve it manually",
                    path.display()
                )
            })
        }
        Err(e) => Err(e)
            .with_context(|| format!("linking commitment key into place at {}", path.display())),
    }
}

/// HMAC-SHA256 commitment over an argv sequence, keyed by the machine-local
/// commitment key. **Order-preserving** (argv order is semantic — never sorted)
/// and unambiguous across element boundaries via length framing, so `["ab","c"]`
/// and `["a","bc"]` commit to different tags.
pub fn commit_argv(key: &CommitmentKey, argv: &[String]) -> ArgvCommitment {
    // `new_from_slice` accepts any key length; ours is always 32 bytes.
    let mut mac = HmacSha256::new_from_slice(&key.0).expect("HMAC accepts a 32-byte key");
    mac.update(ARGV_COMMIT_DOMAIN);
    mac.update(&(argv.len() as u64).to_le_bytes());
    for arg in argv {
        let b = arg.as_bytes();
        mac.update(&(b.len() as u64).to_le_bytes());
        mac.update(b);
    }
    let out = mac.finalize().into_bytes();
    let mut tag = [0u8; 32];
    tag.copy_from_slice(&out);
    ArgvCommitment(tag)
}

// ===== 3b-i: operational AuthorityGrant types, sealed construction =====
//
// STAGED + UNWIRED: nothing constructs or consumes a grant yet. D3
// repository-controlled executable inputs are modeled (`executables`, server-
// tied and validated in `build()`), so the canonical V1 digest/KAT (3b-ii) may
// now freeze over the complete field set.

/// Validated SHA-256 hex (exactly 64 lowercase hex chars). Parsing accepts an
/// optional `sha256:` prefix; consumers emit one canonical form.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct Sha256Hex(String);
impl Sha256Hex {
    fn parse(s: &str) -> Result<Self> {
        let h = s.strip_prefix("sha256:").unwrap_or(s);
        if h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()) {
            Ok(Sha256Hex(h.to_ascii_lowercase()))
        } else {
            bail!("not a sha256 hex digest: {s:?}");
        }
    }
    fn hex(&self) -> &str {
        &self.0
    }
}

/// The AuthorityGrant's own canonical digest.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GrantDigest(Sha256Hex);
impl GrantDigest {
    pub fn parse(s: &str) -> Result<Self> {
        Ok(GrantDigest(Sha256Hex::parse(s)?))
    }
    pub fn hex(&self) -> &str {
        self.0.hex()
    }
}
impl std::fmt::Display for GrantDigest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "sha256:{}", self.0.hex())
    }
}
impl std::fmt::Debug for GrantDigest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self}")
    }
}

/// A trust consent digest bound into a grant.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConsentDigest(Sha256Hex);
impl ConsentDigest {
    pub fn parse(s: &str) -> Result<Self> {
        Ok(ConsentDigest(Sha256Hex::parse(s)?))
    }
    pub fn hex(&self) -> &str {
        self.0.hex()
    }
}
impl std::fmt::Display for ConsentDigest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "sha256:{}", self.0.hex())
    }
}
impl std::fmt::Debug for ConsentDigest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self}")
    }
}

/// A content checksum: input, adapter definition, image, or policy source.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentDigest(Sha256Hex);
impl ContentDigest {
    pub fn parse(s: &str) -> Result<Self> {
        Ok(ContentDigest(Sha256Hex::parse(s)?))
    }
    pub fn hex(&self) -> &str {
        self.0.hex()
    }
}
impl std::fmt::Display for ContentDigest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "sha256:{}", self.0.hex())
    }
}
impl std::fmt::Debug for ContentDigest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self}")
    }
}

/// An absolute, filesystem-canonical, UTF-8 path. Construction is read-only
/// (`--plan`-safe): rejects non-UTF-8 and non-existent paths.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct GrantPath(String);
impl GrantPath {
    pub fn new(p: &Path) -> Result<GrantPath> {
        if p.to_str().is_none() {
            bail!("path is not valid UTF-8: {}", p.display());
        }
        let canon =
            std::fs::canonicalize(p).with_context(|| format!("canonicalizing {}", p.display()))?;
        let s = canon.to_str().ok_or_else(|| {
            anyhow::anyhow!("canonical path is not valid UTF-8: {}", canon.display())
        })?;
        Ok(GrantPath(s.to_string()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// Whether this canonical path lies inside `root` (canonical).
    pub fn is_within(&self, root: &GrantPath) -> bool {
        Path::new(&self.0).starts_with(&root.0)
    }
}

/// Grant schema, tied to the digest domain: `V1` ↔ `agentstack-authority-grant-v1`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GrantSchema {
    V1,
}
impl GrantSchema {
    pub fn slug(self) -> &'static str {
        match self {
            GrantSchema::V1 => "v1",
        }
    }
}

/// Confinement posture (grant-local; slugs match `commands::sandbox::Posture`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GrantPosture {
    Host,
    Sandbox,
    Lockdown,
}
impl GrantPosture {
    pub fn slug(self) -> &'static str {
        match self {
            GrantPosture::Host => "host",
            GrantPosture::Sandbox => "sandbox",
            GrantPosture::Lockdown => "lockdown",
        }
    }
    pub fn from_slug(s: &str) -> Option<GrantPosture> {
        match s.trim() {
            "host" => Some(GrantPosture::Host),
            "sandbox" => Some(GrantPosture::Sandbox),
            "lockdown" => Some(GrantPosture::Lockdown),
            _ => None,
        }
    }
}

/// Egress enforcement, independent of posture.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EgressMode {
    Unconfined,
    ProxyAdvisory,
    NoNetwork,
    LockdownConfined,
}
impl EgressMode {
    pub fn slug(self) -> &'static str {
        match self {
            EgressMode::Unconfined => "unconfined",
            EgressMode::ProxyAdvisory => "proxy-advisory",
            EgressMode::NoNetwork => "no-network",
            EgressMode::LockdownConfined => "lockdown-confined",
        }
    }
}

/// Generated-artifact lifecycle.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ArtifactMode {
    Static,
    CleanAtRest,
    ZeroFiles,
}
impl ArtifactMode {
    pub fn slug(self) -> &'static str {
        match self {
            ArtifactMode::Static => "static",
            ArtifactMode::CleanAtRest => "clean-at-rest",
            ArtifactMode::ZeroFiles => "zero-files",
        }
    }
}

/// Workspace read/write/deny roots — Phase-1 reserved.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum WorkspaceGrant {
    Unbound,
}
impl WorkspaceGrant {
    pub fn slug(&self) -> &'static str {
        match self {
            WorkspaceGrant::Unbound => "unbound",
        }
    }
}

/// Where a resolved capability came from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InputOrigin {
    Inline,
    Library,
}

/// A resolved skill's source (valid-state: Git always carries a revision).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SkillSource {
    Path,
    Git { revision: String },
}

/// An instruction fragment's integrity binding.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum InstructionBinding {
    MachineOwned(ContentDigest),
    ProjectPinned(ContentDigest),
}

/// A server's binding — one discriminated union, so origin and integrity can't
/// contradict. Inline servers are trust-digest-bound but still carry their
/// per-definition checksum; library servers are lock-pinned.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum GrantedServerBinding {
    Inline {
        definition: ContentDigest,
    },
    Library {
        definition: ContentDigest,
        provenance: Option<String>,
    },
}

/// Runtime image identity: `Host` (no image) vs a container image that may be
/// present-but-unbound.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RuntimeImage {
    Host,
    Container {
        reference: String,
        binding: ImageBinding,
    },
}
impl RuntimeImage {
    pub fn slug(&self) -> &'static str {
        match self {
            RuntimeImage::Host => "host",
            RuntimeImage::Container { .. } => "container",
        }
    }
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ImageBinding {
    Unbound,
    Pinned(ContentDigest),
}

/// The harness binary's integrity boundary. Phase 0A: the harness/interpreter is
/// an external `$PATH` binary, always unpinned. D3 pins repository-controlled
/// server commands and script args — NOT the harness binary — so no
/// `RepositoryPinned` variant is added here without revising the contract.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum HarnessIntegrity {
    ExternalUnpinned,
}

/// Adapter source identity; a user override carries its canonical path.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum AdapterSourceIdentity {
    BuiltIn,
    User(GrantPath),
}

/// The `--profile`/`--scope`/`--keep` effect as one valid-state shape.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ProfileEffect {
    None,
    Temporary { name: String, scope: Scope },
    Kept { name: String, scope: Scope },
}

/// Secret authorization scope — concrete server, never unscoped.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum SecretScope {
    Server(String),
}

/// Secret lifetime binding. Phase 0A constructs only `Unbound`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum SecretLifetimeBinding {
    Unbound,
    RunScoped,
}

/// A secret grant, canonically ordered by the full `(reference, scope, lifetime)`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct SecretGrant {
    reference: String,
    scope: SecretScope,
    lifetime: SecretLifetimeBinding,
}
impl SecretGrant {
    pub(crate) fn new(
        reference: &str,
        scope: SecretScope,
        lifetime: SecretLifetimeBinding,
    ) -> Result<SecretGrant> {
        if !agentstack_core::refs::is_ref_name(reference) {
            bail!("invalid secret reference name {reference:?}");
        }
        let SecretScope::Server(server) = &scope;
        if server.trim().is_empty() {
            bail!("secret {reference:?} must be scoped to a non-empty server");
        }
        Ok(SecretGrant {
            reference: reference.to_string(),
            scope,
            lifetime,
        })
    }
}

/// A policy input's identity — explicitly absent, never an empty string.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum PolicySource {
    Absent,
    Digest(ContentDigest),
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PolicyProvenance {
    machine: PolicySource,
    project: PolicySource,
}

/// The effective policy: the actual compiled ruleset plus its input provenance.
#[derive(Clone, Debug)]
pub struct PolicyGrant {
    ruleset: CompiledRuleset,
    provenance: PolicyProvenance,
}

/// Bound adapter identity: the cloned operational descriptor plus its
/// registry-produced definition digest. Constructed ONLY from the registry, so a
/// caller cannot supply a mutated descriptor with a stale digest.
#[derive(Clone, Debug)]
pub struct GrantedAdapter {
    descriptor: AdapterDescriptor,
    source: AdapterSourceIdentity,
    definition_digest: ContentDigest,
}
impl GrantedAdapter {
    pub(crate) fn from_registry(registry: &Registry, id: &str) -> Result<GrantedAdapter> {
        let desc = registry
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("unknown adapter {id:?}"))?;
        let digest = desc.definition_digest().ok_or_else(|| {
            anyhow::anyhow!("adapter {id:?} has no registry definition digest — cannot bind it")
        })?;
        let definition_digest = ContentDigest::parse(digest)?;
        let source = match &desc.source {
            AdapterSource::BuiltIn => AdapterSourceIdentity::BuiltIn,
            AdapterSource::User(p) => AdapterSourceIdentity::User(GrantPath::new(p)?),
        };
        Ok(GrantedAdapter {
            descriptor: desc.clone(),
            source,
            definition_digest,
        })
    }
    pub fn id(&self) -> &str {
        &self.descriptor.id
    }
}

/// The harness binary and its (Phase-0A always-external) integrity boundary.
#[derive(Clone, Debug)]
pub struct HarnessExecutable {
    path: GrantPath,
    integrity: HarnessIntegrity,
}

#[derive(Clone, Debug)]
pub struct ProjectIdentity {
    root: GrantPath,
    consent: ConsentDigest,
}

/// The exact, sensitive invocation. `argv` is stored once, verbatim.
pub struct Invocation {
    adapter: GrantedAdapter,
    executable: HarnessExecutable,
    argv: Vec<String>, // SENSITIVE, exact — the sole argv identity
    cwd: GrantPath,
    profile: ProfileEffect,
}

#[derive(Clone, Debug)]
pub struct GrantedSkill {
    // NOTE: `origin` (inline vs library) and `source` (path vs git) are
    // orthogonal for skills — an inline entry may be `git=`, a library skill may
    // be path or git — so they are two independent fields, unlike a server's
    // single coupled binding.
    path: GrantPath,
    origin: InputOrigin,
    source: SkillSource,
    checksum: ContentDigest,
    provenance: Option<String>,
}
#[derive(Clone, Debug)]
pub struct GrantedInstruction {
    path: GrantPath,
    binding: InstructionBinding,
    targets: BTreeSet<String>,
}
#[derive(Clone, Debug)]
pub struct GrantedServer {
    server: agentstack_core::manifest::Server,
    binding: GrantedServerBinding,
}

/// One verified repository-local executable input (D3, contract §8),
/// server-tied: an auto-detected stdio command/args file or a declared
/// integrity root, with the content digest strict verification proved.
///
/// `path` is the normalized project-relative lock key — deliberately NOT a
/// [`GrantPath`]: `GrantPath::new` canonicalizes (follows symlinks), while
/// these paths were containment-checked and symlink-rejected by the
/// classifier, and the grant must carry the lock's identity byte-for-byte.
#[derive(Clone, Debug)]
pub struct GrantedExecutable {
    path: String,
    kind: ExecutableKind,
    checksum: ContentDigest,
    /// Every granted server whose surface this input belongs to (≥ 1).
    servers: BTreeSet<String>,
}

/// The one operational grant. Sealed: constructed only via `GrantBuilder`
/// (crate-internal). Sensitive: no `Serialize`; `Debug` is a minimal, infallible
/// redaction (identities + counts, never argv or a fallible digest).
pub struct AuthorityGrant {
    schema: GrantSchema,
    project: ProjectIdentity,
    invocation: Invocation,
    skills: BTreeMap<String, GrantedSkill>,
    instructions: BTreeMap<String, GrantedInstruction>,
    servers: BTreeMap<String, GrantedServer>,
    executables: BTreeMap<(String, ExecutableKind), GrantedExecutable>,
    policy: PolicyGrant,
    secrets: BTreeSet<SecretGrant>,
    runtime: RuntimeImage,
    posture: GrantPosture,
    egress: EgressMode,
    workspace: WorkspaceGrant,
    artifacts: ArtifactMode,
}

impl std::fmt::Debug for AuthorityGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthorityGrant")
            .field("schema", &self.schema.slug())
            .field("project_root", &self.project.root.as_str())
            .field("harness", &self.invocation.adapter.id())
            .field("argv_args", &self.invocation.argv.len())
            .field("skills", &self.skills.len())
            .field("instructions", &self.instructions.len())
            .field("servers", &self.servers.len())
            .field("executables", &self.executables.len())
            .field("secrets", &self.secrets.len())
            .field("posture", &self.posture.slug())
            .field("egress", &self.egress.slug())
            .field("runtime", &self.runtime.slug())
            .field("artifacts", &self.artifacts.slug())
            .finish_non_exhaustive()
    }
}

/// Assembles an `AuthorityGrant`. Crate-internal: external callers cannot
/// fabricate authority. Per-add duplicates are rejected non-destructively;
/// `build()` validates cross-field invariants.
pub struct GrantBuilder {
    project: ProjectIdentity,
    invocation: Invocation,
    policy: PolicyGrant,
    runtime: RuntimeImage,
    posture: GrantPosture,
    egress: EgressMode,
    artifacts: ArtifactMode,
    skills: BTreeMap<String, GrantedSkill>,
    instructions: BTreeMap<String, GrantedInstruction>,
    servers: BTreeMap<String, GrantedServer>,
    executables: BTreeMap<(String, ExecutableKind), GrantedExecutable>,
    secrets: BTreeSet<SecretGrant>,
}

impl GrantBuilder {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        project: ProjectIdentity,
        invocation: Invocation,
        policy: PolicyGrant,
        runtime: RuntimeImage,
        posture: GrantPosture,
        egress: EgressMode,
        artifacts: ArtifactMode,
    ) -> GrantBuilder {
        GrantBuilder {
            project,
            invocation,
            policy,
            runtime,
            posture,
            egress,
            artifacts,
            skills: BTreeMap::new(),
            instructions: BTreeMap::new(),
            servers: BTreeMap::new(),
            executables: BTreeMap::new(),
            secrets: BTreeSet::new(),
        }
    }

    pub(crate) fn add_skill(&mut self, name: &str, skill: GrantedSkill) -> Result<&mut Self> {
        insert_unique(&mut self.skills, name, skill, "skill")?;
        Ok(self)
    }
    pub(crate) fn add_instruction(
        &mut self,
        name: &str,
        ins: GrantedInstruction,
    ) -> Result<&mut Self> {
        insert_unique(&mut self.instructions, name, ins, "instruction")?;
        Ok(self)
    }
    pub(crate) fn add_server(&mut self, name: &str, server: GrantedServer) -> Result<&mut Self> {
        insert_unique(&mut self.servers, name, server, "server")?;
        Ok(self)
    }
    /// Record one verified D3 executable input for `server`. Two servers may
    /// legitimately share a payload — the entry merges their ties — but a
    /// checksum conflict for the same `(path, kind)` means two verifications
    /// disagreed about the same bytes, which can never be merged.
    pub(crate) fn add_executable(
        &mut self,
        path: &str,
        kind: ExecutableKind,
        checksum: ContentDigest,
        server: &str,
    ) -> Result<&mut Self> {
        match self.executables.entry((path.to_string(), kind)) {
            Entry::Vacant(slot) => {
                slot.insert(GrantedExecutable {
                    path: path.to_string(),
                    kind,
                    checksum,
                    servers: BTreeSet::from([server.to_string()]),
                });
            }
            Entry::Occupied(mut slot) => {
                if slot.get().checksum != checksum {
                    bail!(
                        "executable {path:?}: conflicting content digests for the same pinned input"
                    );
                }
                slot.get_mut().servers.insert(server.to_string());
            }
        }
        Ok(self)
    }

    pub(crate) fn add_secret(&mut self, secret: SecretGrant) -> Result<&mut Self> {
        // Reject a second authorization for the same (reference, scope) REGARDLESS
        // of lifetime, so a future RunScoped binding cannot silently coexist with
        // an Unbound one for the same secret on the same server.
        if self
            .secrets
            .iter()
            .any(|s| s.reference == secret.reference && s.scope == secret.scope)
        {
            bail!(
                "duplicate secret authorization for {:?} on that server",
                secret.reference
            );
        }
        self.secrets.insert(secret);
        Ok(self)
    }

    pub(crate) fn build(self) -> Result<AuthorityGrant> {
        // Supported posture / runtime / egress combinations only.
        match (self.posture, &self.runtime, self.egress) {
            (GrantPosture::Host, RuntimeImage::Host, EgressMode::Unconfined) => {}
            (GrantPosture::Sandbox, RuntimeImage::Container { .. }, EgressMode::ProxyAdvisory)
            | (GrantPosture::Sandbox, RuntimeImage::Container { .. }, EgressMode::NoNetwork) => {}
            (
                GrantPosture::Lockdown,
                RuntimeImage::Container { .. },
                EgressMode::LockdownConfined,
            ) => {}
            (p, r, e) => bail!(
                "unsupported posture/runtime/egress combination: {}/{}/{}",
                p.slug(),
                r.slug(),
                e.slug()
            ),
        }
        // Container reference must be non-empty when a container image is used.
        if let RuntimeImage::Container { reference, .. } = &self.runtime {
            if reference.trim().is_empty() {
                bail!("container runtime image reference must be non-empty");
            }
        }
        // Profile name must be non-empty when a profile is applied.
        match &self.invocation.profile {
            ProfileEffect::Temporary { name, .. } | ProfileEffect::Kept { name, .. }
                if name.trim().is_empty() =>
            {
                bail!("profile name must be non-empty")
            }
            _ => {}
        }
        // Git-sourced skills must carry a non-empty revision.
        for (name, skill) in &self.skills {
            if let SkillSource::Git { revision } = &skill.source {
                if revision.trim().is_empty() {
                    bail!("skill {name:?}: git source must carry a non-empty revision");
                }
            }
        }
        // Secret authority must name a frozen server that actually declares the ref.
        for s in &self.secrets {
            if s.lifetime != SecretLifetimeBinding::Unbound {
                bail!(
                    "secret {:?}: lifetime enforcement is not available yet (must be Unbound)",
                    s.reference
                );
            }
            let SecretScope::Server(server) = &s.scope;
            let granted = self.servers.get(server).ok_or_else(|| {
                anyhow::anyhow!(
                    "secret {:?} is scoped to server {server:?} which is not in the grant",
                    s.reference
                )
            })?;
            if !granted.server.referenced_secrets().contains(&s.reference) {
                bail!(
                    "secret {:?} is not referenced by its scoped server {server:?}",
                    s.reference
                );
            }
        }
        // D3 server-tied validation, both directions (contract §8):
        // an executable may only cite servers that are in the grant, and a
        // granted server that DECLARES integrity roots must have a matching
        // Root entry for each — a grant must not silently drop a declared
        // root, or the digest would bless less than the manifest demands.
        for ((path, _), exe) in &self.executables {
            for server in &exe.servers {
                if !self.servers.contains_key(server) {
                    bail!(
                        "executable {path:?} is tied to server {server:?} which is not in the grant"
                    );
                }
            }
        }
        for (name, granted) in &self.servers {
            for root in &granted.server.integrity_roots {
                let key = (
                    crate::executable::normalize_declared(root),
                    ExecutableKind::Root,
                );
                let tied = self
                    .executables
                    .get(&key)
                    .is_some_and(|exe| exe.servers.contains(name));
                if !tied {
                    bail!(
                        "server {name:?} declares integrity root {root:?} but the grant carries no verified pin for it"
                    );
                }
            }
        }
        Ok(AuthorityGrant {
            schema: GrantSchema::V1,
            project: self.project,
            invocation: self.invocation,
            skills: self.skills,
            instructions: self.instructions,
            servers: self.servers,
            executables: self.executables,
            policy: self.policy,
            secrets: self.secrets,
            runtime: self.runtime,
            posture: self.posture,
            egress: self.egress,
            workspace: WorkspaceGrant::Unbound,
            artifacts: self.artifacts,
        })
    }
}

/// Non-destructive unique insert: a duplicate leaves the existing entry intact.
fn insert_unique<V>(map: &mut BTreeMap<String, V>, name: &str, value: V, kind: &str) -> Result<()> {
    if name.trim().is_empty() {
        bail!("{kind} name must be non-empty");
    }
    if name != name.trim() {
        bail!("{kind} name {name:?} must not have surrounding whitespace");
    }
    match map.entry(name.to_string()) {
        Entry::Occupied(_) => bail!("duplicate {kind} {name:?} in grant"),
        Entry::Vacant(v) => {
            v.insert(value);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::prelude::*;

    fn with_home<T>(f: impl FnOnce() -> T) -> T {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", home.path());
        let out = f();
        std::env::remove_var("AGENTSTACK_HOME");
        out
    }

    fn write_key(bytes: &[u8], _mode: u32) {
        let path = commit_key_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(_mode)).unwrap();
        }
    }

    /// RFC 4231 Test Case 1 — proves the `hmac`/`sha2` wiring.
    #[test]
    fn hmac_sha256_matches_rfc4231_vector() {
        let mut mac = HmacSha256::new_from_slice(&[0x0bu8; 20]).unwrap();
        mac.update(b"Hi There");
        let hex: String = mac
            .finalize()
            .into_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(
            hex,
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }

    /// Fixed tag from `commit_argv` itself — locks the v1 domain, argv count,
    /// per-element length framing, and ordering (not merely the `hmac` crate).
    #[test]
    fn commit_argv_has_a_stable_v1_tag() {
        let key = CommitmentKey([0x2au8; 32]);
        let argv = ["run", "--flag", "value"].map(String::from).to_vec();
        let hex: String = commit_argv(&key, &argv)
            .0
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(
            hex,
            "c0014367ebb8954c0eb5e84e1629345e8a79df6d5032e80944ebe2fa3d4c7dc8"
        );
    }

    #[test]
    fn commit_argv_frames_elements_unambiguously() {
        let key = CommitmentKey([7u8; 32]);
        let a = commit_argv(&key, &["ab".to_string(), "c".to_string()]);
        let b = commit_argv(&key, &["a".to_string(), "bc".to_string()]);
        assert_ne!(a, b, "length framing must distinguish [ab,c] from [a,bc]");
    }

    #[test]
    fn commit_argv_binds_the_key() {
        let argv = vec!["--token".to_string(), "s3cr3t".to_string()];
        let a = commit_argv(&CommitmentKey([1u8; 32]), &argv);
        let b = commit_argv(&CommitmentKey([2u8; 32]), &argv);
        assert_ne!(a, b, "different keys must produce different commitments");
    }

    #[test]
    fn commit_argv_preserves_order() {
        let key = CommitmentKey([9u8; 32]);
        let a = commit_argv(&key, &["x".to_string(), "y".to_string()]);
        let b = commit_argv(&key, &["y".to_string(), "x".to_string()]);
        assert_ne!(
            a, b,
            "argv order is semantic and must change the commitment"
        );
    }

    #[test]
    fn sensitive_types_have_redacted_debug() {
        assert_eq!(
            format!("{:?}", CommitmentKey([0xABu8; 32])),
            "CommitmentKey(<redacted>)"
        );
        let commit = commit_argv(&CommitmentKey([3u8; 32]), &["x".to_string()]);
        let dbg = format!("{commit:?}");
        assert_eq!(dbg, "ArgvCommitment(<redacted>)");
        assert!(!dbg.contains("ab"), "no tag bytes in Debug: {dbg}");
    }

    #[test]
    fn load_missing_key_blocks_and_creates_nothing() {
        with_home(|| {
            let err = load_commitment_key().unwrap_err().to_string();
            assert!(err.contains("missing"), "{err}");
            assert!(
                !commit_key_path().exists(),
                "read-only load must not create a key"
            );
        });
    }

    #[test]
    fn load_malformed_key_blocks() {
        with_home(|| {
            write_key(&[0u8; 31], 0o600);
            let err = load_commitment_key().unwrap_err().to_string();
            assert!(err.contains("expected exactly 32 bytes"), "{err}");
        });
    }

    #[cfg(unix)]
    #[test]
    fn load_wrong_permission_key_blocks() {
        with_home(|| {
            write_key(&[0u8; 32], 0o644);
            let err = load_commitment_key().unwrap_err().to_string();
            assert!(err.contains("insecure permissions"), "{err}");
        });
    }

    #[cfg(unix)]
    #[test]
    fn load_rejects_a_symlinked_key() {
        with_home(|| {
            use std::os::unix::fs::PermissionsExt;
            let target = commit_key_path().parent().unwrap().join("real-key");
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::write(&target, [0u8; 32]).unwrap();
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
            std::os::unix::fs::symlink(&target, commit_key_path()).unwrap();
            let err = load_commitment_key().unwrap_err().to_string();
            assert!(err.contains("symlink"), "{err}");
        });
    }

    #[cfg(unix)]
    #[test]
    fn load_rejects_insecure_parent_directory() {
        with_home(|| {
            write_key(&[0u8; 32], 0o600);
            use std::os::unix::fs::PermissionsExt;
            let dir = commit_key_path().parent().unwrap().to_path_buf();
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).unwrap();
            let err = load_commitment_key().unwrap_err().to_string();
            assert!(err.contains("writable by group/other"), "{err}");
        });
    }

    #[test]
    fn load_valid_key_succeeds_and_is_usable() {
        with_home(|| {
            write_key(&[5u8; 32], 0o600);
            let key = load_commitment_key().expect("valid 0600 32-byte key loads");
            let _ = commit_argv(&key, &["x".to_string()]);
        });
    }

    #[test]
    fn provision_creates_valid_key_and_is_idempotent() {
        with_home(|| {
            assert!(!commit_key_path().exists());
            provision_commitment_key().expect("provision succeeds");
            let first = std::fs::read(commit_key_path()).unwrap();
            assert_eq!(first.len(), 32);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = std::fs::metadata(commit_key_path())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777;
                assert_eq!(mode, 0o600, "provisioned key must be 0600");
            }
            provision_commitment_key().expect("second provision is a no-op");
            let second = std::fs::read(commit_key_path()).unwrap();
            assert_eq!(first, second, "provision must never replace a valid key");
            assert!(load_commitment_key().is_ok());
        });
    }

    #[test]
    fn provision_refuses_to_replace_an_invalid_key() {
        with_home(|| {
            write_key(&[0u8; 10], 0o600); // present but invalid length
            let err = provision_commitment_key().unwrap_err().to_string();
            assert!(err.contains("refusing to overwrite"), "{err}");
            assert_eq!(
                std::fs::read(commit_key_path()).unwrap().len(),
                10,
                "an invalid key is never auto-replaced"
            );
        });
    }

    #[cfg(unix)]
    #[test]
    fn provision_refuses_symlinked_grant_directory_without_touching_target() {
        with_home(|| {
            use std::os::unix::fs::PermissionsExt;
            let grant = commit_key_path().parent().unwrap().to_path_buf();
            let home = grant.parent().unwrap().to_path_buf();
            let target = home.join("decoy");
            std::fs::create_dir_all(&target).unwrap();
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
            // `grant/` is a symlink to the decoy directory.
            std::os::unix::fs::symlink(&target, &grant).unwrap();

            let err = provision_commitment_key().unwrap_err().to_string();
            assert!(err.contains("not a real directory"), "{err}");
            let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o755, "symlinked target dir perms must be untouched");
        });
    }

    #[test]
    fn concurrent_provision_is_race_safe() {
        with_home(|| {
            let results: Vec<Result<()>> = std::thread::scope(|s| {
                let handles: Vec<_> = (0..8).map(|_| s.spawn(provision_commitment_key)).collect();
                handles.into_iter().map(|h| h.join().unwrap()).collect()
            });
            for r in &results {
                assert!(r.is_ok(), "every concurrent provision must succeed: {r:?}");
            }
            assert!(
                load_commitment_key().is_ok(),
                "exactly one valid key results"
            );
            let dir = commit_key_path().parent().unwrap().to_path_buf();
            let leftovers: Vec<_> = std::fs::read_dir(&dir)
                .unwrap()
                .filter_map(|e| e.ok())
                .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
                .collect();
            assert!(
                leftovers.is_empty(),
                "no temp files should remain: {:?}",
                leftovers.iter().map(|e| e.file_name()).collect::<Vec<_>>()
            );
        });
    }

    // ---- 3b-i witnesses ----

    fn h64(c: char) -> String {
        std::iter::repeat(c).take(64).collect()
    }

    fn granted_server(toml_src: &str, binding: GrantedServerBinding) -> GrantedServer {
        GrantedServer {
            server: toml::from_str(toml_src).unwrap(),
            binding,
        }
    }

    /// A minimal valid host-posture builder over an existing project root.
    fn host_builder(root: &GrantPath) -> GrantBuilder {
        let reg = Registry::load().unwrap();
        let adapter = GrantedAdapter::from_registry(&reg, "claude-code").unwrap();
        let invocation = Invocation {
            adapter,
            executable: HarnessExecutable {
                path: root.clone(),
                integrity: HarnessIntegrity::ExternalUnpinned,
            },
            argv: vec!["--token".into(), "s3cr3t".into()],
            cwd: root.clone(),
            profile: ProfileEffect::None,
        };
        let policy = PolicyGrant {
            ruleset: agentstack_policy::compile(
                &agentstack_core::manifest::Policy::default(),
                &agentstack_core::manifest::Policy::default(),
                &[],
            ),
            provenance: PolicyProvenance {
                machine: PolicySource::Absent,
                project: PolicySource::Absent,
            },
        };
        let project = ProjectIdentity {
            root: root.clone(),
            consent: ConsentDigest::parse(&h64('c')).unwrap(),
        };
        GrantBuilder::new(
            project,
            invocation,
            policy,
            RuntimeImage::Host,
            GrantPosture::Host,
            EgressMode::Unconfined,
            ArtifactMode::Static,
        )
    }

    #[test]
    fn digest_newtypes_parse_normalize_and_reject() {
        assert_eq!(ContentDigest::parse(&h64('a')).unwrap().hex(), h64('a'));
        assert_eq!(
            ContentDigest::parse(&format!("sha256:{}", h64('a')))
                .unwrap()
                .hex(),
            h64('a')
        );
        assert_eq!(ContentDigest::parse(&h64('A')).unwrap().hex(), h64('a'));
        assert_eq!(
            ContentDigest::parse(&h64('a')).unwrap().to_string(),
            format!("sha256:{}", h64('a'))
        );
        assert!(ContentDigest::parse("abc").is_err());
        assert!(ContentDigest::parse(&h64('g')).is_err());
    }

    #[test]
    fn grant_path_rejects_nonexistent_and_reports_containment() {
        assert!(GrantPath::new(Path::new("does/not/exist/xyz-agentstack")).is_err());
        let tmp = assert_fs::TempDir::new().unwrap();
        let root = GrantPath::new(tmp.path()).unwrap();
        assert!(Path::new(root.as_str()).is_absolute());
        let sub = tmp.child("sub");
        sub.create_dir_all().unwrap();
        let gsub = GrantPath::new(sub.path()).unwrap();
        assert!(gsub.is_within(&root));
        assert!(!root.is_within(&gsub));
    }

    #[cfg(unix)]
    #[test]
    fn grant_path_rejects_non_utf8() {
        use std::os::unix::ffi::OsStrExt;
        let p = std::ffi::OsStr::from_bytes(&[0x66, 0xff, 0x66]);
        assert!(GrantPath::new(Path::new(p)).is_err());
    }

    #[test]
    fn slug_round_trips_and_posture_parity() {
        assert_eq!(
            GrantPosture::from_slug("lockdown"),
            Some(GrantPosture::Lockdown)
        );
        assert_eq!(GrantPosture::from_slug("nope"), None);
        use crate::commands::sandbox::Posture;
        assert_eq!(GrantPosture::Host.slug(), Posture::Host.slug());
        assert_eq!(GrantPosture::Sandbox.slug(), Posture::Sandbox.slug());
        assert_eq!(GrantPosture::Lockdown.slug(), Posture::Lockdown.slug());
    }

    #[test]
    fn adapter_bound_only_from_registry() {
        let reg = Registry::load().unwrap();
        assert_eq!(
            GrantedAdapter::from_registry(&reg, "claude-code")
                .unwrap()
                .id(),
            "claude-code"
        );
        assert!(GrantedAdapter::from_registry(&reg, "no-such-adapter").is_err());
    }

    #[test]
    fn secret_grant_validates_and_orders_by_full_tuple() {
        assert!(SecretGrant::new(
            "BAD-NAME",
            SecretScope::Server("s".into()),
            SecretLifetimeBinding::Unbound
        )
        .is_err());
        assert!(SecretGrant::new(
            "TOK",
            SecretScope::Server(String::new()),
            SecretLifetimeBinding::Unbound
        )
        .is_err());
        let a = SecretGrant::new(
            "TOK",
            SecretScope::Server("a".into()),
            SecretLifetimeBinding::Unbound,
        )
        .unwrap();
        let b = SecretGrant::new(
            "TOK",
            SecretScope::Server("b".into()),
            SecretLifetimeBinding::Unbound,
        )
        .unwrap();
        assert!(a < b, "ordered by the full tuple, not the reference alone");
    }

    #[test]
    fn insert_unique_is_non_destructive() {
        let mut m: BTreeMap<String, u32> = BTreeMap::new();
        insert_unique(&mut m, "a", 1, "skill").unwrap();
        assert!(insert_unique(&mut m, "a", 2, "skill").is_err());
        assert_eq!(m["a"], 1, "first value survives the duplicate attempt");
        assert!(insert_unique(&mut m, "  ", 3, "skill").is_err());
    }

    #[test]
    fn builder_builds_valid_host_grant_with_argv_free_debug() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let root = GrantPath::new(tmp.path()).unwrap();
        let grant = host_builder(&root).build().unwrap();
        let dbg = format!("{grant:?}");
        assert!(dbg.contains("argv_args: 2"), "{dbg}");
        assert!(!dbg.contains("s3cr3t"), "no argv bytes in Debug: {dbg}");
    }

    #[test]
    fn builder_ties_executables_to_servers_both_directions() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let root = GrantPath::new(tmp.path()).unwrap();

        // An executable tied to a server outside the grant fails build().
        let mut b = host_builder(&root);
        b.add_executable(
            "scripts/run.sh",
            ExecutableKind::File,
            ContentDigest::parse(&h64('a')).unwrap(),
            "ghost",
        )
        .unwrap();
        let err = b.build().unwrap_err().to_string();
        assert!(err.contains("ghost"), "{err}");

        // A server declaring an integrity root with no verified pin in the
        // grant fails build() — a grant must not bless less than the manifest
        // demands. The declared "./tools" and the pin key "tools" normalize
        // to the same entry.
        let declares_root =
            "type = \"stdio\"\ncommand = \"python\"\nintegrity_roots = [\"./tools\"]\n";
        let mut b = host_builder(&root);
        b.add_server(
            "agent",
            granted_server(
                declares_root,
                GrantedServerBinding::Inline {
                    definition: ContentDigest::parse(&h64('d')).unwrap(),
                },
            ),
        )
        .unwrap();
        let err = b.build().unwrap_err().to_string();
        assert!(err.contains("integrity root"), "{err}");

        // With the pin present and TWO distinct servers sharing it, build()
        // passes and the entry merges both ties into one; a conflicting
        // digest for the same (path, kind) can never merge.
        let mut b = host_builder(&root);
        for name in ["agent", "sidekick"] {
            b.add_server(
                name,
                granted_server(
                    declares_root,
                    GrantedServerBinding::Inline {
                        definition: ContentDigest::parse(&h64('d')).unwrap(),
                    },
                ),
            )
            .unwrap();
            b.add_executable(
                "tools",
                ExecutableKind::Root,
                ContentDigest::parse(&h64('e')).unwrap(),
                name,
            )
            .unwrap();
        }
        assert!(b
            .add_executable(
                "tools",
                ExecutableKind::Root,
                ContentDigest::parse(&h64('f')).unwrap(),
                "agent",
            )
            .is_err());
        let grant = b.build().unwrap();
        let exe = &grant.executables[&("tools".to_string(), ExecutableKind::Root)];
        assert_eq!(
            exe.servers.len(),
            2,
            "one merged entry ties both servers: {:?}",
            exe.servers
        );
        assert!(format!("{grant:?}").contains("executables: 1"));
    }

    #[test]
    fn builder_rejects_duplicate_skill_non_destructively() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let root = GrantPath::new(tmp.path()).unwrap();
        let mut b = host_builder(&root);
        let first = GrantedSkill {
            path: root.clone(),
            origin: InputOrigin::Inline,
            source: SkillSource::Path,
            checksum: ContentDigest::parse(&h64('a')).unwrap(),
            provenance: None,
        };
        let second = GrantedSkill {
            checksum: ContentDigest::parse(&h64('b')).unwrap(),
            ..first.clone()
        };
        b.add_skill("s", first).unwrap();
        assert!(b.add_skill("s", second).is_err());
        let grant = b.build().unwrap();
        assert_eq!(grant.skills["s"].checksum.hex(), h64('a'));
    }

    #[test]
    fn builder_rejects_unsupported_posture_combo() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let root = GrantPath::new(tmp.path()).unwrap();
        let mut b = host_builder(&root);
        b.egress = EgressMode::NoNetwork; // host + no-network is not supported
        assert!(b.build().is_err());
    }

    #[test]
    fn builder_rejects_empty_git_revision() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let root = GrantPath::new(tmp.path()).unwrap();
        let mut b = host_builder(&root);
        b.add_skill(
            "s",
            GrantedSkill {
                path: root.clone(),
                origin: InputOrigin::Library,
                source: SkillSource::Git {
                    revision: "   ".into(),
                },
                checksum: ContentDigest::parse(&h64('a')).unwrap(),
                provenance: None,
            },
        )
        .unwrap();
        assert!(b.build().is_err());
    }

    #[test]
    fn builder_rejects_secret_scoped_to_absent_server() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let root = GrantPath::new(tmp.path()).unwrap();
        let mut b = host_builder(&root);
        b.add_secret(
            SecretGrant::new(
                "TOK",
                SecretScope::Server("ghost".into()),
                SecretLifetimeBinding::Unbound,
            )
            .unwrap(),
        )
        .unwrap();
        assert!(b.build().is_err(), "secret names a server not in the grant");
    }

    #[test]
    fn builder_rejects_secret_ref_not_declared_by_server() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let root = GrantPath::new(tmp.path()).unwrap();
        let mut b = host_builder(&root);
        b.add_server(
            "s",
            granted_server(
                "type = \"stdio\"\ncommand = \"run ${OTHER}\"\n",
                GrantedServerBinding::Inline {
                    definition: ContentDigest::parse(&h64('a')).unwrap(),
                },
            ),
        )
        .unwrap();
        b.add_secret(
            SecretGrant::new(
                "TOK",
                SecretScope::Server("s".into()),
                SecretLifetimeBinding::Unbound,
            )
            .unwrap(),
        )
        .unwrap();
        assert!(
            b.build().is_err(),
            "ref not referenced by its scoped server"
        );
    }

    #[test]
    fn builder_accepts_secret_declared_by_scoped_server() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let root = GrantPath::new(tmp.path()).unwrap();
        let mut b = host_builder(&root);
        b.add_server(
            "s",
            granted_server(
                "type = \"stdio\"\ncommand = \"run ${TOK}\"\n",
                GrantedServerBinding::Inline {
                    definition: ContentDigest::parse(&h64('a')).unwrap(),
                },
            ),
        )
        .unwrap();
        b.add_secret(
            SecretGrant::new(
                "TOK",
                SecretScope::Server("s".into()),
                SecretLifetimeBinding::Unbound,
            )
            .unwrap(),
        )
        .unwrap();
        assert!(b.build().is_ok());
    }

    #[test]
    fn add_secret_rejects_same_reference_and_scope_regardless_of_lifetime() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let root = GrantPath::new(tmp.path()).unwrap();
        let mut b = host_builder(&root);
        b.add_secret(
            SecretGrant::new(
                "TOK",
                SecretScope::Server("s".into()),
                SecretLifetimeBinding::Unbound,
            )
            .unwrap(),
        )
        .unwrap();
        // Same (reference, scope), different lifetime — must be rejected so two
        // contradictory authorizations can never coexist.
        assert!(b
            .add_secret(
                SecretGrant::new(
                    "TOK",
                    SecretScope::Server("s".into()),
                    SecretLifetimeBinding::RunScoped,
                )
                .unwrap()
            )
            .is_err());
    }

    #[test]
    fn builder_accepts_sandbox_container_combo() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let root = GrantPath::new(tmp.path()).unwrap();
        let mut b = host_builder(&root);
        b.runtime = RuntimeImage::Container {
            reference: "example.com/img@sha256:abc".into(),
            binding: ImageBinding::Unbound,
        };
        b.posture = GrantPosture::Sandbox;
        b.egress = EgressMode::ProxyAdvisory;
        assert!(b.build().is_ok());
    }

    #[test]
    fn builder_rejects_empty_container_reference() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let root = GrantPath::new(tmp.path()).unwrap();
        let mut b = host_builder(&root);
        b.runtime = RuntimeImage::Container {
            reference: "   ".into(),
            binding: ImageBinding::Unbound,
        };
        b.posture = GrantPosture::Sandbox;
        b.egress = EgressMode::NoNetwork;
        assert!(b.build().is_err());
    }

    #[test]
    fn builder_rejects_empty_profile_name() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let root = GrantPath::new(tmp.path()).unwrap();
        let mut b = host_builder(&root);
        b.invocation.profile = ProfileEffect::Temporary {
            name: "  ".into(),
            scope: Scope::Project,
        };
        assert!(b.build().is_err());
    }
}
