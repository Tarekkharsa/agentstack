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
    for (name, resolved) in &servers {
        let r = match resolved {
            Ok(r) => r,
            Err(e) => {
                println!("  {} {name}: unresolvable ({e})", "✗".red());
                continue;
            }
        };
        let origin = match r.origin {
            crate::resolve::ServerOrigin::Inline => String::new(),
            crate::resolve::ServerOrigin::Library => match lock.get_server(name) {
                Some(entry) if entry.checksum == r.checksum => "   [library, pinned]".to_string(),
                Some(_) => format!("   [library, {}]", "DRIFTED from lock".red()),
                None => format!(
                    "   [library, {}]",
                    "unpinned — run `agentstack lock`".yellow()
                ),
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
    if !m.skills.is_empty() {
        println!("  skills loadable over MCP: {}", m.skills.len());
    }

    let digest = trust::trust(base)?;
    println!(
        "\n{} trusted at {digest}.\nEditing the manifest or lockfile invalidates this — re-run `agentstack trust` after reviewing changes.\nWithdraw anytime with `agentstack trust --revoke`.",
        "✓".green()
    );
    Ok(())
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
