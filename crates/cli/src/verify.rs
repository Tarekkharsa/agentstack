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

use crate::executable::ExecutableLockStatus;
use crate::resolve::{
    ExtensionLockStatus, FrozenServer, InstructionLockStatus, ServerLockStatus, SkillLockStatus,
};

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

/// Verdict for a D3 executable input's lock status (contract §8). Same
/// fail-closed rule as the other kinds; an underivable surface (symlink,
/// traversal, broken root) blocks outright.
pub fn executable_verdict(status: &ExecutableLockStatus) -> Verdict {
    match status {
        ExecutableLockStatus::Matches => Verdict::Ok,
        ExecutableLockStatus::MissingLockEntry => Verdict::Unpinned,
        ExecutableLockStatus::ChecksumDrift { locked, current } => Verdict::Block(format!(
            "local executable content drifted from agentstack.lock (locked {}, current {})",
            short(locked),
            short(current)
        )),
        ExecutableLockStatus::ResolveFailed { error } => Verdict::Block(error.clone()),
    }
}

/// Verdict for a native extension's lock status (D6). Same fail-closed rule;
/// a retargeted extension blocks like drifted bytes — the reviewed pin bound
/// the code to one harness, and pointing it elsewhere needs a re-review.
pub fn extension_verdict(status: &ExtensionLockStatus) -> Verdict {
    match status {
        ExtensionLockStatus::Matches => Verdict::Ok,
        ExtensionLockStatus::MissingLockEntry => Verdict::Unpinned,
        ExtensionLockStatus::ChecksumDrift { locked, current } => Verdict::Block(format!(
            "extension content drifted from agentstack.lock (locked {}, current {})",
            short(locked),
            short(current)
        )),
        ExtensionLockStatus::TargetDrift { locked, current } => Verdict::Block(format!(
            "extension target changed since it was locked (locked '{locked}', now '{current}') — re-run `agentstack lock`"
        )),
        ExtensionLockStatus::RevDrift { locked, current } => Verdict::Block(format!(
            "extension git rev drifted from agentstack.lock (locked {}, current {})",
            short(locked),
            short(current)
        )),
        ExtensionLockStatus::NotAvailableOffline { source } => Verdict::Block(format!(
            "git source {source} is not cached locally, so its pin can't be verified — run `agentstack install`"
        )),
        ExtensionLockStatus::ResolveFailed { error } => Verdict::Block(error.clone()),
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

/// Strict locked-run input verification (Phase 0A `run <harness> --locked`).
/// **Not wired to any runtime path yet.**
///
/// Unlike [`ensure_activatable`] — which lets an `Unpinned` item through so the
/// first activation can record its pin — every lock-pinnable input must be
/// pinned and matching; every frozen server must resolve and pass its required
/// integrity check. A missing pin, drift, or broken ref all block, and every
/// offender is named together, kind-qualified (`skill 'x'`, `instruction 'x'`,
/// `server 'x'`, `extension 'x'`) so colliding names across capability kinds
/// stay distinct.
///
/// The frozen server set is **borrowed**: this verifier proves the exact set
/// acceptable without mutating or re-resolving it, so the grant builder can then
/// move that same set into the `AuthorityGrant`. Inline servers carry no
/// separate [`ServerLockStatus`] here — their declarations are bound by the
/// trust digest — so a frozen `Err` only ever represents an unresolved or
/// library-pin-unverifiable server (see [`crate::resolve::verify_library_pin`]).
pub fn ensure_locked_inputs(
    what: &str,
    skills: &[(String, SkillLockStatus)],
    instructions: &[(String, InstructionLockStatus)],
    frozen_servers: &[FrozenServer],
    executables: &[(String, ExecutableLockStatus)],
    extensions: &[(String, ExtensionLockStatus)],
) -> anyhow::Result<()> {
    let mut blocked: Vec<(String, String)> = Vec::new();
    for (name, status) in skills {
        if let Some(why) = locked_offender(skill_verdict(status)) {
            blocked.push((format!("skill '{name}'"), why));
        }
    }
    for (name, status) in instructions {
        if let Some(why) = locked_offender(instruction_verdict(status)) {
            blocked.push((format!("instruction '{name}'"), why));
        }
    }
    for (name, status) in extensions {
        if let Some(why) = locked_offender(extension_verdict(status)) {
            blocked.push((format!("extension '{name}'"), why));
        }
    }
    for (name, resolved) in frozen_servers {
        if let Err(reason) = resolved {
            blocked.push((format!("server '{name}'"), reason.clone()));
        }
    }
    // D3 (contract §8): executable labels arrive pre-qualified from
    // `executable_lock_statuses` ("executable 'x' (server 's')") — the kind
    // and owning server are part of the label, not re-derived here.
    for (label, status) in executables {
        if let Some(why) = locked_offender(executable_verdict(status)) {
            blocked.push((label.clone(), why));
        }
    }
    bail_locked(&format!("run {what} with --locked"), blocked)
}

/// Strict offender extraction for locked runs: reuses the activation verdict but
/// treats a missing pin (`Unpinned`) as an offender too — a locked run allows no
/// first-pin, so every input must already be pinned and matching.
fn locked_offender(verdict: Verdict) -> Option<String> {
    match verdict {
        Verdict::Ok => None,
        Verdict::Unpinned => {
            Some("not pinned in agentstack.lock — pin it with `agentstack lock`".to_string())
        }
        Verdict::Block(why) => Some(why),
    }
}

/// Format the aligned offender list shared by every fail-closed bail: one
/// `  name  why` line per offender, names padded to a common width.
fn offender_lines(blocked: &[(String, String)]) -> String {
    let width = blocked.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
    blocked
        .iter()
        .map(|(name, why)| format!("  {name:width$}  {why}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Shared fail-closed bail: name every offender, point at `agentstack lock`
/// (whose byte change is what re-gates trust).
fn bail_blocked(action: &str, blocked: Vec<(String, String)>) -> anyhow::Result<()> {
    if blocked.is_empty() {
        return Ok(());
    }
    anyhow::bail!(
        "refusing to {action}: {} pinned item(s) changed since agentstack.lock was written —\n{}\nReview the changes, then run `agentstack lock` to accept them (re-locking re-gates the project for auto mode).",
        blocked.len(),
        offender_lines(&blocked)
    )
}

/// Fail-closed bail for strict locked verification: reuses the shared offender
/// formatter, but with a lead-in that fits every locked failure mode — a missing
/// pin, drift, an unresolved server, or a failed integrity check — not only
/// "changed" content.
fn bail_locked(action: &str, blocked: Vec<(String, String)>) -> anyhow::Result<()> {
    if blocked.is_empty() {
        return Ok(());
    }
    anyhow::bail!(
        "refusing to {action}: {} input(s) failed locked integrity verification —\n{}\nLock-pinnable inputs must be pinned and matching, and every frozen server must resolve and pass its required integrity check; review them, then run `agentstack lock`.",
        blocked.len(),
        offender_lines(&blocked)
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

    fn ok_server(name: &str) -> FrozenServer {
        let server: agentstack_core::manifest::Server =
            toml::from_str("type = \"stdio\"\ncommand = \"node\"\n").unwrap();
        (
            name.to_string(),
            Ok(crate::resolve::ResolvedServer {
                name: name.to_string(),
                origin: crate::resolve::ServerOrigin::Inline,
                server,
                checksum: String::new(),
                provenance: None,
            }),
        )
    }

    fn failed_server(name: &str, reason: &str) -> FrozenServer {
        (name.to_string(), Err(reason.to_string()))
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

    #[test]
    fn ensure_locked_inputs_blocks_missing_pins_of_all_three_kinds() {
        // Two real MissingLockEntry statuses (skill, instruction) plus a frozen
        // library server whose pin could not be verified — all three block, each
        // kind-qualified in the report.
        let skills = vec![("s".to_string(), SkillLockStatus::MissingLockEntry)];
        let instructions = vec![("i".to_string(), InstructionLockStatus::MissingLockEntry)];
        let servers = vec![failed_server(
            "srv",
            "library server is not pinned in agentstack.lock — pin it with `agentstack lock`",
        )];
        let err = ensure_locked_inputs("claude-code", &skills, &instructions, &servers, &[], &[])
            .unwrap_err()
            .to_string();
        assert!(err.contains("3 input(s)"), "{err}");
        assert!(err.contains("skill 's'"), "{err}");
        assert!(err.contains("instruction 'i'"), "{err}");
        assert!(err.contains("server 'srv'"), "{err}");
        assert!(err.contains("`agentstack lock`"), "{err}");
    }

    #[test]
    fn ensure_locked_inputs_blocks_drifted_and_missing_executables() {
        // Executables follow the same strict rule as every other kind: a
        // missing pin AND drift both block, labels arrive pre-qualified.
        let executables = vec![
            (
                "executable 'scripts/run.sh' (server 'agent')".to_string(),
                ExecutableLockStatus::ChecksumDrift {
                    locked: "aaaaaaaaaaaaaaaa".into(),
                    current: "bbbbbbbbbbbbbbbb".into(),
                },
            ),
            (
                "integrity root 'tools' (server 'agent')".to_string(),
                ExecutableLockStatus::MissingLockEntry,
            ),
            (
                "executable 'ok.sh' (server 'agent')".to_string(),
                ExecutableLockStatus::Matches,
            ),
        ];
        let err = ensure_locked_inputs("claude-code", &[], &[], &[], &executables, &[])
            .unwrap_err()
            .to_string();
        assert!(err.contains("2 input(s)"), "{err}");
        assert!(
            err.contains("executable 'scripts/run.sh' (server 'agent')"),
            "{err}"
        );
        assert!(
            err.contains("integrity root 'tools' (server 'agent')"),
            "{err}"
        );
        assert!(!err.contains("ok.sh"), "matching pins stay out: {err}");
        assert!(err.contains("`agentstack lock`"), "{err}");
    }

    #[test]
    fn extension_verdicts_fail_closed_and_locked_gate_names_them() {
        // The verdict family rule: match passes, missing pin is Unpinned, and
        // every drift flavor — bytes, target, undigestable source — blocks.
        assert_eq!(
            extension_verdict(&ExtensionLockStatus::Matches),
            Verdict::Ok
        );
        assert_eq!(
            extension_verdict(&ExtensionLockStatus::MissingLockEntry),
            Verdict::Unpinned
        );
        for status in [
            ExtensionLockStatus::ChecksumDrift {
                locked: "a".into(),
                current: "b".into(),
            },
            ExtensionLockStatus::TargetDrift {
                locked: "pi".into(),
                current: "opencode".into(),
            },
            ExtensionLockStatus::ResolveFailed {
                error: "nope".into(),
            },
        ] {
            assert!(
                matches!(extension_verdict(&status), Verdict::Block(_)),
                "{status:?} must block"
            );
        }

        // The strict gate treats extensions like every other kind: missing
        // pins block too, kind-qualified in the report.
        let extensions = vec![(
            "checkpoint".to_string(),
            ExtensionLockStatus::MissingLockEntry,
        )];
        let err = ensure_locked_inputs("pi", &[], &[], &[], &[], &extensions)
            .unwrap_err()
            .to_string();
        assert!(err.contains("extension 'checkpoint'"), "{err}");
        assert!(err.contains("`agentstack lock`"), "{err}");
    }

    #[test]
    fn locked_strictness_does_not_leak_into_existing_activation() {
        // The existing gates keep first-pin semantics: a missing pin passes,
        // including ServerLockStatus::MissingLockEntry.
        let skills = vec![("s".to_string(), SkillLockStatus::MissingLockEntry)];
        let servers = vec![("srv".to_string(), ServerLockStatus::MissingLockEntry)];
        assert!(ensure_activatable("'p'", &skills, &servers).is_ok());

        let instructions = vec![("i".to_string(), InstructionLockStatus::MissingLockEntry)];
        assert!(ensure_instructions_compilable("claude-code", &instructions).is_ok());
    }

    #[test]
    fn ensure_locked_inputs_reports_multiple_frozen_server_failures_together() {
        let servers = vec![
            failed_server(
                "a",
                "library definition drifted from agentstack.lock (locked x, current y)",
            ),
            failed_server("b", "library server is not pinned in agentstack.lock"),
        ];
        let err = ensure_locked_inputs("x", &[], &[], &servers, &[], &[])
            .unwrap_err()
            .to_string();
        assert!(err.contains("2 input(s)"), "{err}");
        assert!(err.contains("server 'a'"), "{err}");
        assert!(err.contains("server 'b'"), "{err}");
    }

    #[test]
    fn ensure_locked_inputs_passes_clean_sets_and_leaves_them_usable() {
        // Empty everything is trivially acceptable.
        assert!(ensure_locked_inputs("x", &[], &[], &[], &[], &[]).is_ok());

        // A valid non-empty set: matching skill + instruction + an Ok frozen
        // server all pass.
        let skills = vec![("s".to_string(), SkillLockStatus::Matches)];
        let instructions = vec![("i".to_string(), InstructionLockStatus::Matches)];
        let servers = vec![ok_server("srv")];
        assert!(ensure_locked_inputs("x", &skills, &instructions, &servers, &[], &[]).is_ok());

        // Borrowed, not consumed: the exact frozen set is still usable after.
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].0, "srv");
        assert!(servers[0].1.is_ok());
    }
}
