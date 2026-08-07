//! `agentstack session` — CLI control for ephemeral sessions, mirroring the
//! t3code's Start/End. A safety hatch: if t3code dies mid-session,
//! `agentstack x session end` (or `--all`) still reverts it.

use std::path::Path;

use agentstack_core::paint::OwoColorize;
use anyhow::Result;

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
    out.push_str(
        "  End it with `agentstack x session end` — every file above goes back exactly.\n",
    );
    // …and say why that is the ONLY command that ends it, but only when this
    // session actually materialized skills. The banner above no longer offers
    // `x restore` as an alternative; by here the report knows the names, so it
    // can say what would have gone wrong instead of just withholding a command.
    //
    // `crate::history::SKILLS_COME_OFF_WITH` does not fit: it names
    // `x uninstall --write` and "activate a toolset that omits them", which are
    // the MACHINE-EXIT ways off. The way off a session is `session end`, which
    // the line above already names. Only the reason sentence is shared — that
    // is the half that must never drift between Undo surfaces.
    if !report.skill_adds.is_empty() {
        out.push_str(&format!(
            "    · {}. So `agentstack x restore` is not the way back from a session; \
             `session end` is.\n",
            crate::history::SKILLS_ARE_NOT_RECORDED
        ));
    }
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

/// One row for the `session list` rendering.
struct SessionRow<'a> {
    dir: &'a str,
    profile: &'a str,
    scope: &'a str,
    /// When the session started, as the store recorded it. The text renders
    /// only the derived age; the JSON body ships both (see below).
    started_unix: u64,
    age_secs: u64,
    abandoned: bool,
}

/// The enveloped `session list --json` body — the same rows the text renders,
/// as named fields (contract `json-reads-v1`). `age_seconds` is derived from
/// `started_unix` and the read's own clock; both ship, because a UI polling
/// this wants the stable start time and a one-shot caller wants the age
/// without recomputing it. `abandoned` is the CLI's judgment
/// ([`crate::session::is_abandoned`]), not a threshold the caller re-invents.
///
/// This is READ-ONLY, and deliberately so: an abandoned session is exactly the
/// state a supervising UI died in, and listing it must never be the thing that
/// reverts it. `session end` is the verb that does that.
fn session_list_json(rows: &[SessionRow]) -> serde_json::Value {
    let sessions: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            // The session store records a directory and a toolset name that
            // originated in a manifest — repository content (rule 7). Sanitize
            // both before they reach a consumer's UI, exactly as
            // `use --list --json` does for its `session` object.
            serde_json::json!({
                "dir": crate::text::sanitize_line(r.dir),
                "profile": crate::text::sanitize_line(r.profile),
                "scope": r.scope,
                "started_unix": r.started_unix,
                "age_seconds": r.age_secs,
                "abandoned": r.abandoned,
            })
        })
        .collect();
    crate::ui_contract::envelope(serde_json::json!({ "sessions": sessions }))
}

