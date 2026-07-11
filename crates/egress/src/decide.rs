//! The per-connection policy decision: given the MCP server that opened a
//! connection and the host it wants to reach, allow or block per the compiled
//! ruleset, and produce the recorder event. This is the one place the async
//! proxy server (2.2) will call for every CONNECT it accepts.

use agentstack_policy::{CompiledRuleset, RULESET_VERSION};
use agentstack_recorder::{now_epoch, RunEvent};

/// The outcome of one egress decision: whether to allow the connection, plus
/// the flight-recorder event to append either way (allow and block are BOTH
/// recorded — a run report shows what a sandbox reached, not only what it was
/// denied).
#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    pub allowed: bool,
    pub event: RunEvent,
}

/// Wraps the compiled ruleset the `cli` handed the proxy (serialized across the
/// process boundary) so the proxy enforces the identical policy the gateway
/// does, without re-deriving anything.
pub struct EgressGuard {
    ruleset: CompiledRuleset,
}

impl EgressGuard {
    pub fn new(ruleset: CompiledRuleset) -> Self {
        EgressGuard { ruleset }
    }

    /// Decide one outbound connection. `server` is the MCP server identity the
    /// connection is attributed to (the proxy gives each server its own
    /// identity); `host` is the SNI/CONNECT hostname. Allow-by-default when no
    /// rule constrains the server — the same grammar the gateway uses; the
    /// machine layer narrows it, and the compiled ruleset already folded both
    /// layers.
    pub fn decide(&self, server: &str, host: &str) -> Decision {
        // Fail closed on a ruleset newer than this consumer understands — the
        // artifact's own ruling ("the enforcement artifact, not advisory
        // config"). With the sidecar proxy the ruleset crosses a real process
        // boundary into a separately-built binary, so version skew is
        // possible for real, not just in theory.
        let result = if self.ruleset.version > RULESET_VERSION {
            Err(format!(
                "ruleset version {} is newer than this proxy understands \
                 (max {RULESET_VERSION}) — failing closed",
                self.ruleset.version
            ))
        } else {
            self.ruleset.egress_decision(server, host)
        };
        let allowed = result.is_ok();
        let rule = result.err();
        Decision {
            allowed,
            event: RunEvent::Egress {
                ts: now_epoch(),
                server: server.to_string(),
                host: host.to_string(),
                allowed,
                rule,
            },
        }
    }
}

/// Build a block event for a connection that policy *allowed* but a transport
/// guard (SSRF address-class check, SNI mismatch) then refused. Recorded so a
/// run report shows the connection was stopped and why — the audit trail must
/// reflect the final outcome, not the intermediate policy verdict.
pub fn guard_block_event(server: &str, host: &str, rule: String) -> RunEvent {
    RunEvent::Egress {
        ts: now_epoch(),
        server: server.to_string(),
        host: host.to_string(),
        allowed: false,
        rule: Some(rule),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentstack_core::manifest::Policy;

    /// Build a machine ruleset with `[policy.egress]` entries, directly (no
    /// TOML round-trip, keeping egress's dependency set minimal).
    fn ruleset(entries: &[(&str, &[&str])], server: &str) -> CompiledRuleset {
        let mut machine = Policy::default();
        for (k, patterns) in entries {
            machine.egress.insert(
                k.to_string(),
                patterns.iter().map(|s| s.to_string()).collect(),
            );
        }
        agentstack_policy::compile(&machine, &Policy::default(), &[server])
    }

    #[test]
    fn allows_unconstrained_and_records_it() {
        // No egress policy at all → allow-by-default.
        let guard = EgressGuard::new(CompiledRuleset::default());
        let d = guard.decide("web-search", "api.search.example");
        assert!(d.allowed);
        match d.event {
            RunEvent::Egress {
                server,
                host,
                allowed,
                rule,
                ..
            } => {
                assert_eq!(server, "web-search");
                assert_eq!(host, "api.search.example");
                assert!(allowed);
                assert!(rule.is_none(), "an allow carries no rule");
            }
            other => panic!("expected Egress, got {other:?}"),
        }
    }

    #[test]
    fn blocks_a_denied_host_and_records_the_rule_and_layer() {
        let rs = ruleset(&[("*", &["!evil.example"])], "web-search");
        let guard = EgressGuard::new(rs);
        let d = guard.decide("web-search", "evil.example");
        assert!(!d.allowed);
        match d.event {
            RunEvent::Egress { allowed, rule, .. } => {
                assert!(!allowed);
                let rule = rule.expect("a block names its rule");
                assert!(rule.contains("[policy.egress]"), "{rule}");
                assert!(rule.contains("machine policy"), "{rule}");
            }
            other => panic!("expected Egress, got {other:?}"),
        }
        // A different host on the same server is still allowed.
        assert!(guard.decide("web-search", "api.search.example").allowed);
    }

    /// A ruleset from a future, unknown version denies EVERYTHING — never
    /// guess at semantics you don't understand. NEVER delete or weaken this.
    #[test]
    fn unknown_future_ruleset_version_fails_closed() {
        // Allow-by-default rules… but a version too new to trust.
        let rs = CompiledRuleset {
            version: agentstack_policy::RULESET_VERSION + 1,
            ..CompiledRuleset::default()
        };
        let guard = EgressGuard::new(rs);
        let d = guard.decide("any-server", "api.search.example");
        assert!(!d.allowed, "an unknown version must deny everything");
        match d.event {
            RunEvent::Egress { rule, .. } => {
                let rule = rule.expect("the block names the version mismatch");
                assert!(rule.contains("version"), "{rule}");
            }
            other => panic!("expected Egress, got {other:?}"),
        }
    }

    #[test]
    fn machine_wildcard_deny_is_rename_proof() {
        let rs = ruleset(&[("*", &["!*.internal"])], "anything");
        let guard = EgressGuard::new(rs);
        // Whatever a repo names its server, the machine wildcard deny holds.
        assert!(!guard.decide("renamed-server", "db.internal").allowed);
        assert!(!guard.decide("another-name", "svc.internal").allowed);
    }
}
