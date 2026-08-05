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
    /// The consented value hashes the SAME bytes but is written in a form this
    /// gate does not accept (N6). Split from [`Self::ConsentMismatch`] because
    /// the two demand opposite responses: a real mismatch means re-review the
    /// changed content, while this means re-send the same value verbatim.
    /// Reporting a format problem as "the content changed" sent users to
    /// re-preview, get a byte-identical digest, and loop — and taught them to
    /// distrust the one gate that must be believed.
    #[error(
        "consented digest is the right hash in the wrong form — the surface has NOT changed (given {consented}, expected {actual}); pass the `surface_digest` from `agentstack trust --preview` verbatim, including its `sha256:` prefix"
    )]
    ConsentDigestFormat { consented: String, actual: String },
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
    /// Standing per-item answers from a re-gate: what the human decided about
    /// content that changed under a pin they had already approved.
    ///
    /// Lives with the trust entry because these ARE consent decisions — they
    /// are scoped to this project, and revoking trust must discard them along
    /// with everything else the project was granted. Additive and optional, so
    /// stores written before re-gate answers existed round-trip unchanged.
    ///
    /// Display/behaviour metadata only: like [`Self::surface`], it never enters
    /// [`digest_for`], so recording an answer cannot re-gate the project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decisions: Option<Vec<ItemDecision>>,
}

/// A standing answer to one re-gate question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemDecision {
    pub kind: String,
    pub name: String,
    pub answer: Decision,
}

/// What the human said when content changed under an approved pin.
///
/// `Accept` is deliberately absent: accepting re-pins and re-grants, which
/// leaves no standing state to remember — the new bytes simply become the
/// approved ones. Only the two answers that persist past the moment are here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "answer", rename_all = "kebab-case")]
pub enum Decision {
    /// Keep using the bytes that were approved. The pin stays where it was and
    /// delivery materializes from the content store — never from the live
    /// project directory, which holds the change the user just declined.
    KeepPinned { pin: String },
    /// Refuse the item outright: it is excluded from delivery and stays
    /// excluded until the human revisits it. Recorded so the refusal is a
    /// standing state `status` reports once, not a question re-asked on every
    /// command.
    Blocked,
}

/// The standing re-gate answers recorded for a project, or an empty vec.
pub fn decisions_for(base: &Path) -> Vec<ItemDecision> {
    TrustStore::load()
        .trusted
        .get(&key_for(base))
        .and_then(|e| e.decisions.clone())
        .unwrap_or_default()
}

/// The standing answer for one item, if the human gave one.
pub fn decision_for(base: &Path, kind: &str, name: &str) -> Option<Decision> {
    decisions_for(base)
        .into_iter()
        .find(|d| d.kind == kind && d.name == name)
        .map(|d| d.answer)
}

