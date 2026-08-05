//! The seatbelt: one shape for every enforcement denial the user meets.
//!
//! Strategy v2 Phase 3, "seatbelt legibility". Six families can refuse an
//! action — a gateway tool block, an egress refusal, a secret-scope refusal,
//! the filesystem guard, the content-pinning refusal that withholds a server
//! whose bytes no longer match what was reviewed (added in Phase 4), and the
//! trust-at-dispatch refusal that stops the next call on an already-live
//! connection once the project's consent digest stops holding (W2).
//! Before this module each wrote its own sentence,
//! and half of them said only *what* was stopped, leaving the user to work out
//! why and what to do instead. A denial that a person cannot act on is the
//! moment security stops feeling like a seatbelt and starts feeling like a
//! jam.
//!
//! So a denial says three things, always, in one sentence:
//!
//! - **what was stopped** — the actor and the thing it tried;
//! - **why** — the rule that refused, named so it can be found and changed;
//! - **the safe next step** — the one thing to do about it.
//!
//! And it is backed by evidence: the same denial is recorded as a `Denied`
//! `CallRecord` in the audit log, so `agentstack report run <id>` surfaces it
//! in the Tool-calls section rather than the user having to have been watching
//! the terminal at the time. (The run-scoped `SecretDenied` / `PinRejected`
//! events carry the same fact structured for `--json`; the terminal report
//! renders the `CallRecord`, not those variants. The one exception is
//! `FenceRefused`: a fence refusal is not a call the run made, so the audit
//! row it writes is deliberately outside the Tool-calls section the report
//! builds from `ToolCall` events — the run report renders the event itself,
//! in its own section, or the refusal would be invisible there.)
//!
//! # The invariant this module must never break
//!
//! **A legible denial is still a denial.** Explaining a block is not a step
//! toward relaxing it, and there is no "explain, then allow anyway" path.
//! That is structural here, not a promise: [`refuse`] returns `()`. It has no
//! success value to hand back and no way to signal "carry on", so a call site
//! that composes a denial cannot accidentally acquire permission by doing so —
//! it still has to `continue`, `return`, or `bail!` on its own. Recording is
//! best-effort for the same reason read the other way: a logging failure must
//! never turn a denial into an allow, so the recorder's failures are swallowed
//! and the refusal proceeds regardless.
//!
//! # Recorded is not prevented
//!
//! Two of these families record a decision that a *cooperative* mechanism
//! made (the host guard's pre-tool-use hook, the host-path egress check at
//! render time). An event in the log proves the check ran and what it said —
//! it does not prove anything was stopped at the kernel or the wire.
//! `docs/ENFORCEMENT.md` is the honest matrix; nothing here may be read as
//! upgrading a cell in it.

use agentstack_recorder::{CallOutcome, CallRecord, RunEvent, RunLog};

