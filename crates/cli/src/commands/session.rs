//! `agentstack session` — CLI control for ephemeral sessions, mirroring the
//! t3code's Start/End. A safety hatch: if t3code dies mid-session,
//! `agentstack session end` (or `--all`) still reverts it.

use std::path::Path;

use anyhow::Result;
use owo_colors::OwoColorize;

use crate::cli::{SessionArgs, SessionCmd};
use crate::scope::Scope;

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
            crate::session::start(dir, profile, scope)?;
            println!(
                "{} session '{}' started ({scope})",
                "✓".green(),
                profile.bold(),
            );
        }
        SessionCmd::End { all } => {
            if *all {
                let n = crate::session::end_all()?;
                println!("{} ended {n} session(s) — reverted", "✓".green());
            } else {
                crate::session::end(dir)?;
                println!(
                    "{} session ended — your tools are back to before",
                    "✓".green()
                );
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
