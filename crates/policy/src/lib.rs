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

mod compile;
pub mod ruleset;

pub use compile::compile;
pub use ruleset::{CompiledRuleset, RULESET_VERSION};

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

/// The effective egress decision for one (server, host): machine
/// `[policy.egress]` AND the project's, machine denies named and first.
/// Phase 1 applies this to a server's DECLARED URL host at write/spawn time;
/// runtime filtering is the Phase-2 proxy's job.
pub fn egress_decision(
    machine: &Policy,
    project: &Policy,
    server: &str,
    host: &str,
) -> Result<(), String> {
    machine
        .egress_allowed(server, host)
        .map_err(|rule| format!("{rule} (machine policy — ~/.agentstack/agentstack.toml)"))?;
    project.egress_allowed(server, host)
}

/// The effective secret-access decision for one (server, `${REF}` name):
/// machine `[policy.secrets]` AND the project's. Enforced fail-closed at both
/// substitution sites — a denied ref never resolves, never renders.
pub fn secret_decision(
    machine: &Policy,
    project: &Policy,
    server: &str,
    reference: &str,
) -> Result<(), String> {
    machine
        .secret_allowed(server, reference)
        .map_err(|rule| format!("{rule} (machine policy — ~/.agentstack/agentstack.toml)"))?;
    project.secret_allowed(server, reference)
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

    /// An arbitrary keyed dimension: up to 4 server keys (real names or the
    /// `"*"` wildcard), each with up to 4 patterns.
    fn arb_map() -> impl Strategy<Value = Vec<(String, Vec<String>)>> {
        proptest::collection::vec(
            (
                prop_oneof![Just("*".to_string()), "[a-z_]{1,8}"],
                proptest::collection::vec(pattern(), 0..4),
            ),
            0..4,
        )
    }

    /// An arbitrary tool policy: the tools generator is unchanged from the
    /// original guarded proptest; egress/secrets stay default here so the
    /// original invariant's input distribution is untouched.
    fn arb_policy() -> impl Strategy<Value = Policy> {
        arb_map().prop_map(|entries| Policy {
            tools: entries.into_iter().collect(),
            ..Policy::default()
        })
    }

    /// An arbitrary policy across ALL keyed dimensions (tools + egress +
    /// secrets), for the compiled-ruleset and per-dimension invariants.
    fn arb_policy_full() -> impl Strategy<Value = Policy> {
        (arb_map(), arb_map(), arb_map()).prop_map(|(tools, egress, secrets)| Policy {
            tools: tools.into_iter().collect(),
            egress: egress.into_iter().collect(),
            secrets: secrets.into_iter().collect(),
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

    // ── Per-dimension invariants: same ⊆ M law, one named test each so no
    //    single deletion can silently drop a dimension from coverage.
    //    NEVER delete or weaken these tests.

    proptest! {
        #[test]
        fn effective_egress_never_more_permissive_than_machine(
            machine in arb_policy_full(),
            project in arb_policy_full(),
            server in prop_oneof![9 => "[a-z_]{1,8}", 1 => Just("*".to_string())],
            host in "[a-z_.]{1,12}",
        ) {
            if machine.egress_allowed(&server, &host).is_err() {
                prop_assert!(egress_decision(&machine, &project, &server, &host).is_err());
            }
        }

        #[test]
        fn effective_secrets_never_more_permissive_than_machine(
            machine in arb_policy_full(),
            project in arb_policy_full(),
            server in prop_oneof![9 => "[a-z_]{1,8}", 1 => Just("*".to_string())],
            reference in "[A-Z_]{1,10}",
        ) {
            if machine.secret_allowed(&server, &reference).is_err() {
                prop_assert!(secret_decision(&machine, &project, &server, &reference).is_err());
            }
        }
    }

    /// A machine `"*"` deny survives whatever a hostile repo renames its
    /// server to — for egress and secrets this is the primary attack, so it
    /// gets the same explicit test the tools dimension has.
    #[test]
    fn wildcard_egress_and_secret_rules_survive_server_renaming() {
        let mut machine = Policy::default();
        machine
            .egress
            .insert("*".into(), vec!["!169.254.169.254".into()]);
        machine.secrets.insert("*".into(), vec!["!AWS_*".into()]);
        let project = Policy::default();
        for server in ["github", "gh", "totally-not-github"] {
            let err = egress_decision(&machine, &project, server, "169.254.169.254").unwrap_err();
            assert!(err.contains("machine policy"), "{err}");
            let err = secret_decision(&machine, &project, server, "AWS_SECRET_KEY").unwrap_err();
            assert!(err.contains("machine policy"), "{err}");
        }
        assert!(egress_decision(&machine, &project, "gh", "api.github.com").is_ok());
        assert!(secret_decision(&machine, &project, "gh", "GITHUB_TOKEN").is_ok());
    }

    #[test]
    fn compiled_egress_scopes_by_port() {
        // Machine allows the host only on 443.
        let mut machine = Policy::default();
        machine
            .egress
            .insert("api".into(), vec!["api.example.com:443".into()]);
        let rs = compile(&machine, &Policy::default(), &["api"]);
        // The runtime decision (Some(port)) enforces the port exactly.
        assert!(rs
            .egress_decision("api", "api.example.com", Some(443))
            .is_ok());
        assert!(rs
            .egress_decision("api", "api.example.com", Some(22))
            .is_err());
        // A host-only (write-time) check defers the port and still allows.
        assert!(rs.egress_decision("api", "api.example.com", None).is_ok());
    }

    /// Pinning a port can only NARROW: if the machine allows a host on any port
    /// and the bundle pins 443, the effective decision denies 22 (which the
    /// machine alone would have allowed). Rule 2 in the port dimension.
    #[test]
    fn a_bundle_port_pin_narrows_the_machine_any_port_allow() {
        let mut machine = Policy::default();
        machine
            .egress
            .insert("api".into(), vec!["api.example.com".into()]); // any port
        let mut bundle = Policy::default();
        bundle
            .egress
            .insert("api".into(), vec!["api.example.com:443".into()]); // 443 only
        let rs = compile(&machine, &bundle, &["api"]);
        assert!(rs
            .egress_decision("api", "api.example.com", Some(443))
            .is_ok());
        // 22 is allowed by the machine layer alone but denied by the bundle pin
        // → effective is narrower, never wider.
        assert!(rs
            .egress_decision("api", "api.example.com", Some(22))
            .is_err());
    }

    // ── Compiled-ruleset invariants ─────────────────────────────────────────
    // The compiled artifact must be exactly as strict as the live two-layer
    // decision — never more permissive than the machine (rule 2 restated on
    // the artifact), and behavior-preserving in BOTH directions so Phase 2
    // consumers inherit today's semantics unchanged.
    // NEVER delete or weaken these tests.

    /// Server-name set for compilation, plus a lookup name that is sometimes
    /// in the set, sometimes not (exercising the `any` fallback), sometimes
    /// literally `"*"`.
    fn arb_servers_and_lookup() -> impl Strategy<Value = (Vec<String>, String)> {
        (
            proptest::collection::vec("[a-z_]{1,8}", 0..3),
            prop_oneof![
                6 => "[a-z_]{1,8}",
                3 => Just("__pick__".to_string()),
                1 => Just("*".to_string())
            ],
        )
            .prop_map(|(servers, lookup)| {
                let lookup = if lookup == "__pick__" {
                    servers.first().cloned().unwrap_or_else(|| "alpha".into())
                } else {
                    lookup
                };
                (servers, lookup)
            })
    }

    proptest! {
        /// compile() changes representation, never decisions: for every input
        /// the compiled tool decision matches the live two-layer decision —
        /// including for servers absent from the compiled set (any-bucket
        /// routing) and for policy-named servers the bundle never declared.
        #[test]
        fn compilation_preserves_tool_decisions(
            machine in arb_policy_full(),
            project in arb_policy_full(),
            (servers, lookup) in arb_servers_and_lookup(),
            tool in "[a-z_]{1,8}",
        ) {
            let names: Vec<&str> = servers.iter().map(String::as_str).collect();
            let ruleset = compile(&machine, &project, &names);
            prop_assert_eq!(
                ruleset.tool_decision(&lookup, &tool).is_ok(),
                tool_decision(&machine, &project, &lookup, &tool).is_ok(),
                "compiled and live decisions diverged for {}.{}", lookup, tool
            );
        }

        /// Same equivalence for the egress and secrets dimensions.
        #[test]
        fn compilation_preserves_egress_and_secret_decisions(
            machine in arb_policy_full(),
            project in arb_policy_full(),
            (servers, lookup) in arb_servers_and_lookup(),
            subject in "[a-zA-Z_.]{1,10}",
        ) {
            let names: Vec<&str> = servers.iter().map(String::as_str).collect();
            let ruleset = compile(&machine, &project, &names);
            // Host-only (port=None) so both sides are the same shape — the live
            // reference `egress_decision` is host-only; port-scoping is covered
            // by dedicated tests in ruleset.rs.
            prop_assert_eq!(
                ruleset.egress_decision(&lookup, &subject, None).is_ok(),
                egress_decision(&machine, &project, &lookup, &subject).is_ok()
            );
            prop_assert_eq!(
                ruleset.secret_decision(&lookup, &subject).is_ok(),
                secret_decision(&machine, &project, &lookup, &subject).is_ok()
            );
        }

        /// Rule 2 stated directly on the artifact, independent of the
        /// equivalence test above (defense in depth: if compile and the
        /// equivalence test ever drift together, this still bites): a call
        /// the machine layer denies is denied by the compiled ruleset,
        /// whatever the bundle says.
        #[test]
        fn compiled_is_never_more_permissive_than_machine(
            machine in arb_policy_full(),
            project in arb_policy_full(),
            (servers, lookup) in arb_servers_and_lookup(),
            tool in "[a-z_]{1,8}",
        ) {
            let names: Vec<&str> = servers.iter().map(String::as_str).collect();
            if machine.tool_allowed(&lookup, &tool).is_err() {
                prop_assert!(
                    compile(&machine, &project, &names).tool_decision(&lookup, &tool).is_err(),
                    "machine denied {}.{} but the compiled ruleset allowed it", lookup, tool
                );
            }
        }

        /// Rule 2 on the artifact: whatever the bundle says, the compiled
        /// ruleset never allows a call the machine-only compilation forbids.
        #[test]
        fn compiled_bundle_only_narrows(
            machine in arb_policy_full(),
            project in arb_policy_full(),
            (servers, lookup) in arb_servers_and_lookup(),
            tool in "[a-z_]{1,8}",
        ) {
            let names: Vec<&str> = servers.iter().map(String::as_str).collect();
            let machine_only = compile(&machine, &Policy::default(), &names);
            let both = compile(&machine, &project, &names);
            if machine_only.tool_decision(&lookup, &tool).is_err() {
                prop_assert!(both.tool_decision(&lookup, &tool).is_err());
            }
        }

        /// The wire contract: serde roundtrip is lossless, and the serialized
        /// bytes are identical regardless of the policies' IndexMap insertion
        /// order (canonicalization is what Phase 2 hands across the process
        /// boundary).
        #[test]
        fn compiled_ruleset_serializes_deterministically(
            machine in arb_policy_full(),
            project in arb_policy_full(),
            servers in proptest::collection::vec("[a-z_]{1,8}", 0..3),
        ) {
            let names: Vec<&str> = servers.iter().map(String::as_str).collect();
            let ruleset = compile(&machine, &project, &names);

            // Roundtrip.
            let json = serde_json::to_string(&ruleset).unwrap();
            let back: CompiledRuleset = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(&back, &ruleset);

            // Insertion-order independence: rebuild both policies with their
            // dimension maps reversed and recompile.
            let reverse = |p: &Policy| {
                let mut r = p.clone();
                r.tools = p.tools.iter().rev().map(|(k, v)| (k.clone(), v.clone())).collect();
                r.egress = p.egress.iter().rev().map(|(k, v)| (k.clone(), v.clone())).collect();
                r.secrets = p.secrets.iter().rev().map(|(k, v)| (k.clone(), v.clone())).collect();
                r
            };
            let mut names_rev: Vec<&str> = names.clone();
            names_rev.reverse();
            let recompiled = compile(&reverse(&machine), &reverse(&project), &names_rev);
            prop_assert_eq!(serde_json::to_string(&recompiled).unwrap(), json);
        }
    }

    // ── Workspace write decision (sandbox mount ro/rw) ──────────────────────

    /// Build a `Policy` with these `[policy.filesystem]` write scopes.
    fn fs_write(scopes: &[&str]) -> Policy {
        Policy {
            filesystem: agentstack_core::manifest::FsPolicy {
                read: vec![],
                write: scopes.iter().map(|s| s.to_string()).collect(),
            },
            ..Policy::default()
        }
    }

    /// The sandbox workspace mount is deny-by-default and grants rw only when
    /// a scope covers the workspace root — partial scopes round DOWN to ro.
    #[test]
    fn workspace_write_is_deny_by_default_and_needs_root_coverage() {
        let none = Policy::default();
        // No write scope anywhere → read-only, and the refusal says why.
        let err = compile(&none, &none, &[])
            .workspace_write_decision()
            .unwrap_err();
        assert!(err.contains("deny-by-default"), "{err}");

        // Each root-covering spelling grants rw, from either layer alone.
        for scope in ["./**", "./*", "*", ".", "./"] {
            assert!(
                compile(&fs_write(&[scope]), &none, &[])
                    .workspace_write_decision()
                    .is_ok(),
                "machine scope {scope:?} should grant the workspace"
            );
            assert!(
                compile(&none, &fs_write(&[scope]), &[])
                    .workspace_write_decision()
                    .is_ok(),
                "bundle scope {scope:?} should grant the workspace"
            );
        }

        // A partial scope constrains without covering the root → read-only.
        let err = compile(&fs_write(&["src/**"]), &none, &[])
            .workspace_write_decision()
            .unwrap_err();
        assert!(err.contains("[policy.filesystem]"), "{err}");
    }

    /// Rule 2 on the fs dimension: a bundle cannot widen the workspace mount
    /// past the machine layer. Machine grants only a subpath → ro, whatever
    /// the bundle asks for — and the refusal names the machine layer.
    #[test]
    fn bundle_cannot_widen_workspace_write_past_machine() {
        let machine = fs_write(&["src/**"]);
        let bundle = fs_write(&["./**"]);
        let err = compile(&machine, &bundle, &[])
            .workspace_write_decision()
            .unwrap_err();
        assert!(err.contains("machine policy"), "{err}");
    }

    proptest! {
        /// For ALL bundle write scopes: if the machine layer constrains
        /// writes without covering the workspace root, the mount stays
        /// read-only. (The fs restatement of effective(B, M) ⊆ M.)
        /// NEVER delete or weaken this test.
        #[test]
        fn workspace_write_never_more_permissive_than_machine(
            machine_scopes in proptest::collection::vec("[a-z./*]{0,8}", 1..4),
            bundle_scopes in proptest::collection::vec("[a-z./*]{0,8}", 0..4),
        ) {
            let machine = fs_write(&machine_scopes.iter().map(String::as_str).collect::<Vec<_>>());
            let bundle = fs_write(&bundle_scopes.iter().map(String::as_str).collect::<Vec<_>>());
            let machine_only = compile(&machine, &Policy::default(), &[]);
            let both = compile(&machine, &bundle, &[]);
            if machine_only.workspace_write_decision().is_err() {
                prop_assert!(
                    both.workspace_write_decision().is_err(),
                    "machine kept the workspace read-only but the bundle made it writable"
                );
            }
        }
    }

    /// Guard semantics pinned as plain unit checks: denies win, every
    /// allowlist bound applies (AND across lists), machine refusals name
    /// their layer, and an empty guard allows (uniform allow-by-default).
    #[test]
    fn guard_check_deny_wins_and_allow_bounds_and() {
        use crate::ruleset::{Guard, LayerRules};
        let guard = Guard {
            machine: LayerRules {
                deny: vec!["post_*".into()],
                allow_all_of: vec![vec!["get_*".into(), "list_*".into()]],
            },
            bundle: LayerRules {
                deny: vec![],
                allow_all_of: vec![vec!["*_file".into()]],
            },
        };
        // Passes every bound.
        assert!(guard.check("[policy.tools]", "get_file").is_ok());
        // Machine deny wins and names the layer.
        let err = guard.check("[policy.tools]", "post_file").unwrap_err();
        assert!(err.contains("machine policy"), "{err}");
        // Inside the machine bound, outside the bundle bound → bundle refuses.
        let err = guard.check("[policy.tools]", "get_node").unwrap_err();
        assert!(!err.contains("machine policy"), "{err}");
        // Outside the machine allowlist → machine refuses.
        let err = guard.check("[policy.tools]", "delete_file").unwrap_err();
        assert!(err.contains("machine policy"), "{err}");
        // Empty guard: allow-by-default.
        assert!(Guard::default().check("[policy.tools]", "anything").is_ok());
    }
}