/// Which enforcement family refused. Decides the verb the sentence uses for
/// "nothing happened", and the `tool` slot the audit record files it under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// The gateway refused an MCP tool call against `[policy.tools]`.
    Tool,
    /// An outbound connection was refused against `[policy.egress]` — either
    /// by the sandbox proxy (enforced) or by the host-path check at render
    /// time (coarse; see `docs/ENFORCEMENT.md`).
    Egress,
    /// `[policy.secrets]` refused to resolve a `${REF}` for a server.
    Secret,
    /// The filesystem guard refused a write or a destructive command
    /// (cooperative — the harness chose to ask).
    Filesystem,
    /// A server was withheld because its declared bytes did not verify against
    /// the lockfile pin — the content-pinning refusal.
    ///
    /// The fifth family, added in Phase 4. It is not a policy dimension like
    /// the other four: nothing the user *authored* refused here, the delivered
    /// content simply is not what they reviewed. That is why it gets its own
    /// family rather than borrowing [`Family::Tool`] — the reader needs to know
    /// the answer is "this changed", not "you disallowed this", because the two
    /// have completely different next steps.
    ///
    /// Unlike the two cooperative families, this one is genuine prevention:
    /// the server is dropped before it is ever spawned or dialled.
    Pin,
    /// A dispatch to an already-connected upstream was refused because the
    /// project's consent digest no longer matches the one the connection was
    /// authorized against — trust revoked, manifest edited out of band, or the
    /// lock swapped wholesale (W2, `docs/design/automatic-delivery.md`).
    ///
    /// The sixth family, and — like [`Family::Pin`] — not a policy dimension:
    /// nothing the user authored refused here, the *yes itself* stopped
    /// applying. It is its own family rather than [`Family::Pin`]'s because
    /// the two answer different questions. Pin says "the delivered bytes are
    /// not the bytes you reviewed" and fires before a server ever starts;
    /// this says "the review no longer covers this project" and fires on a
    /// connection that was already live and already working, which is the
    /// surprising part the reader has to be told.
    ///
    /// Genuine prevention: the upstream is never dialled, and the surface it
    /// exposed empties with it. Control-plane tools do not route through the
    /// gateway and so stay reachable, deliberately — a user whose project just
    /// went untrusted needs to be able to see why and fix it.
    ///
    /// The same family also covers the two doors *before* dispatch (W1): a
    /// lease the gateway will not open and a skill it will not load for a
    /// project whose yes does not hold. Same question, same answer, same one
    /// command that fixes it — so the same family, and one `tool: "trust"` tag
    /// a reader can filter the audit log by.
    Trust,
    /// A call named a server this project declares, but the toolset fence is
    /// not open on a toolset that selects it — so nothing exposed the name and
    /// the call was refused (W4 precondition 3).
    ///
    /// The seventh family, and — like [`Family::Pin`] and [`Family::Trust`] —
    /// not a policy dimension. It is its own family for the reason those two
    /// are: the answer here is "nothing has selected this yet", not "you
    /// disallowed this" and not "your yes stopped holding", and only this
    /// answer is fixed by opening a lease. A reader filtering `tool: "tool"`
    /// must not find a fence refusal there and read it as `[policy.tools]`.
    ///
    /// Genuine prevention: the fenced gateway holds no upstream for the name,
    /// so nothing was spawned, dialled, or forwarded.
    ///
    /// Recorded only for a server the project **declares**. A name nothing
    /// declares is a typo, not a security event, and stays an ordinary unknown
    /// tool error — otherwise any caller could write unbounded rows into the
    /// audit log just by inventing names.
    Fence,
}

impl Family {
    /// The reassurance clause. A denial's first job is to stop the thing; its
    /// second is to tell the reader that stopping it did not half-happen.
    fn nothing_clause(self) -> &'static str {
        match self {
            Family::Tool => "nothing ran",
            Family::Egress => "nothing was sent",
            Family::Secret => "nothing was read",
            Family::Filesystem => "nothing was written",
            // The server never started, so no tool of its ever ran. Same
            // clause as `Tool`, and correct for the same reason.
            Family::Pin => "nothing ran",
            // The upstream was live, but this call never reached it: the
            // refusal happens before the round trip, so the same clause is
            // the true one.
            Family::Trust => "nothing ran",
            // No upstream held the name, so there was nothing to run.
            Family::Fence => "nothing ran",
        }
    }

    /// The `tool` slot in `calls.jsonl`. Fixed strings so a reader can filter
    /// the audit log by family without parsing prose.
    fn audit_tool(self) -> &'static str {
        match self {
            Family::Tool => "tool",
            Family::Egress => "egress",
            Family::Secret => "secret",
            Family::Filesystem => "filesystem",
            Family::Pin => "pin",
            Family::Trust => "trust",
            Family::Fence => "fence",
        }
    }
}

