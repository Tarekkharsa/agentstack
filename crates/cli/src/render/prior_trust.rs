//! The trust state a delivery gate is judged against.
//!
//! The gates themselves live next door — [`crate::render::skills::plan`] for
//! skill materialization, [`crate::render::apply::plan_target_with_servers`]
//! for server render. This module holds the one thing they both need and
//! neither can work out for itself: *when* to ask.

use std::path::Path;

/// The project's trust state as it stood when the running command STARTED.
///
/// # Why the gates cannot simply read the store
///
/// `agentstack add <thing> --write` and the panel's edit verbs write the
/// manifest and the lockfile and then deliver, in the same run. Those bytes ARE
/// the consent digest (`trust::ConsentSnapshot::digest`), so the command's own
/// write flips the project to `Changed` — and a gate that reads the store
/// afterwards refuses the very delivery the human typed the command to get. A
/// command cannot be allowed to refuse itself.
///
/// The rule this type carries is the narrowest one that fixes that: judge the
/// delivery against the state that held BEFORE the command wrote its own bytes.
/// The bytes that moved the state are the bytes the human just asked for, so
/// judging that delivery against the pre-command state authorizes nothing that
/// was not already authorized. An untrusted or drifted project is refused
/// exactly as before — because it was untrusted or drifted before the command
/// ran, too.
///
/// # What this deliberately does NOT do
///
/// It does not re-pin. Contrast the owned-server refresh in
/// `commands::apply`, which captures the same pre-write judgement and then
/// calls `trust::repin`: that rewrite is machine-derived from a config the
/// owning harness already executes, so nothing new enters the surface. Here the
/// new bytes are a capability the human added, and they still owe a review. So
/// the project is left reading `Changed`, the very next command re-gates it,
/// and the human meets the review before anything else moves.
///
/// It also does not reach hooks. `render::hooks::trust_refusal` keeps reading
/// the store directly and takes no relaxation at all — hooks always get the
/// full consent ceremony (`STRATEGY.md`), which is a stronger promise than the
/// one made here.
///
/// # Fail closed
///
/// [`PriorTrust::STRICT`] is the `Default`, so a caller that captured nothing
/// gets the unrelaxed gate. That is what makes an explicit parameter safer than
/// a flag or an ambient lookup: forgetting it costs a command a refusal it will
/// print, never a delivery it should not have made.
// `Copy`: one bool, threaded through half a dozen plan calls — callers pass it
// by value without an `&` or a clone at every hop.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PriorTrust {
    /// True only when the project read `Trusted` at command start.
    was_trusted: bool,
}

impl PriorTrust {
    /// Nothing was captured: judge the gate against the trust state on disk at
    /// plan time.
    ///
    /// The correct answer for every command that did not itself author the
    /// change it is delivering — `use`, `apply`, `doctor --fix`, `diff`,
    /// `session start`, `x unrender`, `image`. Also the `Default`, so the
    /// strict reading is what a new call site gets for free.
    pub const STRICT: PriorTrust = PriorTrust { was_trusted: false };

    /// Read the project's trust state NOW, to be judged against later in the
    /// same run.
    ///
    /// **Call this before the command writes anything the consent digest
    /// covers** — `agentstack.toml`, the `agentstack.local.toml` overlay, or
    /// `agentstack.lock`. That ordering is the entire meaning of the value and
    /// no type can enforce it, so the call belongs at the top of the command,
    /// beside the other pre-write captures (`add::preview_and_commit` puts it
    /// next to `ActivationCtx::detect`, which exists for the same reason).
    ///
    /// The state and the digest it is compared against both come from ONE
    /// [`crate::trust::ConsentSnapshot`], never from two disk reads — the same
    /// technique the owned-server refresh uses, so a mid-command edit cannot
    /// pair one read's bytes with another read's verdict.
    pub fn at_command_start(project_dir: &Path) -> PriorTrust {
        let base = crate::manifest::project_root_of(project_dir);
        let was_trusted = crate::trust::ConsentSnapshot::read(&base).is_some_and(|snapshot| {
            crate::trust::check_digest(&base, Some(&snapshot.digest()))
                == crate::trust::TrustState::Trusted
        });
        PriorTrust { was_trusted }
    }

    /// The gate's verdict for the project at `base`: `None` to deliver, or the
    /// sentence fragment saying why not (it follows "project at &lt;path&gt;").
    ///
    /// `Changed` is the ONLY state a pre-command `Trusted` can excuse.
    /// `Untrusted` means the store entry is gone — a revocation, not a byte
    /// edit — and no write this command made could have caused it, so the
    /// relaxation has no claim on it and the gate refuses. Narrower than "the
    /// state at command start wins", on purpose: the excuse only ever covers
    /// the one thing the command is known to have done.
    pub(crate) fn refusal_reason(self, base: &Path) -> Option<&'static str> {
        match crate::trust::check(base) {
            crate::trust::TrustState::Trusted => None,
            crate::trust::TrustState::Changed if self.was_trusted => None,
            crate::trust::TrustState::Changed => Some("changed since it was trusted"),
            crate::trust::TrustState::Untrusted => Some("is not trusted"),
        }
    }
}
