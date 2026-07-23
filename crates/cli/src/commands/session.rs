//! `agentstack session` — CLI control for ephemeral sessions, mirroring the
//! t3code's Start/End. A safety hatch: if t3code dies mid-session,
//! `agentstack session end` (or `--all`) still reverts it.

use std::path::Path;

use anyhow::Result;
use owo_colors::OwoColorize;

use crate::cli::{SessionArgs, SessionCmd};
use crate::scope::Scope;

/// Stage 2.2: `session start` states the facts, not just "started" — which
/// profile, which native files it now manages (the exact set `end` restores),
/// which skills it materialized where, and the one command that reverts it.
/// Pure (no color), so the shape is unit-testable.
fn render_start_report(
    report: &crate::session::StartReport,
    project_root: &std::path::Path,
) -> String {
    let scope = match report.scope {
        Scope::Project => "this project",
        Scope::Global => "machine-wide",
    };
    let mut out = String::new();
    out.push_str(&format!(
        "✓ session '{}' started ({scope})\n",
        report.profile
    ));
    for (display, path) in &report.server_files {
        out.push_str(&format!(
            "    {display} · servers → {}\n",
            super::init::display_path(path, project_root)
        ));
    }
    for (dir, names) in &report.skill_adds {
        out.push_str(&format!(
            "    skills → {}: {}\n",
            super::init::display_path(std::path::Path::new(dir), project_root),
            names.join(", ")
        ));
    }
    out.push_str("  End it with `agentstack session end` — every file above goes back exactly.\n");
    out
}

/// Stage 2.2: `session end` reports exactly what it restored — the files put
/// back to their pre-session bytes and the skills removed — never a bare
/// "ended". Pure, for the same testability.
fn render_end_report(report: &crate::session::EndReport, project_root: &std::path::Path) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "✓ session '{}' ended — your tools are back to before\n",
        report.profile
    ));
    for (path, label) in &report.restored {
        out.push_str(&format!(
            "    restored {}  ({label})\n",
            super::init::display_path(std::path::Path::new(path), project_root)
        ));
    }
    for (dir, names) in &report.removed_skills {
        out.push_str(&format!(
            "    removed skills from {}: {}\n",
            super::init::display_path(std::path::Path::new(dir), project_root),
            names.join(", ")
        ));
    }
    if report.restored.is_empty() && report.removed_skills.is_empty() {
        out.push_str("    nothing to revert — no native file changed during this session\n");
    }
    out
}

pub fn run(args: &SessionArgs, dir: Option<&Path>) -> Result<()> {
    match &args.cmd {
        SessionCmd::Start { profile, scope } => {
            let scope = match scope {
                Some(scope) => *scope,
                None => {
                    let ctx = crate::commands::load(dir)?;
                    Scope::default_for(&ctx.dir)
                }
            };
            let report = crate::session::start(dir, profile, scope)?;
            let root = crate::commands::project_base(dir)?;
            print!("{}", render_start_report(&report, &root));
        }
        SessionCmd::End { all } => {
            if *all {
                let n = crate::session::end_all()?;
                println!("{} ended {n} session(s) — reverted", "✓".green());
            } else {
                let report = crate::session::end(dir)?;
                let root = crate::commands::project_base(dir)?;
                print!("{}", render_end_report(&report, &root));
            }
        }
        SessionCmd::Freeze { name } => {
            let created = crate::session::freeze(dir, name.as_deref())?;
            println!(
                "{} froze the session into profile '{}' — replay it deterministically with `agentstack session start {}`",
                "✓".green(),
                created.bold(),
                created
            );
        }
        SessionCmd::List => {
            let list = crate::session::list_all();
            if list.is_empty() {
                println!("No active sessions.");
            } else {
                for s in list {
                    println!("{}  profile={} scope={}", s.dir.bold(), s.profile, s.scope,);
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    /// Stage 2.2: `session start` states which profile and which native files
    /// it activates — the exact set `end` restores — plus the one command
    /// that reverts it. Never a bare "started".
    #[test]
    fn start_report_names_profile_files_skills_and_the_end_command() {
        let report = crate::session::StartReport {
            profile: "dev".into(),
            scope: Scope::Project,
            server_files: vec![
                ("Claude Code".into(), PathBuf::from("/repo/.mcp.json")),
                (
                    "Codex CLI".into(),
                    PathBuf::from("/repo/.codex/config.toml"),
                ),
            ],
            skill_adds: vec![("/repo/.claude/skills".into(), vec!["helper".into()])],
        };
        let out = render_start_report(&report, Path::new("/repo"));
        assert!(out.contains("session 'dev' started (this project)"));
        assert!(out.contains("Claude Code · servers → .mcp.json"));
        assert!(out.contains("Codex CLI · servers → .codex/config.toml"));
        assert!(out.contains("skills → .claude/skills: helper"));
        assert!(out.contains("agentstack session end"));
    }

    /// Stage 2.2: `session end` reports exactly what it restored, and an
    /// end that had nothing to revert says so instead of implying a restore.
    #[test]
    fn end_report_lists_restored_files_and_removed_skills() {
        let report = crate::session::EndReport {
            profile: "dev".into(),
            restored: vec![("/repo/.mcp.json".into(), "Claude Code · servers".into())],
            removed_skills: vec![("/repo/.claude/skills".into(), vec!["helper".into()])],
        };
        let out = render_end_report(&report, Path::new("/repo"));
        assert!(out.contains("session 'dev' ended"));
        assert!(out.contains("restored .mcp.json  (Claude Code · servers)"));
        assert!(out.contains("removed skills from .claude/skills: helper"));
        assert!(!out.contains("nothing to revert"));

        let empty = crate::session::EndReport {
            profile: "dev".into(),
            restored: Vec::new(),
            removed_skills: Vec::new(),
        };
        let out = render_end_report(&empty, Path::new("/repo"));
        assert!(out.contains("nothing to revert"));
    }
}
