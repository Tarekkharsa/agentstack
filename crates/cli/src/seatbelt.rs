//! The seatbelt: one shape for every enforcement denial the user meets.
//!
//! Strategy v2 Phase 3, "seatbelt legibility". Five families can refuse an
//! action — a gateway tool block, an egress refusal, a secret-scope refusal,
//! the filesystem guard, and (added in Phase 4) the content-pinning refusal
//! that withholds a server whose bytes no longer match what was reviewed.
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
//! renders the `CallRecord`, not those variants.)
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
pub const AUDIT_TOOLS: &[&str] = &["tool", "egress", "secret", "filesystem", "pin"];

/// Whether a call record is a seatbelt enforcement denial rather than a
/// brokered call. Both live in `calls.jsonl`; only the latter is a signal
/// about whether a server is used.
pub fn is_enforcement_record(rec: &agentstack_recorder::CallRecord) -> bool {
    matches!(rec.outcome, agentstack_recorder::CallOutcome::Denied)
        && AUDIT_TOOLS.contains(&rec.tool.as_str())
}

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

fn now_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// F20 witness: a seatbelt enforcement denial is recognizable as one, so
    /// `optimize` can exclude it from brokered-call counts. The tamper is the
    /// exact shape the seatbelt writes — a `Denied` record whose `tool` is a
    /// family tag — which used to be counted as a gateway call to the subject.
    #[test]
    fn an_enforcement_denial_is_distinguishable_from_a_brokered_call() {
        let mk = |tool: &str, outcome: agentstack_recorder::CallOutcome| {
            agentstack_recorder::CallRecord {
                ts: 0,
                run: None,
                pid: 1,
                project: None,
                server: "web-search".into(),
                tool: tool.into(),
                args_digest: String::new(),
                outcome,
                detail: None,
                ms: 0,
            }
        };
        // Every seatbelt family tag, denied → recognized as enforcement.
        for tag in AUDIT_TOOLS {
            assert!(
                is_enforcement_record(&mk(tag, agentstack_recorder::CallOutcome::Denied)),
                "'{tag}' denial must read as enforcement"
            );
        }
        // A real brokered tool call is NOT enforcement, whatever its outcome —
        // even a denied one, because its tool name is not a family tag.
        assert!(!is_enforcement_record(&mk(
            "search_web",
            agentstack_recorder::CallOutcome::Denied
        )));
        assert!(!is_enforcement_record(&mk(
            "egress",
            agentstack_recorder::CallOutcome::Ok
        )));
    }

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
}