/// Record (or with `None`, clear) the standing answer for one item.
///
/// A no-op when the project has no trust entry: an answer about content nobody
/// approved is meaningless, and creating an entry here would be a second grant
/// constructor. Never touches the digest, so this cannot re-gate the project.
pub fn set_decision(base: &Path, kind: &str, name: &str, answer: Option<Decision>) -> Result<bool> {
    let key = key_for(base);
    with_store_lock(|| {
        let mut store = TrustStore::load();
        let Some(entry) = store.trusted.get_mut(&key) else {
            return Ok(false);
        };
        let recorded = answer.is_some();
        let before = entry.decisions.clone();
        let mut list = entry.decisions.take().unwrap_or_default();
        list.retain(|d| !(d.kind == kind && d.name == name));
        if let Some(answer) = answer {
            list.push(ItemDecision {
                kind: kind.to_string(),
                name: name.to_string(),
                answer,
            });
        }
        list.sort_by(|a, b| (&a.kind, &a.name).cmp(&(&b.kind, &b.name)));
        entry.decisions = if list.is_empty() { None } else { Some(list) };
        if entry.decisions == before {
            // Nothing actually changed — re-affirming an identical answer, or
            // clearing one nobody gave (the common case: the trust commit path
            // clears a decision for EVERY accepted item). Writing the same
            // bytes back is not a mutation, so it must neither save nor log:
            // the invariant this crate keeps is "one store write, one line",
            // and a stream of empty `undecide` lines would drown the real
            // answers in the evidence the consent metrics are counted over.
            // Same posture as `repin`, which records nothing when it changes
            // nothing.
            return Ok(true);
        }
        // Held before the save so the `&mut entry` borrow ends here; the digest
        // is what the entry ALREADY stood on — a standing answer re-pins
        // nothing, and identifies which grant the answer sits under.
        let digest = entry.digest.clone();
        store.save()?;
        // P0.2: a standing answer is a mutation of the trust store, so it
        // appends a line like every other one — identity only. The item, the
        // answer, and the pin a keep-pinned answer names are consent CONTENT
        // and stay out of the log; the action carries only the direction.
        record_mutation(
            if recorded {
                agentstack_recorder::TrustAction::Decide
            } else {
                agentstack_recorder::TrustAction::Undecide
            },
            key.clone(),
            digest,
        );
        Ok(true)
    })
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
    /// The content digest this item was pinned to when consent was given, for
    /// the kinds whose bytes live outside the manifest (skills, instructions).
    ///
    /// Deliberately SEPARATE from `identity` rather than folded into it.
    /// `identity` means "what the human agreed to run/contact" and is the diff
    /// key; overloading it with the pin would (a) contradict the documented
    /// meaning above, and (b) make every already-recorded skill read as
    /// `~ changed` on the first re-trust after upgrade — training the user to
    /// wave through a diff that says nothing real.
    ///
    /// This is display/provenance metadata, exactly like the rest of
    /// `SurfaceItem`: it does NOT enter the consent digest, which covers the
    /// manifest, local, and lock bytes only (see [`ConsentSnapshot::digest`]).
    /// Adding it therefore re-gates nothing — witnessed in this module's tests.
    ///
    /// `None` means "no pin was recorded" and is the honest, permanent state
    /// for entries written before this field existed: there is no backfill,
    /// because the bytes that were approved then were never captured. A
    /// re-gate against a `None` pin says so plainly rather than inventing a
    /// diff.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin: Option<String>,
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
    let key = key_for(base);
    with_store_lock(|| {
        let mut store = TrustStore::load();
        let Some(entry) = store.trusted.get_mut(&key) else {
            return Ok(false);
        };
        entry.digest = digest.clone();
        entry.trusted_at = now_secs();
        store.save()?;
        // P0.2: recorded as `Repin`, never `Regrant` — no human consented
        // here, and the consent metrics must be able to exclude it.
        record_mutation(agentstack_recorder::TrustAction::Repin, key.clone(), digest);
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
        // N6: distinguish "you sent a different hash" from "you sent the same
        // hash without its prefix". The accepted alternative form is derived
        // from `actual` itself — never by parsing an algorithm label out of
        // `consented` — so a differently-labelled digest (`md5:<hex>`) can
        // never be read as equal to a `sha256:` one. The hex body must still
        // match in full; this narrows what counts as a *diagnosis*, never what
        // counts as a *match*.
        let same_bytes = actual
            .strip_prefix("sha256:")
            .is_some_and(|hex| consented == hex);
        return Err(if same_bytes {
            TrustError::ConsentDigestFormat {
                consented: consented.to_string(),
                actual,
            }
        } else {
            TrustError::ConsentMismatch {
                consented: consented.to_string(),
                actual,
            }
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
    let key = key_for(base);
    with_store_lock(|| {
        let mut store = TrustStore::load();
        // Standing re-gate answers survive a re-grant. Accepting a change to
        // ONE item must not silently discard a keep-pinned or blocked answer
        // the human gave about a DIFFERENT one — that would turn "I refused
        // this" into "I refused this until something unrelated happened".
        // Accepting an item clears its own decision at the call site, where
        // the item is known.
        let carried = store.trusted.get(&key).and_then(|e| e.decisions.clone());
        let prior = store.trusted.insert(
            key.clone(),
            TrustEntry {
                digest: digest.clone(),
                trusted_at: now_secs(),
                surface,
                decisions: carried,
            },
        );
        store.save()?;
        // P0.2: evidence, appended only after the save succeeded and inside
        // the store lock (so the log's order is the store's order). Best-
        // effort by contract — a recording hiccup must never fail the grant.
        record_mutation(
            if prior.is_some() {
                agentstack_recorder::TrustAction::Regrant
            } else {
                agentstack_recorder::TrustAction::Grant
            },
            key.clone(),
            digest.clone(),
        );
        Ok(())
    })
}

/// The one seam between the trust store and the recorder: every mutation path
/// funnels its event through here. Identity only (project key + digest);
/// never the reviewed surface, the manifest bytes, or a standing answer's
/// item and content.
fn record_mutation(action: agentstack_recorder::TrustAction, project: String, digest: String) {
    agentstack_recorder::record_trust(&agentstack_recorder::TrustMutation {
        ts: agentstack_recorder::now_epoch(),
        action,
        project,
        digest,
    });
}

/// Remove trust for `base`. Returns whether an entry existed.
pub fn revoke(base: &Path) -> Result<bool> {
    let key = key_for(base);
    with_store_lock(|| {
        let mut store = TrustStore::load();
        let removed = store.trusted.shift_remove(&key);
        let Some(entry) = removed else {
            return Ok(false);
        };
        store.save()?;
        // P0.2: the event carries the digest the removed entry had pinned, so
        // the history says WHAT trust was revoked, not merely that some was.
        record_mutation(
            agentstack_recorder::TrustAction::Revoke,
            key.clone(),
            entry.digest,
        );
        Ok(true)
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
    // RAII, not a manual `remove_dir` after the call: `f` is caller code that
    // may panic, and a panic unwinds past a plain statement while it still runs
    // `Drop`. Without the guard a panicking writer leaks the sentinel, and every
    // later trust mutation waits STORE_LOCK_WAIT and then fails closed until the
    // STORE_LOCK_STALE window passes.
    struct LockGuard(std::path::PathBuf);
    impl Drop for LockGuard {
        fn drop(&mut self) {
            // Best-effort: a failed removal leaves the stale-break path to
            // recover, exactly as a crashed process would.
            let _ = std::fs::remove_dir(&self.0);
        }
    }
    let _guard = LockGuard(lock_dir);
    f()
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

    /// P0.2 witness: every store mutation appends exactly one event, in store
    /// order, with the store's own key and the pinned/removed digest — and a
    /// repin that finds no entry (mutates nothing) records nothing.
    #[test]
    fn store_mutations_are_recorded_as_events() {
        use agentstack_recorder::{read_trust_all, TrustAction};
        with_home(|_| {
            let proj = project_with_manifest();
            let key = key_for(proj.path());

            // First-ever trust: exactly one event, action = grant.
            let d1 = trust_unreviewed(proj.path()).unwrap();
            let events = read_trust_all();
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].action, TrustAction::Grant);
            assert_eq!(events[0].project, key);
            assert_eq!(events[0].digest, d1);

            // Trusting again over an existing entry: regrant.
            proj.child(".agentstack/agentstack.toml")
                .write_str("version = 1\n[servers.y]\ntype = \"http\"\nurl = \"https://y/mcp\"\n")
                .unwrap();
            let d2 = trust_unreviewed(proj.path()).unwrap();
            assert_ne!(d1, d2);

            // Repin of the existing entry: repin, distinct from regrant.
            assert!(repin(proj.path(), "sha256:repinned".into()).unwrap());

            // Revoke: the event carries the digest the entry had pinned.
            assert!(revoke(proj.path()).unwrap());

            // Repin with no entry left mutates nothing — and records nothing.
            assert!(!repin(proj.path(), "sha256:ghost".into()).unwrap());

            let actions: Vec<_> = read_trust_all()
                .into_iter()
                .map(|e| (e.action, e.digest))
                .collect();
            assert_eq!(
                actions,
                vec![
                    (TrustAction::Grant, d1),
                    (TrustAction::Regrant, d2),
                    (TrustAction::Repin, "sha256:repinned".to_string()),
                    (TrustAction::Revoke, "sha256:repinned".to_string()),
                ]
            );
        });
    }

    /// P0.2 witness (G16): a standing re-gate answer mutates the trust store,
    /// so it appends one line too — the claim is "every mutation of the store",
    /// not "every mutation that re-pins a digest". And the line stays
    /// IDENTITY-ONLY: which item was answered, what the answer was, and the pin
    /// a keep-pinned answer names are consent content, and none of it reaches
    /// the log. NEVER widen this — the log says a decision happened under this
    /// grant, never what was decided.
    #[test]
    fn standing_decisions_are_recorded_as_identity_only_events() {
        use agentstack_recorder::{read_trust_all, TrustAction};
        with_home(|_| {
            let proj = project_with_manifest();
            let key = key_for(proj.path());
            let granted = trust_unreviewed(proj.path()).unwrap();

            // Recording an answer: one line, and it carries the digest the
            // entry ALREADY stood on — a standing answer re-pins nothing.
            assert!(set_decision(
                proj.path(),
                "skill",
                "alpha-witness",
                Some(Decision::KeepPinned {
                    pin: "sha256:decision-pin-must-not-leak".into(),
                }),
            )
            .unwrap());
            // Withdrawing it: one line, and a DISTINCT action — with the item
            // and the answer absent by design, the action is the only place the
            // direction of the change can live.
            assert!(set_decision(proj.path(), "skill", "alpha-witness", None).unwrap());
            // Clearing an answer nobody gave changes nothing — records nothing.
            assert!(set_decision(proj.path(), "skill", "never-answered", None).unwrap());
            // Nor does an answer about a project nobody trusted: no entry, no
            // mutation, no line.
            let untrusted = project_with_manifest();
            assert!(
                !set_decision(untrusted.path(), "skill", "ghost", Some(Decision::Blocked)).unwrap()
            );

            let events: Vec<_> = read_trust_all()
                .into_iter()
                .map(|e| (e.action, e.project, e.digest))
                .collect();
            assert_eq!(
                events,
                vec![
                    (TrustAction::Grant, key.clone(), granted.clone()),
                    (TrustAction::Decide, key.clone(), granted.clone()),
                    (TrustAction::Undecide, key, granted),
                ]
            );

            // The identity-only assertion, made against the raw bytes on disk
            // rather than the parsed struct: nothing about WHAT was decided may
            // appear in the file, whatever shape a future field takes.
            let raw = std::fs::read_to_string(agentstack_recorder::trust_log_path()).unwrap();
            for leaked in [
                "alpha-witness",
                "decision-pin-must-not-leak",
                "keep-pinned",
                "blocked",
            ] {
                assert!(
                    !raw.contains(leaked),
                    "the decision's content leaked into the trust log: {leaked}\n{raw}"
                );
            }
        });
    }

    /// P0.2 witness for the failure posture: recording adds events, NEVER
    /// gates. With the audit path unwritable (a file squats where the
    /// directory must go), the grant still lands and verification still
    /// reads Trusted — only the event is lost.
    #[test]
    #[cfg(unix)]
    fn recording_failure_never_blocks_the_grant() {
        with_home(|home| {
            std::fs::create_dir_all(home.path()).unwrap();
            std::fs::write(home.path().join("audit"), b"not a directory").unwrap();

            let proj = project_with_manifest();
            trust_unreviewed(proj.path()).expect("grant must succeed without the recorder");
            assert_eq!(check(proj.path()), TrustState::Trusted);
            assert!(agentstack_recorder::read_trust_all().is_empty());
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

    /// Phase 2 LOAD-COMPAT WITNESS: a `trust.json` written before `SurfaceItem`
    /// gained its `pin` field must still deserialize, and must still render its
    /// card. The field is additive and optional precisely so an existing grant
    /// is never invalidated by an upgrade — a store that failed to load would
    /// silently drop every project to `Untrusted` and re-prompt for everything,
    /// which is the worst possible way to ship a consent improvement.
    ///
    /// `None` is the permanent, honest answer for these entries: the bytes they
    /// approved were never captured, so there is nothing to backfill.
    #[test]
    fn a_trust_store_written_before_the_pin_field_still_loads_and_keeps_its_surface() {
        with_home(|_| {
            let proj = project_with_manifest();
            let key = key_for(proj.path());
            let digest = digest_for(proj.path()).unwrap();
            // Hand-written in the OLD shape: surface items with no `pin` key at
            // all. This is the byte shape already on the maintainer's disk.
            let legacy = format!(
                r#"{{"trusted":{{"{key}":{{"digest":"{digest}","trusted_at":1,"surface":[
                    {{"kind":"server","name":"fs","identity":"node fs.js"}},
                    {{"kind":"skill","name":"greet","identity":"library"}}
                ]}}}}}}"#
            );
            // Written through `store_path()` so this witness tracks wherever
            // the store actually lives, rather than re-deriving the layout.
            let path = store_path();
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&path, legacy).unwrap();

            // It loads, the project is still trusted, and the surface survives.
            assert_eq!(check(proj.path()), TrustState::Trusted);
            let PriorSurface::Recorded(items) = prior_surface(proj.path()) else {
                panic!("a legacy surface must still read as Recorded, not Untracked");
            };
            assert_eq!(items.len(), 2);
            assert_eq!(items[1].name, "greet");
            assert_eq!(items[1].identity, "library");
            // Absent in the file => None in memory. Never a fabricated pin.
            assert!(
                items.iter().all(|i| i.pin.is_none()),
                "a legacy entry must not invent pins it never recorded"
            );
        });
    }

    /// Phase 2 CONSENT WITNESS: a standing re-gate answer about one item
    /// survives a re-grant driven by a DIFFERENT item.
    ///
    /// This exists because the first implementation got it wrong: `store_entry`
    /// replaced the whole `TrustEntry`, so accepting a change to skill B
    /// silently discarded a refusal the human had recorded about skill A. That
    /// turns "I refused this" into "I refused this until something unrelated
    /// happened" — a consent decision quietly undone by an action that never
    /// mentioned it. NEVER weaken this.
    #[test]
    fn a_standing_answer_survives_a_regrant_driven_by_another_item() {
        with_home(|_| {
            let proj = project_with_manifest();
            let first = digest_for(proj.path()).unwrap();
            trust_reviewed(proj.path(), first, Vec::new()).unwrap();

            // The human keeps the approved bytes of skill A.
            set_decision(
                proj.path(),
                "skill",
                "alpha",
                Some(Decision::KeepPinned {
                    pin: "sha256:aaa".into(),
                }),
            )
            .unwrap();
            // …and blocks skill C outright.
            set_decision(proj.path(), "skill", "gamma", Some(Decision::Blocked)).unwrap();

            // Something unrelated changes and the project is re-granted — the
            // manifest moved, which is what accepting a change to B looks like
            // at this layer.
            proj.child(".agentstack/agentstack.toml")
                .write_str(
                    "version = 1\n[servers.beta]\ntype = \"http\"\nurl = \"https://b/mcp\"\n",
                )
                .unwrap();
            let second = digest_for(proj.path()).unwrap();
            trust_reviewed(proj.path(), second, Vec::new()).unwrap();

            assert_eq!(
                decision_for(proj.path(), "skill", "alpha"),
                Some(Decision::KeepPinned {
                    pin: "sha256:aaa".into()
                }),
                "a keep-pinned answer was discarded by an unrelated re-grant"
            );
            assert_eq!(
                decision_for(proj.path(), "skill", "gamma"),
                Some(Decision::Blocked),
                "a block was discarded by an unrelated re-grant"
            );
        });
    }

    /// The inverse, and the reason decisions live on the trust entry at all:
    /// revoking consent discards them with everything else the project was
    /// granted. A refusal that outlived its trust would be a standing behaviour
    /// with no consent behind it.
    #[test]
    fn revoking_trust_discards_standing_answers_with_the_rest_of_consent() {
        with_home(|_| {
            let proj = project_with_manifest();
            let digest = digest_for(proj.path()).unwrap();
            trust_reviewed(proj.path(), digest, Vec::new()).unwrap();
            set_decision(proj.path(), "skill", "alpha", Some(Decision::Blocked)).unwrap();
            assert!(decision_for(proj.path(), "skill", "alpha").is_some());

            assert!(revoke(proj.path()).unwrap());
            assert!(
                decisions_for(proj.path()).is_empty(),
                "standing answers outlived the consent they belonged to"
            );

            // Re-granting starts clean: the human answers again, from scratch.
            let digest = digest_for(proj.path()).unwrap();
            trust_reviewed(proj.path(), digest, Vec::new()).unwrap();
            assert!(decisions_for(proj.path()).is_empty());
        });
    }

    /// An answer about a project nobody trusted is meaningless, and recording
    /// one must not create a trust entry — that would be a second grant
    /// constructor, which invariant 6 forbids.
    #[test]
    fn recording_an_answer_never_creates_trust() {
        with_home(|_| {
            let proj = project_with_manifest();
            assert!(!set_decision(proj.path(), "skill", "a", Some(Decision::Blocked)).unwrap());
            assert_eq!(check(proj.path()), TrustState::Untrusted);
            assert!(decisions_for(proj.path()).is_empty());
        });
    }

    /// Phase 2 NO-REGATE WITNESS: recording a pin changes no digest. The consent
    /// digest covers the manifest, local overlay, and lock bytes — the surface
    /// snapshot is metadata stored beside it. If the pin leaked into the digest,
    /// upgrading would re-gate every project on the machine for a change the
    /// user did not make. NEVER weaken this.
    #[test]
    fn recording_a_pin_changes_no_digest_and_re_gates_nothing() {
        with_home(|_| {
            let proj = project_with_manifest();
            let before = digest_for(proj.path()).unwrap();

            let pinned = vec![SurfaceItem {
                kind: "skill".into(),
                name: "greet".into(),
                identity: "library".into(),
                pin: Some("sha256:cafe".into()),
            }];
            trust_reviewed(proj.path(), before.clone(), pinned.clone()).unwrap();

            // The digest over the same bytes is unchanged by what we recorded…
            assert_eq!(digest_for(proj.path()).unwrap(), before);
            // …and the project stays trusted rather than reading as Changed.
            assert_eq!(check(proj.path()), TrustState::Trusted);
            // The pin round-trips through JSON exactly as written.
            assert_eq!(prior_surface(proj.path()), PriorSurface::Recorded(pinned));

            // The same surface WITHOUT pins yields the same digest too: the two
            // differ only in metadata, so neither can re-gate the other.
            let unpinned = vec![SurfaceItem {
                kind: "skill".into(),
                name: "greet".into(),
                identity: "library".into(),
                pin: None,
            }];
            trust_reviewed(proj.path(), before.clone(), unpinned).unwrap();
            assert_eq!(digest_for(proj.path()).unwrap(), before);
            assert_eq!(check(proj.path()), TrustState::Trusted);
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
                    pin: None,
                },
                SurfaceItem {
                    kind: "skill".into(),
                    name: "greet".into(),
                    identity: "library".into(),
                    pin: None,
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

    // SECURITY WITNESS (N6): a prefix-less digest still REFUSES — the fix was
    // the diagnosis, never the acceptance rule. The gate must keep rejecting
    // anything that is not byte-equal to the computed digest; it just has to
    // say which of the two problems it is, because "the content changed" sent
    // users to re-preview an unchanged project forever. NEVER relax this into
    // accepting the bare form.
    #[test]
    fn consent_grant_refuses_bare_hex_but_names_it_a_format_problem() {
        with_home(|_| {
            let proj = project_with_manifest();
            let previewed = digest_for(proj.path()).unwrap();
            let bare = previewed.strip_prefix("sha256:").unwrap().to_string();
            assert_ne!(bare, previewed, "fixture must exercise the prefix");

            // Same bytes, wrong form: still refused, still nothing granted…
            let err = trust_with_consent(proj.path(), Vec::new(), &bare).unwrap_err();
            assert_eq!(check(proj.path()), TrustState::Untrusted);
            // …but reported as a format problem, and the message must NOT
            // claim the surface changed.
            assert!(matches!(err, TrustError::ConsentDigestFormat { .. }));
            let msg = err.to_string();
            assert!(msg.contains("has NOT changed"));
            assert!(!msg.contains("changed since the preview"));

            // An algorithm label is never normalized away: a different label
            // over the same hex is a real mismatch, not a format problem.
            let mislabelled = format!("md5:{bare}");
            let err = trust_with_consent(proj.path(), Vec::new(), &mislabelled).unwrap_err();
            assert!(matches!(err, TrustError::ConsentMismatch { .. }));
            assert_eq!(check(proj.path()), TrustState::Untrusted);

            // And a genuinely different hash stays a genuine mismatch.
            let err = trust_with_consent(proj.path(), Vec::new(), "deadbeef").unwrap_err();
            assert!(matches!(err, TrustError::ConsentMismatch { .. }));
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
                pin: None,
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

    #[test]
    fn a_panic_inside_the_store_lock_still_releases_it() {
        with_home(|home| {
            let lock_dir = home.path().join("trust.lock.d");
            let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                with_store_lock(|| -> Result<()> { panic!("writer exploded mid-save") })
            }));
            assert!(caught.is_err(), "the panic must propagate");
            assert!(
                !lock_dir.exists(),
                "the sentinel must not survive an unwinding writer"
            );
            // The witness that matters: the next mutation acquires the lock at
            // once instead of waiting out STORE_LOCK_WAIT (5s) and failing
            // closed until the STORE_LOCK_STALE window (30s) passes.
            let start = std::time::Instant::now();
            with_store_lock(|| Ok(())).unwrap();
            assert!(
                start.elapsed() < std::time::Duration::from_secs(1),
                "a leaked lock made the next writer wait {:?}",
                start.elapsed()
            );
        });
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