/// Every `tool` tag [`record`] writes — the closed set of ENFORCEMENT-denial
/// families, distinct from a brokered gateway call to a server.
///
/// Exposed so consumers of the shared audit log can tell the two apart
/// without re-deriving the list (F20). A seatbelt record means "an attempt to
/// reach a host / resolve a ref / load drifted content was refused" — it is
/// not evidence that a server was contacted, so `optimize` must not fold it
/// into per-server brokered-call counts. This is the same non-overloading
/// discipline the recorder applies with `SecretDenied` vs `SecretAccess`, one
/// layer up: a denial that never happened as a call must not be counted as
/// one.
pub const AUDIT_TOOLS: &[&str] = &[
    "tool",
    "egress",
    "secret",
    "filesystem",
    "pin",
    "trust",
    "fence",
];

/// Bound a refusal reason before it is printed or recorded.
///
/// Every other family's `why` is policy text this machine authored, which is
/// what makes it safe to pass through untouched. The pin refusal's reason is
/// the exception: it is composed from lockfile and manifest fragments, which
/// are repository content and therefore hostile input (invariant 7). An
/// attacker-authored server name full of ANSI escapes could otherwise rewrite
/// the terminal around the denial — turning the one sentence that says
/// "this was stopped" into whatever they liked.
///
/// So: control characters (which includes escape, CR, and LF) become spaces,
/// and the result is truncated. Deliberately lossy — a denial the reader can
/// trust to be a denial is worth more than a complete one.
pub fn bounded_reason(raw: &str) -> String {
    const MAX: usize = 200;
    let mut out: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(MAX)
        .collect();
    // `take` counts chars, so this only fires when the input really was
    // longer — and says so, rather than silently presenting a prefix as whole.
    if raw.chars().count() > MAX {
        out.push('…');
    }
    out
}

/// One refusal, in the three parts a reader needs.
pub struct Denial<'a> {
    pub family: Family,
    /// Who tried — the server, the harness tool, or the guard subject.
    pub subject: &'a str,
    /// What it tried, as a phrase that follows the subject: `"reach
    /// api.evil.example"`, `"read ${SEARCH_KEY}"`.
    pub attempted: &'a str,
    /// The rule that refused, named. Policy-authored text from this machine's
    /// own configuration — never upstream- or repository-authored, which is
    /// what makes it safe to print and to record (invariant 7).
    pub why: &'a str,
    /// The one safe thing to do about it. Not a way around the rule: either
    /// how to change the rule deliberately, or what to do instead.
    pub next_step: &'a str,
}

impl Denial<'_> {
    /// The one plain sentence. Two lines, because "what and why" and "what
    /// now" are two different reads and cramming them into one line makes the
    /// reader parse rather than scan.
    pub fn sentence(&self) -> String {
        format!(
            "blocked: {} tried to {} — {}\n  {} · {}",
            self.subject,
            self.attempted,
            self.why,
            self.family.nothing_clause(),
            self.next_step
        )
    }
}

/// Record the denial as evidence, best-effort.
///
/// Two destinations, mirroring what the gateway's own call log already does:
/// the machine-global `calls.jsonl` always, so a denial outside a tracked run
/// is not lost; and, when this happened inside an `agentstack run`, the
/// run-scoped event log too, so `agentstack report run <id>` shows it beside
/// the rest of that run's story.
///
/// Additive, identity-shaped, and never gating — the P0.2 pattern. Every
/// failure mode is swallowed inside the recorder; nothing here returns a
/// `Result`, so no caller can be tempted to treat a recording failure as a
/// reason to proceed.
pub fn record(d: &Denial, project: Option<String>, run: Option<&str>) {
    agentstack_recorder::record(&CallRecord {
        ts: now_epoch(),
        run: run.map(str::to_string),
        pid: std::process::id(),
        project,
        server: d.subject.to_string(),
        tool: d.family.audit_tool().to_string(),
        // There are no arguments to digest: the denial is about reaching a
        // host or a ref, not about a call payload. The empty digest is the
        // honest value — inventing one would imply arguments were examined.
        args_digest: String::new(),
        outcome: CallOutcome::Denied,
        detail: Some(d.why.to_string()),
        ms: 0,
    });
}

