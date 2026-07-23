//! direnv-style trust store for zero-files auto mode.
//!
//! A globally registered `agentstack mcp --auto-project` bridge discovers
//! whatever manifest the current project carries. Auto-loading that manifest's
//! servers would let any cloned repo spawn stdio commands and receive secrets —
//! so discovery is gated: a project's runtime surface stays control-plane-only
//! until a human runs `agentstack trust`, and trust is pinned to the content
//! digest of the manifest layers plus `agentstack.lock` (which pins the
//! definition digests of library-referenced servers). Change any of them (a
//! `git pull`, say) and the project must be re-trusted, exactly like `direnv
//! allow`.

#![forbid(unsafe_code)]

pub mod sign;

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use agentstack_core::lock::LOCK_FILE;
use agentstack_core::manifest::load::{LOCAL_FILE, MANIFEST_FILE};
use agentstack_core::util::paths;

const TRUST_DIGEST_DOMAIN: &[u8] = b"agentstack-trust-digest-v3\0";

/// The reviewed crate gets a closed error enum instead of `anyhow` (rule 6):
/// every failure a caller can see is named here, nothing is stringly ad-hoc.
/// `thiserror` derives `Display` from the `#[error]` attributes and
/// `std::error::Error` for free — the Rust analogue of a TS discriminated
/// union of failure cases. The cli's `anyhow` call sites keep working because
/// `?` auto-converts any `std::error::Error` into `anyhow::Error`.
#[derive(Debug, thiserror::Error)]
pub enum TrustError {
    /// The project has no manifest — there is nothing to pin, so there is
    /// nothing to trust.
    #[error("no agentstack manifest under {}", base.display())]
    NoManifest { base: PathBuf },
    /// The trust store could not be serialized or written to disk. Carries the
    /// underlying error's rendered text (the writer in `core` has its own
    /// error type; we keep only its message so this crate's dependency list
    /// stays the strict one).
    #[error("saving trust store: {0}")]
    Store(String),
    /// The consented digest does not match the bytes being granted (§7.2 of
    /// the UI control-plane design): the surface a human previewed is not the
    /// surface on disk now, so the grant refuses — nothing is written.
    #[error(
        "consented digest does not match the current surface — the manifest/lock changed since the preview (consented {consented}, current {actual}); re-run the preview and review again"
    )]
    ConsentMismatch { consented: String, actual: String },
}

pub type Result<T> = std::result::Result<T, TrustError>;

/// Where trust decisions live: `~/.agentstack/trust.json`.
///
/// Format note (2026-07-11, rule-6 sweep): the store moved from `trust.toml`
/// to JSON so this crate needs no TOML parser. Deliberately NO migration shim
/// (no external users): a leftover `trust.toml` is ignored, which fails
/// CLOSED — every project simply reads as untrusted until re-trusted.
pub fn store_path() -> PathBuf {
    paths::agentstack_home().join("trust.json")
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct TrustStore {
    /// Canonical project base dir → the trust decision for it.
    #[serde(default)]
    pub trusted: IndexMap<String, TrustEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustEntry {
    /// `sha256:<hex>` over the manifest (+ local overlay + lockfile) at trust
    /// time.
    pub digest: String,
    pub trusted_at: u64,
    /// The reviewed loadable surface at trust time, for re-trust diffing (P14).
    ///
    /// Additive and optional: entries written before this field simply
    /// deserialize to `None` (`serde(default)`), and a grant that records no
    /// snapshot serializes nothing extra (`skip_serializing_if`), so older
    /// stores round-trip byte-for-byte. It is *display metadata only* — never
    /// folded into [`digest_for`], so it cannot change what re-gates a project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface: Option<Vec<SurfaceItem>>,
}

/// One reviewed item of a project's loadable surface, captured at trust time so
/// a later re-trust can mark it `+ added` / `~ changed` / `- removed` against
/// the last consented set instead of re-listing everything flat (P14).
///
/// `identity` is exactly what the review shows for the item — a server's
/// command line, an HTTP url, an extension's target — NOT its pin/lock status:
/// pin drift is already a hard blocker, so the diff tracks *what the human
/// agreed to run/contact*, not whether it happens to be locked right now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceItem {
    pub kind: String,
    pub name: String,
    pub identity: String,
}

/// What a prior `trust` recorded for a project, for re-trust diffing (P14).
/// The three cases the review must tell apart — independent of digest match,
/// so a re-trust after a manifest edit still diffs:
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PriorSurface {
    /// No trust entry at all — first-ever trust: show the flat full review.
    NeverTrusted,
    /// An entry exists but predates surface snapshots (an older trust): show
    /// the flat review plus one line saying there is nothing to diff against.
    Untracked,
    /// A prior surface was recorded — diff the current review against it.
    Recorded(Vec<SurfaceItem>),
}

/// Where a project stands with the zero-files bridge.
// `Copy`: all variants are data-free, so copying is a register move — callers
// compare it by value (`self.trust == Some(TrustState::Trusted)`) without the
// `.as_ref()`/`&` dance a non-Copy enum forces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustState {
    /// Trusted and the manifest is byte-identical to what was trusted.
    Trusted,
    /// Trusted once, but the manifest changed since — re-review + re-trust.
    Changed,
    /// Never trusted on this machine.
    Untrusted,
}

