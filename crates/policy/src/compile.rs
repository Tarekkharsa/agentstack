//! `compile` — collapse (machine ∩ bundle) into the [`CompiledRuleset`].
//!
//! Pure and canonical (sorted keys, sorted glob lists, empties dropped) so
//! identical inputs produce identical bytes — with ONE deliberate ambient
//! read: [`fs_deny_layer`] expands home-anchored `[policy.filesystem] deny`
//! globs (`~`, `$HOME`) against this process's `$HOME`. That is intended, not
//! a determinism hazard: the compiled ruleset is already a machine-specific
//! enforcement artifact (it bakes in *this* machine's policy), never a pinned
//! or digested one, and `~` in a deny glob means "this machine's home". The
//! subject side is expanded the same way in the CLI's guard hook, so the two
//! must agree — see `cli::guard::normalize`. The caller still supplies the two
//! `Policy` values (the CLI's fail-closed machine-policy provider + the
//! project manifest) and the trusted bundle's server names.

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
            deny: Guard {
                machine: fs_deny_layer(&machine.filesystem.deny),
                bundle: fs_deny_layer(&bundle.filesystem.deny),
            },
        },
        // Derived from resolved server URLs, not from policy + names, so the
        // CLI populates it post-compile for lockdown runs (D4). Empty here.
        gateway_only_hosts: Default::default(),
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

/// Filesystem read/write lists have no deny grammar — the globs are carried
/// verbatim as a single allow bound. The matching semantics live in
/// `CompiledRuleset::workspace_write_decision`, which relies on the no-deny
/// property here (see its doc comment before adding `!` support). The
/// blocklist dimension is `[policy.filesystem] deny`, compiled separately by
/// [`fs_deny_layer`] — never folded into these guards.
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

/// `[policy.filesystem] deny` is the inverse shape: every glob is a deny
/// entry, there are no allow bounds. With denies in both layers unioned at
/// check time, the effective blocklist is machine ∪ bundle — a bundle can
/// add denies, never subtract (deny is monotonic, CLAUDE.md rule 2).
///
/// Home-anchored globs (`~/.aws/credentials`, `~/.ssh/**`) are expanded to
/// absolute patterns HERE so the compiled ruleset carries them absolute and
/// every enforcer (the guard hook, the sandbox) blocks them uniformly. The
/// matcher (`glob_match`) is `*`-only with no `~` awareness, and the guard
/// already expands `~` on the *subject* side; leaving `~` verbatim on the
/// *pattern* side would compare the literal two chars `~/…` against absolute
/// spellings and match nothing — a deny that reads as protective but grants
/// zero protection. Expanding both sides against the same `$HOME` closes that.
fn fs_deny_layer(globs: &[String]) -> LayerRules {
    // Read $HOME once. `std::env::var_os` (bytes, not UTF-8) mirrors what the
    // guard's `dirs::home_dir()` resolves to on Unix; `policy` may only depend
    // on `core`, so the `dirs` crate is off-limits and we expand by hand.
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .filter(|h| h.is_absolute());
    let mut expanded: Vec<String> = globs
        .iter()
        .map(|g| expand_home(g, home.as_deref()))
        .collect();
    expanded.sort();
    expanded.dedup();
    LayerRules {
        deny: expanded,
        allow_all_of: Vec::new(),
    }
}

/// Expand a leading `~` / `$HOME` / `${HOME}` in a deny glob against `home`,
/// mirroring `cli::guard::normalize` on the subject side so pattern and
/// subject meet at the same absolute path. Only the anchor is rewritten; the
/// remaining `*` glob syntax is left untouched for `glob_match`. When `$HOME`
/// is absent or non-absolute the glob is returned verbatim — no worse than the
/// pre-expansion behavior for that one degenerate case, and the common case
/// (HOME set) is fixed.
fn expand_home(glob: &str, home: Option<&std::path::Path>) -> String {
    let Some(home) = home else {
        return glob.to_string();
    };
    let rest = if glob == "~" || glob == "$HOME" || glob == "${HOME}" {
        Some("")
    } else {
        glob.strip_prefix("~/")
            .or_else(|| glob.strip_prefix("$HOME/"))
            .or_else(|| glob.strip_prefix("${HOME}/"))
    };
    match rest {
        // `home` came from the OS as bytes; `to_string_lossy` keeps a valid
        // pattern even for the vanishingly rare non-UTF-8 home path.
        Some("") => home.to_string_lossy().into_owned(),
        Some(rest) => home.join(rest).to_string_lossy().into_owned(),
        None => glob.to_string(),
    }
}
