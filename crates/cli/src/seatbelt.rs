//! The seatbelt: one shape for every enforcement denial the user meets.
//!
//! Strategy v2 Phase 3, "seatbelt legibility". Four families can refuse an
//! action — a gateway tool block, an egress refusal, a secret-scope refusal,
//! and the filesystem guard. Before this module each wrote its own sentence,
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
//! And it is backed by evidence: the same denial is recorded, so
//! `agentstack report` can show it later rather than the user having to have
//! been watching the terminal at the time.
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
        }
    }
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
        ] {
            assert_eq!(family.nothing_clause(), clause);
        }
    }
}
