//! Fail-closed content verification: does resolved content still match its
//! `agentstack.lock` pin? One decision seam so every use path — activation
//! (`use --write` / sessions / dashboard), the MCP skill loader, the gateway —
//! gates identically instead of hand-rolling four slightly different checks.
//!
//! The re-gate chain this module completes: content drifts → the use path
//! blocks and directs the human to `agentstack lock` → re-locking changes the
//! lockfile bytes → the trust digest (manifest + local + lock) flips → the
//! zero-files bridge drops to control-plane-only until the project is
//! reviewed and re-trusted. Trust stays bound to content because nothing is
//! used that doesn't match the lock the human trusted.

use crate::resolve::{InstructionLockStatus, ServerLockStatus, SkillLockStatus};

/// The verdict for one capability vs its lock pin.
///
/// (TS mental model: a discriminated union — every consumer `match`es it
/// exhaustively, so adding a variant is a compile error at each gate, never a
/// silently unhandled case.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Pinned and matching — safe to use.
    Ok,
    /// No lock entry yet. Sites differ deliberately: an explicit-consent
    /// activation may proceed (recording the first pin IS the pinning act,
    /// and it re-gates trust via the lock bytes); strict serve paths
    /// (gateway, inline-skill loads) must refuse.
    Unpinned,
    /// Drifted, broken, or unverifiable — never proceed. The message says why.
    Block(String),
}

/// Verdict for a skill's lock status. Anything that can't be positively
/// verified against a pin — drift, a broken ref, an uncached git source —
/// fails closed.
pub fn skill_verdict(status: &SkillLockStatus) -> Verdict {
    match status {
        SkillLockStatus::Matches => Verdict::Ok,
        SkillLockStatus::MissingLockEntry => Verdict::Unpinned,
        SkillLockStatus::ChecksumDrift { locked, current } => Verdict::Block(format!(
            "skill content drifted from agentstack.lock (locked {}, current {})",
            short(locked),
            short(current)
        )),
        SkillLockStatus::RevDrift { locked, current } => Verdict::Block(format!(
            "git rev drifted from agentstack.lock (locked {}, current {})",
            short(locked),
            short(current)
        )),
        SkillLockStatus::NotAvailableOffline { source } => Verdict::Block(format!(
            "git source {source} is not cached locally, so its pin can't be verified — run `agentstack install`"
        )),
        SkillLockStatus::ResolveFailed { error } => Verdict::Block(format!("broken ref — {error}")),
    }
}

/// Verdict for a server definition's lock status. Same fail-closed rule.
pub fn server_verdict(status: &ServerLockStatus) -> Verdict {
    match status {
        ServerLockStatus::Matches => Verdict::Ok,
        ServerLockStatus::MissingLockEntry => Verdict::Unpinned,
        ServerLockStatus::ChecksumDrift { locked, current } => Verdict::Block(format!(
            "server definition drifted from agentstack.lock (locked {}, current {})",
            short(locked),
            short(current)
        )),
        ServerLockStatus::ResolveFailed { error } => {
            Verdict::Block(format!("broken ref — {error}"))
        }
    }
}

/// Verdict for an instruction fragment's lock status. Same fail-closed rule:
/// drift and unreadable files block; a missing pin is `Unpinned` (the write
/// site records the first pin).
pub fn instruction_verdict(status: &InstructionLockStatus) -> Verdict {
    match status {
        InstructionLockStatus::Matches => Verdict::Ok,
        InstructionLockStatus::MissingLockEntry => Verdict::Unpinned,
        InstructionLockStatus::ChecksumDrift { locked, current } => Verdict::Block(format!(
            "instruction content drifted from agentstack.lock (locked {}, current {})",
            short(locked),
            short(current)
        )),
        InstructionLockStatus::ResolveFailed { error } => {
            Verdict::Block(format!("broken ref — {error}"))
        }
    }
}

/// The activation gate (`use --write`, session start, dashboard): block when
/// anything in the resolved set drifted or broke, naming every offender.
/// `Unpinned` passes — first activation records the pin, which itself flips
/// the trust digest (the consent chain, not a loophole).
pub fn ensure_activatable(
    what: &str,
    skills: &[(String, SkillLockStatus)],
    servers: &[(String, ServerLockStatus)],
) -> anyhow::Result<()> {
    let mut blocked: Vec<(String, String)> = Vec::new();
    for (name, status) in skills {
        if let Verdict::Block(why) = skill_verdict(status) {
            blocked.push((name.clone(), why));
        }
    }
    for (name, status) in servers {
        if let Verdict::Block(why) = server_verdict(status) {
            blocked.push((name.clone(), why));
        }
    }
    bail_blocked(&format!("activate {what}"), blocked)
}

/// The instruction-compile gate (`apply --write`, `instructions --write`):
/// same semantics as activation — drift/broken block, unpinned passes and the
/// write that follows records the first pin.
pub fn ensure_instructions_compilable(
    what: &str,
    instructions: &[(String, InstructionLockStatus)],
) -> anyhow::Result<()> {
    let blocked: Vec<(String, String)> = instructions
        .iter()
        .filter_map(|(name, status)| match instruction_verdict(status) {
            Verdict::Block(why) => Some((name.clone(), why)),
            _ => None,
        })
        .collect();
    bail_blocked(&format!("compile instructions for {what}"), blocked)
}

