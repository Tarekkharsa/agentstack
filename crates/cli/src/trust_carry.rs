//! Carrying VALID trust across agentstack's own rewrite of a manifest layer.
//!
//! The sibling rule lives in [`crate::render::PriorTrust`], which answers a
//! different question — "may this command deliver what it just wrote?" — and
//! deliberately does not re-pin. This module answers "may the NEXT command
//! deliver it?", and is allowed to, for a strictly narrower class of writes.

use anyhow::Result;
use std::path::{Path, PathBuf};

use crate::manifest::load::{LOCAL_FILE, MANIFEST_FILE};

/// A pre-write capture that lets one command re-pin the project's trust to the
/// bytes it is about to write.
///
/// # The rule, stated once
///
/// **A command that rewrites a manifest layer but authorizes NO new executable
/// content re-pins trust — and only when trust was valid immediately before its
/// own write.**
///
/// `agentstack.toml`, the `agentstack.local.toml` overlay and `agentstack.lock`
/// ARE the consent digest (`trust::ConsentSnapshot`), so *any* write to them
/// flips a trusted project to `Changed`. For a write that added a server, a
/// skill, a hook, an extension, a workflow, an instruction fragment or a
/// command line, that is exactly right: new content owes a human review, and
/// the next command must re-gate. But a write that only records a preference —
/// a delivery mode, a routing choice, bookkeeping — moved the digest without
/// moving the surface, and re-gating it walls the user off from the very
/// journey the command exists to start (`x delivery render-locally --write`
/// followed by `apply --write`). Nothing new is authorized, so trust that was
/// valid before the write is carried across it.
///
/// The precedent is the owned-server refresh in [`crate::commands::apply`],
/// which is now one of this type's callers rather than a second copy of it.
///
/// # The four properties this type is built to hold
///
/// 1. **It never creates trust.** [`crate::trust::repin`] only ever UPDATES an
///    existing entry and writes nothing when there is none, so an untrusted
///    project stays untrusted no matter which command ran.
/// 2. **It never resolves a pending review.** The capture is `Some` only when
///    the project read `Trusted` at that instant; a project already reading
///    `Changed` captures `None` and this whole type becomes a no-op.
/// 3. **It never blesses a race.** The re-pinned digest comes from the
///    PRE-write snapshot with the caller's own new bytes spliced in — never a
///    post-write disk re-read. A hostile edit landing between the capture and
///    the write therefore cannot be signed: the store holds the digest of what
///    WE wrote, the project reads `Changed`, and review stays pending.
/// 4. **It fails closed on anything it does not recognize.** Only the two
///    manifest layers can be spliced. A path that is neither re-pins nothing.
///    `agentstack.lock` is excluded ON PURPOSE: re-locking accepts content
///    digests, which is content acceptance, and content acceptance is a human's
///    to give.
///
/// # Where it must NOT be used
///
/// Any command that adds or edits a capability — `add`, the panel's
/// `add-*`/`create-profile` verbs, `adopt`, `lock --write`, `upgrade`,
/// `workflow declare`, `init` with declarations. Those writes are precisely
/// what review exists for; they use [`crate::render::PriorTrust`] to avoid
/// refusing their OWN delivery and leave the project reading `Changed` so the
/// human meets the review before anything else moves.
pub struct TrustCarry {
    base: PathBuf,
    /// `Some` only when the project read `Trusted` against exactly these bytes
    /// at capture time. `None` covers untrusted, drifted, and no-manifest
    /// alike — every case in which re-pinning would be a promotion rather than
    /// a carry.
    valid_before: Option<crate::trust::ConsentSnapshot>,
}

impl TrustCarry {
    /// Capture the project's trust state and its consenting bytes, RIGHT NOW.
    ///
    /// **Call this before the command writes anything the consent digest
    /// covers.** That ordering is the entire meaning of the value and no type
    /// can enforce it; reading it afterwards would just re-read the digest the
    /// command itself moved and re-pin whatever happens to be on disk.
    ///
    /// `manifest_dir` is the directory holding the manifest (a `Context.dir`);
    /// the trust key is derived from it, so callers never compute the base.
    pub fn before_write(manifest_dir: &Path) -> TrustCarry {
        let base = crate::manifest::project_root_of(manifest_dir);
        let valid_before = crate::trust::ConsentSnapshot::read(&base).filter(|snapshot| {
            crate::trust::check_digest(&base, Some(&snapshot.digest()))
                == crate::trust::TrustState::Trusted
        });
        TrustCarry { base, valid_before }
    }

    /// True when the project read `Trusted` at capture time.
    ///
    /// For callers whose report distinguishes "trust was carried" from "trust
    /// was already broken before I touched anything" — the second is not a
    /// failure of the carry and must not be reported as one.
    pub fn was_valid(&self) -> bool {
        self.valid_before.is_some()
    }

    /// Re-pin trust to the snapshot with `new_text` spliced in as `path`.
    ///
    /// **Call this after the write succeeded**, with the exact bytes written.
    /// Returns `true` when the store was updated, and `false` — writing
    /// nothing — whenever the carry does not apply: no valid trust before the
    /// write, no trust entry at all, or a `path` that is not one of this
    /// project's two manifest layers.
    ///
    /// Consumes `self`: one capture authorizes one carry, so a stale capture
    /// cannot be reused after a second write moved the bytes again.
    pub fn across_write(self, path: &Path, new_text: &str) -> Result<bool> {
        let Some(mut snapshot) = self.valid_before else {
            return Ok(false);
        };
        let dir = crate::manifest::resolve_manifest_dir(&self.base);
        if same_file(path, &dir.join(MANIFEST_FILE)) {
            snapshot.manifest = new_text.as_bytes().to_vec();
        } else if same_file(path, &dir.join(LOCAL_FILE)) {
            snapshot.local = Some(new_text.as_bytes().to_vec());
        } else {
            // A layer this type cannot splice (the lockfile, a central library
            // file, an inherited manifest elsewhere). Re-pinning the unspliced
            // snapshot would sign bytes that are no longer on disk, so re-pin
            // nothing and let the project re-gate.
            return Ok(false);
        }
        Ok(crate::trust::repin(&self.base, snapshot.digest())?)
    }
}

/// Do these two paths name the same file?
///
/// The caller's path and the one derived from the trust base can spell the same
/// file differently (a symlinked project dir, `/tmp` vs `/private/tmp` on
/// macOS), and a spelling mismatch here would silently skip a carry that should
/// have happened. `canonicalize` resolves both when the file exists — it does
/// after the write — and plain equality is the fallback, which errs toward NOT
/// re-pinning.
fn same_file(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}