impl TrustStore {
    pub fn load() -> TrustStore {
        let Ok(text) = std::fs::read_to_string(store_path()) else {
            return TrustStore::default();
        };
        // A corrupt store parses as the EMPTY store — fail closed: everything
        // reads untrusted until a human re-trusts it.
        serde_json::from_str(&text).unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let text =
            serde_json::to_string_pretty(self).map_err(|e| TrustError::Store(e.to_string()))?;
        agentstack_core::util::atomic::write(&store_path(), &text)
            .map_err(|e| TrustError::Store(format!("{e:#}")))
    }
}

/// The trust key for a project: its canonicalized base dir (the dir holding
/// `.agentstack/` or a legacy root manifest — NOT the manifest dir itself).
pub fn key_for(base: &Path) -> String {
    std::fs::canonicalize(base)
        .unwrap_or_else(|_| base.to_path_buf())
        .display()
        .to_string()
}

/// The consent surface read ONCE as immutable bytes: the manifest, the local
/// overlay, and the lockfile. A caller that must both *display* the surface
/// (parse) and *identify* it (digest) derives both from one snapshot, closing
/// the read–reread window in which a mid-preview edit could pair an old
/// display with a new digest (UI control-plane §7.2). Absent overlay/lock
/// files are framed distinctly from present-but-empty ones (v3) — and
/// `digest_for` IS this snapshot's digest, so the two can never diverge.
#[derive(Debug)]
pub struct ConsentSnapshot {
    pub manifest: Vec<u8>,
    pub local: Option<Vec<u8>>,
    pub lock: Option<Vec<u8>>,
}

impl ConsentSnapshot {
    /// Read the three pinned files at `base` in one pass. `None` when there
    /// is no readable manifest — nothing to consent to.
    pub fn read(base: &Path) -> Option<ConsentSnapshot> {
        let dir = agentstack_core::manifest::resolve_manifest_dir(base);
        let manifest = std::fs::read(dir.join(MANIFEST_FILE)).ok()?;
        let local = std::fs::read(dir.join(LOCAL_FILE)).ok();
        let lock = std::fs::read(dir.join(LOCK_FILE)).ok();
        Some(ConsentSnapshot {
            manifest,
            local,
            lock,
        })
    }