/// Shared fail-closed bail: name every offender, point at `agentstack lock`
/// (whose byte change is what re-gates trust).
fn bail_blocked(action: &str, blocked: Vec<(String, String)>) -> anyhow::Result<()> {
    if blocked.is_empty() {
        return Ok(());
    }
    let width = blocked.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
    let lines: Vec<String> = blocked
        .iter()
        .map(|(name, why)| format!("  {name:width$}  {why}"))
        .collect();
    anyhow::bail!(
        "refusing to {action}: {} pinned item(s) changed since agentstack.lock was written —\n{}\nReview the changes, then run `agentstack lock` to accept them (re-locking re-gates the project for auto mode).",
        blocked.len(),
        lines.join("\n")
    )
}

/// First 12 hex chars of a digest (or the whole string when shorter) — enough
/// to identify, short enough to read, like git's abbreviated object ids.
fn short(digest: &str) -> &str {
    digest.get(..12).unwrap_or(digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drift() -> SkillLockStatus {
        SkillLockStatus::ChecksumDrift {
            locked: "aaaaaaaaaaaaaaaa".into(),
            current: "bbbbbbbbbbbbbbbb".into(),
        }
    }

    #[test]
    fn skill_verdicts_fail_closed_on_everything_but_match_and_missing() {
        assert_eq!(skill_verdict(&SkillLockStatus::Matches), Verdict::Ok);
        assert_eq!(
            skill_verdict(&SkillLockStatus::MissingLockEntry),
            Verdict::Unpinned
        );
        for status in [
            drift(),
            SkillLockStatus::RevDrift {
                locked: "abc".into(),
                current: "def".into(),
            },
            SkillLockStatus::NotAvailableOffline {
                source: "https://example.com/x.git".into(),
            },
            SkillLockStatus::ResolveFailed {
                error: "nope".into(),
            },
        ] {
            assert!(
                matches!(skill_verdict(&status), Verdict::Block(_)),
                "{status:?} must block"
            );
        }
    }

    #[test]
    fn server_verdicts_fail_closed_on_drift_and_broken() {
        assert_eq!(server_verdict(&ServerLockStatus::Matches), Verdict::Ok);
        assert_eq!(
            server_verdict(&ServerLockStatus::MissingLockEntry),
            Verdict::Unpinned
        );
        for status in [
            ServerLockStatus::ChecksumDrift {
                locked: "a".into(),
                current: "b".into(),
            },
            ServerLockStatus::ResolveFailed {
                error: "nope".into(),
            },
        ] {
            assert!(
                matches!(server_verdict(&status), Verdict::Block(_)),
                "{status:?} must block"
            );
        }
    }

    #[test]
    fn block_messages_abbreviate_digests() {
        let Verdict::Block(msg) = skill_verdict(&SkillLockStatus::ChecksumDrift {
            locked: "aaaaaaaaaaaaaaaaaaaaaaaa".into(),
            current: "bbbbbbbbbbbbbbbbbbbbbbbb".into(),
        }) else {
            panic!("drift must block");
        };
        assert!(msg.contains("aaaaaaaaaaaa"), "{msg}");
        assert!(!msg.contains("aaaaaaaaaaaaa"), "not the full digest: {msg}");
    }

    #[test]
    fn instruction_verdicts_fail_closed_on_drift_and_broken() {
        assert_eq!(
            instruction_verdict(&InstructionLockStatus::Matches),
            Verdict::Ok
        );
        assert_eq!(
            instruction_verdict(&InstructionLockStatus::MissingLockEntry),
            Verdict::Unpinned
        );
        for status in [
            InstructionLockStatus::ChecksumDrift {
                locked: "a".into(),
                current: "b".into(),
            },
            InstructionLockStatus::ResolveFailed {
                error: "nope".into(),
            },
        ] {
            assert!(
                matches!(instruction_verdict(&status), Verdict::Block(_)),
                "{status:?} must block"
            );
        }
    }

    #[test]
    fn ensure_instructions_compilable_blocks_on_drift_passes_on_unpinned() {
        let ok = vec![
            ("house".to_string(), InstructionLockStatus::Matches),
            ("style".to_string(), InstructionLockStatus::MissingLockEntry),
        ];
        assert!(ensure_instructions_compilable("claude-code", &ok).is_ok());

        let drifted = vec![(
            "house".to_string(),
            InstructionLockStatus::ChecksumDrift {
                locked: "aaaa".into(),
                current: "bbbb".into(),
            },
        )];
        let err = ensure_instructions_compilable("claude-code", &drifted)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("refusing to compile instructions for claude-code"),
            "{err}"
        );
        assert!(err.contains("house"), "{err}");
        assert!(err.contains("`agentstack lock`"), "{err}");
    }

    #[test]
    fn ensure_activatable_passes_on_matches_and_unpinned() {
        let skills = vec![
            ("a".to_string(), SkillLockStatus::Matches),
            ("b".to_string(), SkillLockStatus::MissingLockEntry),
        ];
        let servers = vec![("s".to_string(), ServerLockStatus::MissingLockEntry)];
        assert!(ensure_activatable("'p'", &skills, &servers).is_ok());
    }

    #[test]
    fn ensure_activatable_blocks_naming_every_offender() {
        let skills = vec![
            ("good".to_string(), SkillLockStatus::Matches),
            ("code-review".to_string(), drift()),
        ];
        let servers = vec![(
            "kibana".to_string(),
            ServerLockStatus::ChecksumDrift {
                locked: "x".into(),
                current: "y".into(),
            },
        )];
        let err = ensure_activatable("'backend'", &skills, &servers)
            .unwrap_err()
            .to_string();
        assert!(err.contains("refusing to activate 'backend'"), "{err}");
        assert!(err.contains("2 pinned item(s)"), "{err}");
        assert!(err.contains("code-review"), "{err}");
        assert!(err.contains("kibana"), "{err}");
        assert!(!err.contains("good"), "unchanged items stay out: {err}");
        assert!(err.contains("`agentstack lock`"), "{err}");
    }
}
