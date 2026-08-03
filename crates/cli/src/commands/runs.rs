//! `agentstack run` / `runs` / `kill` — the CLI layer over [`crate::runs`].
//! Launching is a foreground, terminal-attached act; listing and killing also
//! work from external supervisors.

use std::path::Path;

use anyhow::Result;
use owo_colors::OwoColorize;

use crate::cli::{KillArgs, RunArgs, RunsArgs};

/// `run` is protected by default.
///
/// The fail-closed gate that `--locked` used to opt IN to — content trust,
/// strict lock verification, policy admission, a frozen grant — is what a plain
/// `agentstack run <cli>` does now. Three ways out, all explicit:
///
/// - `--unprotected` — the plain host run, `HOST / ADVISORY`, no gate;
/// - `--sandbox` / `--lockdown` — the isolation opt-ins, unchanged, each with
///   its own honest posture label;
/// - `--locked` — still accepted, still means the default, still owns its own
///   combination rules.
///
/// Order matters and is deliberate: every explicitly-flagged invocation routes
/// exactly where it routed before this flip, so nothing an existing script
/// types behaves differently. Only the *unflagged* run moved, and it moved in
/// the fail-closed direction.
pub fn run(args: &RunArgs, dir: Option<&Path>) -> Result<()> {
    // A run cannot both be and not be protected. Refuse rather than let flag
    // order decide which one the user meant.
    if args.locked && args.unprotected {
        anyhow::bail!(
            "--locked and --unprotected contradict each other — nothing was launched\n\
             \n  \
             protected (the default):  agentstack run {h}\n  \
             opt out of the gate:      agentstack run {h} --unprotected",
            h = args.harness
        );
    }
    // `--prompt` exists only as the governed child-run primitive: headless
    // delivery is defined by the locked contract (grant-committed argv,
    // bounded output evidence). Anywhere else it would silently skip every
    // gate the flag's semantics promise — refuse loudly and name the valid
    // form instead.
    if args.prompt.is_some() && (args.unprotected || args.sandbox || args.lockdown) {
        anyhow::bail!(
            "--prompt needs the protected run — nothing was launched\n\
             \n  \
             governed headless run:  agentstack run {h} --prompt \"<text>\"",
            h = args.harness
        );
    }
    // --locked is now a request for the default, but it keeps its own branch so
    // that an invocation naming it explicitly still reaches the identical
    // refusals it always did (--locked --sandbox is a named not-yet limitation
    // there, not a silent fall-through into the container path).
    if args.locked {
        return crate::commands::locked::run_locked(dir, args);
    }
    // --lockdown is the stronger sandbox mode; it implies --sandbox. The
    // isolation opt-ins are checked BEFORE the protected default so that
    // `run --sandbox` still means what it has always meant.
    if args.sandbox || args.lockdown {
        return crate::commands::sandbox::run_sandboxed(dir, args);
    }
    if !args.unprotected {
        // The default: the Protected tier, gates first.
        return crate::commands::locked::run_locked(dir, args);
    }
    // ---- `--unprotected`: the explicit opt-out, unchanged from the old
    // default host run. -------------------------------------------------
    //
    // `--plan` promises "print the plan, run NOTHING" — it is only defined for
    // the protected and sandbox flows above. Bare `run --plan` used to fall
    // through and launch the CLI anyway (audit finding: an unintended launch
    // during a read-only review); an unprotected run has no plan to print, so
    // refuse and name the two real forms.
    if args.plan {
        anyhow::bail!(
            "--plan needs a gated run mode — nothing was launched\n\
             \n  \
             protected host plan:  agentstack run --plan {h}\n  \
             sandbox plan:         agentstack run --sandbox --plan {h}",
            h = args.harness
        );
    }
    // Validate BEFORE the banner: a missing manifest, unknown id, or absent
    // binary must be the first (and only) thing the user reads — never below a
    // "▶ launching…" line claiming something started.
    let plan = crate::runs::prepare(dir, &args.harness)?;
    let scope = args.scope.unwrap_or_else(|| plan.default_scope());
    if let Some(p) = &args.profile {
        println!(
            "{} launching {} with toolset '{}' ({})…",
            "▶".green(),
            args.harness.bold(),
            p,
            scope
        );
    } else {
        println!("{} launching {}…", "▶".green(), args.harness.bold());
    }
    // Host mode has no container: name the posture and say — once, honestly, in
    // the same style as the sandbox "unreviewed bundle" warning — that policy is
    // advisory here. The gateway still brokers MCP tool calls, but nothing
    // confines this process's own egress or filesystem; `--sandbox`/`--lockdown`
    // are what enforce those at runtime. The posture label is unchanged: this
    // is the same run it always was, only reached by an explicit flag now.
    use crate::commands::sandbox::Posture;
    println!("  posture: {}", Posture::Host.to_string().yellow().bold());
    eprintln!(
        "  {} --unprotected: the pre-launch gate is OFF for this run — content trust, \
         strict lock verification, and policy admission were not checked, and no grant \
         was frozen. Policy is advisory here: the gateway brokers MCP tool calls, but \
         this process's own egress and filesystem are not confined. Drop the flag for \
         the protected default; use `--sandbox` or `--lockdown` to enforce confinement \
         at runtime.",
        "⚠".yellow()
    );
    crate::runs::launch(
        plan,
        dir,
        args.profile.as_deref(),
        scope,
        &args.args,
        args.keep,
    )
}