    /// The consent digest over exactly these captured bytes — disk edits after
    /// the snapshot cannot change it.
    pub fn digest(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(TRUST_DIGEST_DOMAIN);
        for segment in [
            Some(self.manifest.as_slice()),
            self.local.as_deref(),
            self.lock.as_deref(),
        ] {
            // Each segment is framed as presence byte + length + bytes: the
            // length prefix makes file boundaries unambiguous, and the
            // presence byte distinguishes an ABSENT overlay/lock from a
            // present zero-byte file (v3) — creating an empty
            // `agentstack.lock` after consent must re-gate like any other
            // byte change, not collide with "no lock at all".
            match segment {
                Some(bytes) => {
                    hasher.update([1u8]);
                    hasher.update((bytes.len() as u64).to_le_bytes());
                    hasher.update(bytes);
                }
                None => hasher.update([0u8]),
            }
        }
        format!("sha256:{:x}", hasher.finalize())
    }
}

/// Content digest of the consent surface at `base`: the manifest layers
/// (`agentstack.toml` plus the `agentstack.local.toml` overlay, both of which
/// declare runnable servers) and `agentstack.lock`, which pins the definition
/// digests of library-referenced servers the gateway will serve. Re-pinning
/// the lock changes what a name ref runs, so it re-gates the project exactly
/// like a manifest edit. `None` when there is no manifest.
pub fn digest_for(base: &Path) -> Option<String> {
    Some(ConsentSnapshot::read(base)?.digest())
}

/// Where `base` stands right now (digest recomputed against the store).
pub fn check(base: &Path) -> TrustState {
    check_digest(base, digest_for(base).as_deref())
}

/// Where `base` stands for a GIVEN current-content digest (`None` = no
/// manifest). The seam that lets a [`ConsentSnapshot`] holder evaluate trust
/// state against the same bytes it displays and digests, instead of a third
/// disk read; [`check`] is this over `digest_for`, so state semantics keep
/// one implementation.
pub fn check_digest(base: &Path, digest: Option<&str>) -> TrustState {
    let store = TrustStore::load();
    let Some(entry) = store.trusted.get(&key_for(base)) else {
        return TrustState::Untrusted;
    };
    match digest {
        Some(d) if d == entry.digest => TrustState::Trusted,
        // Manifest gone or rewritten since trust — either way, re-review.
        _ => TrustState::Changed,
    }
}

/// Test-fixture grant: record trust for `base` at whatever its manifest
/// digests to RIGHT NOW, with no review and no consent binding. This exists
/// so integration tests can put a temp project into the trusted state in one
/// line. Production command paths must never call it — they go through
/// [`trust_reviewed`] (a digest the caller's rendered review derived) or
/// [`trust_with_consent`] (a digest a previewing human presented back); the
/// name is deliberately greppable so a review catches any new caller.
pub fn trust_unreviewed(base: &Path) -> Result<String> {
    let digest = digest_for(base).ok_or_else(|| TrustError::NoManifest {
        base: base.to_path_buf(),
    })?;
    store_entry(base, digest.clone(), None)?;
    Ok(digest)
}

/// Record trust at `digest` — the digest of the exact byte snapshot whose
/// review the caller just rendered — plus the reviewed surface for re-trust
/// diffing (P14). No disk re-read happens here: if the files changed after
/// the caller's snapshot, the store holds the SNAPSHOT digest, the project
/// immediately reads as `Changed`, and every use site fails closed — the
/// same fail-closed shape as [`trust_with_consent`], closing the window in
/// which an interactive review could bless bytes the human never saw.
pub fn trust_reviewed(base: &Path, digest: String, surface: Vec<SurfaceItem>) -> Result<()> {
    store_entry(base, digest, Some(surface))
}

/// Re-pin an EXISTING trust entry to `digest` — the digest of bytes the
/// caller itself just wrote (an owned-manifest refresh), computed from the
/// written content, never from a disk re-read. Preserves the recorded
/// reviewed surface so re-trust diffing keeps its baseline. Returns `false`
/// (writing nothing) when no entry exists: re-pinning must never CREATE
/// trust, only carry valid trust across agentstack's own rewrite.
pub fn repin(base: &Path, digest: String) -> Result<bool> {
    with_store_lock(|| {
        let mut store = TrustStore::load();
        let Some(entry) = store.trusted.get_mut(&key_for(base)) else {
            return Ok(false);
        };
        entry.digest = digest;
        entry.trusted_at = now_secs();
        store.save()?;
        Ok(true)
    })
}

