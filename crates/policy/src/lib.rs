//! The policy engine: composes the machine's `[policy.tools]` with a
//! project's into one effective decision.
//!
//! This is the seed of the Phase 1 intersection engine. The shipped v0
//! semantics move here unchanged from the gateway: machine AND project,
//! machine checked first so its refusal names the layer, and nothing a
//! repo declares can loosen the machine layer.
//!
//! Everything in this crate is pure — no I/O, no config loading. The
//! call sites load the two `Policy` values (`manifest::machine_policy()`
//! and the project manifest) and hand them in. That purity is what makes
//! the property test below meaningful: the whole decision surface is a
//! function of its arguments.

#![forbid(unsafe_code)]

use agentstack_core::manifest::Policy;

/// The effective firewall decision for one tool call: it must pass the
/// machine `[policy.tools]` AND the project's. Machine denies win by
/// construction — nothing a repo declares is consulted before the user's
/// own rules — and the error says which layer refused.
pub fn tool_decision(
    machine: &Policy,
    project: &Policy,
    server: &str,
    tool: &str,
) -> Result<(), String> {
    machine
        .tool_allowed(server, tool)
        .map_err(|rule| format!("{rule} (machine policy — ~/.agentstack/agentstack.toml)"))?;
    project.tool_allowed(server, tool)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `Policy` whose `[policy.tools]` holds these server → patterns
    /// entries. Constructed directly (no TOML round-trip) so the crate's
    /// dev-dependencies stay on the rule-6 strict list.
    fn tools(entries: &[(&str, &[&str])]) -> Policy {
        Policy {
            tools: entries
                .iter()
                .map(|(k, v)| (k.to_string(), v.iter().map(|s| s.to_string()).collect()))
                .collect(),
            ..Policy::default()
        }
    }

    /// Machine `[policy.tools]` and project `[policy.tools]` compose as AND:
    /// the machine layer is checked first (its refusal names the layer), the
    /// project layer cannot loosen it, and each still denies on its own.
    #[test]
    fn machine_policy_composes_with_deny_precedence() {
        let machine = tools(&[("figma", &["!post_*"])]);
        let project = tools(&[("figma", &["!delete_*"])]);
        // Machine deny wins and says so, even though the project allows it.
        let err = tool_decision(&machine, &project, "figma", "post_comment").unwrap_err();
        assert!(err.contains("machine policy"), "{err}");
        // Project deny still applies on its own.
        let err = tool_decision(&machine, &project, "figma", "delete_file").unwrap_err();
        assert!(!err.contains("machine policy"), "{err}");
        // A tool neither layer names passes.
        assert!(tool_decision(&machine, &project, "figma", "get_file").is_ok());
        // Other servers are untouched by either layer.
        assert!(tool_decision(&machine, &project, "github", "delete_repo").is_ok());
    }

    /// Two allowlists compose as nested bounds: the machine allowlist is the
    /// outer bound, the project's can only restrict further — never broaden.
    #[test]
    fn machine_and_project_allowlists_nest() {
        let machine = tools(&[("figma", &["get_*"])]);
        let project = tools(&[("figma", &["get_file"])]);
        // Inside both bounds.
        assert!(tool_decision(&machine, &project, "figma", "get_file").is_ok());
        // Inside the machine bound, outside the project's → project refuses.
        let err = tool_decision(&machine, &project, "figma", "get_node").unwrap_err();
        assert!(!err.contains("machine policy"), "{err}");
        // Outside the machine bound → machine refuses, whatever the project says.
        let err = tool_decision(&machine, &project, "figma", "delete_file").unwrap_err();
        assert!(err.contains("machine policy"), "{err}");
    }

    /// The `"*"` wildcard key constrains every server — the rename-proof form
    /// for machine rules, since named rules bind to repo-chosen server names.
    #[test]
    fn wildcard_policy_key_survives_server_renaming() {
        let machine = tools(&[("*", &["!delete_*"])]);
        let project = Policy::default();
        // Whatever a repo names the server, delete_* is refused…
        for server in ["github", "gh", "totally-not-github"] {
            let err = tool_decision(&machine, &project, server, "delete_repo").unwrap_err();
            assert!(err.contains("machine policy"), "{err}");
        }
        // …and everything else still passes.
        assert!(tool_decision(&machine, &project, "gh", "get_repo").is_ok());
    }

    // ── Property test: effective(B, M) ⊆ M (CLAUDE.md rule 2) ──────────────
    // NEVER delete or weaken this test. It is the first machine-checked
    // instance of the intersection invariant: for ALL machine policies M,
    // project policies B, servers, and tools — if M refuses the call, the
    // effective decision refuses it, regardless of anything B says. No code
    // path may ever produce an effective policy more permissive than the
    // machine policy.

    use proptest::prelude::*;

    /// A `[policy.tools]` pattern: optionally deny-prefixed, over a small
    /// alphabet with wildcards so allow/deny lists, globs, and the empty
    /// pattern all get exercised.
    fn pattern() -> impl Strategy<Value = String> {
        (any::<bool>(), "[a-z_*]{0,6}")
            .prop_map(|(deny, body)| if deny { format!("!{body}") } else { body })
    }

    /// An arbitrary tool policy: up to 4 server keys (real names or the `"*"`
    /// wildcard), each with up to 4 patterns. Other `Policy` fields don't
    /// participate in tool decisions and stay default.
    fn arb_policy() -> impl Strategy<Value = Policy> {
        proptest::collection::vec(
            (
                prop_oneof![Just("*".to_string()), "[a-z_]{1,8}"],
                proptest::collection::vec(pattern(), 0..4),
            ),
            0..4,
        )
        .prop_map(|entries| Policy {
            tools: entries.into_iter().collect(),
            ..Policy::default()
        })
    }

    proptest! {
        #[test]
        fn effective_is_never_more_permissive_than_machine(
            machine in arb_policy(),
            project in arb_policy(),
            // A server literally named "*" is generated too: tool_allowed
            // routes it to the wildcard key only, and the invariant must
            // hold on that path as well as for ordinary names.
            server in prop_oneof![9 => "[a-z_]{1,8}", 1 => Just("*".to_string())],
            tool in "[a-z_]{1,8}",
        ) {
            if machine.tool_allowed(&server, &tool).is_err() {
                prop_assert!(
                    tool_decision(&machine, &project, &server, &tool).is_err(),
                    "machine denied {server}.{tool} but the effective decision allowed it"
                );
            }
        }
    }
}
