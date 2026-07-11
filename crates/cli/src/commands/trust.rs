//! `agentstack trust` — the human gate for the zero-files bridge.
//!
//! `connect` registers one global gateway per harness; `mcp --auto-project`
//! then discovers whatever manifest the current repo carries. This command is
//! what stands between "cloned a repo" and "that repo's manifest spawns stdio
//! servers and receives secrets": trust is granted per project, pinned to the
//! manifest's content digest, and shown to the human as the list of things the
//! manifest would actually run.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use owo_colors::OwoColorize;

use crate::cli::TrustArgs;
use crate::manifest::ServerType;
use crate::trust::{self, TrustState, TrustStore};

pub fn run(args: &TrustArgs) -> Result<()> {
    if args.list {
        return list();
    }
    let base = resolve_base(args.path.as_deref())?;
    if args.revoke {
        return revoke(&base);
    }
    grant(&base)
}

/// Resolve the project base to act on: walk up from the given path (or cwd) so
/// `agentstack trust` works from a subdirectory too.
fn resolve_base(path: Option<&Path>) -> Result<PathBuf> {
    let start = match path {
        Some(p) => p
            .canonicalize()
            .with_context(|| format!("no such directory: {}", p.display()))?,
        None => std::env::current_dir()?,
    };
    crate::manifest::discover_project_base(&start).with_context(|| {
        format!(
            "no agentstack manifest at or above {} — run `agentstack init` first",
            start.display()
        )
    })
}