/// Consent-bound grant (UI control-plane §7.2): record trust only if the
/// current content digest equals `consented` — the digest a human received
/// from `trust --preview` alongside the surface they reviewed. Enforced HERE,
/// at the store-write point, so "a human reviewed this exact surface" holds
/// even when the caller is a headless RPC server and no UI was in the loop:
/// both the preview and this check compute the same [`digest_for`] over the
/// same pinned bytes — no second source of truth — and any byte changed
/// between preview and grant flips the digest and refuses the write.
pub fn trust_with_consent(
    base: &Path,
    surface: Vec<SurfaceItem>,
    consented: &str,
) -> Result<String> {
    let actual = digest_for(base).ok_or_else(|| TrustError::NoManifest {
        base: base.to_path_buf(),
    })?;
    if consented != actual {
        return Err(TrustError::ConsentMismatch {
            consented: consented.to_string(),
            actual,
        });
    }
    // Record the digest we just VERIFIED, not a re-read of disk: if a byte
    // changes between this check and the write, the store then holds the
    // consented digest, the project reads as Changed, and every use site
    // fails closed — instead of silently blessing bytes nobody reviewed.
    store_entry(base, actual.clone(), Some(surface))?;
    Ok(actual)
}

/// The reviewed surface a prior `trust` recorded for `base` — the input to
/// re-trust diffing (P14). Independent of digest match: a re-trust after a
/// manifest edit still diffs against the last consented set.
pub fn prior_surface(base: &Path) -> PriorSurface {
    let store = TrustStore::load();
    match store.trusted.get(&key_for(base)) {
        None => PriorSurface::NeverTrusted,
        Some(entry) => match &entry.surface {
            None => PriorSurface::Untracked,
            Some(items) => PriorSurface::Recorded(items.clone()),
        },
    }
}

/// The single store-write for a grant: pin `base` at exactly `digest`. Split
/// out so the consent path can record the digest it verified rather than
/// re-reading disk (see [`trust_with_consent`]).
fn store_entry(base: &Path, digest: String, surface: Option<Vec<SurfaceItem>>) -> Result<()> {
    with_store_lock(|| {
        let mut store = TrustStore::load();
        store.trusted.insert(
            key_for(base),
            TrustEntry {
                digest,
                trusted_at: now_secs(),
                surface,
            },
        );
        store.save()
    })
}

/// Remove trust for `base`. Returns whether an entry existed.
pub fn revoke(base: &Path) -> Result<bool> {
    with_store_lock(|| {
        let mut store = TrustStore::load();
        let existed = store.trusted.shift_remove(&key_for(base)).is_some();
        if existed {
            store.save()?;
        }
        Ok(existed)
    })
}