/// Refuse: say it, and record it.
///
/// Returns `()` on purpose — see the module invariant. The caller still owns
/// the refusal itself; this only makes it legible and evidenced.
pub fn refuse(d: &Denial, project: Option<String>, run: Option<&str>) {
    eprintln!("{}", d.sentence());
    record(d, project, run);
}

/// The run-scoped mirror for a host-path egress refusal. Reuses the existing
/// [`RunEvent::Egress`] shape, which already carries `allowed: false` — the
/// gap was never the schema, it was that nothing on the host path emitted one.
pub fn record_egress_denied(run: Option<&str>, server: &str, host: &str, rule: &str) {
    let Some(run) = run else { return };
    let Some(log) = RunLog::create(run) else {
        return;
    };
    log.append(&RunEvent::Egress {
        ts: now_epoch(),
        server: server.to_string(),
        host: host.to_string(),
        allowed: false,
        rule: Some(rule.to_string()),
    });
}

/// The run-scoped mirror for a secret-scope refusal.
pub fn record_secret_denied(run: Option<&str>, server: &str, reference: &str, rule: &str) {
    let Some(run) = run else { return };
    let Some(log) = RunLog::create(run) else {
        return;
    };
    log.append(&RunEvent::SecretDenied {
        ts: now_epoch(),
        server: server.to_string(),
        reference: reference.to_string(),
        rule: rule.to_string(),
    });
}

/// The run-scoped mirror for a content-pinning refusal: a server the gateway
/// withheld because its bytes did not verify against the lockfile pin.
///
/// `reason` must already have been through [`bounded_reason`] — it is passed
/// bounded rather than bounded here so the identical string is what the user
/// read on their terminal and what a reviewer finds in the log. Evidence that
/// differs from what was shown is worse than no evidence.
pub fn record_pin_rejected(run: Option<&str>, server: &str, reason: &str) {
    let Some(run) = run else { return };
    let Some(log) = RunLog::create(run) else {
        return;
    };
    log.append(&RunEvent::PinRejected {
        ts: now_epoch(),
        server: server.to_string(),
        reason: reason.to_string(),
    });
}

/// The run-scoped mirror for a trust-at-dispatch refusal: a call the gateway
/// refused to forward because the project's consent digest stopped holding
/// mid-connection (W2).
///
/// `server`, `tool`, and `reason` must already have been through
/// [`bounded_reason`], for the same reason [`record_pin_rejected`] insists on
/// it: the first two are manifest- and upstream-derived (hostile input,
/// invariant 7), and passing all three pre-bounded keeps the log line and the
/// terminal line byte-identical. Evidence that differs from what was shown is
/// worse than no evidence.
pub fn record_trust_refused(
    run: Option<&str>,
    server: &str,
    tool: &str,
    state: &str,
    reason: &str,
) {
    let Some(run) = run else { return };
    let Some(log) = RunLog::create(run) else {
        return;
    };
    log.append(&RunEvent::TrustRefused {
        ts: now_epoch(),
        server: server.to_string(),
        tool: tool.to_string(),
        state: state.to_string(),
        reason: reason.to_string(),
    });
}

/// The run-scoped mirror for a toolset-fence refusal: a call the gateway did
/// not hold an upstream for, because the project fences its servers behind
/// toolsets and no open lease selects one that exposes this server (W4).
///
/// `server`, `tool`, and `toolset` must already have been through
/// [`bounded_reason`], for the same reason [`record_trust_refused`] insists on
/// it: all three are manifest- or wire-derived (hostile input, invariant 7),
/// and passing them pre-bounded keeps the log line and the terminal line
/// identical. Evidence that differs from what was shown is worse than no
/// evidence.
pub fn record_fence_refused(
    run: Option<&str>,
    server: &str,
    tool: &str,
    toolset: Option<&str>,
    reason: &str,
) {
    let Some(run) = run else { return };
    let Some(log) = RunLog::create(run) else {
        return;
    };
    log.append(&RunEvent::FenceRefused {
        ts: now_epoch(),
        server: server.to_string(),
        tool: tool.to_string(),
        toolset: toolset.map(str::to_string),
        reason: reason.to_string(),
    });
}