fn grant(base: &Path) -> Result<()> {
    let dir = crate::manifest::resolve_manifest_dir(base);
    let loaded = crate::manifest::load_from_dir(&dir)?;
    let m = &loaded.manifest;

    println!(
        "Trusting {} for the zero-files bridge.\n",
        base.display().to_string().bold()
    );
    // Preview the gateway's actual runtime surface, not just the inline
    // `[servers.*]` tables: library name refs resolve here exactly like they
    // will at gateway time, so the human reviews everything auto-mode may run.
    let library = crate::library::Library::load_default_or_warn();
    let lib_home = crate::util::paths::lib_home();
    // A broken lockfile must fail the trust review loudly: its pins are part
    // of what the human is consenting to, and the gateway will refuse
    // library-backed servers under an unreadable lock anyway.
    let lock = crate::lock::Lock::load(&dir)?;
    let servers = crate::resolve::effective_runtime_servers(m, &library, &lib_home, None);
    println!("This project declares — review what auto-mode may run/contact:");
    if servers.is_empty() {
        println!("  (no servers)");
    }
    // Trusting pins the lock bytes into the trust digest, so trusting over a
    // drifted or unpinned surface would bless pins that don't match content
    // (or bless no pin at all). Everything that must be lock-verified at use
    // time therefore has to be pinned and matching BEFORE trust is granted:
    // `agentstack lock` is a prerequisite of `agentstack trust`.
    let mut blockers: Vec<(String, String)> = Vec::new();
    for (name, resolved) in &servers {
        let r = match resolved {
            Ok(r) => r,
            Err(e) => {
                println!("  {} {name}: unresolvable ({e})", "✗".red());
                blockers.push((name.clone(), format!("broken server ref — {e}")));
                continue;
            }
        };
        let origin = match r.origin {
            crate::resolve::ServerOrigin::Inline => String::new(),
            crate::resolve::ServerOrigin::Library => match lock.get_server(name) {
                Some(entry) if entry.checksum == r.checksum => "   [library, pinned]".to_string(),
                Some(_) => {
                    blockers.push((
                        name.clone(),
                        "library server definition DRIFTED from lock".to_string(),
                    ));
                    format!("   [library, {}]", "DRIFTED from lock".red())
                }
                None => {
                    blockers.push((
                        name.clone(),
                        "library server unpinned — run `agentstack lock`".to_string(),
                    ));
                    format!("   [library, {}]", "unpinned".red())
                }
            },
        };
        match r.server.server_type {
            // A stdio server is arbitrary local code execution — the thing the
            // trust gate exists for. Call it out explicitly.
            ServerType::Stdio => println!(
                "  {} {name}: runs `{} {}`{origin}",
                "▶".yellow(),
                r.server.command.as_deref().unwrap_or("?"),
                r.server.args.join(" ")
            ),
            ServerType::Http => println!(
                "  {} {name}: contacts {}{origin}",
                "→".cyan(),
                r.server.url.as_deref().unwrap_or("?")
            ),
        }
    }
    let refs = m.referenced_secrets();
    if !refs.is_empty() {
        println!("  secrets referenced: {}", refs.join(", "));
    }

    // Skills, reviewed like servers: name + origin + pin status. Their bodies
    // are exactly the bytes the trust digest does NOT cover, so the pin is
    // the only thing binding what the human reviews to what gets served.
    let skill_names = review_skill_names(m);
    if !skill_names.is_empty() {
        println!("  skills loadable over MCP:");
        let store = crate::store::Store::default_store();
        for name in &skill_names {
            let report = crate::resolve::skill_lock_status(
                name,
                m,
                &dir,
                &library,
                &lib_home,
                &store,
                &lock,
                crate::resolve::ResolveMode::NoFetch,
            );
            use crate::resolve::{SkillLockStatus, SkillOrigin};
            let origin_word = match report.origin {
                Some(SkillOrigin::Inline) => "inline",
                Some(SkillOrigin::Library) => "library",
                None => "?",
            };
            match &report.status {
                SkillLockStatus::Matches => {
                    println!("  · {name}   [{origin_word}, pinned]");
                }
                SkillLockStatus::ChecksumDrift { .. } | SkillLockStatus::RevDrift { .. } => {
                    println!(
                        "  {} {name}   [{origin_word}, {}]",
                        "✗".red(),
                        "DRIFTED from lock".red()
                    );
                    blockers.push((name.clone(), "skill content drifted from lock".to_string()));
                }
                SkillLockStatus::MissingLockEntry => match report.origin {
                    // An inline skill's bytes live in the repo under review —
                    // unpinned means trusting would leave them ungoverned.
                    Some(SkillOrigin::Inline) => {
                        println!("  {} {name}   [inline, {}]", "✗".red(), "unpinned".red());
                        blockers.push((
                            name.clone(),
                            "inline skill unpinned — run `agentstack lock`".to_string(),
                        ));
                    }
                    // A library skill's bytes are the user's own curated,
                    // scan-gated content — worth pinning, not worth blocking.
                    _ => println!(
                        "  · {name}   [{origin_word}, {}]",
                        "unpinned — run `agentstack lock`".yellow()
                    ),
                },
                // Reproducibility can't be checked offline; not a blocker.
                SkillLockStatus::NotAvailableOffline { .. } => println!(
                    "  · {name}   [{origin_word}, {}]",
                    "offline — pin unverified".yellow()
                ),
                SkillLockStatus::ResolveFailed { error } => {
                    println!("  {} {name}: broken ref ({error})", "✗".red());
                    blockers.push((name.clone(), format!("broken ref — {error}")));
                }
            }
        }
    }

    // Instruction fragments, same review: they compile into CLAUDE.md /
    // AGENTS.md — straight into agent context — and their bytes are repo
    // content the trust digest doesn't cover. The pin is what binds them.
    // (grant loads the project manifest only, so machine-layer fragments
    // can't appear here; the filter guards the invariant regardless.)
    let instructions: Vec<_> = m
        .instructions
        .iter()
        .filter(|(_, i)| !i.from_user_layer)
        .collect();
    if !instructions.is_empty() {
        println!("  instruction fragments (compile into CLAUDE.md / AGENTS.md):");
        for (name, instr) in instructions {
            use crate::resolve::InstructionLockStatus;
            match crate::resolve::instruction_lock_status(name, instr, &dir, &lock) {
                InstructionLockStatus::Matches => println!("  · {name}   [pinned]"),
                InstructionLockStatus::ChecksumDrift { .. } => {
                    println!("  {} {name}   [{}]", "✗".red(), "DRIFTED from lock".red());
                    blockers.push((
                        name.clone(),
                        "instruction content drifted from lock".to_string(),
                    ));
                }
                InstructionLockStatus::MissingLockEntry => {
                    println!("  {} {name}   [{}]", "✗".red(), "unpinned".red());
                    blockers.push((
                        name.clone(),
                        "instruction unpinned — run `agentstack lock`".to_string(),
                    ));
                }
                InstructionLockStatus::ResolveFailed { error } => {
                    println!("  {} {name}: broken ref ({error})", "✗".red());
                    blockers.push((name.clone(), format!("broken ref — {error}")));
                }
            }
        }
    }

    // Requested policy, shown at the trust boundary (ARCHITECTURE: "review
    // shows … policy changes"). Display-only: a bundle's policy can only
    // narrow — the machine layer caps everything at runtime regardless — so
    // there is nothing here to block on, but the human should see what the
    // repo asks for before blessing it.
    review_policy(&m.policy);

    if !blockers.is_empty() {
        let width = blockers.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
        let lines: Vec<String> = blockers
            .iter()
            .map(|(name, why)| format!("  {name:width$}  {why}"))
            .collect();
        anyhow::bail!(
            "cannot trust {}: its loadable surface isn't fully pinned — {} item(s) need locking or review:\n{}\nRun `agentstack lock`, review the result, then `agentstack trust` again.",
            base.display(),
            blockers.len(),
            lines.join("\n")
        );
    }

    let digest = trust::trust(base)?;
    println!(
        "\n{} trusted at {digest}.\nEditing the manifest or lockfile invalidates this — re-run `agentstack trust` after reviewing changes.\nPinned skill/server content that drifts is blocked at use time until re-locked.\nWithdraw anytime with `agentstack trust --revoke`.",
        "✓".green()
    );
    Ok(())
}