/// Stage 2.2: `session list` names every active session with its age, flags
/// the ones that read as abandoned, and offers the exact safe recovery for
/// each — so a session an interrupted terminal or panel left behind is
/// visible and one command from being reverted. Pure (no color) so the shape
/// is unit-testable.
fn render_session_list(rows: &[SessionRow]) -> String {
    if rows.is_empty() {
        return "No active sessions.\n".to_string();
    }
    let mut out = String::from("Active sessions:\n");
    for r in rows {
        let flag = if r.abandoned {
            " · looks abandoned"
        } else {
            ""
        };
        out.push_str(&format!(
            "  '{}' ({}) · {}{flag}\n      {}\n",
            r.profile,
            r.scope,
            crate::commands::overview::session_age(r.age_secs),
            r.dir,
        ));
        if r.abandoned {
            out.push_str(
                "      recover: run `agentstack x session end` in that project (or `session end --all`) — it restores your files\n",
            );
        }
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
            // Moment 5: the way back is named BEFORE the first byte changes,
            // not only in the report afterwards. `session start` writes
            // immediately and has no preview step — that is the point of a
            // temporary activation — so the banner is the honest form of the
            // rule here. The report below still repeats it, because by then it
            // can name the exact files.
            //
            // `agentstack x restore --last --write` used to be offered here as
            // an equal alternative, and it is not one. `session::start` records
            // ONLY the server-config snapshots in the history ledger; the
            // skills it materializes are tracked in the session store and
            // removed by `session end` through its own mechanism (G31: the
            // ledger holds a file's bytes, and a delivered skill is a linked
            // directory). So that restore replays the file edits and leaves the
            // skills on disk — and a session that materialized skills and
            // nothing else records no ledger entry at all, so it fails outright
            // with "nothing to undo". `session end` is the whole revert, which
            // is why it is now the only command this banner names.
            println!(
                "  {} temporary: `agentstack x session end` puts every file back — and takes off any skills this activates.",
                "↩".dimmed()
            );
            let report = crate::session::start(dir, profile, scope)?;
            let root = crate::commands::project_base(dir)?;
            print!("{}", render_start_report(&report, &root));
        }
        SessionCmd::End { all } => {
            if *all {
                let n = crate::session::end_all()?;
                println!(
                    "{} ended {} — reverted",
                    "✓".green(),
                    super::count(n, "session")
                );
            } else {
                let report = crate::session::end(dir)?;
                let root = crate::commands::project_base(dir)?;
                print!("{}", render_end_report(&report, &root));
            }
        }
        SessionCmd::Freeze { name } => {
            let created = crate::session::freeze(dir, name.as_deref())?;
            println!(
                "{} froze the session into toolset '{}' — replay it deterministically with `agentstack x session start {}`",
                "✓".green(),
                created.bold(),
                created
            );
        }
        SessionCmd::List { json } => {
            let list = crate::session::list_all();
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let rows: Vec<SessionRow> = list
                .iter()
                .map(|s| SessionRow {
                    dir: &s.dir,
                    profile: &s.profile,
                    scope: &s.scope,
                    started_unix: s.started_unix,
                    age_secs: now.saturating_sub(s.started_unix),
                    abandoned: s.is_abandoned(now),
                })
                .collect();
            if *json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&session_list_json(&rows))?
                );
            } else {
                print!("{}", render_session_list(&rows));
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
        assert!(out.contains("agentstack x session end"));

        // This session materialized skills, so the report says why `session
        // end` is the ONLY command that reverts it — the shared sentence, not a
        // paraphrase, so this surface cannot drift from the other Undo doors.
        assert!(out.contains(crate::history::SKILLS_ARE_NOT_RECORDED));
        assert!(out.contains("is not the way back from a session"));

        // NEGATIVE CONTROL, on the pure function: same report, same closing
        // line, no materialized skills — so there is nothing the promise
        // overstates and the bound is not printed. A caveat on every `session
        // start` is read by no one. (The behaviour behind both branches is
        // pinned by tests/session_promises_only_the_undo_it_has.rs, which runs
        // the restore that used to be offered and looks at the disk.)
        let no_skills = crate::session::StartReport {
            skill_adds: Vec::new(),
            ..report
        };
        let out = render_start_report(&no_skills, Path::new("/repo"));
        assert!(out.contains("agentstack x session end"));
        assert!(!out.contains(crate::history::SKILLS_ARE_NOT_RECORDED));
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

    /// Stage 2.2: `session list` names each session with its age, flags the
    /// abandoned ones, and offers the safe recovery only for those.
    #[test]
    fn session_list_flags_abandoned_and_offers_recovery() {
        assert_eq!(render_session_list(&[]), "No active sessions.\n");

        let rows = sample_rows();
        let out = render_session_list(&rows);
        assert!(out.contains("'dev' (project) · started 4m ago"));
        assert!(out.contains("/repo/a"));
        assert!(out.contains("'ops' (project) · started 14h 0m ago · looks abandoned"));
        // Recovery is offered for the abandoned session only.
        assert!(out.contains("recover: run `agentstack x session end`"));
        let recover_lines = out.matches("recover:").count();
        assert_eq!(recover_lines, 1, "only the abandoned row offers recovery");
    }

    /// One live session and one that reads as abandoned — the two shapes both
    /// renderings have to handle, built once so the text and JSON witnesses
    /// are demonstrably reading the same rows.
    fn sample_rows() -> [SessionRow<'static>; 2] {
        [
            SessionRow {
                dir: "/repo/a",
                profile: "dev",
                scope: "project",
                started_unix: 1_700_000_000,
                age_secs: 240,
                abandoned: false,
            },
            SessionRow {
                dir: "/repo/b",
                profile: "ops",
                scope: "project",
                started_unix: 1_699_000_000,
                age_secs: 14 * 3600,
                abandoned: true,
            },
        ]
    }

    /// `json-reads-v1`: `session list --json` carries every row the text
    /// renders, in named fields — including the `abandoned` judgment, which is
    /// the whole reason a supervising UI polls this listing.
    #[test]
    fn session_list_json_names_every_row_the_text_renders() {
        let empty = session_list_json(&[]);
        assert_eq!(empty["schema_version"], crate::ui_contract::SCHEMA_VERSION);
        assert_eq!(empty["sessions"].as_array().unwrap().len(), 0);

        let out = session_list_json(&sample_rows());
        let sessions = out["sessions"].as_array().unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0]["profile"], "dev");
        assert_eq!(sessions[0]["dir"], "/repo/a");
        assert_eq!(sessions[0]["scope"], "project");
        assert_eq!(sessions[0]["started_unix"], 1_700_000_000u64);
        assert_eq!(sessions[0]["age_seconds"], 240);
        assert_eq!(sessions[0]["abandoned"], false);
        assert_eq!(sessions[1]["abandoned"], true);
        // No ANSI, no padding, no prose: the text listing's recovery sentence
        // belongs to the human screen, not to a machine consumer that already
        // knows `abandoned`.
        let raw = out.to_string();
        assert!(!raw.contains('\u{1b}'), "no escape sequences: {raw}");
        assert!(!raw.contains("recover"), "no human prose: {raw}");
    }

    /// Rule 7: a toolset name and directory reach this listing from a
    /// manifest, so an escape sequence smuggled through one must not survive
    /// into a consumer's UI.
    #[test]
    fn session_list_json_sanitizes_store_supplied_strings() {
        let rows = [SessionRow {
            dir: "/repo/\u{1b}[31mred",
            profile: "dev\u{1b}]0;title\u{7}",
            scope: "project",
            started_unix: 1,
            age_secs: 1,
            abandoned: false,
        }];
        let out = session_list_json(&rows);
        let raw = out.to_string();
        assert!(!raw.contains('\u{1b}'), "escapes stripped: {raw}");
        assert_eq!(out["sessions"][0]["profile"], "dev");
    }
}