fn now_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sentence carries all three parts, in the order a reader needs them.
    #[test]
    fn a_denial_says_what_why_and_what_next() {
        let d = Denial {
            family: Family::Egress,
            subject: "web-search",
            attempted: "reach api.evil.example",
            why: "not in this project's allowed hosts ([policy.egress])",
            next_step: "add the host to [policy.egress] if you meant to allow it",
        };
        let s = d.sentence();
        assert!(s.contains("web-search"), "{s}");
        assert!(s.contains("reach api.evil.example"), "{s}");
        assert!(s.contains("[policy.egress]"), "{s}");
        assert!(s.contains("nothing was sent"), "{s}");
        assert!(s.contains("add the host"), "{s}");
    }

    /// Each family reassures with the right verb — "nothing ran" over a
    /// blocked write would be a small lie about what was at stake.
    #[test]
    fn each_family_names_what_did_not_happen() {
        for (family, clause) in [
            (Family::Tool, "nothing ran"),
            (Family::Egress, "nothing was sent"),
            (Family::Secret, "nothing was read"),
            (Family::Filesystem, "nothing was written"),
            (Family::Pin, "nothing ran"),
            (Family::Trust, "nothing ran"),
            (Family::Fence, "nothing ran"),
        ] {
            assert_eq!(family.nothing_clause(), clause);
        }
    }

    /// A refusal reason built from repository content cannot rewrite the
    /// terminal around the sentence that says it was refused. Invariant 7 at
    /// the one call site whose `why` is not machine-authored policy text.
    #[test]
    fn a_hostile_reason_cannot_escape_the_denial_sentence() {
        let hostile = "drifted\u{1b}[2J\u{1b}[H\nallowed: server started fine";
        let safe = bounded_reason(hostile);
        assert!(
            !safe.contains('\u{1b}') && !safe.contains('\n') && !safe.contains('\r'),
            "control characters must not survive: {safe:?}"
        );
        // Bounded, so a reason cannot scroll the denial off the screen.
        let long = "x".repeat(5_000);
        let bounded = bounded_reason(&long);
        assert!(
            bounded.chars().count() <= 201,
            "{}",
            bounded.chars().count()
        );
        assert!(
            bounded.ends_with('…'),
            "truncation must be visible, not silent"
        );
    }

    /// The pinning refusal must read as "this changed", not "you disallowed
    /// this" — the two have different next steps, which is why it is its own
    /// family rather than borrowing `Tool`.
    #[test]
    fn the_pin_denial_points_at_review_not_at_policy() {
        let d = Denial {
            family: Family::Pin,
            subject: "web-search",
            attempted: "be served by the gateway",
            why: "library definition drifted from agentstack.lock",
            next_step: "run `agentstack trust .` to review what changed",
        };
        let s = d.sentence();
        assert!(s.contains("nothing ran"), "{s}");
        assert!(s.contains("agentstack.lock"), "{s}");
        assert!(s.contains("trust"), "{s}");
    }

    /// The fence denial points at the lease, not at policy: nothing was
    /// disallowed, nothing has been selected yet, and opening a lease is the
    /// whole fix. It files under its own `tool` tag for the same reason.
    #[test]
    fn the_fence_denial_points_at_the_lease_not_at_policy() {
        let d = Denial {
            family: Family::Fence,
            subject: "demo",
            attempted: "call exfiltrate",
            why: "this project fences its servers behind toolsets and no open lease selects one that exposes it",
            next_step: "open a lease for the toolset that selects it: `agentstack_lease_open` profile=default",
        };
        let s = d.sentence();
        assert!(s.contains("nothing ran"), "{s}");
        assert!(s.contains("lease"), "{s}");
        assert!(s.contains("profile=default"), "{s}");
        assert!(!s.contains("[policy.tools]"), "{s}");
        assert_eq!(Family::Fence.audit_tool(), "fence");
        assert!(AUDIT_TOOLS.contains(&"fence"));
    }
}