/// Print what the project's `[policy]` requests, per dimension. Bundles can
/// only narrow, so this is review signal, not a gate. Filesystem scopes are
/// labelled honestly: the write scope decides the sandbox workspace mount
/// (ro unless covered); read scopes are informational, and host mode
/// enforces neither.
fn review_policy(p: &crate::manifest::Policy) {
    if p.tools.is_empty() && p.egress.is_empty() && p.secrets.is_empty() && p.filesystem.is_empty()
    {
        return;
    }
    println!("  policy requested by this project (can only narrow the machine layer):");
    let dims: [(&str, &indexmap::IndexMap<String, Vec<String>>); 3] = [
        ("tools", &p.tools),
        ("egress", &p.egress),
        ("secrets", &p.secrets),
    ];
    for (label, map) in dims {
        for (server, rules) in map {
            println!("  · {label:<7} {server}: {}", rules.join(", "));
        }
    }
    if !p.filesystem.read.is_empty() {
        println!(
            "  · filesystem read {} (informational — the sandbox mounts one whole workspace)",
            p.filesystem.read.join(", ")
        );
    }
    if !p.filesystem.write.is_empty() {
        println!(
            "  · filesystem write {} (sandbox mode mounts the workspace read-only unless this covers it; advisory in host mode)",
            p.filesystem.write.join(", ")
        );
    }
}

/// The skill names a trust review covers: the manifest's inline `[skills.*]`
/// plus every profile-referenced name (which may resolve to the central
/// library), deduped in first-seen order. The `"*"` wildcard expands to inline
/// skills only — the same rule as activation — so it adds nothing new here.
fn review_skill_names(m: &crate::manifest::Manifest) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let push = |n: &str, names: &mut Vec<String>| {
        if n != "*" && !names.iter().any(|x| x == n) {
            names.push(n.to_string());
        }
    };
    for n in m.skills.keys() {
        push(n, &mut names);
    }
    for p in m.profiles.values() {
        for n in &p.skills {
            push(n, &mut names);
        }
    }
    names
}

fn revoke(base: &Path) -> Result<()> {
    if trust::revoke(base)? {
        println!(
            "{} trust revoked for {} — auto-mode is control-plane only there now.",
            "✓".green(),
            base.display()
        );
    } else {
        println!("{} was not trusted; nothing to revoke.", base.display());
    }
    Ok(())
}

fn list() -> Result<()> {
    let store = TrustStore::load();
    if store.trusted.is_empty() {
        println!("No trusted projects. Grant one with `agentstack trust <dir>`.");
        return Ok(());
    }
    for (path, entry) in &store.trusted {
        let state = trust::check(Path::new(path));
        let (mark, note) = match state {
            TrustState::Trusted => ("✓".green().to_string(), "current".to_string()),
            TrustState::Changed => (
                "⚠".yellow().to_string(),
                "manifest or lockfile changed since trusted — re-run `agentstack trust` there"
                    .to_string(),
            ),
            // An entry exists, so Untrusted can't come back here; kept for
            // completeness.
            TrustState::Untrusted => ("⚠".yellow().to_string(), "stale entry".to_string()),
        };
        println!("  {mark} {path} · {} · {note}", entry.digest);
    }
    Ok(())
}