/// Serialize every load→modify→save of the whole-file trust store across
/// processes, so a concurrent grant can never resurrect an entry a racing
/// revoke just removed (each writer would otherwise save its own stale copy
/// of the entire map). `create_dir` is the atomic primitive — it either
/// creates the sentinel or fails because it exists — giving mutual exclusion
/// from the standard library alone, no new dependency. A sentinel older than
/// [`STORE_LOCK_STALE`] is treated as a crashed writer and broken; a healthy
/// writer holds it for the few milliseconds one read+write takes, so the
/// bounded wait fails (closed, no store write) only under real contention.
fn with_store_lock<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    const STORE_LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(5);
    const STORE_LOCK_STALE: std::time::Duration = std::time::Duration::from_secs(30);
    let lock_dir = paths::agentstack_home().join("trust.lock.d");
    let deadline = std::time::Instant::now() + STORE_LOCK_WAIT;
    loop {
        match std::fs::create_dir(&lock_dir) {
            Ok(()) => break,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let stale = std::fs::metadata(&lock_dir)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .is_some_and(|age| age > STORE_LOCK_STALE);
                if stale {
                    // Best-effort: losing this race just means retrying.
                    let _ = std::fs::remove_dir(&lock_dir);
                    continue;
                }
                if std::time::Instant::now() >= deadline {
                    return Err(TrustError::Store(format!(
                        "trust store is locked by another agentstack process ({} exists) — retry, or remove it if no other process is running",
                        lock_dir.display()
                    )));
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // First write on this machine: the home dir itself is missing.
                std::fs::create_dir_all(paths::agentstack_home())
                    .map_err(|err| TrustError::Store(err.to_string()))?;
            }
            Err(e) => return Err(TrustError::Store(e.to_string())),
        }
    }
    let out = f();
    let _ = std::fs::remove_dir(&lock_dir);
    out
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::prelude::*;

    fn with_home<T>(f: impl FnOnce(&assert_fs::TempDir) -> T) -> T {
        let _guard = agentstack_core::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", home.path());
        let out = f(&home);
        std::env::remove_var("AGENTSTACK_HOME");
        out
    }

    fn project_with_manifest() -> assert_fs::TempDir {
        let proj = assert_fs::TempDir::new().unwrap();
        proj.child(".agentstack/agentstack.toml")
            .write_str("version = 1\n[servers.x]\ntype = \"http\"\nurl = \"https://x/mcp\"\n")
            .unwrap();
        proj
    }

    #[test]
    fn snapshot_digest_is_immutable_and_equals_the_path_digest() {
        with_home(|_| {
            let proj = project_with_manifest();
            let snap = ConsentSnapshot::read(proj.path()).unwrap();
            // Equivalence: digest_for IS the snapshot digest — one
            // implementation, two entry points, so they can never diverge.
            assert_eq!(Some(snap.digest()), digest_for(proj.path()));

            // §7.2 witness: a disk edit AFTER the snapshot changes the path
            // digest but never the snapshot's — a preview that derives both
            // its display and its digest from one snapshot cannot pair an old
            // display with a new digest, whatever the edit interleaving.
            proj.child(".agentstack/agentstack.toml")
                .write_str("version = 1\n[servers.evil]\ntype = \"stdio\"\ncommand = \"sh\"\n")
                .unwrap();
            assert_ne!(Some(snap.digest()), digest_for(proj.path()));
        });
    }

    #[test]
    fn trust_then_check_then_change_then_revoke() {
        with_home(|_| {
            let proj = project_with_manifest();
            assert_eq!(check(proj.path()), TrustState::Untrusted);

            trust_unreviewed(proj.path()).unwrap();
            assert_eq!(check(proj.path()), TrustState::Trusted);

            // Any manifest edit invalidates trust (direnv semantics).
            proj.child(".agentstack/agentstack.toml")
                .write_str("version = 1\n[servers.evil]\ntype = \"stdio\"\ncommand = \"sh\"\n")
                .unwrap();
            assert_eq!(check(proj.path()), TrustState::Changed);

            // Re-trusting the new content restores it; revoking clears it.
            trust_unreviewed(proj.path()).unwrap();
            assert_eq!(check(proj.path()), TrustState::Trusted);
            assert!(revoke(proj.path()).unwrap());
            assert_eq!(check(proj.path()), TrustState::Untrusted);
        });
    }

    #[test]
    fn local_overlay_participates_in_the_digest() {
        with_home(|_| {
            let proj = project_with_manifest();
            trust_unreviewed(proj.path()).unwrap();
            // The gitignored overlay also declares servers — adding one must
            // invalidate trust too.
            proj.child(".agentstack/agentstack.local.toml")
                .write_str("[servers.local]\ntype = \"stdio\"\ncommand = \"sh\"\n")
                .unwrap();
            assert_eq!(check(proj.path()), TrustState::Changed);
        });
    }

    #[test]
    fn lockfile_participates_in_the_digest() {
        with_home(|_| {
            let proj = project_with_manifest();
            trust_unreviewed(proj.path()).unwrap();
            // The lock pins the library server definitions the gateway will
            // run — re-pinning changes the runtime surface, so it re-gates
            // exactly like a manifest edit.
            proj.child(".agentstack/agentstack.lock")
                .write_str(
                    "version = 1\n[[server]]\nname = \"kibana\"\nsource = \"library\"\nchecksum = \"sha256:aaa\"\n",
                )
                .unwrap();
            assert_eq!(check(proj.path()), TrustState::Changed);
        });
    }

    // P14: the reviewed surface round-trips through the store, and the three
    // prior-surface cases are distinguished — while the snapshot stays out of
    // the digest, so recording one must NOT re-gate the project.
    #[test]
    fn surface_snapshot_round_trips_and_stays_out_of_the_digest() {
        with_home(|_| {
            let proj = project_with_manifest();
            // First trust with no snapshot (an "older" entry) reads as Untracked.
            trust_unreviewed(proj.path()).unwrap();
            assert_eq!(prior_surface(proj.path()), PriorSurface::Untracked);
            let digest_flat = check_digest(proj.path());

            // Re-trust WITH a surface: it persists and reads back identically…
            let surface = vec![
                SurfaceItem {
                    kind: "server".into(),
                    name: "evil".into(),
                    identity: "sh -c pwn".into(),
                },
                SurfaceItem {
                    kind: "skill".into(),
                    name: "greet".into(),
                    identity: "library".into(),
                },
            ];
            let reviewed = digest_for(proj.path()).unwrap();
            trust_reviewed(proj.path(), reviewed, surface.clone()).unwrap();
            assert_eq!(prior_surface(proj.path()), PriorSurface::Recorded(surface));
            // …and the digest is unchanged — the snapshot is display-only, so
            // the project stays Trusted rather than re-gating.
            assert_eq!(check(proj.path()), TrustState::Trusted);
            assert_eq!(check_digest(proj.path()), digest_flat);

            // A never-trusted project reports NeverTrusted.
            let untouched = project_with_manifest();
            assert_eq!(prior_surface(untouched.path()), PriorSurface::NeverTrusted);
        });
    }

    // SECURITY WITNESS (trust granting, UI control-plane §7.2): the consent-
    // digest binding. A grant presented with a digest that does not match the
    // bytes on disk must refuse and leave the store untouched — this is what
    // makes "a human reviewed this exact surface" a CLI-enforced guarantee
    // instead of a UI-rendered one. NEVER delete or weaken this test.
    #[test]
    fn consent_grant_refuses_mismatched_digest_and_binds_to_reviewed_bytes() {
        with_home(|_| {
            let proj = project_with_manifest();
            let previewed = digest_for(proj.path()).unwrap();

            // (a) A wrong/stale digest refuses, and nothing was granted.
            let err = trust_with_consent(proj.path(), Vec::new(), "sha256:deadbeef").unwrap_err();
            assert!(matches!(err, TrustError::ConsentMismatch { .. }));
            assert_eq!(check(proj.path()), TrustState::Untrusted);

            // (b) The previewed digest grants, pinned at exactly that digest.
            let granted = trust_with_consent(proj.path(), Vec::new(), &previewed).unwrap();
            assert_eq!(granted, previewed);
            assert_eq!(check(proj.path()), TrustState::Trusted);

            // (c) The preview-then-edit race: bytes change after the preview,
            // so the old digest no longer matches — the grant refuses.
            proj.child(".agentstack/agentstack.toml")
                .write_str("version = 1\n[servers.evil]\ntype = \"stdio\"\ncommand = \"sh\"\n")
                .unwrap();
            let err = trust_with_consent(proj.path(), Vec::new(), &previewed).unwrap_err();
            assert!(matches!(err, TrustError::ConsentMismatch { .. }));
            // The earlier grant is still pinned to the OLD bytes, so the
            // edited project reads as Changed — fail closed, not blessed.
            assert_eq!(check(proj.path()), TrustState::Changed);
        });
    }

    /// The digest currently recorded for `base` in the store, for asserting the
    /// snapshot leaves it untouched.
    fn check_digest(base: &Path) -> String {
        TrustStore::load()
            .trusted
            .get(&key_for(base))
            .unwrap()
            .digest
            .clone()
    }

    #[test]
    fn digest_is_stable_for_identical_inputs() {
        let proj = project_with_manifest();

        assert_eq!(digest_for(proj.path()), digest_for(proj.path()));
    }

    #[test]
    fn digest_frames_manifest_and_local_as_distinct_segments() {
        let first = assert_fs::TempDir::new().unwrap();
        first
            .child(".agentstack/agentstack.toml")
            .write_binary(b"")
            .unwrap();
        first
            .child(".agentstack/agentstack.local.toml")
            .write_binary(b"\0")
            .unwrap();

        let second = assert_fs::TempDir::new().unwrap();
        second
            .child(".agentstack/agentstack.toml")
            .write_binary(b"\0")
            .unwrap();
        second
            .child(".agentstack/agentstack.local.toml")
            .write_binary(b"")
            .unwrap();

        assert_ne!(digest_for(first.path()), digest_for(second.path()));
    }

    /// v3 presence framing: an ABSENT lockfile and a present ZERO-BYTE
    /// lockfile are different consent surfaces — creating an empty
    /// `agentstack.lock` after a grant must re-gate the project (review
    /// finding: absent and empty previously collided). NEVER weaken this.
    #[test]
    fn absent_and_empty_pinned_files_digest_differently() {
        with_home(|_| {
            let proj = project_with_manifest();
            let before = digest_for(proj.path()).unwrap();
            trust_unreviewed(proj.path()).unwrap();
            assert_eq!(check(proj.path()), TrustState::Trusted);

            proj.child(".agentstack/agentstack.lock")
                .write_binary(b"")
                .unwrap();
            let after = digest_for(proj.path()).unwrap();
            assert_ne!(before, after, "empty lock must change the digest");
            assert_eq!(check(proj.path()), TrustState::Changed);
        });
    }

    /// `trust_reviewed` stores the CALLER's digest with no disk re-read: when
    /// disk changed after the caller's snapshot, the store holds the snapshot
    /// digest and the project reads Changed — the interactive-grant race
    /// fails closed instead of blessing unseen bytes. NEVER weaken this.
    #[test]
    fn trust_reviewed_pins_the_snapshot_digest_not_current_disk() {
        with_home(|_| {
            let proj = project_with_manifest();
            let reviewed = digest_for(proj.path()).unwrap();
            // The mid-review edit: bytes change AFTER the review rendered.
            proj.child(".agentstack/agentstack.toml")
                .write_str(
                    "version = 1\n[servers.evil]\ntype = \"http\"\nurl = \"https://evil/mcp\"\n",
                )
                .unwrap();
            trust_reviewed(proj.path(), reviewed, Vec::new()).unwrap();
            // The swapped-in bytes are NOT blessed.
            assert_eq!(check(proj.path()), TrustState::Changed);
        });
    }

    /// `repin` carries valid trust across agentstack's own rewrite: it never
    /// creates an entry, and it preserves the recorded reviewed surface.
    #[test]
    fn repin_updates_existing_entry_only_and_preserves_surface() {
        with_home(|_| {
            let proj = project_with_manifest();
            // No entry: repin refuses to create one.
            assert!(!repin(proj.path(), "sha256:beef".into()).unwrap());
            assert_eq!(check(proj.path()), TrustState::Untrusted);

            let surface = vec![SurfaceItem {
                kind: "server".into(),
                name: "x".into(),
                identity: "https://x/mcp".into(),
            }];
            let reviewed = digest_for(proj.path()).unwrap();
            trust_reviewed(proj.path(), reviewed, surface.clone()).unwrap();

            proj.child(".agentstack/agentstack.toml")
                .write_str("version = 1\n")
                .unwrap();
            let refreshed = digest_for(proj.path()).unwrap();
            assert!(repin(proj.path(), refreshed).unwrap());
            assert_eq!(check(proj.path()), TrustState::Trusted);
            // P14 baseline survives the re-pin.
            assert_eq!(prior_surface(proj.path()), PriorSurface::Recorded(surface));
        });
    }

    #[test]
    fn no_manifest_means_no_digest_and_trust_errors() {
        with_home(|_| {
            let empty = assert_fs::TempDir::new().unwrap();
            assert!(digest_for(empty.path()).is_none());
            assert!(trust_unreviewed(empty.path()).is_err());
            assert_eq!(check(empty.path()), TrustState::Untrusted);
        });
    }

    // ── Property test: the re-gate invariant (CLAUDE.md rule 4) ────────────
    // NEVER delete or weaken this test. It is the machine-checked form of
    // "any pinned byte changes → bundle re-gates": for ALL contents of the
    // pinned files, ALL choices of file, ALL byte positions, and ALL nonzero
    // bit patterns, flipping that one byte demotes Trusted to Changed.
    //
    // How proptest works, for the record: a `Strategy` is a value generator
    // (like fast-check arbitraries in the TS world). `proptest!` runs the
    // test body against many generated inputs, and when a case fails it
    // *shrinks* — re-runs with progressively simpler inputs (shorter files,
    // index 0, delta 1) and reports the minimal failing case instead of a
    // random haystack. `prop_flat_map` builds dependent generators: the
    // flip index must be generated *after* (and within) the chosen file's
    // length, so the second stage's ranges depend on the first stage's
    // output.

    use proptest::prelude::*;

    /// (manifest, local, lock, which file to corrupt, byte index, xor delta).
    /// All three files non-empty so every (which, idx) pair is valid; delta
    /// is drawn from 1..=255 so `byte ^ delta` is guaranteed to differ.
    fn pinned_surface() -> impl Strategy<Value = (Vec<u8>, Vec<u8>, Vec<u8>, usize, usize, u8)> {
        (
            prop::collection::vec(any::<u8>(), 1..256),
            prop::collection::vec(any::<u8>(), 1..256),
            prop::collection::vec(any::<u8>(), 1..256),
            0usize..3,
            1u8..=255u8,
        )
            .prop_flat_map(|(manifest, local, lock, which, delta)| {
                let len = [manifest.len(), local.len(), lock.len()][which];
                (
                    Just(manifest),
                    Just(local),
                    Just(lock),
                    Just(which),
                    0..len,
                    Just(delta),
                )
            })
    }

    proptest! {
        // Each case touches the real filesystem (tempdir + env var), so run
        // fewer, bigger cases than proptest's default 256.
        #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

        #[test]
        fn any_single_byte_flip_in_any_pinned_file_regates(
            (manifest, local, lock, which, idx, delta) in pinned_surface()
        ) {
            with_home(|_| {
                let proj = assert_fs::TempDir::new().unwrap();
                // digest_for hashes raw bytes — the files need not parse, so
                // the invariant holds over arbitrary (hostile) content.
                proj.child(".agentstack/agentstack.toml").write_binary(&manifest).unwrap();
                proj.child(".agentstack/agentstack.local.toml").write_binary(&local).unwrap();
                proj.child(".agentstack/agentstack.lock").write_binary(&lock).unwrap();

                trust_unreviewed(proj.path()).unwrap();
                prop_assert_eq!(check(proj.path()), TrustState::Trusted);

                let (name, bytes) = match which {
                    0 => ("agentstack.toml", manifest),
                    1 => ("agentstack.local.toml", local),
                    _ => ("agentstack.lock", lock),
                };
                let mut corrupted = bytes;
                corrupted[idx] ^= delta;
                proj.child(format!(".agentstack/{name}")).write_binary(&corrupted).unwrap();

                prop_assert_eq!(check(proj.path()), TrustState::Changed);
                Ok(())
            })?;
        }
    }
}