pub fn list(args: &RunsArgs) -> Result<()> {
    let runs = crate::runs::list();
    if args.json {
        println!("{}", serde_json::to_string_pretty(&runs)?);
        return Ok(());
    }
    if runs.is_empty() {
        println!("No live runs.");
        return Ok(());
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    for r in runs {
        let profile = r
            .profile
            .as_ref()
            .map(|p| format!(" · toolset {p}"))
            .unwrap_or_default();
        println!(
            "{}  {} pid={} up={}{}  {}",
            r.id.bold(),
            r.display,
            r.pid,
            fmt_uptime(now.saturating_sub(r.started_unix)),
            profile,
            r.cwd.dimmed()
        );
    }
    Ok(())
}

pub fn kill(args: &KillArgs) -> Result<()> {
    crate::runs::kill(&args.id, args.force)?;
    let how = if args.force { " (forced)" } else { "" };
    println!("{} killed run {}{}", "✓".green(), args.id.bold(), how);
    Ok(())
}

/// Compact human uptime: `45s`, `12m`, `3h05m`.
pub(crate) fn fmt_uptime(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(harness: &str) -> RunArgs {
        RunArgs {
            harness: harness.to_string(),
            locked: false,
            unprotected: false,
            prompt: None,
            profile: None,
            scope: None,
            keep: false,
            sandbox: false,
            lockdown: false,
            plan: false,
            model: None,
            effort: None,
            args: Vec::new(),
        }
    }

    /// W2 witness, moved with the default: `--prompt` outside the protected run
    /// refuses loudly — before any manifest resolution or launch — and names
    /// the one valid form. It used to be `--prompt` without `--locked`; now the
    /// protected run is the default, so the refusal fires on the flags that opt
    /// OUT of it.
    #[test]
    fn prompt_outside_the_protected_run_refuses_and_names_the_valid_form() {
        for opt_out in ["unprotected", "sandbox", "lockdown"] {
            let mut a = args("claude-code");
            a.prompt = Some("say hi".to_string());
            match opt_out {
                "unprotected" => a.unprotected = true,
                "sandbox" => a.sandbox = true,
                _ => a.lockdown = true,
            }
            let msg = format!("{:#}", run(&a, None).unwrap_err());
            assert!(
                msg.contains("--prompt needs the protected run"),
                "{opt_out}: {msg}"
            );
            assert!(
                msg.contains("run claude-code --prompt"),
                "{opt_out}: valid form named: {msg}"
            );
        }
    }

    /// The two ways to say "protected" and "not protected" cannot both be
    /// given: flag order must never decide which gate a run got.
    #[test]
    fn locked_and_unprotected_together_refuse() {
        let mut a = args("codex");
        a.locked = true;
        a.unprotected = true;
        let msg = format!("{:#}", run(&a, None).unwrap_err());
        assert!(msg.contains("contradict each other"), "{msg}");
        assert!(msg.contains("nothing was launched"), "{msg}");
    }

    /// `--plan` still needs a gated mode, but the protected plan no longer
    /// needs `--locked` spelled out — only the explicit opt-out has no plan.
    #[test]
    fn plan_under_the_opt_out_refuses_and_names_the_protected_plan() {
        let mut a = args("codex");
        a.unprotected = true;
        a.plan = true;
        let msg = format!("{:#}", run(&a, None).unwrap_err());
        assert!(msg.contains("--plan needs a gated run mode"), "{msg}");
        assert!(msg.contains("agentstack run --plan codex"), "{msg}");
    }
}
