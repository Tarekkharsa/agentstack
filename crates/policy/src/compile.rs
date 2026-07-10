//! `compile` — collapse (machine ∩ bundle) into the [`CompiledRuleset`].
//!
//! Pure, no I/O: the caller loads the two `Policy` values
//! (`manifest::machine_policy()` + the project manifest) and passes the
//! trusted bundle's server names. The output is canonical (sorted keys,
//! sorted glob lists, empties dropped) so identical inputs always produce
//! identical bytes.

use std::collections::{BTreeMap, BTreeSet};

use agentstack_core::manifest::Policy;
use indexmap::IndexMap;

use crate::ruleset::{CompiledRuleset, FsRules, Guard, LayerRules, ServerRules, RULESET_VERSION};

/// Compile the two policy layers for a concrete server set.
///
/// The compiled server map covers `servers` (the trusted bundle's runtime
/// names) UNION every server either policy names. The union matters: a
/// machine rule for a server the bundle doesn't declare must still compile
/// into a named entry — otherwise a lookup for that name would fall to the
/// `any` bucket and see only the `"*"` rules, i.e. the compiled form would be
/// MORE permissive than the live two-layer check. (The behavior-preservation
/// property test in lib.rs pins this.)
pub fn compile(machine: &Policy, bundle: &Policy, servers: &[&str]) -> CompiledRuleset {
    let mut names: BTreeSet<String> = servers.iter().map(|s| s.to_string()).collect();
    for map in [
        &machine.tools,
        &bundle.tools,
        &machine.egress,
        &bundle.egress,
        &machine.secrets,
        &bundle.secrets,
    ] {
        for key in map.keys() {
            if key != "*" {
                names.insert(key.clone());
            }
        }
    }

    let mut out = BTreeMap::new();
    for name in &names {
        out.insert(name.clone(), server_rules(machine, bundle, name));
    }

    CompiledRuleset {
        version: RULESET_VERSION,
        defaults: Default::default(),
        any: server_rules(machine, bundle, "*"),
        servers: out,
        filesystem: FsRules {
            read: Guard {
                machine: fs_layer(&machine.filesystem.read),
                bundle: fs_layer(&bundle.filesystem.read),
            },
            write: Guard {
                machine: fs_layer(&machine.filesystem.write),
                bundle: fs_layer(&bundle.filesystem.write),
            },
        },
    }
}

fn server_rules(machine: &Policy, bundle: &Policy, name: &str) -> ServerRules {
    ServerRules {
        tools: Guard {
            machine: fold_layer(&machine.tools, name),
            bundle: fold_layer(&bundle.tools, name),
        },
        egress: Guard {
            machine: fold_layer(&machine.egress, name),
            bundle: fold_layer(&bundle.egress, name),
        },
        secrets: Guard {
            machine: fold_layer(&machine.secrets, name),
            bundle: fold_layer(&bundle.secrets, name),
        },
    }
}

/// Fold one layer's rules for one server name: denies from its named key and
/// its `"*"` key unioned (sorted, deduped); each key's non-empty allowlist
/// becomes an independent bound. `name == "*"` folds the wildcard key only —
/// the same key routing as `Policy::tool_allowed`.
fn fold_layer(map: &IndexMap<String, Vec<String>>, name: &str) -> LayerRules {
    let keys: &[&str] = if name == "*" { &["*"] } else { &[name, "*"] };
    let mut deny: BTreeSet<String> = BTreeSet::new();
    let mut allow_all_of: Vec<Vec<String>> = Vec::new();
    for key in keys {
        let Some(rules) = map.get(*key) else {
            continue;
        };
        let mut allows: Vec<String> = Vec::new();
        for r in rules {
            match r.strip_prefix('!') {
                Some(d) => {
                    deny.insert(d.to_string());
                }
                None => allows.push(r.clone()),
            }
        }
        if !allows.is_empty() {
            allows.sort();
            allows.dedup();
            allow_all_of.push(allows);
        }
    }
    allow_all_of.sort();
    LayerRules {
        deny: deny.into_iter().collect(),
        allow_all_of,
    }
}

/// Filesystem lists have no deny grammar in Phase 1 — the globs are carried
/// verbatim as a single allow bound (matching semantics land with Phase 2's
/// mount code).
fn fs_layer(globs: &[String]) -> LayerRules {
    if globs.is_empty() {
        return LayerRules::default();
    }
    let mut sorted = globs.to_vec();
    sorted.sort();
    sorted.dedup();
    LayerRules {
        deny: Vec::new(),
        allow_all_of: vec![sorted],
    }
}
