//! `agentstack doctor` — the trust layer. Static, offline checks across every
//! wired-up surface: adapters/CLIs, bridge/trust, secrets, drift, instructions,
//! quirks, skills, library, content, reproducibility, recipes, and policy.
//! Every check always runs; the default report shows only the sections relevant
//! to this project (plus anything warning/erroring) — `--all` prints the rest.
//! `--ci` exits nonzero on any error (team gate) and always shows everything;
//! `--live` adds MCP `initialize` handshakes; `--fix` re-applies drifted target
//! configs (safe class). Drift/fix operate on global scope.

use std::path::Path;

use anyhow::Result;
use owo_colors::OwoColorize;

use crate::cli::DoctorArgs;
use crate::manifest::{validate_with_context, Manifest, Server, ServerType};
use crate::render::{
    declared_host, plan_hooks, plan_target_with_servers, resolve_targets, ruleset_for,
};
use crate::scope::Scope;
use crate::secret::Resolver;
use crate::state::{self, target_key, State};
use crate::util::paths;

#[derive(PartialEq)]
enum Level {
    Ok,
    /// True but not actionable here: an undetected CLI, another manifest's
    /// leftovers. Rendered dimmed, counted in neither total — so the closing
    /// "N error(s), M warning(s)" only counts things this project should act on.
    Info,
    /// True and worth stating, but not a defect in *this* setup the user is
    /// expected to repair — an ecosystem caveat (a bare `npx` launcher is how
    /// nearly every published MCP server ships). Counted in its own total so a
    /// healthy project still reports `ready`, and never eligible as the one
    /// "start with" next action. Between [`Level::Info`] and [`Level::Warn`]:
    /// louder than context, quieter than a thing to fix.
    Advisory,
    /// The check could not examine anything, because the project declares
    /// nothing for it to examine. Distinct from [`Level::Ok`] on purpose:
    /// green has to mean *verified*, and a check that read zero items verified
    /// nothing. Rendering it as a pass is the same false-ready shape the
    /// v0.17.1 fix removed from `status` — the report looked healthier than
    /// the evidence behind it. Counted in no total, never a next action, and
    /// never forces a section into view; it only refuses to claim a pass.
    Unchecked,
    Warn,
    Error,
}

/// Accumulates every check result (grouped by section). Nothing prints while
/// checks run — `print` renders the terminal report at the end, filtered by
/// per-section relevance, and external integrations render `to_json`. The
/// error/warning counters are display-independent: every check always runs and
/// always counts, whether or not its section is shown.
struct Report {
    errors: usize,
    warnings: usize,
    /// [`Level::Advisory`] findings. Deliberately *not* folded into
    /// `warnings`: `state()` and `first_fix()` both read "is there something
    /// to act on?", and an advisory answers no.
    advisories: usize,
    sections: Vec<Section>,
    /// The project's machine-readable trust state (`trusted` / `drifted` /
    /// `untrusted`), set when a project context is checked. `None` when doctor
    /// ran with no project. Exposed in the JSON so consumers don't have to
    /// parse the gateway section's prose.
    trust: Option<&'static str>,
    /// The project's delivery mode (`static` / `clean-at-rest` / `zero-files`),
    /// derived by the SAME `mode_from_signals` reading `agentstack status`
    /// prints — set when a project context is checked, `None` when doctor ran
    /// with no project. In the JSON so consumers stop inferring the mode from
    /// section prose (`doctor-mode-v1`).
    mode: Option<&'static str>,
    /// Whether this project was ever activated (`locked`, a lockfile exists)
    /// or not (`never_activated`) — the same fact `status` words as
    /// "not locked (never activated)". `None` when doctor ran with no project.
    activation: Option<&'static str>,
    /// Whether this project maintains the managed `.gitignore` block
    /// (`[meta] gitignore`). `None` when doctor ran with no project. A panel
    /// needs it to label its toggle honestly — offering "Keep .gitignore as
    /// is" to a project that already opted out is the kind of small lie the
    /// consent work exists to remove (`gitignore-opt-out-v1`).
    gitignore: Option<bool>,
    /// This machine's CLI coverage: how many CLIs are installed, and how many
    /// of those can host the stdio bridge — the honest denominator for any
    /// "served live" claim. A zero-files project reaches `bridge_capable` of
    /// `detected`, and the ones it cannot reach are NAMED, because a coverage
    /// number that shrinks silently is worse than no number
    /// (`doctor-cli-coverage-v1`). `None` when doctor ran with no project.
    clis: Option<CliCoverage>,
    /// Structured `--probe` results. `None` on every other invocation, so the
    /// JSON omits the key entirely: a consumer can tell "not probed" from
    /// "probed, and there was nothing to probe" (`ran: true`, empty list).
    probe: Option<ProbeResults>,
    /// Whether the manifest declares ANY capability (F11 coverage term).
    /// `readiness` was purely a findings-and-trust verdict, so a manifest
    /// reduced to `version = 1` with a leftover lockfile reported `ready` —
    /// "ready" over nothing to be ready with. Set in `run_checks`; `None`
    /// when doctor ran with no project.
    declares_anything: Option<bool>,
}

/// The machine's bridge coverage, from the ONE definition `gateway connect`
/// itself uses ([`crate::commands::connect::bridge_capable`]) — so this field,
/// the connect command, and the `set-mode` plan can never disagree.
struct CliCoverage {
    /// CLIs detected on this machine.
    detected: usize,
    /// Of those, how many can host the stdio bridge (zero-files delivery).
    bridge_capable: usize,
    /// Display names of detected CLIs that cannot — the ones a "served live"
    /// project must not claim to reach.
    bridge_incapable: Vec<String>,
}

/// The `--probe` outcome as a whole, so the JSON can distinguish a probe that
/// ran from one the trust gate refused. Prose in the section lines says the
/// same thing; a UI should not have to parse it.
struct ProbeResults {
    ran: bool,
    /// Why nothing was spawned: `untrusted` / `drifted`, or `None` when it ran.
    skipped_reason: Option<&'static str>,
    /// One entry per stdio server, in manifest order.
    servers: Vec<serde_json::Value>,
}

struct Section {
    title: String,
    /// Does this project use the feature this section checks? Irrelevant
    /// sections are hidden from the default terminal report (never from
    /// `--all`, `--ci`, or the JSON) — progressive disclosure, not skipping.
    relevant: bool,
    /// (level, message) — level is `ok` / `warn` / `error`.
    lines: Vec<(&'static str, String)>,
}

impl Report {
    fn new() -> Self {
        Report {
            errors: 0,
            warnings: 0,
            advisories: 0,
            sections: Vec::new(),
            trust: None,
            mode: None,
            activation: None,
            gitignore: None,
            clis: None,
            probe: None,
            declares_anything: None,
        }
    }

    fn section(&mut self, title: &str) {
        self.sections.push(Section {
            title: title.to_string(),
            relevant: true,
            lines: Vec::new(),
        });
    }

    /// Mark the current section as not relevant to this project. Call once the
    /// section's own data shows the feature is unused — a section with any
    /// warn/error line is shown regardless, so this only ever hides all-Ok noise.
    fn mark_irrelevant(&mut self) {
        if let Some(s) = self.sections.last_mut() {
            s.relevant = false;
        }
    }

    fn line(&mut self, level: Level, msg: impl AsRef<str>) {
        let tag = match level {
            Level::Ok => "ok",
            Level::Info => "info",
            Level::Unchecked => "unchecked",
            Level::Advisory => {
                self.advisories += 1;
                "advisory"
            }
            Level::Warn => {
                self.warnings += 1;
                "warn"
            }
            Level::Error => {
                self.errors += 1;
                "error"
            }
        };
        if self.sections.is_empty() {
            // Validation issues land before the first titled section.
            self.sections.push(Section {
                title: "Manifest".to_string(),
                relevant: true,
                lines: Vec::new(),
            });
        }
        self.sections
            .last_mut()
            .expect("section exists")
            .lines
            .push((tag, msg.as_ref().to_string()));
    }

    /// Render the terminal report. Default: only sections that are relevant to
    /// this project or carry a warn/error. `show_all` (from `--all` or `--ci`)
    /// prints everything, matching the complete JSON response.
    fn print(&self, show_all: bool) {
        let mut hidden = 0;
        for s in &self.sections {
            // A section that produced no lines has nothing to say — print no
            // bare header for it, even under `--all`. (This happens when every
            // line a section would emit is a healthy state we no longer restate,
            // e.g. Policy on a machine whose only policy is a healthy machine
            // layer.) It isn't a "feature this project doesn't use", so it
            // doesn't count toward the hidden-sections footer.
            if s.lines.is_empty() {
                continue;
            }
            // Info lines are context, not findings — they don't force an
            // otherwise-irrelevant section into view. Advisories do: they are
            // real findings the user should read once, just not ones that
            // count against the project's readiness.
            let flagged = s
                .lines
                .iter()
                .any(|(tag, _)| *tag == "warn" || *tag == "error" || *tag == "advisory");
            if !(show_all || s.relevant || flagged) {
                hidden += 1;
                continue;
            }
            println!("{}", s.title.bold());
            for (tag, msg) in &s.lines {
                let mark = match *tag {
                    "warn" => "⚠".yellow().to_string(),
                    "error" => "✗".red().to_string(),
                    // An advisory keeps a readable body (unlike `info`, which
                    // is dimmed whole) but a quiet marker, so the eye reads it
                    // once without filing it next to the things to fix.
                    "info" | "advisory" => "·".dimmed().to_string(),
                    // Deliberately not "✓": the check verified nothing. The
                    // marker is quiet but the body stays at full contrast, so
                    // the non-coverage is legible rather than buried — the
                    // point is that the user *reads* "nothing declared to
                    // check" instead of scanning a green tick.
                    "unchecked" => "–".dimmed().to_string(),
                    _ => "✓".green().to_string(),
                };
                // Dim info lines whole so the eye skips them on a first read.
                if *tag == "info" {
                    println!("  {mark} {}", msg.dimmed());
                } else {
                    println!("  {mark} {msg}");
                }
            }
        }
        if hidden > 0 {
            let verb = if hidden == 1 { "is" } else { "are" };
            println!(
                "{} {} for features this project doesn't use {verb} hidden — {} shows everything.",
                "·".dimmed(),
                super::count(hidden, "section"),
                "agentstack doctor --all".bold()
            );
        }
    }

    /// The fix from the first error line carrying a `↳ fix` hint — or the
    /// first warning's, but only when there are NO errors at all. An
    /// outstanding error usually blocks the warnings' commands (an invalid
    /// manifest refuses `apply --write`), so pointing at a warning fix while
    /// an error exists would send the user into a wall; better no triage line
    /// than a misleading one. Reuses the `↳` convention every actionable line
    /// in this file already follows, so it needs no extra bookkeeping.
    /// Among the candidates at that level, one AgentStack can run itself wins
    /// over one that hands the user off to another tool. Both are honest, but
    /// "run this command" converges and "go and do a thing in Codex" is a
    /// detour — and the section order that used to decide this was an
    /// implementation detail, not a ranking. Section order still breaks ties
    /// *within* each class, which is a stable, reviewable rule.
    ///
    /// Advisories are never candidates: they have nothing to converge on.
    fn first_fix(&self) -> Option<&str> {
        let want = if self.errors > 0 { "error" } else { "warn" };
        let candidates: Vec<&str> = self
            .sections
            .iter()
            .flat_map(|s| &s.lines)
            .filter(|(tag, _)| *tag == want)
            .filter_map(|(_, msg)| msg.split_once("↳ ").map(|(_, fix)| fix.trim()))
            .collect();
        candidates
            .iter()
            .find(|fix| fix.starts_with("agentstack "))
            .or_else(|| candidates.first())
            .copied()
    }

    /// Exactly one recommended command, always — the Phase-3 "status as one
    /// next action" rule. [`first_fix`](Self::first_fix) answers "what should
    /// I repair?", which is `None` for a healthy setup and for findings whose
    /// remedy is prose; this answers the strictly broader "what do I do now?",
    /// which always has an answer. A report that ends in findings-without-a-
    /// path, or in nothing at all, makes the user invent the next step — the
    /// one thing a status surface exists to remove.
    ///
    /// The ladder below is ordered by what blocks what: an error first (it
    /// blocks the commands every other rung would name), then the review that
    /// gates activation, then an unrepaired warning, then the one-screen
    /// summary. Each rung is reachable and non-destructive.
    ///
    /// Consent outranks warning-level repairs on purpose. `status`'s own
    /// ladder ([`super::overview::next_step`]) puts a pending or stale review
    /// above every setup step, and the two surfaces answering "the one next
    /// action" differently is the disagreement this order removes: a project
    /// that is both drifted and missing, say, the t3code guard used to hear
    /// `agentstack trust .` from `status` and `agentstack guard install` from
    /// `doctor`, purely because of which section registered first. Nothing
    /// below trust is reordered — [`first_fix`](Self::first_fix) keeps its
    /// documented section-order tie-break.
    fn next_action(&self) -> (String, &'static str) {
        // Errors stay on top: an outstanding error usually blocks the very
        // command the review or a warning fix would name.
        if self.errors > 0 {
            if let Some(fix) = self.first_fix() {
                return (fix.to_string(), "the finding to start with");
            }
            // A finding with no parseable `↳ fix` still needs a real next step
            // (F21): `first_fix` returns `None` for a prose remedy, and falling
            // through to "nothing to repair" printed `1 error` directly above a
            // next step that claimed there was nothing wrong. Point at the
            // findings themselves — the report is right here — rather than at
            // another surface.
            return (
                "review the errors above".to_string(),
                "each is a problem this project has to fix before it is ready",
            );
        }
        if self.trust == Some("untrusted") {
            return (
                "agentstack trust .".to_string(),
                "review this project — nothing it declares is active until you do",
            );
        }
        if self.trust == Some("drifted") {
            return (
                "agentstack trust .".to_string(),
                "the content changed since you last said yes — review what moved",
            );
        }
        // Trusted (or trust is not this report's concern): the warning-level
        // repairs take over, in `first_fix`'s documented order.
        if let Some(fix) = self.first_fix() {
            return (fix.to_string(), "the finding to start with");
        }
        if self.warnings > 0 {
            return (
                "review the warnings above".to_string(),
                "none blocks activation, but each names something worth knowing",
            );
        }
        // Nothing to repair, trusted, no findings. This used to name
        // `agentstack status`, which names `agentstack doctor` right back —
        // the A↔B dead end the pilot hit (F21). The honest terminal is the
        // next RUNG, not a lateral hop to the other summary: the wiring is
        // verified, so the journey continues at Switch.
        (
            "agentstack toolset create <name> --server <server>".to_string(),
            "nothing to repair — group these for a task, then switch between toolsets",
        )
    }

    /// The one-word answer an external status surface leads with (UI
    /// control-plane §"Status"). `needs_setup` never comes from here — a
    /// report only exists once a manifest loaded; [`run`] emits that state
    /// before checks can run.
    ///
    /// Advisories are excluded on purpose. A status chip that sits on
    /// "needs attention" for a setup with nothing to repair trains the user to
    /// ignore it, which costs more than the advisory was worth; only findings
    /// with an action behind them may move this off `ready`.
    fn state(&self) -> &'static str {
        if self.errors + self.warnings > 0 {
            "needs_attention"
        } else {
            "ready"
        }
    }

    /// What `state` would say if it answered the question users actually ask.
    ///
    /// [`state`](Self::state) reports whether any check found something to
    /// repair, and nothing more — so it says `ready` over a project that is
    /// untrusted and never activated, where zero findings is true and "ready"
    /// is a lie: nothing the project declares is live. That word cannot be
    /// changed in place (a panel rendering "Ready" from it would silently
    /// change meaning under its users), so the honest answer ships beside it
    /// under `status-honesty-v1` and `state` keeps its `status-v1` meaning.
    ///
    /// The order is what-blocks-what: repair the findings, then pass the
    /// consent gate, then activate. It deliberately reports `needs_attention`
    /// over a warning that [`next_action`](Self::next_action) ranks *below*
    /// the review — the two answer different questions ("is anything wrong?"
    /// versus "what do I do first?"), and a warning is still something wrong.
    fn readiness(&self) -> &'static str {
        // No project context (doctor ran outside one): there is no project
        // readiness to report, and reporting the machine's as the project's is
        // the substitution this whole item exists to remove.
        if self.trust.is_none() && self.activation.is_none() {
            return "unknown";
        }
        if self.errors + self.warnings > 0 {
            return "needs_attention";
        }
        match self.trust {
            Some("untrusted") => return "untrusted",
            Some("drifted") => return "drifted",
            _ => {}
        }
        // A coverage term (F11): "ready" is a claim that this project has a
        // working setup, and a manifest that declares nothing has no setup to
        // be ready with — a `version = 1` husk beside a leftover lockfile used
        // to report `ready`. This is the term that was missing.
        if self.declares_anything == Some(false) {
            return "empty";
        }
        if self.activation == Some("never_activated") {
            return "never_activated";
        }
        "ready"
    }

    /// The readiness verdict as a human line for the terminal footer, or
    /// `None` when there is no project verdict to state (`unknown`). One
    /// short clause each, matching the JSON verdict word for word so the two
    /// surfaces cannot drift (F11).
    fn readiness_line(&self) -> Option<String> {
        let (word, gloss) = match self.readiness() {
            "unknown" => return None,
            "needs_attention" => ("needs attention", "fix the findings above first"),
            "untrusted" => (
                "not ready",
                "untrusted — nothing it declares is active until you review it",
            ),
            "drifted" => (
                "not ready",
                "the content changed since you approved it — review what moved",
            ),
            "empty" => (
                "not ready",
                "this setup declares nothing yet — add a server, skill, or instruction",
            ),
            "never_activated" => (
                "not ready",
                "set up but never activated — `agentstack use --write` makes it live",
            ),
            _ => ("ready", "trusted, activated, and verified"),
        };
        Some(format!("{}: {}", word, gloss).to_string())
    }

    /// The `probe` key, or `null` when `--probe` was not asked for. Absent and
    /// `ran: false` mean different things (never asked vs asked and refused),
    /// which is exactly why this is an object rather than a bare array.
    fn probe_json(&self) -> serde_json::Value {
        match &self.probe {
            None => serde_json::Value::Null,
            Some(p) => serde_json::json!({
                "ran": p.ran,
                "skipped_reason": p.skipped_reason,
                "servers": p.servers,
            }),
        }
    }

    fn to_json(&self) -> serde_json::Value {
        // Which stronger protections are ACTIVE on this machine — factual
        // booleans, not a posture claim. `guard` is the cooperative pre-tool
        // hook (advisory, not confinement); `machine_policy` is the presence
        // of a machine ceiling. A UI may say "Protected" only over these
        // facts; the enforcement matrix stays the honest source for what each
        // actually stops.
        let guard = matches!(
            crate::manifest::machine_guard_health(),
            Some(Ok(cfg)) if cfg.enabled()
        );
        let machine_policy = !matches!(
            crate::machine_policy::inspect().status,
            crate::machine_policy::Status::Unconfigured
        );
        serde_json::json!({
            "state": self.state(),
            // The honest readiness (`status-honesty-v1`). `state` above stays
            // exactly as `status-v1` defined it; this is the field a UI should
            // render, because it does not call an untrusted, never-activated
            // project ready. See `Report::readiness`.
            "readiness": self.readiness(),
            // The same line the text report prints ("next: …") — exactly one
            // recommended command, always. It was previously null whenever
            // there was nothing to *repair*, which left a consumer with a
            // healthy report and no step to offer; `state` already carries
            // "is anything wrong?", so this key is free to answer the broader
            // "what now?" without ambiguity. Never a command that would refuse.
            "next_action": self.next_action().0,
            "protection": { "guard": guard, "machine_policy": machine_policy },
            "errors": self.errors,
            "warnings": self.warnings,
            // Findings that are true but carry no action for this project.
            // A UI may show the count as a quiet note; it must not drive the
            // status chip (see `state`).
            "advisories": self.advisories,
            "trust": self.trust,
            // Delivery mode + activation, the same readings `status` prints
            // (`doctor-mode-v1`) — so a UI never has to reverse them out of
            // section prose like "Mode zero-files" or "never activated".
            "mode": self.mode,
            "activation": self.activation,
            "gitignore": self.gitignore,
            "clis": self.clis.as_ref().map(|c| serde_json::json!({
                "detected": c.detected,
                "bridge_capable": c.bridge_capable,
                "bridge_incapable": c.bridge_incapable,
            })),
            // Per-server startup results (see `doctor-probe-v1`); null unless
            // `--probe` ran.
            "probe": self.probe_json(),
            "sections": self.sections.iter().map(|s| serde_json::json!({
                "title": s.title,
                "relevant": s.relevant,
                "lines": s.lines.iter().map(|(level, msg)| serde_json::json!({
                    "level": level,
                    "msg": msg,
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        })
    }
}

pub fn run(args: &DoctorArgs, manifest_dir: Option<&Path>) -> Result<()> {
    // JSON callers (the t3code status panel) need "no project yet" as a
    // STATE, not a load error: an uninitialized directory is the normal
    // starting point of the setup journey, and the panel's whole job is to
    // say so and point at the one next action. The terminal path keeps its
    // error (the message already names `agentstack init`), and `--ci` keeps
    // failing loudly — a gate pointed at an uninitialized directory is a
    // misconfiguration, not a setup journey.
    if args.json && !args.ci {
        let base = super::project_base(manifest_dir)?;
        let dir = crate::manifest::resolve_manifest_dir(&base);
        if !dir.join(crate::manifest::load::MANIFEST_FILE).exists() {
            let out = crate::ui_contract::envelope(serde_json::json!({
                "state": "needs_setup",
                // Hand-written twin of `Report::to_json` — every key that
                // contract promises has to appear in BOTH, or a consumer sees
                // the field vanish on the one path where it is least able to
                // guess. `needs_setup` is a readiness like any other.
                "readiness": "needs_setup",
                "next_action": "agentstack init",
                "protection": serde_json::Value::Null,
                "errors": 0,
                "warnings": 0,
                "trust": serde_json::Value::Null,
                // Present and null, not absent: `doctor-mode-v1` promises the
                // keys, and a consumer should not have to treat "missing on a
                // binary that advertises the name" as a third case.
                "mode": serde_json::Value::Null,
                "activation": serde_json::Value::Null,
                "gitignore": serde_json::Value::Null,
                "clis": serde_json::Value::Null,
                "probe": serde_json::Value::Null,
                "sections": [],
            }));
            println!("{}", serde_json::to_string_pretty(&out)?);
            return Ok(());
        }
    }

    let mut report = Report::new();
    let fixed = run_checks(args, manifest_dir, &mut report)?;

    if args.json {
        // Machine-readable: the full structured report (this is the surface the
        // retired `audit --json` used to occupy — doctor now owns it, as a
        // superset that carries every section, not just the content scan).
        // Nothing else goes to stdout so the output stays parseable.
        println!(
            "{}",
            serde_json::to_string_pretty(&crate::ui_contract::envelope(report.to_json()))?
        );
    } else {
        // `--ci` always shows the full report: a team gate should print exactly
        // what it evaluated, not a per-project selection of it.
        report.print(args.all || args.ci);

        println!();
        if fixed > 0 {
            println!(
                "{} re-applied {}.",
                "✓".green(),
                super::count(fixed, "drifted target")
            );
        }
        // Notes are reported separately from warnings so the headline count
        // answers "what must I fix?" and nothing else.
        let notes = if report.advisories > 0 {
            format!(", {}", super::count(report.advisories, "note"))
        } else {
            String::new()
        };
        println!(
            "{}, {}{notes}.",
            super::count(report.errors, "error"),
            super::count(report.warnings, "warning")
        );
        // Mirror the JSON readiness verdict to the terminal (F11): the count
        // line alone read `0 errors, 0 warnings` over an untrusted, empty, or
        // drifted project — true about findings, silent about whether the
        // project is actually ready. The one-word verdict the JSON already
        // computed belongs on the screen the human reads too.
        if let Some(line) = report.readiness_line() {
            println!("  {line}");
        }
        // Every report ends the same way: one command, and why it is the one.
        // Unconditional — a clean report that simply stops leaves the reader
        // to guess whether "0 errors" means "done" or "I forgot a step".
        let (cmd, why) = report.next_action();
        println!("  next: {}   {}", cmd.green().bold(), why.dimmed());
    }

    // In CI mode any error fails the trust gate. Return an error rather than
    // exiting inline so `main` owns the single exit point and this path stays
    // testable. Gating is independent of the output format above.
    if args.ci && report.errors > 0 {
        anyhow::bail!(
            "doctor found {} — see report above",
            super::count(report.errors, "error")
        );
    }
    Ok(())
}

/// The same checks `doctor` runs, with fix/live off and nothing printed —
/// read-only integration entry point. Deep stays on because an explicit
/// check-up surface must keep the content scan's findings.
pub fn collect(manifest_dir: Option<&Path>) -> Result<serde_json::Value> {
    let mut report = Report::new();
    run_checks(
        &DoctorArgs {
            ci: false,
            live: false,
            probe: false,
            fix: false,
            deep: true,
            all: true,
            json: false,
            skip_drift: false,
        },
        manifest_dir,
        &mut report,
    )?;
    Ok(report.to_json())
}

/// One scanned capability (skill or instruction fragment) and its findings —
/// the unit the content scan reports. Serializable so `doctor --json` can carry
/// it in the structured report.
#[derive(serde::Serialize)]
pub struct Unit {
    /// `skill` or `instruction`.
    pub kind: &'static str,
    pub name: String,
    /// Set when the source couldn't be scanned (not materialized, read error).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
    pub findings: Vec<crate::scan::Finding>,
}

/// Scan every manifest skill materialized locally (path sources and store
/// clones of the lock references) plus every instruction file. Offline: a git
/// skill not yet in the store is reported as skipped, never fetched. This is
/// the content scan `doctor --deep` (and `--ci`) drives — the sole owner now
/// that the standalone `audit` verb is gone.
pub fn collect_content_units(
    manifest: &Manifest,
    dir: &Path,
    store: &crate::store::Store,
) -> Vec<Unit> {
    use crate::scan;
    let lock = crate::lock::Lock::load(dir).unwrap_or_default();
    let mut units = Vec::new();
    for (name, skill) in &manifest.skills {
        let pinned_rev = lock.get(name).and_then(|entry| entry.rev.as_deref());
        let unit = match crate::store::local_source_dir(store, skill, dir, pinned_rev) {
            None => skipped_unit(
                "skill",
                name,
                "not materialized ↳ agentstack install".into(),
            ),
            Some(src) => match scan::scan_tree(&src) {
                Ok(findings) => Unit {
                    kind: "skill",
                    name: name.clone(),
                    skipped: None,
                    findings,
                },
                Err(e) => skipped_unit("skill", name, format!("scan failed: {e}")),
            },
        };
        units.push(unit);
    }
    for (name, instr) in &manifest.instructions {
        let path = resolve_scan_path(dir, &instr.path);
        let unit = if !path.exists() {
            skipped_unit("instruction", name, format!("missing file {}", instr.path))
        } else {
            match scan::scan_file(&path, &instr.path) {
                Ok(findings) => Unit {
                    kind: "instruction",
                    name: name.clone(),
                    skipped: None,
                    findings,
                },
                Err(e) => skipped_unit("instruction", name, format!("scan failed: {e}")),
            }
        };
        units.push(unit);
    }
    units
}

fn skipped_unit(kind: &'static str, name: &str, reason: String) -> Unit {
    Unit {
        kind,
        name: name.to_string(),
        skipped: Some(reason),
        findings: Vec::new(),
    }
}

fn resolve_scan_path(dir: &Path, p: &str) -> std::path::PathBuf {
    let pb = std::path::PathBuf::from(p);
    if pb.is_absolute() {
        pb
    } else {
        dir.join(pb)
    }
}

fn run_checks(
    args: &DoctorArgs,
    manifest_dir: Option<&Path>,
    report: &mut Report,
) -> Result<usize> {
    let ctx = super::load(manifest_dir)?;
    let manifest = &ctx.loaded.manifest;

    // Manifest-level validation first — library-aware, so a profile ref to a
    // central-library server/skill is not flagged as unknown.
    let libctx = ctx.library_ctx();
    let vctx = libctx.validate_ctx(&ctx.dir);
    let validation_targets: Vec<&str> = ctx.registry.ids().collect();
    for issue in validate_with_context(manifest, validation_targets, &vctx) {
        // Mirror apply/bootstrap: structural issues (is_error) are errors so
        // `doctor --ci` fails the trust gate; softer issues stay warnings.
        let level = if issue.kind.is_error() {
            Level::Error
        } else {
            Level::Warn
        };
        // Carry the repair command in the `↳` voice so the line is actionable
        // in place and the closing `start with:` triage can pick it up.
        let msg = match &issue.fix {
            Some(fix) => format!("{} ↳ {fix}", issue.message),
            None => issue.message,
        };
        report.line(level, msg);
    }

    let target_ids = resolve_targets(manifest, &ctx.registry, &[], &ctx.dir)?;
    let mut state = State::load()?;
    let mut fixed = 0;

    report.section("Adapters & CLIs");
    for id in &target_ids {
        match ctx.registry.get(id) {
            None => report.line(Level::Error, format!("{id}: unknown adapter")),
            Some(desc) => {
                // "<path> parses" only makes sense when there is a config to
                // parse; an adapter with none (e.g. Pi) gets an honest label
                // instead of the garbled "no MCP config parses".
                let wiring = desc
                    .config
                    .as_ref()
                    .map(|c| format!("{} parses", tidy_path(&paths::expand_tilde(&c.path))))
                    .unwrap_or_else(|| "no MCP config to check".to_string());
                if desc.is_installed() {
                    match desc.read_config_value() {
                        Ok(_) => report.line(
                            Level::Ok,
                            format!("{:<14} installed · {}", desc.display, wiring),
                        ),
                        Err(e) => report.line(
                            Level::Error,
                            format!("{}: config invalid · {e}", desc.display),
                        ),
                    }
                } else if desc.config_present() {
                    // Advisory, not Warn, for the same reason the branch below
                    // is Info: not having a CLI installed is a fact, not a
                    // fault. A config file outliving its editor does not change
                    // that — an uninstall leaves the directory behind, so this
                    // fires on a machine where nothing is wrong and nothing can
                    // be repaired. As a warning it never cleared, which put a
                    // healthy project permanently at "needs attention" over
                    // somebody else's leftovers, and made the one state the
                    // user is meant to act on mean less every time they saw it.
                    //
                    // It stays louder than Info because there IS something
                    // here: we would render for a tool that cannot launch, and
                    // the remedy (install it, or drop it from
                    // `[targets].default`) is worth stating. That is precisely
                    // what Advisory is for — counted in its own total, never
                    // the "start with" next action, and `state()` stays ready.
                    report.line(
                        Level::Advisory,
                        format!("{:<14} config present but binary not on PATH", desc.display),
                    );
                } else if desc.project_config_present(&ctx.dir) {
                    // Not on this machine, but this project carries its config —
                    // the shape `status` now reports as detected here. Printing
                    // "not detected" over a `.mcp.json` in the working directory
                    // is the same one-screen contradiction, one surface over.
                    report.line(
                        Level::Info,
                        format!(
                            "{:<14} configured in this project · binary not on PATH",
                            desc.display
                        ),
                    );
                } else {
                    // Not having a CLI installed is a fact, not a fault — Info,
                    // so a machine with 5 of 13 CLIs isn't greeted by 8 warnings.
                    report.line(
                        Level::Info,
                        format!("{:<14} not detected (ok unless you use it)", desc.display),
                    );
                }
            }
        }
    }

    // Which build is this? A release binary compiles the optional `sandbox`
    // feature in; a source `cargo build --release` does not, and nothing else
    // in the binary distinguishes them — same version, same `--help`, different
    // capabilities. Info, not a warning: a build without it is a fact like an
    // uninstalled CLI, not a fault.
    //
    // Gated to the advanced paths (`--all`, `--ci`, and every JSON caller —
    // `collect` passes `all`) because the words it has to use name a mode the
    // beginner journey never mentions. tests/ordinary_journey_vocab.rs is the
    // witness: a scripted init → apply → doctor must not print "sandbox" at
    // all. Progressive disclosure, not omission — the fact is always in the
    // JSON, and the person who needs it is already running `doctor --all`.
    if args.all || args.ci || args.json {
        report.line(
            Level::Info,
            if crate::cli::SANDBOX_SUPPORT {
                format!(
                    "{:<14} sandbox support compiled in · run --sandbox needs a running Docker daemon",
                    "this build"
                )
            } else {
                format!(
                    "{:<14} no sandbox support · run --sandbox/--lockdown refuse ↳ install a release binary, or cargo build --release --features sandbox",
                    "this build"
                )
            },
        );
    }

    // Servers configured natively HERE that this manifest does not declare.
    //
    // This is the Status pillar's load-bearing check: without it, a project
    // whose whole setup sits in an unimported `.mcp.json` got
    // "0 error(s), 0 warning(s)" — doctor reporting health over a setup it had
    // silently ignored (pilot Run B). A clean doctor has to MEAN ready.
    //
    // Warn, not Error: the setup is not broken, it is uncovered, and `adopt`
    // is a one-command fix that is named right here. Project scope only —
    // machine-wide configs belong to whichever manifest manages them, and
    // warning every project about them would be noise, not a finding.
    report.section("Unmanaged setup");
    let unmanaged = crate::discover::native_configs(
        &ctx.registry,
        &ctx.dir,
        &manifest.servers,
        false, // project scope only
    );
    let pending: Vec<_> = unmanaged
        .iter()
        .filter(|n| !n.unimported.is_empty())
        .collect();
    if pending.is_empty() {
        report.line(
            Level::Ok,
            "no servers configured here outside this manifest",
        );
        report.mark_irrelevant();
    } else {
        for n in &pending {
            report.line(
                Level::Warn,
                format!(
                    "{:<14} {} in {} not in this manifest: {} ↳ agentstack adopt",
                    n.display,
                    super::count(n.unimported.len(), "server"),
                    tidy_path(&n.path),
                    crate::text::sanitize_line(&n.unimported.join(", ")),
                ),
            );
        }
    }

    check_t3code(report);

    // Zero-files gateway: which harnesses have the global `agentstack mcp
    // --auto-project` gateway registered, and where this project stands with
    // the trust gate. Not being connected is a choice, not a fault — only a
    // stale trust digest warns.
    report.section("Zero-files gateway");
    let mut connected = 0;
    for id in &target_ids {
        let Some(desc) = ctx.registry.get(id) else {
            continue;
        };
        let (Some(cfg), Some(mcp)) = (desc.config.as_ref(), desc.mcp.as_ref()) else {
            continue;
        };
        if !desc.detected() {
            continue;
        }
        let path = paths::expand_tilde(&cfg.path);
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        if crate::commands::connect::has_bridge_entry(&existing, &mcp.location, cfg.format) {
            connected += 1;
            report.line(
                Level::Ok,
                format!("{:<14} gateway registered (agentstack mcp)", desc.display),
            );
        }
    }
    if connected == 0 {
        report.line(
            Level::Ok,
            "no CLI connected — optional ↳ agentstack gateway connect --all",
        );
    }
    // W4 precondition 6 — gateway unavailable. A registered bridge whose
    // command is not runnable is not "not connected": that harness gets no
    // tools at all, and AgentStack writes nothing in its place (there is no
    // silent fallback into the rendered lane). One sentence, the same one
    // `status` prints, naming the one recovery command.
    for outage in crate::commands::connect::gateway_outages(&ctx.registry, &target_ids) {
        report.line(Level::Warn, outage.finding());
    }
    let base = crate::manifest::project_root_of(&ctx.dir);
    let trust_state = crate::trust::check(&base);
    report.trust = Some(match trust_state {
        crate::trust::TrustState::Trusted => "trusted",
        crate::trust::TrustState::Changed => "drifted",
        crate::trust::TrustState::Untrusted => "untrusted",
    });
    // W1: what this state has already cost. An advisory clause on the existing
    // findings rather than a check of its own — the condition is the same one
    // the trust findings already report, and a second finding would make one
    // fact read as two problems. The `↳ fix` suffix stays exactly where it is,
    // so `first_fix` still parses these lines and still names `agentstack
    // trust`.
    let pending = super::overview::needs_your_yes(&ctx.dir, &base, trust_state);
    let refused_clause = pending
        .as_ref()
        .map(|p| {
            format!(
                " — {} already refused here",
                super::count(p.refused, "call")
            )
        })
        .unwrap_or_default();
    match trust_state {
        crate::trust::TrustState::Trusted => {
            report.line(Level::Ok, "this project is trusted for auto mode")
        }
        crate::trust::TrustState::Changed => report.line(
            Level::Warn,
            format!(
                "trusted, but the manifest or lockfile changed since it was last reviewed{refused_clause} \
                 ↳ agentstack trust"
            ),
        ),
        // Untrusted is a choice, not a fault (Ok) — unless a harness actually
        // uses the bridge AND the project declares a runtime surface (inline
        // servers or profile/library name refs): then every session here
        // silently gets control-plane tools only, which is worth a warning.
        crate::trust::TrustState::Untrusted => {
            let runtime = crate::resolve::runtime_server_names(manifest, None);
            if connected > 0 && !runtime.is_empty() {
                let clis = if connected == 1 {
                    "1 CLI uses".to_string()
                } else {
                    format!("{connected} CLIs use")
                };
                let servers = if runtime.len() == 1 {
                    "1 server is".to_string()
                } else {
                    format!("{} servers are", runtime.len())
                };
                report.line(
                    Level::Warn,
                    format!(
                        "not trusted — {clis} the gateway, but this project's {servers} not proxied{refused_clause} ↳ agentstack trust {}",
                        base.display()
                    ),
                );
            } else if let Some(p) = &pending {
                // Untrusted is normally a choice, not a fault — but not once
                // something tried to work here and was refused. That is the
                // one signal that turns "you have not reviewed this" into
                // "this is costing you calls", so it earns the warning level
                // even without a registered bridge.
                report.line(
                    Level::Warn,
                    format!("not trusted for auto mode{refused_clause} ↳ {}", p.fix),
                );
            } else {
                report.line(
                    Level::Ok,
                    "not trusted for auto mode — untrusted repos get control-plane tools only ↳ agentstack trust",
                );
            }
        }
    }
    // Relevant once the bridge is actually in play: a harness registered it.
    //
    // Trust used to also count, on the reasoning that a project which had
    // "entered the trust lifecycle" had a user who went looking. That proxy
    // died with review finding H1: a consented `init` now records trust for the
    // manifest it wrote, so every ordinary first run is trusted and this
    // section — gateway, auto mode, control-plane tools — surfaced to exactly
    // the newcomer the progressive-disclosure rule says must not meet it.
    // `doctor --all` still prints it, and connecting a CLI still brings it back.
    if connected == 0 {
        report.mark_irrelevant();
    }

    // Delivery mode + activation, via the SAME derivations `status` uses
    // (overview::detect_mode / the lockfile's existence) — computed once here
    // so the Drift section below and the JSON body can't disagree with the
    // orientation screen about which mode this project is in. Like `status`,
    // the mode reads over ALL registry ids, not the resolved `[targets]`
    // subset: a bridge registered in a harness this project doesn't pin still
    // makes the machine's delivery story zero-files.
    let all_ids: Vec<String> = ctx.registry.ids().map(str::to_string).collect();
    let mode = super::overview::detect_mode(&ctx, &all_ids);
    let locked = crate::lock::Lock::path(&ctx.dir).exists();
    report.mode = Some(mode.label());
    report.activation = Some(if locked { "locked" } else { "never_activated" });
    // Coverage term for `readiness` (F11): does this project declare anything
    // at all? Every inert and executable kind counts — a project can be a pure
    // instruction or settings setup — so "ready" is never reported over a husk.
    report.declares_anything = Some(
        !manifest.skills.is_empty()
            || !manifest.servers.is_empty()
            || !manifest.instructions.is_empty()
            || !manifest.settings.is_empty()
            || !manifest.hooks.is_empty()
            || !manifest.extensions.is_empty(),
    );
    report.gitignore = Some(manifest.meta.manages_gitignore());
    let (detected, capable, incapable) =
        crate::commands::mode_switch::bridge_coverage(&ctx.registry);
    report.clis = Some(CliCoverage {
        detected,
        bridge_capable: capable,
        bridge_incapable: incapable,
    });

    report.section("Secrets");
    let refs = manifest.referenced_secrets();
    if refs.is_empty() {
        report.line(Level::Unchecked, "no secrets referenced — nothing to check");
        report.mark_irrelevant();
    }
    // Name the layer each ref resolves from (env / varlock / keychain / .env) —
    // the same provenance `secret list` and `setup` surface, via one
    // `SecretSources::detect` over the resolution chain rather than a bare
    // `resolve()` that only answers yes/no. `source_of` returns `None` exactly
    // when nothing in the chain resolves, so the not-found arm is unchanged.
    let sources = crate::secret::SecretSources::detect(&ctx.dir);
    // Resolving is only half the answer. `[policy.secrets]` decides whether
    // the servers that reference a ref may actually READ it, and a ref every
    // one of them is refused is dead weight no matter how well it resolves —
    // so a green "resolved from env" over it is the same vacuous pass P3.1
    // removed elsewhere: technically true, and it tells the user the opposite
    // of what will happen. `check_effective_policy` already errors on each
    // refusal in the Policy section; this stops the Secrets section from
    // contradicting it. Read-only: `secret_decision` is a pure verdict over
    // the compiled ruleset, so nothing is resolved, recorded, or spawned here.
    let refusal = SecretRefusal::compute(manifest);
    for name in &refs {
        match sources.source_of(name) {
            Some(source) => match refusal.verdict(name) {
                RefVerdict::Usable => {
                    report.line(Level::Ok, format!("{name:<20} resolved from {source}"))
                }
                // Not an Error: the Policy section already raised one per
                // refused (server, ref) pair, and counting the same defect
                // twice would inflate the total the closing line reports.
                RefVerdict::RefusedEverywhere => report.line(
                    Level::Warn,
                    format!(
                        "{name:<20} resolves from {source}, but [policy.secrets] refuses it for \
                         every server that references it — nothing can read it \
                         ↳ widen that policy, or drop the reference"
                    ),
                ),
                RefVerdict::RefusedSomewhere(refused) => report.line(
                    Level::Warn,
                    format!(
                        "{name:<20} resolves from {source}, but [policy.secrets] refuses it for \
                         {refused} of the servers that reference it — see Policy below"
                    ),
                ),
            },
            None => report.line(
                Level::Error,
                format!("{name:<20} not found ↳ agentstack secret set {name}"),
            ),
        }
    }
    // A `.env` is the one store that holds secret VALUES in plain text on disk,
    // so its mode is part of whether the secret is actually protected. Versions
    // before 0.16 wrote it at the ambient umask (0644 on a normal machine),
    // leaving tokens readable by every local account; `write_private` fixes new
    // writes, and this names the files already on disk. Warn, not error: the
    // secret still resolves, and only the owner can fix the mode.
    let env_path = ctx.dir.join(".env");
    if env_path.exists() && crate::util::atomic::is_group_or_world_readable(&env_path) == Some(true)
    {
        report.line(
            Level::Warn,
            format!(
                ".env is readable by other local accounts — it holds real token values \
                 ↳ chmod 600 {}",
                env_path.display()
            ),
        );
    }

    report.section("Dropped files");
    // Dropped-but-undeclared content in this project's own intake dirs. Not a
    // defect — the files are inert and the setup is still `ready` — so it is
    // Advisory: an offer the user has to be told exists, not a repair.
    let dropped = crate::intake::scan(
        &ctx.dir,
        &crate::manifest::project_root_of(&ctx.dir),
        manifest,
    );
    for item in &dropped.items {
        report.line(
            Level::Advisory,
            format!(
                "{} '{}' is in {} but not in this setup — inert until you take it live ↳ agentstack yes",
                item.kind.noun(),
                item.name,
                item.rel_path,
            ),
        );
    }
    for c in &dropped.collisions {
        // Same refusal the intake notice gives, and it must name the same
        // thing: `status` is where a user most often meets a collision, so a
        // renderer that stopped at "already declared" here would undo the
        // point of the collision card for the surface that shows it most.
        report.line(
            Level::Warn,
            format!(
                "{} '{}' in {} is not adopted — declared as {} \u{21b3} rename the file or remove the existing entry",
                c.kind.noun(),
                crate::text::sanitize_line(&c.name),
                crate::text::sanitize_line(&c.rel_path),
                crate::text::sanitize_line(&c.declared_as),
            ),
        );
        for line in &c.diff {
            report.line(Level::Warn, format!("  {line}"));
        }
    }
    if dropped.is_empty() {
        // Nothing waiting: true, but not something this project must act on,
        // so the section stays hidden unless `--all`.
        report.line(Level::Ok, "nothing dropped in waiting to be adopted");
        report.mark_irrelevant();
    }

    report.section("Drift");
    if args.skip_drift || mode == super::overview::Mode::ZeroFiles {
        // Both delivery modes that keep rendered configs off disk ON PURPOSE:
        // the usual "declared servers not on disk → apply --write" comparison
        // would be a false alarm pointing the user straight back at the render
        // they opted out of. Zero-files is detected here (the mode is derived,
        // never flagged), and it also spares the project the machine-wide
        // global-scope keys other manifests recorded — the old behavior
        // recommended `apply --write --scope global` to a project whose
        // truthful story was "served live, nothing to render" (2026-07-30
        // panel audit). Note ZeroFiles implies nothing rendered: any recorded
        // render flips the derivation to Static, which still gets the full
        // comparison. Suppress the section (hidden unless --all).
        report.line(
            Level::Ok,
            if args.skip_drift {
                "not rendering configs — clean-at-rest keeps them off disk"
            } else {
                "not rendering configs — zero-files serves this project live through the gateway"
            },
        );
        report.mark_irrelevant();
    } else {
        let mut any_drift = false;
        let identity = state::manifest_identity(&ctx.dir);
        // Context-default scope: project for a repo manifest, global for the
        // machine manifest — the scope `apply` writes here when none is passed
        //.
        let default_scope = Scope::default_for(&ctx.dir);
        // Owner-refreshed servers: the drift check renders with the owning app's
        // on-disk values, so an owned server that changed on disk is reported as
        // "refresh the manifest", never as a pending revert of what the app wrote
        // (see render::owned).
        let mut server_map: indexmap::IndexMap<String, crate::manifest::Server> =
            manifest.servers.clone();
        let owned = crate::render::refresh_owned_servers(
            &mut server_map,
            &ctx.registry,
            default_scope,
            &ctx.dir,
        );
        for o in owned.iter().filter(|o| o.stale) {
            any_drift = true;
            report.line(
                Level::Warn,
                format!(
                    "{:<14} changed in {} (owner) ↳ refresh manifest + re-fan out: \
                 agentstack apply --write",
                    o.name, o.owner_display
                ),
            );
        }
        let ruleset = match crate::render::ruleset_for(manifest) {
            Ok(ruleset) => Some(ruleset),
            Err(error) => {
                report.line(
                    Level::Error,
                    format!(
                    "effective machine policy unavailable — drift rendering is BLOCKED ({error:#})"
                ),
                );
                None
            }
        };
        for id in &target_ids {
            let Some(desc) = ctx.registry.get(id) else {
                continue;
            };
            // Which scopes to check for this target: every scope a previous write
            // recorded state at — a deliberate `--scope` choice keeps being
            // honored, never second-guessed — falling back to the context default
            // for fresh setups so quickstart → doctor stays clean.
            let mut scopes: Vec<Scope> = [Scope::Global, Scope::Project]
                .into_iter()
                .filter(|s| state.targets.contains_key(&target_key(id, *s, &ctx.dir)))
                .collect();
            if scopes.is_empty() {
                scopes.push(default_scope);
            }
            for scope in scopes {
                // Fix-command hints must name the scope when it isn't the default the
                // bare command would pick here.
                let scope_flag = if scope == default_scope {
                    String::new()
                } else {
                    format!(" --scope {scope}")
                };
                let key = target_key(id, scope, &ctx.dir);
                // Active-toolset awareness: a `use <p> --write` narrowed this
                // key's render to p's selection, so drift compares against that
                // selection — without it a fresh activation reads as "changes
                // pending" and the cue (`apply --write`) would widen the render
                // back, silently undoing the switch (2026-07-29 dry-run
                // finding). Name-filtering `server_map` keeps this section's
                // inline semantics; a recorded toolset that has since left the
                // manifest falls back to the full render.
                let active_profile = state
                    .active_profile(&key)
                    .filter(|p| manifest.profiles.contains_key(p));
                let profile_map: indexmap::IndexMap<String, crate::manifest::Server>;
                let render_map = match &active_profile {
                    Some(p) => {
                        profile_map = server_map
                            .iter()
                            .filter(|(n, _)| manifest.profiles[p.as_str()].servers.contains(n))
                            .map(|(n, s)| (n.clone(), s.clone()))
                            .collect();
                        &profile_map
                    }
                    None => &server_map,
                };
                // The command that brings disk back in line with the expected
                // render above — re-activate the selection when one is active.
                let fix_cmd = match &active_profile {
                    Some(p) => format!("agentstack use {p} --write"),
                    None => "agentstack apply --write".to_string(),
                };
                // Was this key's managed set recorded by a different manifest? Its
                // leftover entries are then not ours to prune (see
                // State::foreign_prunes): `--fix` keeps them, and the report points at
                // `apply --prune-foreign` instead of `apply --write`.
                let foreign_key = state
                    .manifest_source(&key)
                    .is_some_and(|src| src != identity);
                let mut previously = state.managed_servers(&key);
                let kept = if args.fix {
                    state.foreign_prunes(&key, scope, &ctx.dir, &mut previously, |n| {
                        server_map.contains_key(n)
                    })
                } else {
                    Vec::new()
                };
                // Names an earlier guarded write kept on disk (state bookkeeping —
                // they left `managed_servers` when the writing manifest recorded its
                // own set, so neither `foreign_key` nor the plan sees them). Keep
                // reporting the adopt-or-prune choice until one of them happens.
                let mut kept_report: Vec<String> = state
                    .kept_foreign(&key)
                    .into_iter()
                    .filter(|n| !server_map.contains_key(n))
                    .collect();
                for k in &kept {
                    if !kept_report.contains(k) {
                        kept_report.push(k.clone());
                    }
                }
                let Some(ruleset) = ruleset.as_ref() else {
                    continue;
                };
                let Some(plan) = plan_target_with_servers(
                    desc,
                    &ctx.resolver,
                    ruleset,
                    render_map,
                    &previously,
                    scope,
                    &ctx.dir,
                )?
                else {
                    continue;
                };

                if !kept_report.is_empty() {
                    any_drift = true;
                    // Another manifest's entries: apply already refuses to touch
                    // them without --prune-foreign, so this is context (Info),
                    // not something a fresh project must act on.
                    report.line(
                        Level::Info,
                        format!(
                            "{:<14} kept {} — applied by another manifest ↳ keep them: \
                     agentstack adopt{scope_flag} · prune them: agentstack apply \
                     --prune-foreign{scope_flag}",
                            desc.display,
                            kept_report.join(", ")
                        ),
                    );
                }
                // Hand-edit since our last write? `last_hash` covers the WHOLE
                // file, so on its own it flips on any on-disk change — managed OR
                // unmanaged. A config that doubles as a live state store (e.g.
                // ~/.claude.json, which running Claude Code sessions rewrite
                // constantly) churns its unmanaged keys all the time; comparing the
                // whole file made doctor flap "edited on disk" forever while
                // `agentstack diff` reported "in sync". Gate the warning on
                // `plan.changed()` — the exact managed-content comparison `diff`
                // uses (TargetPlan::changed → diff::differs over the rendered
                // region) — so doctor and diff always agree: the warning fires only
                // when the touch actually reached the region we manage.
                if let Some(ts) = state.targets.get(&key) {
                    // Did the file change on disk at all since our write? (Cheap
                    // whole-file guard; the managed-region check below decides
                    // whether that change is worth reporting.)
                    let touched =
                        !ts.last_hash.is_empty() && state::hash(&plan.existing) != ts.last_hash;
                    if touched && plan.changed() {
                        // The managed region on disk differs from what we'd render.
                        // A hand-edit is the common cause, but not the only one — a
                        // session ending onto a stale baseline reaches this same
                        // state with no hand-edit involved (review finding H8's
                        // fold-in). So the report states the observed fact, not a
                        // diagnosis: the file no longer matches what we last wrote.
                        // (A pure manifest change leaves the file untouched, so
                        // `touched` is false and this stays quiet; the
                        // pending-changes branch below reports that case instead.)
                        // When the last write came from a DIFFERENT manifest,
                        // "since last apply" isn't this project's story — demote to
                        // Info so machine-wide state doesn't drown a fresh project's
                        // report.
                        any_drift = true;
                        report.line(
                            if foreign_key {
                                Level::Info
                            } else {
                                Level::Warn
                            },
                            format!(
                                "{:<14} no longer matches what agentstack last wrote ↳ review: \
                             agentstack diff{scope_flag} · adopt the on-disk version: agentstack \
                             adopt{scope_flag}",
                                desc.display
                            ),
                        );
                    } else if touched && owned.iter().any(|o| o.stale && o.owner == **id) {
                        // The file changed but our managed region still matches the
                        // render, and this target owns a server its app rewrites by
                        // design — name that churn so a whole-file change on an owned
                        // config isn't left a mystery.
                        report.line(
                            Level::Ok,
                            format!(
                                "{:<14} rewritten by the app itself (owned server) — \
                             refresh the manifest: agentstack apply --write{scope_flag}",
                                desc.display
                            ),
                        );
                    }
                    // Otherwise: file untouched, or only unmanaged keys changed
                    // (benign live-state churn) — silent, so doctor agrees with diff.
                }
                // Pending manifest changes?
                if plan.changed() {
                    // An unresolved `${REF}` must never reach a live config — same gate
                    // as `apply`/`toggle`. `doctor --fix` has no override, so we refuse.
                    if args.fix
                        && (!plan.unresolved.is_empty()
                            || !plan.failed.is_empty()
                            || !plan.denied.is_empty())
                    {
                        any_drift = true;
                        if !plan.unresolved.is_empty() {
                            report.line(
                                Level::Error,
                                format!(
                                    "{:<14} not fixed — {}: {}",
                                    desc.display,
                                    if plan.unresolved.len() == 1 {
                                        "unresolved secret"
                                    } else {
                                        "unresolved secrets"
                                    },
                                    plan.unresolved.join(", ")
                                ),
                            );
                        }
                        if !plan.failed.is_empty() {
                            report.line(
                                Level::Error,
                                format!(
                                    "{:<14} not fixed — {}: {}",
                                    desc.display,
                                    if plan.failed.len() == 1 {
                                        "secret read failure"
                                    } else {
                                        "secret read failures"
                                    },
                                    plan.failed.join(", ")
                                ),
                            );
                        }
                    } else if args.fix {
                        plan.write()?;
                        state.record(&key, plan.managed.clone(), &plan.proposed, &identity);
                        // A --fix write is a guarded write too: keep the kept-foreign
                        // names reachable for a later `apply --prune-foreign`.
                        state.record_kept_foreign(&key, kept_report.clone());
                        fixed += 1;
                        report.line(
                            Level::Ok,
                            format!(
                                "{:<14} re-applied {}",
                                desc.display,
                                super::count(plan.managed.len(), "change")
                            ),
                        );
                    } else if plan.removed.is_empty() {
                        any_drift = true;
                        report.line(
                            Level::Warn,
                            format!(
                                "{:<14} {} pending ↳ {fix_cmd}{scope_flag}",
                                desc.display,
                                super::count(plan.managed.len(), "change")
                            ),
                        );
                    } else {
                        // A pending prune deletes real entries from a live config —
                        // name the victims and offer the keep path, never just the
                        // one-way "apply --write" hint (which would silently remove
                        // them, e.g. hand-added or foreign-manifest servers).
                        any_drift = true;
                        let prune_cmd = if foreign_key {
                            // apply's guard keeps foreign entries — pruning them
                            // takes the explicit flag.
                            "agentstack apply --prune-foreign".to_string()
                        } else {
                            fix_cmd.clone()
                        };
                        // Foreign entries are safe by default (apply keeps them),
                        // so that case is Info; a removal of entries THIS manifest
                        // wrote stays a warning.
                        report.line(
                            if foreign_key {
                                Level::Info
                            } else {
                                Level::Warn
                            },
                            format!(
                            "{:<14} would REMOVE {} ↳ keep them: agentstack adopt{scope_flag} · \
                         prune them: {prune_cmd}{scope_flag}",
                            desc.display,
                            plan.removed.join(", ")
                        ),
                        );
                    }
                }
            }
        }
        if fixed > 0 {
            state.save()?;
        }
        if !any_drift {
            report.line(Level::Ok, "all targets in sync");
        }
        // No servers declared and nothing drifting: there is nothing to fall out
        // of sync (any leftover prune/foreign findings above are warn lines, which
        // keep the section visible on their own).
        if manifest.servers.is_empty() && !any_drift {
            report.mark_irrelevant();
        }
    } // end else (drift comparison ran: not skip_drift, not zero-files)

    // Instruction fragments: the managed region of each CLAUDE.md / AGENTS.md
    // must match what the manifest would compile at SOME scope the project
    // actually uses (apply picks the scope per invocation), and every declared
    // fragment source must exist. A missing source is an error (`--ci` gates
    // it); a stale region is drift, so it warns.
    report.section("Instructions");
    if manifest.instructions.is_empty() {
        report.line(
            Level::Unchecked,
            "no instruction fragments declared — nothing to check",
        );
        // Codex quirk checks below may still append warn lines here — a
        // flagged section is shown regardless of relevance.
        report.mark_irrelevant();
    } else {
        // Provenance: fragments inherited from the machine-level manifest.
        let inherited = manifest
            .instructions
            .values()
            .filter(|i| i.from_user_layer)
            .count();
        if let (Some(up), true) = (&ctx.loaded.user_path, inherited > 0) {
            report.line(
                Level::Ok,
                format!(
                    "{} inherited from the machine manifest ({})",
                    super::count(inherited, "fragment"),
                    up.display()
                ),
            );
        }
        let mut instr_issues = 0;
        for id in &target_ids {
            let Some(desc) = ctx.registry.get(id) else {
                continue;
            };
            // W5: package instruction members are part of what compiles, so
            // doctor's drift/missing view has to see them too.
            let pinned = crate::lock::Lock::load(&ctx.dir).unwrap_or_default();
            let packages = crate::package::effective_members(&pinned);
            let global = crate::render::instructions::plan_instructions(
                manifest,
                desc,
                Scope::Global,
                &ctx.dir,
                packages,
            );
            let project = crate::render::instructions::plan_instructions(
                manifest,
                desc,
                Scope::Project,
                &ctx.dir,
                packages,
            );
            // Missing sources are scope-independent; the global plan sees
            // every declared fragment (project scope filters out inherited
            // machine-layer ones), so report from it alone.
            if let Some(plan) = &global {
                for m in &plan.missing {
                    instr_issues += 1;
                    report.line(
                        Level::Error,
                        format!("{:<14} fragment '{m}' source missing", desc.display),
                    );
                }
            }
            // Staleness: `apply`/`instructions` pick the scope per invocation,
            // so a project compiled at project scope must not warn forever
            // against a global file it never writes. Warn only when NO scope
            // that actually compiles fragments is in sync, naming the stale
            // scope(s).
            let mut stale_scopes: Vec<&str> = Vec::new();
            let mut in_sync = false;
            for (label, plan) in [("global", &global), ("project", &project)] {
                let Some(plan) = plan else { continue };
                if plan.fragments.is_empty() && plan.missing.is_empty() {
                    continue; // nothing compiles at this scope
                }
                if plan.changed() {
                    stale_scopes.push(label);
                } else {
                    in_sync = true;
                }
            }
            if !in_sync && !stale_scopes.is_empty() {
                instr_issues += 1;
                report.line(
                    Level::Warn,
                    format!(
                        "{:<14} managed region stale ({} scope) ↳ agentstack instructions --write",
                        desc.display,
                        stale_scopes.join("/")
                    ),
                );
            }
        }
        // A fragment that EXPLICITLY names a CLI with no instruction file (not
        // via `"*"`) reaches it nowhere — an authoring mistake worth a warning,
        // shared with the `instructions` command's per-fragment notice.
        for (frag, target) in crate::render::instructions::explicit_incapable_instruction_targets(
            manifest,
            &ctx.registry,
        ) {
            instr_issues += 1;
            report.line(
                Level::Warn,
                format!(
                    "instruction '{frag}' targets '{target}', which has no instructions file ↳ remove the target or use a supported CLI"
                ),
            );
        }
        if instr_issues == 0 {
            report.line(Level::Ok, "all instruction files match the manifest");
        }
    }
    // Codex-specific instruction/trust quirks, checked whenever codex is a
    // target. Codex's semantics, per its docs: AGENTS.override.md in a
    // directory silently wins over AGENTS.md; the EFFECTIVE
    // project_doc_max_bytes (configured, 32 KiB default) caps the COMBINED
    // instruction chain; and .codex/config.toml is ignored until the project
    // is trusted (projects.<path>.trust_level in ~/.codex/config.toml).
    // Truncation/ignoring is silent on Codex's side — doctor is the alarm.
    if target_ids.iter().any(|id| id == "codex") {
        let root = crate::manifest::project_root_of(&ctx.dir);
        if root.join("AGENTS.override.md").exists() && root.join("AGENTS.md").exists() {
            report.line(
                Level::Warn,
                "AGENTS.override.md exists beside AGENTS.md — Codex reads ONLY the override; the managed AGENTS.md is shadowed",
            );
        }
        if paths::expand_tilde("~/.codex/AGENTS.override.md").exists()
            && paths::expand_tilde("~/.codex/AGENTS.md").exists()
        {
            report.line(
                Level::Warn,
                "~/.codex/AGENTS.override.md exists beside ~/.codex/AGENTS.md — Codex reads ONLY the override; the managed global file is shadowed",
            );
        }
        let limit = codex_doc_limit(&root);
        let (chain_bytes, chain_files) = codex_instruction_chain(&root);
        if chain_bytes > limit {
            report.line(
                Level::Warn,
                format!(
                    "instruction chain for Codex is {} KiB across {} ({}) — over the effective project_doc_max_bytes ({} KiB); Codex truncates silently ↳ raise the limit or split fragments",
                    chain_bytes / 1024,
                    if chain_files.len() == 1 { "1 file".to_string() } else { format!("{} files", chain_files.len()) },
                    chain_files.join(", "),
                    limit / 1024
                ),
            );
        }
        // Project-scope render exists but Codex won't read it until trusted —
        // a healthy-looking render that silently does nothing.
        //
        // N1: severity depends on whether Codex is actually IN USE here. Codex
        // is detected by binary-on-PATH and lands in `targets.default`, so we
        // render `.codex/config.toml` for projects that never mentioned Codex
        // and whose owner may never open it — and as a warning this pinned the
        // whole product at `needs_attention` on any machine with Codex
        // installed, which is the primary persona. `codex_in_use` reads
        // whether the user has ever trusted ANY project in Codex: if they
        // have, the render silently doing nothing is a real problem worth a
        // warning; if they never have, it is a note about a tool they are not
        // using, and it must not gate readiness.
        if root.join(".codex/config.toml").exists() && !codex_project_trusted(&root) {
            report.line(
                if codex_in_use() {
                    Level::Warn
                } else {
                    Level::Advisory
                },
                format!(
                    "Codex will IGNORE {}/.codex/config.toml — the project is not trusted in ~/.codex/config.toml (projects.\"{}\".trust_level) ↳ open Codex in this folder once and accept the trust prompt",
                    tidy_path(&root),
                    root.display()
                ),
            );
        }
    }

    report.section("Quirks");
    let quirks = check_quirks(manifest);
    if quirks.is_empty() {
        report.line(Level::Ok, "no unsupported syntax for any target");
        if manifest.servers.is_empty() {
            report.mark_irrelevant();
        }
    }
    for q in quirks {
        let level = if q.advisory {
            Level::Advisory
        } else {
            Level::Warn
        };
        report.line(level, q.msg);
    }

    // Lifecycle hooks: the same staleness contract as instructions — the
    // rendered hooks key of each hook-capable target must match what the
    // manifest would compile (global scope, mirroring drift/fix).
    // Phase 3 item 5, kind convergence: `[settings.*]` was the one declared
    // kind with no doctor section at all — it rendered into native config and
    // `doctor` never mentioned it, so "check the setup" quietly meant "check
    // the setup except this part". The check is deliberately modest, matching
    // what settings actually are: values the user wrote, merged into a named
    // CLI's own file. There is nothing fetched to pin and nothing remote to
    // verify, so the honest checks are that the CLI is one we know and that
    // the value is an object we can merge.
    report.section("Settings");
    if manifest.settings.is_empty() {
        report.line(
            Level::Unchecked,
            "no CLI settings declared — nothing to check",
        );
        report.mark_irrelevant();
    } else {
        for (id, value) in &manifest.settings {
            match ctx.registry.get(id) {
                None => report.line(
                    Level::Warn,
                    format!(
                        "{id}: unknown CLI — these settings reach nothing ↳ agentstack doctor --all"
                    ),
                ),
                Some(desc) if !value.is_object() => report.line(
                    Level::Error,
                    format!(
                        "{:<14} settings must be a table of keys to merge, got {} ↳ edit [settings.{id}] in the manifest",
                        desc.display,
                        match value {
                            serde_json::Value::Array(_) => "a list",
                            serde_json::Value::Null => "nothing",
                            _ => "a single value",
                        }
                    ),
                ),
                Some(desc) => report.line(
                    Level::Ok,
                    format!(
                        "{:<14} {} merged into its settings",
                        desc.display,
                        super::count(value.as_object().map(|o| o.len()).unwrap_or(0), "key")
                    ),
                ),
            }
        }
    }

    report.section("Hooks");
    if manifest.hooks.is_empty() {
        report.line(Level::Unchecked, "no hooks declared — nothing to check");
        report.mark_irrelevant();
    } else {
        let machine_hooks = crate::commands::guard::machine_hooks_for_apply();
        let mut hook_issues = 0;
        let mut hook_capable = 0;
        for id in &target_ids {
            let Some(desc) = ctx.registry.get(id) else {
                continue;
            };
            if desc.hooks.is_none() {
                continue;
            }
            hook_capable += 1;
            let prev = !state
                .managed_hooks(&target_key(id, Scope::Global, &ctx.dir))
                .is_empty();
            match plan_hooks(
                manifest,
                desc,
                &ctx.resolver,
                prev,
                Scope::Global,
                &ctx.dir,
                &machine_hooks,
            ) {
                Ok(Some(hp)) if hp.changed() => {
                    hook_issues += 1;
                    report.line(
                        Level::Warn,
                        format!(
                            "{:<14} hooks stale ↳ agentstack apply --write",
                            desc.display
                        ),
                    );
                }
                Ok(_) => {}
                Err(e) => {
                    hook_issues += 1;
                    report.line(
                        Level::Error,
                        format!("{}: hooks plan failed — {e:#}", desc.display),
                    );
                }
            }
        }
        if hook_capable == 0 {
            report.line(
                Level::Warn,
                format!(
                    "{} defined but no selected target supports hooks",
                    super::count(manifest.hooks.len(), "hook")
                ),
            );
        } else if hook_issues == 0 {
            report.line(
                Level::Ok,
                format!(
                    "{} in sync across {}",
                    super::count(manifest.hooks.len(), "hook"),
                    super::count(hook_capable, "hook-capable target")
                ),
            );
        }
    }

    report.section("Skills");
    // The same name set a trust review covers — inline `[skills.*]` PLUS every
    // profile-referenced name (which may resolve from the central library), not
    // just inline entries. Counting inline-only made this section say "no skills
    // defined" while the Reproducibility section below listed a pinned
    // library skill the profile pulls in.
    let skill_names = super::trust::review_skill_names(manifest);
    if skill_names.is_empty() {
        report.line(Level::Unchecked, "no skills declared — nothing to check");
        // The broken-symlink sweep below still appends warn lines when a
        // detected adapter's skills dir is unhealthy — those keep it visible.
        report.mark_irrelevant();
    }
    let store = crate::store::Store::default_store();
    let skills_library = crate::library::Library::load_default().unwrap_or_default();
    let skills_lib_home = paths::lib_home();
    let skills_lock = crate::lock::Lock::load(&ctx.dir).unwrap_or_default();
    for name in &skill_names {
        // Inline definitions resolve straight to their local dir; a
        // profile-only name resolves through the central library (offline —
        // NoFetch, so a git body that isn't cached reports as not installed
        // rather than triggering a fetch).
        let dir = if let Some(skill) = manifest.skills.get(name) {
            crate::store::local_source_dir(
                &store,
                skill,
                &ctx.dir,
                skills_lock.get(name).and_then(|entry| entry.rev.as_deref()),
            )
        } else {
            crate::resolve::resolve_skill_with_pin(
                manifest,
                &ctx.dir,
                &skills_library,
                &skills_lib_home,
                &store,
                name,
                crate::resolve::ResolveMode::NoFetch,
                skills_lock.get(name).and_then(|entry| entry.rev.as_deref()),
            )
            .ok()
            .map(|r| r.path)
        };
        match dir {
            None => report.line(
                Level::Warn,
                format!("{name:<20} not installed ↳ agentstack install"),
            ),
            Some(dir) if !dir.join("SKILL.md").exists() => report.line(
                Level::Warn,
                format!("{name:<20} no SKILL.md in {}", dir.display()),
            ),
            // A described skill is a discoverable skill: search matching and
            // the loadable index an agent sees both come from this one line.
            Some(dir) if !crate::library::skill_has_description(&dir) => report.line(
                Level::Warn,
                format!(
                    "{name:<20} SKILL.md has no frontmatter description \
                     ↳ add `description:` so search and agents can find it"
                ),
            ),
            // F11: content drift against the pin, on the DEFAULT path. The
            // trust digest covers the manifest and lockfile bytes, not the
            // skill BODY — so editing an approved skill in place leaves
            // `trust::check` reading `trusted` and this section reading `ok`,
            // while the bytes an agent loads have changed under the approval.
            // The pin is what binds them, so doctor verifies it here rather
            // than leaving drift to surface only under `agentstack trust .`
            // (which nothing on a green screen recommends). Unpinned skills
            // are not drift — the first activation records the pin — so only
            // a real checksum mismatch flags.
            Some(dir)
                if skills_lock
                    .get(name)
                    .zip(crate::store::dir_digest(&dir).ok())
                    .is_some_and(|(entry, live)| entry.checksum.hex() != live.hex()) =>
            {
                report.line(
                    Level::Error,
                    format!(
                        "{name:<20} content changed since you approved it — the pinned bytes \
                         and the files on disk differ ↳ agentstack trust ."
                    ),
                );
            }
            Some(_) => report.line(Level::Ok, format!("{name:<20} present · SKILL.md ok")),
        }
    }
    // Name-contract sweep (design §C.3): entries that predate the contract
    // (or were hand-edited in) are diagnosed here — the dangerous operations
    // (add, pack parse, materialize) fail closed, doctor explains. Also flag
    // pairs that collide once case-folded: the index keeps them distinct but
    // a case-insensitive filesystem (macOS default) gives them one body dir.
    {
        let mut names: Vec<String> = skill_names.clone();
        names.extend(skills_library.skills.iter().map(|s| s.name.clone()));
        names.sort();
        names.dedup();
        let mut by_folded: std::collections::BTreeMap<String, Vec<&String>> =
            std::collections::BTreeMap::new();
        for name in &names {
            if crate::text::validate_name(name).is_err() {
                report.line(
                    Level::Warn,
                    format!(
                        "skill name '{}' violates the name contract (lowercase [a-z0-9._-], \
                         starts alphanumeric, ≤64 chars) ↳ rename it: remove and re-add \
                         under a conforming name",
                        name.escape_debug()
                    ),
                );
            }
            by_folded.entry(name.to_lowercase()).or_default().push(name);
        }
        for group in by_folded.values().filter(|g| g.len() > 1) {
            let list = group
                .iter()
                .map(|n| format!("'{}'", n.escape_debug()))
                .collect::<Vec<_>>()
                .join(", ");
            report.line(
                Level::Warn,
                format!(
                    "skill names {list} collide case-insensitively — on a case-insensitive \
                     filesystem they share one directory ↳ rename so only one remains"
                ),
            );
        }
    }
    // Stale staging leftovers from crashed `add skill` previews — harmless
    // (never reused; random ids) but worth naming with the remedy. Same for
    // `try` support dirs, which deliberately outlive their process.
    for (dir, what) in [
        ("stage", "crashed previews"),
        ("try", "ephemeral `try` support files"),
    ] {
        let root = paths::agentstack_home().join(dir);
        let stale = std::fs::read_dir(&root)
            .map(|entries| entries.count())
            .unwrap_or(0);
        if stale > 0 {
            report.line(
                Level::Warn,
                format!(
                    "{} under {} — {what}; remove with `rm -rf {}`",
                    super::count(stale, "leftover dir"),
                    tidy_path(&root),
                    root.display()
                ),
            );
        }
    }
    // Broken skill links on disk: a symlink in a detected CLI's skills dir
    // whose target is gone loads nothing, so name it here with the fix
    // instead of leaving the skill silently dead. Every detected adapter is
    // walked, not just the manifest's targets: the dead link breaks that CLI
    // regardless of what this project fans out to.
    for desc in ctx.registry.iter().filter(|d| d.detected()) {
        for scope in [Scope::Global, Scope::Project] {
            let Some(dir) = desc.skills_dir_for(scope, &ctx.dir) else {
                continue;
            };
            for sk in desc.discover_skills(scope, &ctx.dir) {
                if !sk.broken {
                    continue;
                }
                let entry = dir.join(&sk.name);
                let target = std::fs::read_link(&entry).unwrap_or_else(|_| sk.source.clone());
                report.line(
                    Level::Warn,
                    format!(
                        "{:<14} broken skill link '{}' → {} (target missing) \
                         ↳ remove it: rm {} · or reinstall the skill it points at",
                        desc.display,
                        sk.name,
                        target.display(),
                        entry.display()
                    ),
                );
            }
        }
    }

    // The central library is machine-global, so check ALL of it here, not
    // just the skills this project references: `lib add` warns at entry, but
    // pre-existing skills have no other surface that tells the user they're
    // undiscoverable. A skill without a frontmatter description only matches
    // search by name and shows as a bare name in every loadable index — warn
    // with the fix, don't block (it still works by name).
    let undescribed: Vec<&str> = skills_library
        .skills
        .iter()
        .filter(|entry| {
            // Only judge bodies that are locally readable — a git skill that
            // isn't cached yet is "not installed", not "undescribed".
            let readable = entry
                .body_dir(&skills_lib_home)
                .is_some_and(|dir| dir.join("SKILL.md").exists());
            readable
                && entry
                    .description(&skills_lib_home)
                    .is_none_or(|d| d.trim().is_empty())
        })
        .map(|entry| entry.name.as_str())
        .collect();
    if !undescribed.is_empty() {
        report.section("Central library");
        for name in undescribed {
            report.line(
                Level::Warn,
                format!(
                    "{name:<20} no frontmatter description \
                     ↳ add `description:` to its SKILL.md so search and agents can find it"
                ),
            );
        }
    }

    // Supply-chain content scan: hidden Unicode is an error so `--ci` gates it;
    // injection heuristics only warn. It reads every skill body, so the everyday
    // run skips it — `--deep` opts in and `--ci` (the trust gate) always
    // includes it. This is the only on-demand content re-scan surface (the
    // standalone `audit` verb was folded in here).
    report.section("Content scan");
    // Nothing with scannable content declared → nothing this could ever find.
    if skill_names.is_empty() && manifest.servers.is_empty() {
        report.mark_irrelevant();
    }
    if args.ci || args.deep {
        let mut flagged = 0usize;
        for unit in collect_content_units(manifest, &ctx.dir, &store) {
            for f in &unit.findings {
                flagged += 1;
                let level = match f.severity {
                    crate::scan::Severity::High => Level::Error,
                    crate::scan::Severity::Warn => Level::Warn,
                };
                report.line(level, format!("{:<20} {}", unit.name, f.describe()));
            }
        }
        if flagged == 0 {
            report.line(Level::Ok, "no hidden-unicode or injection findings");
        }
    } else {
        // Not `Level::Ok` (F19): a green ✓ over a scan that did not run claims
        // a clean result nobody checked. `Unchecked` renders the quiet `–`
        // marker at full contrast, so the reader sees "this was skipped" —
        // the same non-coverage honesty the skills section uses for
        // "nothing declared to check".
        report.line(
            Level::Unchecked,
            "not scanned (reads every skill body) ↳ agentstack doctor --deep — always on in --ci",
        );
    }

    // Reproducibility: profile skill refs resolve to the same content their
    // agentstack.lock pins. Central-library (and inline path) skills are checked
    // offline; git-backed refs are skipped (resolution would fetch).
    report.section("Reproducibility");
    // Anything lockable at all? Profiles (skill/server pins), instruction
    // fragments, extensions, and server executables are what the sub-checks
    // verify; with none declared this is definitionally a no-op section.
    if manifest.profiles.is_empty()
        && manifest.instructions.is_empty()
        && manifest.extensions.is_empty()
        && manifest.workflows.is_empty()
        && manifest.servers.is_empty()
    {
        report.mark_irrelevant();
    }
    check_reproducibility(manifest, &ctx.dir, &store, report);
    check_server_reproducibility(manifest, &ctx.dir, report);
    check_instruction_reproducibility(manifest, &ctx.dir, report);
    check_executable_integrity(manifest, &ctx.dir, report);
    check_extension_reproducibility(manifest, &ctx.dir, report);
    check_rendered_extensions(&ctx.dir, &ctx.registry, report);
    check_workflow_reproducibility(manifest, &ctx.dir, report);
    check_workflow_ceilings(manifest, report);

    // P9: when any pin drifted above, state the rule once — drift is an error
    // until you re-lock, and re-locking re-gates trust (new pins are new
    // consent). All the sub-checks append to this same section, so a scan of its
    // lines for the shared "drifted from lock" phrase tells us drift occurred.
    let drifted = report
        .sections
        .last()
        .is_some_and(|s| s.lines.iter().any(|(_, m)| m.contains("drifted from lock")));
    if drifted {
        report.line(
            Level::Warn,
            "lock drift is an error until you re-lock; `agentstack lock` re-pins and re-gates trust (new pins = new consent → re-run `agentstack trust .`)",
        );
    }

    // Project policy is optional; the machine layer applies either way, so the
    // "Policy" section shows whenever EITHER has something to report.
    let machine_policy = crate::machine_policy::inspect();
    // Machine-policy summary: one honest word (open / mixed / restrictive) for
    // how locked-down THIS machine's own firewall layer is. A machine that
    // HAS a policy file always sees this — "open" is the case most worth
    // stating out loud. A machine with no file at all ("unconfigured") and a
    // project declaring no [policy] hasn't used the feature, so the line joins
    // the hidden-by-default sections (progressive disclosure; `--all` shows
    // it). ("posture" is reserved for the per-run enforcement label — HOST /
    // ADVISORY etc. — so this section deliberately avoids it.)
    // Borrows `machine_policy`; the Policy section below still
    // moves it into `check_machine_policy`.
    report.section("Machine policy");
    let (posture, why) = classify_machine_posture(&machine_policy);
    report.line(Level::Ok, format!("{posture} — {why}"));
    if posture == "unconfigured" && manifest.policy.is_empty() {
        report.mark_irrelevant();
    }
    if !manifest.policy.is_empty() || machine_policy_reports(&machine_policy) {
        report.section("Policy");
        if !manifest.policy.is_empty() {
            check_policy(manifest, report);
        }
        check_machine_policy(&machine_policy, report);
        // The EFFECTIVE (machine ∩ project) ruleset, not just the project
        // layer — a machine-only deny must surface here too, same as it
        // would bite at apply/gateway time.
        check_effective_policy(manifest, report);
    }

    // Is this binary out of date? An [`Level::Advisory`], because it is true
    // and worth reading once but is not a defect in *this* setup — it must
    // never move the status chip off `ready` or become the "start with" fix.
    // Costs at most one short, cached network call a day, is silent when
    // offline, and does nothing at all under AGENTSTACK_NO_UPDATE_CHECK
    // (see `crate::update`). The section is only created when there is
    // something to say, so a current binary prints no header.
    if let Some(note) = crate::update::advisory() {
        report.section("Updates");
        report.line(Level::Advisory, note);
    }

    if args.live {
        report.section("MCP connectivity (--live)");
        // `--live` is the HTTP sibling of `--probe`, and it needs the same gate
        // for a sharper reason. It contacts a URL THE REPOSITORY DECLARES and
        // resolves that server's `${REF}` headers to do it — so on an untrusted
        // clone, the repo chooses the destination and we supply the
        // credentials. That is the exfiltration shape workspace invariant 3
        // exists to prevent ("untrusted repository content … cannot contact
        // servers or resolve secrets before the trust gate succeeds"), and the
        // check was simply missing here while `session start`, the gateway, and
        // now `--probe` all enforce it.
        //
        // Costs a trusted project nothing: `--live` is already opt-in, and a
        // project you have reviewed still probes exactly as before.
        let base = crate::manifest::project_root_of(&ctx.dir);
        let live_refusal = match crate::trust::check(&base) {
            crate::trust::TrustState::Trusted => None,
            crate::trust::TrustState::Changed => Some(
                "refusing to contact servers: the manifest or lockfile changed since this \
                 project was trusted ↳ agentstack trust",
            ),
            crate::trust::TrustState::Untrusted => Some(
                "refusing to contact servers: this project is not trusted — probing would \
                 reach URLs it declares, with its secrets resolved into the request \
                 ↳ agentstack trust",
            ),
        };
        let http: Vec<_> = if live_refusal.is_some() {
            Vec::new()
        } else {
            manifest
                .servers
                .iter()
                .filter(|(_, s)| s.server_type == ServerType::Http)
                .collect()
        };
        if let Some(msg) = live_refusal {
            report.line(Level::Warn, msg);
        } else if http.is_empty() {
            report.line(
                Level::Unchecked,
                "no HTTP servers declared — nothing to probe",
            );
        }
        for (name, server) in http {
            let Some(url) = &server.url else { continue };
            let url = resolve_str(url, &ctx.resolver);
            let headers = resolve_headers(server, &ctx.resolver);
            match crate::mcp::handshake(&url, &headers, std::time::Duration::from_secs(10)) {
                Ok(hs) => {
                    let tools = hs
                        .tool_count
                        .map(|n| format!("{n} tools"))
                        .unwrap_or_else(|| "handshake OK".into());
                    let who = hs.server_name.unwrap_or_else(|| name.clone());
                    report.line(Level::Ok, format!("{name:<14} {who} · {tools}"));
                }
                Err(e) => report.line(Level::Error, format!("{name:<14} {e}")),
            }
        }
    }

    if args.probe {
        probe_stdio_servers(manifest, &ctx, trust_state, report);
    }

    Ok(fixed)
}

/// Substitute `${REF}`s in a single string with resolved values (unresolved
/// refs are left in place).
fn resolve_str(s: &str, resolver: &dyn Resolver) -> String {
    let mut out = s.to_string();
    for name in crate::secret::refs_in(s) {
        if let Some(v) = resolver.resolve(&name) {
            out = out.replace(&format!("${{{name}}}"), &v);
        }
    }
    out
}

fn resolve_headers(
    server: &crate::manifest::Server,
    resolver: &dyn Resolver,
) -> indexmap::IndexMap<String, String> {
    server
        .headers
        .iter()
        .map(|(k, v)| (k.clone(), resolve_str(v, resolver)))
        .collect()
}

/// Like [`resolve_str`], but records the `${REF}`s that did NOT resolve on this
/// machine. `--probe` needs the names, not just "something is missing": a
/// server it refuses to start has to say which secret to set.
fn resolve_tracked(s: &str, resolver: &dyn Resolver, missing: &mut Vec<String>) -> String {
    let mut out = s.to_string();
    for name in crate::secret::refs_in(s) {
        match resolver.resolve(&name) {
            Some(v) => out = out.replace(&format!("${{{name}}}"), &v),
            None => missing.push(name),
        }
    }
    out
}

/// Hard wall on one stdio probe: spawn, `initialize`, and the best-effort
/// `tools/list` all share it. Matches the `--live` HTTP budget so the two
/// probes feel like one feature, and is generous enough for a cold `npx` that
/// has to unpack a package before it can say hello.
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// `--probe`: start every stdio server the manifest declares, speak MCP to it,
/// and stop it again — the only doctor check with side effects.
///
/// Two gates run before any process does. First trust: a session, a run, and a
/// render all refuse an untrusted project because untrusted repository content
/// must stay inert, and "spawn the command this repo's manifest names" is the
/// most direct way there is to break that. Second secrets: a server whose
/// `${REF}` does not resolve is reported as not-probeable rather than started
/// with a half-substituted environment, which would produce an auth failure
/// that blames the server for a missing secret.
fn probe_stdio_servers(
    manifest: &Manifest,
    ctx: &super::Context,
    trust_state: crate::trust::TrustState,
    report: &mut Report,
) {
    report.section("MCP server startup (--probe)");

    // Same fail-closed rule and the same voice as `session start` — the other
    // verb an external surface drives that materializes a runtime surface.
    // A warning rather than a hard exit: `doctor` is the diagnosis command, so
    // it finishes the report and names the one next step, and an untrusted
    // project is a choice this line has to be able to explain rather than
    // abort over.
    let refusal = match trust_state {
        crate::trust::TrustState::Trusted => None,
        crate::trust::TrustState::Changed => Some((
            "drifted",
            "refusing to probe: the manifest or lockfile changed since this project was trusted \
             ↳ agentstack trust",
        )),
        crate::trust::TrustState::Untrusted => Some((
            "untrusted",
            "refusing to probe: this project is not trusted — starting its servers would run \
             code nobody has reviewed ↳ agentstack trust",
        )),
    };
    if let Some((reason, msg)) = refusal {
        report.line(Level::Warn, msg);
        report.probe = Some(ProbeResults {
            ran: false,
            skipped_reason: Some(reason),
            servers: Vec::new(),
        });
        return;
    }

    let stdio: Vec<_> = manifest
        .servers
        .iter()
        .filter(|(_, s)| s.server_type == ServerType::Stdio)
        .collect();
    if stdio.is_empty() {
        report.line(
            Level::Unchecked,
            "no stdio servers declared — nothing to probe",
        );
    }

    // Ctrl-C would otherwise kill agentstack outright and orphan whatever
    // child is running: a probed server is its own process group leader, so
    // the terminal's SIGINT never reaches it. Intercepting the signal for the
    // duration of this section keeps `ProbeChild`'s Drop reachable — the
    // in-flight probe still ends inside its timeout and reaps its child — and
    // stops the loop before it starts another one. Best-effort: if the handler
    // won't install, the loop simply never sees an interrupt.
    let sigint = crate::sys::SigintGuard::install().ok();
    let interrupted = || {
        sigint
            .as_ref()
            .is_some_and(crate::sys::SigintGuard::interrupted)
    };

    let root = crate::manifest::project_root_of(&ctx.dir);
    let mut results = Vec::new();
    for (name, server) in stdio {
        if interrupted() {
            report.line(
                Level::Warn,
                "interrupted — the remaining servers were not probed",
            );
            break;
        }
        let Some(command) = &server.command else {
            continue;
        };
        let mut missing = Vec::new();
        let command = resolve_tracked(command, &ctx.resolver, &mut missing);
        let args: Vec<String> = server
            .args
            .iter()
            .map(|a| resolve_tracked(a, &ctx.resolver, &mut missing))
            .collect();
        let env: indexmap::IndexMap<String, String> = server
            .env
            .iter()
            .map(|(k, v)| (k.clone(), resolve_tracked(v, &ctx.resolver, &mut missing)))
            .collect();
        // Manifest `cwd` (relative paths anchor at the project root; `join`
        // keeps absolute ones), defaulting to the project root — the same
        // working directory a rendered config gives a harness, so the probe
        // reproduces the real launch rather than an approximation of it.
        let cwd = match &server.cwd {
            Some(c) => root.join(resolve_tracked(c, &ctx.resolver, &mut missing)),
            None => root.clone(),
        };

        if !missing.is_empty() {
            missing.sort();
            missing.dedup();
            let first = missing[0].clone();
            report.line(
                Level::Warn,
                format!(
                    "{name:<14} not probed — {} does not resolve ↳ agentstack secret set {first}",
                    missing.join(", ")
                ),
            );
            results.push(serde_json::json!({
                "server": name,
                "status": "not_probeable",
                "detail": format!(
                    "{}: {}",
                    if missing.len() == 1 {
                        "unresolved secret"
                    } else {
                        "unresolved secrets"
                    },
                    missing.join(", ")
                ),
            }));
            continue;
        }

        match crate::mcp::probe_stdio(&command, &args, &env, &cwd, PROBE_TIMEOUT) {
            Ok(p) => {
                let tools = p
                    .tool_count
                    .map(|n| format!("{n} tools"))
                    .unwrap_or_else(|| "handshake OK".into());
                let who = p.server_name.clone().unwrap_or_else(|| name.clone());
                let ms = p.elapsed.as_millis();
                report.line(
                    Level::Ok,
                    format!("{name:<14} started in {ms}ms · {who} · {tools}"),
                );
                results.push(serde_json::json!({
                    "server": name,
                    "status": "ok",
                    "server_name": p.server_name,
                    "protocol": p.protocol,
                    "tools": p.tool_count,
                    "elapsed_ms": ms as u64,
                }));
            }
            Err(e) => {
                // `e`'s Display already sanitized every child-supplied byte.
                let detail = e.to_string();
                report.line(Level::Error, format!("{name:<14} {detail}"));
                results.push(serde_json::json!({
                    "server": name,
                    "status": "failed",
                    "detail": detail,
                }));
            }
        }
    }

    report.probe = Some(ProbeResults {
        ran: true,
        skipped_reason: None,
        servers: results,
    });
}

/// Check that each profile's active skills resolve to the content their
/// `agentstack.lock` pins. Drift (checksum/rev mismatch) and broken refs are
/// errors so `doctor --ci` gates reproducibility; a library skill that is not
/// locked yet is a warning. Resolution is offline (`NoFetch`): a git source not
/// cached locally is reported, not fetched.
fn check_reproducibility(
    manifest: &Manifest,
    dir: &Path,
    store: &crate::store::Store,
    report: &mut Report,
) {
    use crate::resolve::{
        active_skill_names, skill_lock_status, ResolveMode, SkillLockStatus, SkillOrigin,
    };
    let lock = crate::lock::Lock::load(dir).unwrap_or_default();
    let library = crate::library::Library::load_default().unwrap_or_default();
    let lib_home = paths::lib_home();

    let mut seen = std::collections::BTreeSet::new();
    let mut emitted = 0usize;
    // Read once, outside the loop: standing answers are per-project state, and
    // re-reading the trust store per skill would be a lot of file I/O for a
    // status command that runs constantly.
    let standing: Vec<crate::trust::ItemDecision> =
        crate::trust::decisions_for(&crate::manifest::project_root_of(dir))
            .into_iter()
            .filter(|d| d.kind == "skill")
            .collect();
    for pname in manifest.profiles.keys() {
        for name in active_skill_names(manifest, pname) {
            if !seen.insert(name.clone()) {
                continue;
            }
            let r = skill_lock_status(
                &name,
                manifest,
                dir,
                &library,
                &lib_home,
                store,
                &lock,
                ResolveMode::NoFetch,
            );
            // A standing re-gate answer is reported as a STATE, once, with the
            // way out named — not as a question re-asked on every command.
            // Crucially it is reported ALONGSIDE the drift below, never
            // instead of it: keep-pinned resolves one consent moment, it does
            // not silence the fact that the live file and the delivered
            // version have diverged.
            match standing.iter().find(|d| d.name == *name).map(|d| &d.answer) {
                Some(crate::trust::Decision::Blocked) => {
                    report.line(
                        Level::Warn,
                        format!("{name:<20} blocked by you — not delivered ↳ agentstack trust"),
                    );
                    emitted += 1;
                }
                Some(crate::trust::Decision::KeepPinned { .. }) => {
                    report.line(
                        Level::Warn,
                        format!("{name:<20} using the version you approved ↳ agentstack trust"),
                    );
                    emitted += 1;
                }
                None => {}
            }
            match &r.status {
                SkillLockStatus::ResolveFailed { error } => {
                    report.line(Level::Error, format!("{name:<20} broken ref — {error}"));
                    emitted += 1;
                }
                SkillLockStatus::NotAvailableOffline { .. } => {
                    // Not a failure — a git body just isn't cached; can't verify
                    // reproducibility offline. Warn, never gate.
                    report.line(
                        Level::Warn,
                        format!("{name:<20} git-backed, not cached — not checked offline"),
                    );
                    emitted += 1;
                }
                SkillLockStatus::ChecksumDrift { .. } => {
                    report.line(
                        Level::Error,
                        format!("{name:<20} content drifted from lock ↳ agentstack lock"),
                    );
                    emitted += 1;
                }
                SkillLockStatus::RevDrift { locked, current } => {
                    report.line(
                        Level::Error,
                        format!("{name:<20} rev drifted: locked {locked}, now {current}"),
                    );
                    emitted += 1;
                }
                SkillLockStatus::MissingLockEntry => {
                    // Only nag for library skills; inline-unlocked skills are
                    // already covered by the Skills section above.
                    if r.origin == Some(SkillOrigin::Library) {
                        report.line(
                            Level::Warn,
                            format!("{name:<20} from library, not locked ↳ agentstack lock"),
                        );
                        emitted += 1;
                    }
                }
                SkillLockStatus::Matches => {
                    if r.origin == Some(SkillOrigin::Library) {
                        report.line(Level::Ok, format!("{name:<20} library · matches lock"));
                        emitted += 1;
                    }
                }
            }
        }
    }
    if emitted == 0 {
        report.line(
            Level::Unchecked,
            "reproducibility: nothing declared to check — no toolset here pulls a skill from the library",
        );
    }
}

/// Check that each project-declared instruction fragment's bytes match its
/// `agentstack.lock` pin. Drift and unreadable files are errors (`doctor --ci`
/// gates on them); an unpinned fragment is a warning. Machine-layer fragments
/// are the user's own content and are never pinned — skipped.
fn check_instruction_reproducibility(manifest: &Manifest, dir: &Path, report: &mut Report) {
    use crate::resolve::{instruction_lock_status, InstructionLockStatus};
    let lock = crate::lock::Lock::load(dir).unwrap_or_default();
    for (name, instr) in manifest
        .instructions
        .iter()
        .filter(|(_, i)| !i.from_user_layer)
    {
        match instruction_lock_status(name, instr, dir, &lock) {
            InstructionLockStatus::ResolveFailed { error } => report.line(
                Level::Error,
                format!("{name:<20} broken instruction ref — {error}"),
            ),
            InstructionLockStatus::ChecksumDrift { .. } => report.line(
                Level::Error,
                format!("{name:<20} instruction content drifted from lock ↳ agentstack lock"),
            ),
            InstructionLockStatus::MissingLockEntry => report.line(
                Level::Warn,
                format!("{name:<20} instruction not locked ↳ agentstack lock"),
            ),
            InstructionLockStatus::Matches => {
                report.line(Level::Ok, format!("{name:<20} instruction · matches lock"))
            }
        }
    }
}

/// Check that each profile's server refs resolve to the definition their
/// `agentstack.lock` pins. Definition drift and broken refs are errors (so
/// `doctor --ci` gates reproducibility); a library server not locked yet is a
/// warning. Only the definition digest is compared — never a resolved secret.
fn check_server_reproducibility(manifest: &Manifest, dir: &Path, report: &mut Report) {
    use crate::resolve::{server_lock_status, ServerLockStatus, ServerOrigin};
    let lock = crate::lock::Lock::load(dir).unwrap_or_default();
    let library = crate::library::Library::load_default().unwrap_or_default();
    let lib_home = paths::lib_home();

    let mut seen = std::collections::BTreeSet::new();
    for profile in manifest.profiles.values() {
        for name in &profile.servers {
            if !seen.insert(name.clone()) {
                continue;
            }
            let r = server_lock_status(name, manifest, &library, &lib_home, &lock);
            match &r.status {
                ServerLockStatus::ResolveFailed { error } => {
                    report.line(
                        Level::Error,
                        format!("{name:<20} broken server ref — {error}"),
                    );
                }
                ServerLockStatus::ChecksumDrift { .. } => report.line(
                    Level::Error,
                    format!("{name:<20} server definition drifted from lock ↳ agentstack lock"),
                ),
                ServerLockStatus::MissingLockEntry => {
                    if r.origin == Some(ServerOrigin::Library) {
                        report.line(
                            Level::Warn,
                            format!("{name:<20} library server, not locked ↳ agentstack lock"),
                        );
                    }
                }
                ServerLockStatus::Matches => {
                    if r.origin == Some(ServerOrigin::Library) {
                        report.line(
                            Level::Ok,
                            format!("{name:<20} library server · matches lock"),
                        );
                    }
                }
            }
        }
    }
}

/// Check that each declared native extension (D6/E3) resolves to the content
/// its `agentstack.lock` pins. Manifest-global (no profile refs), NoFetch
/// (offline), library-aware — mirrors `check_server_reproducibility`. Drift,
/// retarget, rev-drift, and broken refs are errors (so `doctor --ci` gates
/// reproducibility); an un-cached git source is a warning (can't verify
/// offline); a declared-but-unlocked extension is a warning too.
fn check_extension_reproducibility(manifest: &Manifest, dir: &Path, report: &mut Report) {
    use crate::resolve::ExtensionLockStatus;
    if manifest.extensions.is_empty() {
        return;
    }
    let lock = crate::lock::Lock::load(dir).unwrap_or_default();
    let library = crate::library::Library::load_default().unwrap_or_default();
    let lib_home = paths::lib_home();
    let store = crate::store::Store::default_store();
    for (name, ext) in &manifest.extensions {
        let report_ext = crate::resolve::extension_lock_status(
            name,
            ext,
            dir,
            &library,
            &lib_home,
            &store,
            &lock,
            crate::resolve::ResolveMode::NoFetch,
        );
        match report_ext.status {
            ExtensionLockStatus::ResolveFailed { error } => {
                report.line(Level::Error, format!("{name:<20} broken extension ref — {error}"));
            }
            ExtensionLockStatus::ChecksumDrift { .. } | ExtensionLockStatus::RevDrift { .. } => {
                report.line(
                    Level::Error,
                    format!("{name:<20} extension drifted from lock ↳ agentstack lock"),
                );
            }
            ExtensionLockStatus::TargetDrift { .. } => report.line(
                Level::Error,
                format!("{name:<20} extension retargeted since locked ↳ agentstack lock"),
            ),
            ExtensionLockStatus::MissingLockEntry => report.line(
                Level::Warn,
                format!("{name:<20} extension not locked ↳ agentstack lock"),
            ),
            ExtensionLockStatus::NotAvailableOffline { .. } => report.line(
                Level::Warn,
                format!("{name:<20} git extension not cached — can't verify offline ↳ agentstack install"),
            ),
            ExtensionLockStatus::Matches => {
                report.line(Level::Ok, format!("{name:<20} extension · matches lock"));
            }
        }
    }
}

/// Check that each declared governed workflow (D7 W1) resolves to the content,
/// role set, and rev its `agentstack.lock` pins. Drift — including a roles
/// change with unchanged bytes, reported distinctly — and broken sources are
/// errors (`doctor --ci` gates reproducibility); a declared-but-unlocked
/// workflow is a warning; an un-cached git source can't be verified offline
/// (info, not a failure — admission still requires a verified pin).
fn check_workflow_reproducibility(manifest: &Manifest, dir: &Path, report: &mut Report) {
    use crate::resolve::WorkflowLockStatus;
    if manifest.workflows.is_empty() {
        return;
    }
    let lock = crate::lock::Lock::load(dir).unwrap_or_default();
    let store = crate::store::Store::default_store();
    for (name, wf) in &manifest.workflows {
        match crate::resolve::workflow_lock_status(
            name,
            wf,
            dir,
            &store,
            &lock,
            crate::resolve::ResolveMode::NoFetch,
        ) {
            WorkflowLockStatus::ResolveFailed { error } => {
                report.line(Level::Error, format!("{name:<20} broken workflow ref — {error}"));
            }
            WorkflowLockStatus::ChecksumDrift { .. } | WorkflowLockStatus::RevDrift { .. } => {
                report.line(
                    Level::Error,
                    format!("{name:<20} workflow drifted from lock ↳ agentstack lock"),
                );
            }
            WorkflowLockStatus::RolesDrift { .. } => report.line(
                Level::Error,
                format!("{name:<20} workflow roles changed since locked ↳ agentstack lock"),
            ),
            WorkflowLockStatus::MissingLockEntry => report.line(
                Level::Warn,
                format!("{name:<20} workflow not locked ↳ agentstack lock"),
            ),
            WorkflowLockStatus::NotAvailableOffline { .. } => report.line(
                Level::Info,
                format!("{name:<20} git workflow not cached — can't verify offline ↳ agentstack install"),
            ),
            WorkflowLockStatus::Matches => {
                report.line(Level::Ok, format!("{name:<20} workflow · matches lock"));
            }
        }
    }
}

/// Flag workflow ceiling requests above the machine `[policy.workflows]` cap.
/// Admission clamps such a request to the cap (it can never take effect as
/// written — rule 2), so the manifest is stating an authority it will not
/// get: an error, fixed by lowering the request or raising the machine cap.
fn check_workflow_ceilings(manifest: &Manifest, report: &mut Report) {
    if manifest.workflows.is_empty() {
        return;
    }
    let machine = crate::machine_policy::inspect();
    let Some(machine_policy) = machine.policy else {
        // Blocked machine policy is reported by the Machine policy section;
        // nothing to compare against here.
        return;
    };
    let caps = &machine_policy.workflows;
    for (name, wf) in &manifest.workflows {
        if let (Some(req), Some(cap)) = (wf.max_agents, caps.max_agents) {
            if req > cap {
                report.line(
                    Level::Error,
                    format!("{name:<20} requests max_agents = {req} above the machine [policy.workflows] cap {cap} — admission clamps to {cap}; lower the request or raise the cap"),
                );
            }
        }
        if let (Some(req), Some(cap)) = (wf.max_wall_seconds, caps.max_wall_seconds) {
            if req > cap {
                report.line(
                    Level::Error,
                    format!("{name:<20} requests max_wall_seconds = {req} above the machine [policy.workflows] cap {cap} — admission clamps to {cap}; lower the request or raise the cap"),
                );
            }
        }
    }
}

/// Verify the rendered extension *copies* — the bytes a harness actually loads
/// — still match the pin they were rendered from (E3b, design doc §6). Distinct
/// from `check_extension_reproducibility`, which verifies the *source*: a
/// delivered copy can be tampered after render while its source stays clean, so
/// only checking the source would let doctored bytes reach the harness
/// unreviewed. Walks every governed extensions directory (each adapter with an
/// `extensions` surface, both scopes) using the ownership ledger:
///
/// - a ledger-owned artifact whose current digest no longer matches the pin it
///   was rendered from (or that has vanished) is an **error** naming the
///   extension — re-render with `agentstack apply`;
/// - a file agentstack's ledger does not own is a hand-installed extension: an
///   informational note only (never an error, never touched).
///
/// Read-only throughout; the digest is the same strict integrity-root walk the
/// pin used, so a copy and its source can never disagree spuriously.
fn check_rendered_extensions(dir: &Path, registry: &crate::adapter::Registry, report: &mut Report) {
    use crate::render::extensions::{managed_artifacts, GUARD_PREFIX};
    // Dedupe resolved dirs: an adapter's two scopes may resolve to the same
    // path, and we must audit each directory exactly once.
    let mut seen_dirs = std::collections::BTreeSet::new();
    for desc in registry.iter() {
        if desc.extensions.is_none() {
            continue;
        }
        for scope in [Scope::Global, Scope::Project] {
            let Some(ext_dir) = desc.extensions_dir_for(scope, dir) else {
                continue;
            };
            if !seen_dirs.insert(ext_dir.clone()) {
                continue;
            }
            let managed = match managed_artifacts(&ext_dir) {
                Ok(m) => m,
                Err(e) => {
                    report.line(
                        Level::Error,
                        format!("{:<20} unreadable extension ledger — {e:#}", desc.id),
                    );
                    continue;
                }
            };
            let owned: std::collections::BTreeSet<&str> =
                managed.iter().map(|m| m.filename.as_str()).collect();
            // Ledger-owned copies: bytes must still match the pin they were
            // rendered from. Compare to the ledger's recorded checksum (what
            // this exact copy was rendered from), so a shared global dir's
            // other-project artifacts verify without a project-scoped lock.
            for m in &managed {
                if m.checksum.is_empty() {
                    continue; // pre-checksum ledger entry: nothing to verify against
                }
                match agentstack_core::digest::integrity_root_digest(&ext_dir, &m.filename) {
                    // Bytes match the pin: healthy, nothing to act on. A pure
                    // digest confirmation is internal state, so it earns no
                    // line — only a drift or an unreadable copy (below) does.
                    Ok(current) if current.hex() == m.checksum => {}
                    Ok(_) => report.line(
                        Level::Error,
                        format!(
                            "{:<20} rendered extension copy drifted from its pin ↳ agentstack apply",
                            m.name
                        ),
                    ),
                    Err(_) => report.line(
                        Level::Error,
                        format!(
                            "{:<20} rendered extension copy missing or unreadable ↳ agentstack apply",
                            m.name
                        ),
                    ),
                }
            }
            // Non-ledger files: hand-installed extensions. Surfaced, never
            // touched. Guard artifacts are agentstack-managed elsewhere, so they
            // are not strangers.
            for disc in desc.discover_extensions(scope, dir) {
                if disc.name.starts_with(GUARD_PREFIX) || owned.contains(disc.name.as_str()) {
                    continue;
                }
                report.line(
                    Level::Ok,
                    format!(
                        "{:<20} unmanaged extension in {} — not placed by agentstack, left untouched",
                        disc.name, desc.id
                    ),
                );
            }
        }
    }
}

/// D3 (contract §8): compare each declared server's repository-local
/// executable surface — auto-detected stdio command/args files plus declared
/// integrity roots — to its `agentstack.lock` pins. Drift and underivable
/// surfaces (symlink, traversal, broken root) are errors so `doctor --ci`
/// gates them; executable-but-unpinned local code is a warning here (the
/// trust gate is what blocks it).
fn check_executable_integrity(manifest: &Manifest, dir: &Path, report: &mut Report) {
    use crate::executable::ExecutableLockStatus;
    let lock = crate::lock::Lock::load(dir).unwrap_or_default();
    let library = crate::library::Library::load_default().unwrap_or_default();
    let lib_home = paths::lib_home();
    // The effective runtime surface (inline + library, like the trust
    // preview), not just profile refs: any declared server's local code can
    // run once activated. Unresolvable servers are already reported by
    // check_server_reproducibility.
    let servers: Vec<(String, crate::manifest::Server)> =
        crate::resolve::effective_runtime_servers(manifest, &library, &lib_home, None)
            .into_iter()
            .filter_map(|(n, r)| r.ok().map(|r| (n, r.server)))
            .collect();
    for (label, status) in crate::executable::executable_lock_statuses(dir, &servers, &lock) {
        match status {
            ExecutableLockStatus::ResolveFailed { error } => {
                report.line(Level::Error, format!("{label} — {error}"));
            }
            ExecutableLockStatus::ChecksumDrift { .. } => report.line(
                Level::Error,
                format!("{label} content drifted from lock ↳ agentstack lock"),
            ),
            ExecutableLockStatus::MissingLockEntry => report.line(
                Level::Warn,
                format!("{label} executable local code not pinned ↳ agentstack lock"),
            ),
            ExecutableLockStatus::Matches => {}
        }
    }

    for (name, server) in &servers {
        if let Some(err) = missing_command_error(name, server) {
            report.line(Level::Error, err);
        }
    }
}

/// A stdio server whose ABSOLUTE command path no longer exists fails on every
/// CLI at startup ("ENOENT … posix_spawn"), and the cause is knowable right
/// here. The live case: an `owner`-synced app-bundled binary whose owning app
/// relocated itself (Codex.app → ChatGPT.app) — the owner's config has the
/// fresh path, one `apply --write` away. Relative and bare-name commands are
/// skipped: they resolve against a cwd or PATH the CLI controls, not knowable
/// statically. Pure, so it is unit-testable without a `Report`.
fn missing_command_error(name: &str, server: &crate::manifest::Server) -> Option<String> {
    if server.server_type != crate::manifest::ServerType::Stdio {
        return None;
    }
    let cmd = server.command.as_ref()?;
    if !Path::new(cmd).is_absolute() || cmd.contains("${") || Path::new(cmd).exists() {
        return None;
    }
    let hint = match &server.owner {
        Some(owner) => format!(
            "its owner ('{owner}') may have moved it ↳ agentstack apply --write refreshes from the owner's config"
        ),
        None => "fix the path in the manifest or remove the server".to_string(),
    };
    Some(format!(
        "server '{name}' command does not exist on this machine ({cmd}) — every CLI will fail it at startup; {hint}"
    ))
}

/// A machine policy deny keyed to a specific server *name* can be dodged: the
/// rule binds to the name the repo chose, so a repo that renames its server
/// escapes it. The `"*"` key is rename-proof — it constrains every server
/// whatever a manifest calls it. Returns one advisory per named deny that has
/// no identical `"*"` companion, for `dimension` (`"tools"`, `"egress"`, or
/// `"secrets"` — same keyed grammar on all three maps). Pure, so it is
/// unit-testable without a `Report`.
fn rename_dodgeable_denies(
    dimension: &str,
    map: &indexmap::IndexMap<String, Vec<String>>,
) -> Vec<String> {
    let wildcard_denies: std::collections::HashSet<&str> = map
        .get("*")
        .into_iter()
        .flatten()
        .filter_map(|r| r.strip_prefix('!'))
        .collect();
    let mut out = Vec::new();
    for (server, rules) in map {
        if server == "*" {
            continue;
        }
        for pat in rules.iter().filter_map(|r| r.strip_prefix('!')) {
            if !wildcard_denies.contains(pat) {
                out.push(format!(
                    "machine [policy.{dimension}] deny '!{pat}' on server '{server}' can be dodged if a repo renames its server — add '!{pat}' under the \"*\" key to make it rename-proof"
                ));
            }
        }
    }
    out
}

/// Enforce the `[policy]` block: required/forbidden capabilities + source
/// allowlist. Violations are errors (so `doctor --ci` fails).
fn check_policy(manifest: &Manifest, report: &mut Report) {
    let known =
        |name: &String| manifest.servers.contains_key(name) || manifest.skills.contains_key(name);

    for name in &manifest.policy.require {
        if known(name) {
            report.line(Level::Ok, format!("require '{name}' — present"));
        } else {
            report.line(Level::Error, format!("require '{name}' — MISSING"));
        }
    }
    for name in &manifest.policy.forbid {
        if known(name) {
            report.line(
                Level::Error,
                format!("forbid '{name}' — present (not allowed)"),
            );
        } else {
            report.line(Level::Ok, format!("forbid '{name}' — absent"));
        }
    }
    if !manifest.policy.allowed_sources.is_empty() {
        let mut bad = 0;
        for (name, skill) in &manifest.skills {
            let source = skill_source_label(skill);
            if !manifest.policy.source_allowed(&source) {
                bad += 1;
                report.line(
                    Level::Error,
                    format!("skill '{name}' source '{source}' not in allowed_sources"),
                );
            }
        }
        if bad == 0 {
            report.line(Level::Ok, "all skill sources within allowlist");
        }
    }
    // Every per-server-keyed policy dimension's rules must name a real
    // server — a typo'd key would silently firewall nothing. `"*"` is the
    // wildcard key (every server). Same check, same wording, across all
    // three dimensions — only the label and where it's enforced differ.
    check_named_policy_keys("tools", &manifest.policy.tools, manifest, report);
    check_named_policy_keys("egress", &manifest.policy.egress, manifest, report);
    check_named_policy_keys("secrets", &manifest.policy.secrets, manifest, report);
    // [policy.filesystem] scopes are bundle-global (not per-server). The
    // write scope is enforced in sandbox mode (the workspace mounts
    // read-only unless it covers the workspace root — deny-by-default);
    // host mode remains advisory, and read scopes stay informational while
    // the only mount is the whole workspace.
    if !manifest.policy.filesystem.read.is_empty() {
        report.line(
            Level::Ok,
            format!(
                "[policy.filesystem] read — {} — informational (the sandbox mounts one whole workspace)",
                super::count(manifest.policy.filesystem.read.len(), "scope")
            ),
        );
    }
    if !manifest.policy.filesystem.write.is_empty() {
        report.line(
            Level::Ok,
            format!(
                "[policy.filesystem] write — {} — enforced in sandbox mode (workspace mounts read-only unless covered); advisory in host mode",
                super::count(manifest.policy.filesystem.write.len(), "scope")
            ),
        );
    }
}

/// One per-server-keyed policy dimension's key-validation: every key in
/// `map` must be `"*"` or a real server in the manifest, else it silently
/// firewalls nothing (a typo'd server name). Only the typo case earns a
/// line — a valid key's allow/deny count is internal state, not a decision
/// the user must act on. `dimension` is the bare name (`"tools"`, `"egress"`,
/// `"secrets"`); the Error line names the bracketed `[policy.<dimension>]`
/// form to match how a maintainer would grep the manifest for it.
fn check_named_policy_keys(
    dimension: &str,
    map: &indexmap::IndexMap<String, Vec<String>>,
    manifest: &Manifest,
    report: &mut Report,
) {
    for server in map.keys() {
        if server != "*" && !manifest.servers.contains_key(server) {
            report.line(
                Level::Error,
                format!("[policy.{dimension}] '{server}' — no such server in the manifest"),
            );
        }
    }
}

/// Cross-check every manifest server against the EFFECTIVE (machine ∩
/// project) ruleset — the same artifact `apply` and the gateway consult —
/// and flag anything that will fail closed at apply/gateway time: a `${REF}`
/// the server uses but `[policy.secrets]` would deny it (Error, since it
/// will simply never resolve for this server), and an HTTP server's
/// declared URL host that `[policy.egress]` would refuse (Error). A host
/// hidden behind a `${REF}` can't be checked statically; that's only worth a
/// Warn when this particular server IS actually egress-constrained (an
/// unconstrained server passes regardless, so silence is correct there).
/// Whether a `${REF}` that RESOLVES is actually readable by the servers that
/// reference it, per the effective (machine ∩ project) `[policy.secrets]`.
enum RefVerdict {
    /// At least one referencing server may read it — or no server references
    /// it at all, in which case `[policy.secrets]` (a per-server dimension)
    /// has no opinion and silence is the correct answer.
    Usable,
    /// Every server that references it is refused: it resolves and nothing
    /// can read it.
    RefusedEverywhere,
    /// Refused for some referencing servers, allowed for others. Carries the
    /// refused count so the line can say how many without listing them (the
    /// Policy section names each one).
    RefusedSomewhere(usize),
}

/// Per-ref refusal tallies, computed once over the compiled ruleset rather
/// than per line — `ruleset_for` intersects the machine ceiling, and doing
/// that inside the render loop would be both slower and a second place for
/// the two sections to disagree.
struct SecretRefusal {
    /// ref name -> (servers referencing it, of those, servers refused)
    tallies: std::collections::HashMap<String, (usize, usize)>,
}

impl SecretRefusal {
    fn compute(manifest: &Manifest) -> Self {
        let mut tallies: std::collections::HashMap<String, (usize, usize)> =
            std::collections::HashMap::new();
        // A ruleset that will not compile is already an Error in the Policy
        // section; here it means "no verdict available", and the Secrets
        // section falls back to reporting resolution only. Claiming a refusal
        // we could not compute would be the same dishonesty in the mirror.
        let Ok(ruleset) = ruleset_for(manifest) else {
            return SecretRefusal { tallies };
        };
        for (name, server) in &manifest.servers {
            for r in server.referenced_secrets() {
                let entry = tallies.entry(r.clone()).or_insert((0, 0));
                entry.0 += 1;
                if ruleset.secret_decision(name, &r).is_err() {
                    entry.1 += 1;
                }
            }
        }
        SecretRefusal { tallies }
    }

    fn verdict(&self, reference: &str) -> RefVerdict {
        match self.tallies.get(reference) {
            Some(&(referencing, refused)) if refused > 0 && refused == referencing => {
                RefVerdict::RefusedEverywhere
            }
            Some(&(_, refused)) if refused > 0 => RefVerdict::RefusedSomewhere(refused),
            _ => RefVerdict::Usable,
        }
    }
}

fn check_effective_policy(manifest: &Manifest, report: &mut Report) {
    let ruleset = match ruleset_for(manifest) {
        Ok(ruleset) => ruleset,
        Err(error) => {
            report.line(
                Level::Error,
                format!("effective policy is BLOCKED — {error:#}"),
            );
            return;
        }
    };
    for (name, server) in &manifest.servers {
        for r in server.referenced_secrets() {
            if let Err(rule) = ruleset.secret_decision(name, &r) {
                report.line(
                    Level::Error,
                    format!(
                        "{name:<20} references ${{{r}}} but {rule} — will fail to resolve at apply/gateway time"
                    ),
                );
            }
        }
        if server.server_type != ServerType::Http {
            continue;
        }
        let Some(url) = &server.url else { continue };
        match declared_host(url) {
            Some(host) => {
                if let Err(rule) = ruleset.egress_decision(name, &host, None) {
                    report.line(
                        Level::Error,
                        format!(
                            "{name:<20} declared host '{host}' — {rule} — will be refused at apply/gateway time"
                        ),
                    );
                }
            }
            None if ruleset.egress_constrained(name) => {
                report.line(
                    Level::Warn,
                    format!(
                        "{name:<20} URL host is a ${{REF}} — cannot verify against [policy.egress] statically, and this server IS constrained by it"
                    ),
                );
            }
            None => {}
        }
    }
}

/// Whether the machine policy layer has anything for `doctor` to report — a
/// non-empty `[policy.tools]`/`[policy.egress]`/`[policy.secrets]`, or an
/// degraded/blocked machine-policy state that must be surfaced. Used
/// to decide whether the "Policy" section is warranted at all when the
/// project declares no policy of its own.
fn machine_policy_reports(machine: &crate::machine_policy::Inspection) -> bool {
    !matches!(machine.status, crate::machine_policy::Status::Unconfigured)
        || machine
            .policy
            .as_ref()
            .is_some_and(|policy| !policy.is_empty())
}

/// One machine-layer dimension's rename-dodge lint: one Warn per named deny
/// not mirrored under `"*"` (silent when the dimension is unused). The bare
/// rule-set count is internal state — the section's posture headline already
/// says whether the machine layer constrains anything — so only the
/// actionable rename-dodge advisory is emitted here.
fn report_machine_dimension(
    dimension: &str,
    map: &indexmap::IndexMap<String, Vec<String>>,
    report: &mut Report,
) {
    if map.is_empty() {
        return;
    }
    // Rename-dodge lint: a named-server deny escapes a repo that renames its
    // server; the "*" key is the rename-proof form.
    for advisory in rename_dodgeable_denies(dimension, map) {
        report.line(Level::Warn, advisory);
    }
}

/// Classify the machine policy layer's overall posture in one honest word, for
/// `doctor`. Deliberately simple — this is a one-line headline, not the section
/// detail below it:
///
/// - **unconfigured** — no machine manifest exists; this is a benign explicit
///   absence, not corruption.
/// - **degraded** — the source is unreadable and a validated last-known-good
///   policy is being enforced.
/// - **blocked** — both source and snapshot are unusable, so enforcement paths
///   refuse to proceed.
/// - **open** — the current machine manifest has an empty `[policy]`.
/// - **restrictive** — at least one dimension carries a rename-proof `"*"` rule
///   (tools/egress/secrets), or a `[policy.filesystem]` scope is set: the
///   firewall binds every server, whatever a repo renames it to.
/// - **mixed** — some machine policy, but only named-server rules, which a repo
///   can dodge by renaming its server (see the rename-dodge lint above).
///
/// Never overstates: a `"*"` rule earns "restrictive", not "locked down" — a
/// `"*"` allowlist can still be broad. Pure (takes a borrow, returns static
/// strings) so it is unit-testable without a `Report` or a real machine file.
fn classify_machine_posture(machine: &crate::machine_policy::Inspection) -> (&'static str, String) {
    match &machine.status {
        crate::machine_policy::Status::Unconfigured => {
            return (
                "unconfigured",
                "no machine policy file — projects use their own policy".into(),
            );
        }
        crate::machine_policy::Status::LastKnownGood { source_error, .. } => {
            return (
                "degraded",
                format!("enforcing last-known-good policy; source unreadable ({source_error})"),
            );
        }
        crate::machine_policy::Status::Blocked {
            source_error,
            snapshot_error,
        } => {
            return (
                "blocked",
                format!("source unreadable ({source_error}); snapshot unusable ({snapshot_error})"),
            );
        }
        crate::machine_policy::Status::Current { .. } => {}
    }
    let Some(policy) = machine.policy.as_ref() else {
        return ("blocked", "validated machine policy is unavailable".into());
    };
    let dims = [&policy.tools, &policy.egress, &policy.secrets];
    if dims.iter().all(|m| m.is_empty()) && policy.filesystem.is_empty() {
        return (
            "open",
            "machine [policy] is empty — nothing here narrows what a project may do".into(),
        );
    }
    let has_wildcard = dims.iter().any(|m| m.contains_key("*"));
    if has_wildcard || !policy.filesystem.is_empty() {
        (
            "restrictive",
            "a rename-proof \"*\" rule (or a filesystem scope) constrains every server".into(),
        )
    } else {
        (
            "mixed",
            "only named-server rules — a repo can dodge them by renaming its server".into(),
        )
    }
}

/// Diagnose the machine `[policy.tools]`/`[policy.egress]`/`[policy.secrets]`
/// layers. Runs regardless of whether the project declares its own
/// `[policy]` — the machine layer is independent and applies to every
/// project, so gating it behind a project policy would hide it exactly when
/// a machine-only firewall is the whole setup. Takes the pre-computed health
/// so the caller reads the machine manifest once.
fn check_machine_policy(machine: &crate::machine_policy::Inspection, report: &mut Report) {
    match &machine.status {
        crate::machine_policy::Status::Unconfigured => {}
        crate::machine_policy::Status::Current {
            source_digest,
            cache_error: Some(error),
            ..
        } => report.line(
            Level::Warn,
            format!("machine policy CURRENT — source {source_digest}; snapshot refresh failed ({error})"),
        ),
        // Healthy + in sync: nothing to act on. The digest-first line only
        // restated internal state, and `snapshot_synced == false` never
        // reaches here — that case always carries a `cache_error`, caught by
        // the Warn arm above — so silence here is honest, not a hidden drift.
        crate::machine_policy::Status::Current { .. } => {}
        crate::machine_policy::Status::LastKnownGood {
            source_error,
            source_digest,
        } => report.line(
            Level::Warn,
            format!("machine policy DEGRADED — enforcing last-known-good source {source_digest}; current source unreadable ({source_error})"),
        ),
        crate::machine_policy::Status::Blocked {
            source_error,
            snapshot_error,
        } => report.line(
            Level::Error,
            format!("machine policy BLOCKED — source: {source_error}; snapshot: {snapshot_error}"),
        ),
    }
    if let Some(policy) = &machine.policy {
        report_machine_dimension("tools", &policy.tools, report);
        report_machine_dimension("egress", &policy.egress, report);
        report_machine_dimension("secrets", &policy.secrets, report);
    }
}

/// Codex's effective `project_doc_max_bytes`: the project `.codex/config.toml`
/// layer wins over the global one — but ONLY for a trusted project, because
/// Codex ignores the whole untrusted project layer (an untrusted 64 KiB must
/// not mask truncation at the real 32 KiB). 32 KiB when nothing sets it.
/// Best-effort parses — a garbled config just yields the next layer; this
/// feeds a warning, not a gate.
fn codex_doc_limit(root: &Path) -> u64 {
    const DEFAULT: u64 = 32 * 1024;
    let read = |path: std::path::PathBuf| -> Option<u64> {
        let text = std::fs::read_to_string(path).ok()?;
        let value: toml::Value = toml::from_str(&text).ok()?;
        value
            .get("project_doc_max_bytes")?
            .as_integer()?
            .try_into()
            .ok()
    };
    let project = if codex_project_trusted(root) {
        read(root.join(".codex/config.toml"))
    } else {
        None
    };
    project
        .or_else(|| read(paths::expand_tilde("~/.codex/config.toml")))
        .unwrap_or(DEFAULT)
}

/// The instruction chain Codex reads for a session at the project root. At
/// every level — the global ~/.codex/ dir included — AGENTS.override.md wins
/// over AGENTS.md and only the first non-empty file counts. Returns total
/// bytes and the file names counted. Sessions started in subdirectories add
/// more chain levels — this is the floor, which is what a warning needs.
fn codex_instruction_chain(root: &Path) -> (u64, Vec<String>) {
    let mut total = 0u64;
    let mut files = Vec::new();
    let mut count = |path: std::path::PathBuf, label: &str| {
        if let Ok(meta) = std::fs::metadata(&path) {
            if meta.len() > 0 {
                total += meta.len();
                files.push(label.to_string());
                return true;
            }
        }
        false
    };
    // At EVERY level — the global ~/.codex/ dir included — the override wins
    // and only the first non-empty file counts.
    if !count(
        paths::expand_tilde("~/.codex/AGENTS.override.md"),
        "~/.codex/AGENTS.override.md",
    ) {
        count(
            paths::expand_tilde("~/.codex/AGENTS.md"),
            "~/.codex/AGENTS.md",
        );
    }
    if !count(root.join("AGENTS.override.md"), "AGENTS.override.md") {
        count(root.join("AGENTS.md"), "AGENTS.md");
    }
    (total, files)
}

/// Whether Codex trusts `root`: `projects."<canonical path>".trust_level ==
/// "trusted"` in the global ~/.codex/config.toml. Codex ignores a project's
/// .codex/ layer entirely until this is set (its gate, recorded when the user
/// accepts the first-run prompt in that folder).
/// Whether Codex is actually used on this machine, as opposed to merely
/// installed (N1). The signal is "has the user ever accepted Codex's own trust
/// prompt for any project" — that is a deliberate act inside Codex, so a
/// non-empty `projects` table means they really run it. Detection by
/// binary-on-PATH cannot tell the difference, and treating "installed" as
/// "used" is what made an unusable-config note gate every project's readiness.
fn codex_in_use() -> bool {
    let Ok(text) = std::fs::read_to_string(paths::expand_tilde("~/.codex/config.toml")) else {
        return false;
    };
    let Ok(value) = toml::from_str::<toml::Value>(&text) else {
        return false;
    };
    value
        .get("projects")
        .and_then(|p| p.as_table())
        .is_some_and(|t| !t.is_empty())
}

fn codex_project_trusted(root: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(paths::expand_tilde("~/.codex/config.toml")) else {
        return false;
    };
    let Ok(value) = toml::from_str::<toml::Value>(&text) else {
        return false;
    };
    let canonical = root
        .canonicalize()
        .unwrap_or_else(|_| root.to_path_buf())
        .display()
        .to_string();
    value
        .get("projects")
        .and_then(|p| p.get(&canonical))
        .and_then(|e| e.get("trust_level"))
        .and_then(|t| t.as_str())
        == Some("trusted")
}

// ── t3code (supervisor GUI) awareness ────────────────────────────────────────
// t3code (pingdotgg) is deliberately NOT a 14th adapter: it has no MCP config,
// instructions, skills, or hook surface of its own. It spawns the CLIs
// agentstack already governs (Claude Code, Codex, Cursor, OpenCode) and
// delegates config discovery to each CLI's native files, so everything
// agentstack renders applies to its sessions unchanged. Two things CAN quietly
// break that chain, and both are invisible from inside t3code: its Full-access
// default disables the providers' own approval prompts (leaving the guard hook
// as the only pre-tool-use gate), and a provider instance with a custom home
// escapes every global-scope artifact. This section states the chain and
// checks those two — adaptation happens entirely on our side; a t3code user
// installs or changes nothing.

/// t3code's base dir: `$T3CODE_HOME` (its own override) or `~/.t3`.
fn t3code_home() -> Option<std::path::PathBuf> {
    let home = std::env::var("T3CODE_HOME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| paths::expand_tilde("~/.t3"));
    home.is_dir().then_some(home)
}

/// What we read out of t3code's `userdata/settings.json`. Parsed defensively
/// (bundle-content rule: someone else's config is hostile input) — any
/// unexpected shape yields empty findings, never an error.
#[derive(Default, PartialEq, Debug)]
struct T3codeFindings {
    /// `(instance, driver, field, path)` — a custom home that global
    /// agentstack artifacts (MCP config, instructions, guard hooks) don't
    /// reach. `field` is `homePath` or `shadowHomePath`.
    overrides: Vec<(String, String, &'static str, String)>,
    /// Enabled drivers with no agentstack adapter — nothing observes them.
    ungoverned: Vec<String>,
    /// `(instance, shim path)` — instances whose binaryPath launches through
    /// the agentstack shim, so their sessions get per-run evidence.
    shimmed: Vec<(String, String)>,
}

/// t3code drivers whose sessions flow through an agentstack adapter.
const T3CODE_GOVERNED_DRIVERS: &[&str] = &["claudeAgent", "codex", "cursor", "opencode"];

fn t3code_findings(settings_json: &str, shims_dir: &Path) -> T3codeFindings {
    let mut out = T3codeFindings::default();
    let Ok(v) = serde_json::from_str::<serde_json::Value>(settings_json) else {
        return out;
    };
    let Some(instances) = v.get("providerInstances").and_then(|p| p.as_object()) else {
        return out;
    };
    for (name, inst) in instances {
        // Disabled instances never spawn sessions — nothing to warn about.
        if !inst
            .get("enabled")
            .and_then(|e| e.as_bool())
            .unwrap_or(true)
        {
            continue;
        }
        let driver = inst.get("driver").and_then(|d| d.as_str()).unwrap_or(name);
        if !T3CODE_GOVERNED_DRIVERS.contains(&driver) {
            out.ungoverned.push(driver.to_string());
            continue;
        }
        let Some(cfg) = inst.get("config").and_then(|c| c.as_object()) else {
            continue;
        };
        // A binaryPath under our shims dir means launches mint a per-run
        // identity — worth confirming out loud.
        if let Some(bin) = cfg.get("binaryPath").and_then(|b| b.as_str()) {
            let expanded = paths::expand_tilde(bin.trim());
            if !bin.trim().is_empty() && expanded.starts_with(shims_dir) {
                out.shimmed.push((name.clone(), bin.trim().to_string()));
            }
        }
        // homePath moves the CLI's whole config home (CLAUDE_CONFIG_DIR /
        // CODEX_HOME); shadowHomePath is Codex's auth-overlay variant.
        for field in ["homePath", "shadowHomePath"] {
            if let Some(p) = cfg.get(field).and_then(|p| p.as_str()) {
                if !p.trim().is_empty() {
                    out.overrides.push((
                        name.clone(),
                        driver.to_string(),
                        field,
                        p.trim().to_string(),
                    ));
                }
            }
        }
    }
    out
}

fn check_t3code(report: &mut Report) {
    report.section("t3code (supervisor)");
    let Some(home) = t3code_home() else {
        report.line(Level::Info, "not detected (ok unless you use it)");
        report.mark_irrelevant();
        return;
    };
    report.line(
        Level::Ok,
        format!(
            "detected ({}) — t3code sessions run CLIs agentstack already governs \
             (Claude Code, Codex, Cursor, OpenCode); nothing extra to render",
            home.display()
        ),
    );

    // Guard posture. t3code's Full-access default maps to bypassPermissions /
    // danger-full-access on the providers, so in those sessions the guard hook
    // is the only pre-tool-use gate left standing.
    let guard_enabled = matches!(
        crate::manifest::machine_guard_health(),
        Some(Ok(cfg)) if cfg.enabled()
    );
    if !guard_enabled {
        report.line(
            Level::Warn,
            "guard not enabled — t3code's Full-access mode disables the providers' own \
             approval prompts, so those sessions run with no pre-tool-use gate at all \
             ↳ agentstack guard install",
        );
    } else {
        let coverage = crate::commands::guard::coverage();
        let mut missing = 0;
        for (provider, prefix) in [
            ("Claude Code", "claude-code"),
            ("Codex", "codex"),
            ("Cursor", "cursor"),
            ("OpenCode", "opencode"),
        ] {
            let Some((_, detected, installed)) =
                coverage.iter().find(|(id, _, _)| id.starts_with(prefix))
            else {
                continue;
            };
            if *detected && !installed {
                missing += 1;
                report.line(
                    Level::Warn,
                    format!(
                        "{provider}: guard hook missing — t3code Full-access sessions on \
                         this provider run ungated ↳ agentstack guard install"
                    ),
                );
            }
        }
        if missing == 0 {
            report.line(
                Level::Ok,
                "guard hooks cover the detected providers — in t3code's Full-access mode \
                 the guard is the only remaining pre-tool-use gate",
            );
        }
    }

    // Provider-instance home overrides. A custom home relocates the CLI's
    // entire config surface, so global agentstack artifacts silently stop
    // applying to sessions of that instance.
    let settings = home.join("userdata").join("settings.json");
    if !settings.exists() {
        report.line(
            Level::Info,
            "no userdata/settings.json — provider-instance checks skipped",
        );
        return;
    }
    match crate::util::read_to_string_bounded(&settings, crate::util::MAX_CONFIG_BYTES) {
        Err(e) => report.line(
            Level::Info,
            format!("settings.json unreadable ({e:#}) — provider-instance checks skipped"),
        ),
        Ok(text) => {
            let shims = paths::agentstack_home().join("shims");
            let findings = t3code_findings(&text, &shims);
            for (instance, driver, field, path) in &findings.overrides {
                // A shadow home is an auth overlay that symlinks parts of the
                // real home — config MAY still resolve, so it gets the softer
                // "verify" voice instead of a flat "doesn't reach".
                let consequence = if *field == "shadowHomePath" {
                    "auth overlay — verify guard hooks and rendered config still resolve there"
                } else {
                    "global agentstack artifacts (MCP config, instructions, guard hooks) \
                     don't reach that home"
                };
                report.line(
                    Level::Warn,
                    format!(
                        "instance '{instance}' ({driver}): custom {field} {path} — {consequence}"
                    ),
                );
            }
            for driver in &findings.ungoverned {
                report.line(
                    Level::Info,
                    format!(
                        "provider '{driver}' has no agentstack adapter — its t3code \
                         sessions are unobserved"
                    ),
                );
            }
            for (instance, path) in &findings.shimmed {
                report.line(
                    Level::Ok,
                    format!(
                        "instance '{instance}' launches via the agentstack shim ({path}) — \
                         its sessions record per-run evidence"
                    ),
                );
            }
            if findings.overrides.is_empty() && findings.ungoverned.is_empty() {
                report.line(
                    Level::Ok,
                    "no provider-instance home overrides — global artifacts apply to \
                     every t3code session",
                );
            }
            if findings.shimmed.is_empty() {
                report.line(
                    Level::Info,
                    "sessions attribute to the global audit only — for per-run evidence: \
                     agentstack shim make <cli>, then point the instance's binary path at it",
                );
            }
        }
    }
}

/// A policy-matchable source label for a skill, e.g. `git:github.com/acme/repo`
/// or `path:./skills/x`.
fn skill_source_label(skill: &crate::manifest::Skill) -> String {
    match skill.source() {
        Ok(crate::manifest::SkillSource::Git { url, .. }) => format!("git:{}", git_host_path(&url)),
        Ok(crate::manifest::SkillSource::Path(p)) => format!("path:{p}"),
        Err(_) => "invalid".into(),
    }
}

/// Normalize a git URL to `host/owner/repo` for allowlist matching.
fn git_host_path(url: &str) -> String {
    let u = url.trim().trim_end_matches(".git");
    let u = u.splitn(2, "://").last().unwrap_or(u);
    // scp-style: git@github.com:owner/repo
    if let Some(rest) = u.strip_prefix("git@") {
        return rest.replacen(':', "/", 1);
    }
    u.to_string()
}

/// Interpreter/launcher commands that resolve through `PATH` and typically live
/// only under a version-manager dir the login shell adds (nvm, pyenv, …). A
/// GUI-launched harness (Claude Code.app, Claude Desktop, VS Code) inherits a
/// minimal `PATH` that may not contain them at all — or resolves them to the
/// wrong runtime version — so a bare invocation can fail to spawn.
const PATH_DEPENDENT_LAUNCHERS: &[&str] = &[
    "npx", "node", "uvx", "uv", "bunx", "bun", "deno", "python", "python3", "pipx", "pip", "ruby",
    "pnpm", "yarn", "npm",
];

/// POSIX/login shells. `command = "zsh", args = ["-lc", "exec … "]` is the
/// *recommended* fix (the login shell sources the version manager and repairs
/// `PATH`), so a shell command is never itself the fragile case.
const SHELL_COMMANDS: &[&str] = &["zsh", "bash", "sh", "fish"];

/// Tidy an absolute path for the report: fold away `.` and `..` segments, then
/// abbreviate `$HOME` to `~`. Purely lexical — no `canonicalize`, which would
/// hit the filesystem, fail on paths that do not exist yet, and rewrite
/// symlinked prefixes into something the user never typed.
///
/// Display-only. The JSON contract and every fix command keep the full path;
/// this exists so a diagnostic does not print `/home/me/proj/../.claude.json`
/// at someone who is trying to read it quickly.
fn tidy_path(path: &Path) -> String {
    let mut parts: Vec<std::ffi::OsString> = Vec::new();
    for comp in path.components() {
        match comp {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                let root = std::path::Component::RootDir.as_os_str();
                let up = std::path::Component::ParentDir.as_os_str();
                match parts.last().map(|p| p.as_os_str()) {
                    // Nothing above the root to climb into: `/..` is `/`.
                    Some(p) if p == root => {}
                    // A real segment to fold away.
                    Some(p) if p != up => {
                        parts.pop();
                    }
                    // A leading (or stacked) `..` in a relative path still
                    // means something — folding it would change the path.
                    _ => parts.push(comp.as_os_str().to_os_string()),
                }
            }
            other => parts.push(other.as_os_str().to_os_string()),
        }
    }
    let cleaned: std::path::PathBuf = parts.iter().collect();
    if let Some(home) = dirs::home_dir() {
        if let Ok(rel) = cleaned.strip_prefix(&home) {
            return format!("~/{}", rel.display());
        }
    }
    cleaned.display().to_string()
}

/// A quirk finding plus whether the user is expected to act on it.
#[derive(Debug)]
struct Quirk {
    /// True for ecosystem caveats (see [`Level::Advisory`]) rather than
    /// defects in this manifest. Advisory quirks are stated once and do not
    /// count against readiness.
    advisory: bool,
    msg: String,
}

impl Quirk {
    fn warn(msg: String) -> Self {
        Quirk {
            advisory: false,
            msg,
        }
    }

    fn advisory(msg: String) -> Self {
        Quirk {
            advisory: true,
            msg,
        }
    }
}

/// Is this a stdio server whose `command` is a bare, `PATH`-dependent launcher
/// — no path separator, not a tilde path, and in [`PATH_DEPENDENT_LAUNCHERS`]?
/// We only flag that known set (not every bare command) so intentional `PATH`
/// binaries with a stable install location don't produce false positives.
fn bare_launcher(server: &Server) -> Option<&str> {
    if server.server_type != ServerType::Stdio {
        return None;
    }
    let cmd = server.command.as_deref()?;
    // An explicit path (`/usr/local/bin/node`, `./bin/x`, `~/bin/x`) already
    // pins the binary, and a login shell is the recommended wrapper, not a bug.
    if cmd.contains('/') || cmd.starts_with('~') || SHELL_COMMANDS.contains(&cmd) {
        return None;
    }
    if !PATH_DEPENDENT_LAUNCHERS.contains(&cmd) {
        return None;
    }
    Some(cmd)
}

/// One advisory covering every bare-launcher server, not one per server.
/// Nearly every published MCP server ships as `npx -y …`, so per-server lines
/// scale with the size of a normal setup while saying the same thing N times.
fn bare_launcher_advisory(hits: &[(&str, &str)]) -> String {
    let servers = hits
        .iter()
        .map(|(name, cmd)| format!("{name} ({cmd})"))
        .collect::<Vec<_>>()
        .join(", ");
    let n = hits.len();
    let verb = if n == 1 { "uses" } else { "use" };
    let pronoun = if n == 1 { "it" } else { "them" };
    format!(
        "{} {verb} a bare launcher that resolves via PATH: {servers}. A GUI-launched \
         harness (Claude Code.app, Claude Desktop, VS Code) may inherit a minimal PATH and fail \
         to spawn {pronoun}. Terminal-launched CLIs are unaffected. To pin {pronoun}, use an absolute path \
         or a login-shell wrapper: command = \"zsh\", args = [\"-lc\", \"exec <launcher> …\"]",
        super::count(n, "server")
    )
}

/// Detect per-target syntax a CLI can't handle, before it breaks at runtime.
///
/// Everything here is a real finding, but they split two ways: a manifest that
/// says something a target cannot express is the user's to fix, while a bare
/// `npx` launcher is how the ecosystem ships and is only worth stating once.
fn check_quirks(manifest: &Manifest) -> Vec<Quirk> {
    let mut out = Vec::new();
    let mut bare = Vec::new();
    for (name, server) in &manifest.servers {
        if let Some(cmd) = bare_launcher(server) {
            bare.push((name.as_str(), cmd));
        }
        // Codex has no ${VAR:-default} expansion; flag it generally since the
        // manifest is meant to render to every target.
        for val in server
            .headers
            .values()
            .chain(server.env.values())
            .chain(server.url.iter())
        {
            if val.contains(":-") && val.contains("${") {
                out.push(Quirk::warn(format!(
                    "server '{name}': ${{VAR:-default}} syntax is unsupported by Codex"
                )));
                break;
            }
        }
        // stdio servers with http-only fields, or vice versa.
        if server.server_type == ServerType::Stdio && !server.headers.is_empty() {
            out.push(Quirk::warn(format!(
                "server '{name}': stdio transport ignores `headers`"
            )));
        }
        if server.server_type == ServerType::Http && server.command.is_some() {
            out.push(Quirk::warn(format!(
                "server '{name}': http transport ignores `command`"
            )));
        }
    }
    if !bare.is_empty() {
        out.push(Quirk::advisory(bare_launcher_advisory(&bare)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// F11 witness: `readiness` gains a coverage term. A project that declares
    /// nothing has no setup to be "ready" with — before this, a `version = 1`
    /// husk beside a leftover lockfile reported `ready`. The tamper is the
    /// coverage signal itself.
    #[test]
    fn readiness_is_not_ready_over_an_empty_manifest() {
        let mut r = Report::new();
        r.trust = Some("trusted");
        r.activation = Some("locked");
        r.declares_anything = Some(false);
        assert_eq!(r.readiness(), "empty");
        assert!(r.readiness_line().unwrap().starts_with("not ready"));

        // Same project, once it declares something: ready is now honest.
        r.declares_anything = Some(true);
        assert_eq!(r.readiness(), "ready");
        assert!(r.readiness_line().unwrap().starts_with("ready"));
    }

    /// F21 witness: a real error with no parseable `↳ fix` must still produce a
    /// real next action — never "nothing to repair" printed above a nonzero
    /// error count, and never a lateral hop to `status` (which names `doctor`
    /// right back, the A↔B dead end).
    #[test]
    fn an_error_without_a_fix_still_yields_a_real_next_action() {
        let mut r = Report::new();
        r.trust = Some("trusted");
        r.section("X");
        // An error line with no `↳` — first_fix() returns None for it.
        r.line(Level::Error, "something is wrong but there is no one-liner");
        let (cmd, _) = r.next_action();
        assert!(
            !cmd.contains("nothing to repair") && !cmd.contains("agentstack status"),
            "an error must not route to 'nothing to repair' or bounce to status: {cmd}"
        );
        assert!(cmd.contains("review the errors"), "{cmd}");

        // The clean terminal must not name `status` either (the mutual
        // referral); it names the next rung.
        let mut clean = Report::new();
        clean.trust = Some("trusted");
        clean.declares_anything = Some(true);
        clean.activation = Some("locked");
        let (cmd, _) = clean.next_action();
        assert!(
            !cmd.contains("agentstack status"),
            "the clean terminal must not bounce back to status: {cmd}"
        );
    }

    /// `status` and `doctor` must not name different "one next actions" for
    /// the same project. A drifted project that also carries an earlier
    /// section's warning fix used to hear the warning's command here and
    /// `agentstack trust .` from `status`; consent now outranks warning-level
    /// repairs on both surfaces. Below trust, section order still decides.
    #[test]
    fn a_pending_review_outranks_an_earlier_sections_warning_fix() {
        let mut r = Report::new();
        r.trust = Some("drifted");
        // Registered FIRST, so `first_fix` would pick it under the old order.
        r.section("T3 Code");
        r.line(
            Level::Warn,
            "the guard is missing\n  ↳ agentstack guard install",
        );
        r.section("Trust");
        r.line(Level::Warn, "the content changed since you said yes");
        let (cmd, _) = r.next_action();
        assert_eq!(cmd, "agentstack trust .", "the review must lead: {cmd}");

        // Same findings, review settled: the warning fix wins again, and the
        // section-order tie-break below trust is untouched.
        r.trust = Some("trusted");
        let (cmd, _) = r.next_action();
        assert_eq!(cmd, "agentstack guard install", "{cmd}");

        // An error still outranks the review: its command blocks the rest.
        r.trust = Some("drifted");
        r.section("Manifest");
        r.line(Level::Error, "the manifest is invalid\n  ↳ agentstack init");
        let (cmd, _) = r.next_action();
        assert_eq!(cmd, "agentstack init", "{cmd}");
    }

    #[test]
    fn t3code_findings_flags_overrides_and_ungoverned_defensively() {
        let json = r#"{
          "providerInstances": {
            "claudeAgent": {"driver":"claudeAgent","enabled":true,"config":{"homePath":"/custom/claude","binaryPath":"/shims/claude"}},
            "codex-alt":   {"driver":"codex","enabled":true,"config":{"shadowHomePath":"/overlay/codex","binaryPath":"/usr/local/bin/codex"}},
            "cursor":      {"driver":"cursor","enabled":false,"config":{"homePath":"/ignored"}},
            "grok":        {"driver":"grok","enabled":true,"config":{}}
          }
        }"#;
        let shims = std::path::PathBuf::from("/shims");
        let f = t3code_findings(json, &shims);
        // Enabled overrides surface with their field; the disabled cursor
        // instance is skipped (it never spawns sessions).
        assert_eq!(f.overrides.len(), 2);
        assert!(f.overrides.iter().any(|(i, d, field, p)| {
            i == "claudeAgent"
                && d == "claudeAgent"
                && *field == "homePath"
                && p == "/custom/claude"
        }));
        assert!(f.overrides.iter().any(|(i, _, field, p)| i == "codex-alt"
            && *field == "shadowHomePath"
            && p == "/overlay/codex"));
        // An enabled driver with no adapter is reported, not ignored.
        assert_eq!(f.ungoverned, vec!["grok".to_string()]);
        // A binaryPath under the shims dir is confirmed; an ordinary one is not.
        assert_eq!(
            f.shimmed,
            vec![("claudeAgent".to_string(), "/shims/claude".to_string())]
        );
        // Hostile-input rule: garbage never errors, it yields empty findings.
        assert_eq!(
            t3code_findings("not json", &shims),
            T3codeFindings::default()
        );
        assert_eq!(t3code_findings("{}", &shims), T3codeFindings::default());
        assert_eq!(
            t3code_findings(r#"{"providerInstances": 7}"#, &shims),
            T3codeFindings::default()
        );
    }
    use assert_fs::prelude::*;

    /// DX witness for the closing triage line: `first_fix` returns the fix
    /// from the first ERROR carrying a `↳` hint, falls back to the first
    /// warning, and Info lines never count as findings.
    #[test]
    fn first_fix_prefers_errors_and_ignores_info() {
        let mut r = Report::new();
        r.section("A");
        r.line(Level::Info, "cursor not detected ↳ not a real fix");
        r.line(Level::Warn, "drift pending ↳ agentstack apply --write");
        r.section("B");
        r.line(
            Level::Error,
            "TOKEN not found ↳ agentstack secret set TOKEN",
        );
        // The error's fix wins even though the warning came first.
        assert_eq!(r.first_fix(), Some("agentstack secret set TOKEN"));

        let mut warn_only = Report::new();
        warn_only.section("A");
        warn_only.line(Level::Info, "context ↳ never surfaces");
        warn_only.line(Level::Warn, "drift ↳ agentstack apply --write");
        assert_eq!(warn_only.first_fix(), Some("agentstack apply --write"));
        // Info lines count in neither total.
        assert_eq!((warn_only.errors, warn_only.warnings), (0, 1));

        // T3: an error WITHOUT a fix never falls through to a warning's fix —
        // the error usually blocks that very command (an invalid manifest
        // refuses `apply --write`). No triage line beats a misleading one.
        let mut fixless_error = Report::new();
        fixless_error.section("A");
        fixless_error.line(Level::Warn, "drift ↳ agentstack apply --write");
        fixless_error.line(Level::Error, "manifest invalid, no hint");
        assert_eq!(fixless_error.first_fix(), None);
    }

    /// Build a `[policy.<dimension>]`-shaped map from `(server, patterns)`
    /// entries — the same keyed grammar underlies tools/egress/secrets.
    fn rules_map(entries: &[(&str, &[&str])]) -> indexmap::IndexMap<String, Vec<String>> {
        entries
            .iter()
            .map(|(k, v)| (k.to_string(), v.iter().map(|s| s.to_string()).collect()))
            .collect()
    }

    /// The rendered-artifact audit: a delivered copy whose bytes drifted from
    /// the pin (source left clean) is an error naming the extension, while a
    /// hand-installed file is an informational note only — never an error, never
    /// touched. HOME + AGENTSTACK_HOME are redirected to temps so the global
    /// scope resolves into empty temp dirs, keeping the check off the real
    /// machine's extension directories.
    #[test]
    fn rendered_extension_drift_is_error_and_stranger_is_a_note() {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let orig_home = std::env::var_os("HOME");
        let home = assert_fs::TempDir::new().unwrap();
        let ast_home = assert_fs::TempDir::new().unwrap();
        let proj = assert_fs::TempDir::new().unwrap();
        std::env::set_var("HOME", home.path());
        std::env::set_var("AGENTSTACK_HOME", ast_home.path());

        const TOML: &str = "version = 1\n[extensions.checkpoint]\npath = \"./extensions/checkpoint\"\ntarget = \"pi\"\n";
        proj.child("extensions/checkpoint/index.ts")
            .write_str("export default (pi) => {}\n")
            .unwrap();
        proj.child("agentstack.toml").write_str(TOML).unwrap();
        let manifest: Manifest = toml::from_str(TOML).unwrap();
        let registry = crate::adapter::Registry::load().unwrap();

        // Pin + trust + render, so a ledger and a rendered copy exist.
        crate::commands::lock::record_extension_pins(
            proj.path(),
            &manifest,
            &crate::library::Library::default(),
            &crate::util::paths::lib_home(),
            &crate::store::Store::default_store(),
        )
        .unwrap();
        crate::trust::trust_unreviewed(proj.path()).unwrap();
        crate::render::extensions::render(&manifest, &registry, Scope::Project, proj.path(), true)
            .unwrap();
        let ext_dir = proj.path().join(".pi/extensions");
        assert!(ext_dir.join("checkpoint/index.ts").exists());

        // Clean: the rendered copy matches its pin — no error.
        let mut clean = Report::new();
        check_rendered_extensions(proj.path(), &registry, &mut clean);
        assert_eq!(clean.errors, 0, "a matching rendered copy is not an error");

        // Tamper the delivered COPY (its source stays clean) and plant a
        // hand-installed stranger file.
        std::fs::write(
            ext_dir.join("checkpoint/index.ts"),
            b"export default (pi) => { evil() }\n",
        )
        .unwrap();
        std::fs::write(ext_dir.join("stranger.js"), b"// hand-installed\n").unwrap();

        let mut report = Report::new();
        check_rendered_extensions(proj.path(), &registry, &mut report);
        let text = report.to_json().to_string();

        match orig_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        std::env::remove_var("AGENTSTACK_HOME");

        assert!(report.errors >= 1, "drifted rendered copy must be an error");
        assert!(
            text.contains("checkpoint") && text.contains("drifted"),
            "drift error must name the extension: {text}"
        );
        assert!(
            text.contains("stranger.js") && text.contains("unmanaged"),
            "stranger file must be surfaced as a note: {text}"
        );
        assert!(
            ext_dir.join("stranger.js").exists(),
            "the stranger file is never touched"
        );
    }

    #[test]
    fn rename_dodge_lint_flags_named_deny_without_wildcard() {
        // A named-server deny with no "*" companion is dodgeable.
        let m = rules_map(&[("github", &["!delete_*"])]);
        let out = rename_dodgeable_denies("tools", &m);
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].contains("!delete_*") && out[0].contains("github"));
    }

    #[test]
    fn rename_dodge_lint_silent_when_wildcard_covers_it() {
        // The identical deny under "*" makes the named one rename-proof.
        let m = rules_map(&[("*", &["!delete_*"]), ("github", &["!delete_*"])]);
        assert!(rename_dodgeable_denies("tools", &m).is_empty());
    }

    #[test]
    fn rename_dodge_lint_silent_for_wildcard_only_and_for_allows() {
        // Wildcard-only deny: nothing to dodge.
        assert!(rename_dodgeable_denies("tools", &rules_map(&[("*", &["!delete_*"])])).is_empty());
        // Allow-only named rule: no deny to dodge.
        assert!(rename_dodgeable_denies("tools", &rules_map(&[("github", &["get_*"])])).is_empty());
    }

    #[test]
    fn rename_dodge_lint_flags_only_the_uncovered_deny() {
        // "*" covers !delete_* but not !post_* → exactly one advisory.
        let m = rules_map(&[("github", &["!delete_*", "!post_*"]), ("*", &["!delete_*"])]);
        let out = rename_dodgeable_denies("tools", &m);
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].contains("!post_*"));
    }

    #[test]
    fn rename_dodge_lint_covers_egress_and_secrets_dimensions() {
        // Same lint, generalized: a named-server deny under [policy.egress]
        // or [policy.secrets] is just as dodgeable, and the advisory names
        // the dimension it came from.
        let egress = rules_map(&[("figma", &["!evil.example"])]);
        let out = rename_dodgeable_denies("egress", &egress);
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].contains("[policy.egress]") && out[0].contains("figma"));

        let secrets = rules_map(&[("figma", &["!EVIL_*"])]);
        let out = rename_dodgeable_denies("secrets", &secrets);
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].contains("[policy.secrets]") && out[0].contains("EVIL_*"));
    }

    /// Machine-policy posture classification: the simple open / mixed /
    /// restrictive heuristic, and its honest handling of the no-file and
    /// unreadable cases (both fail open).
    #[test]
    fn machine_posture_classification() {
        let policy = |toml_body: &str| -> crate::manifest::Policy {
            let m: Manifest = toml::from_str(&format!("version = 1\n{toml_body}")).unwrap();
            m.policy
        };

        let current = |policy| crate::machine_policy::Inspection {
            policy: Some(policy),
            status: crate::machine_policy::Status::Current {
                source_digest: "a".repeat(64),
                snapshot_synced: true,
                cache_error: None,
            },
        };
        // No machine file at all → benign but explicit unconfigured state.
        let unconfigured = crate::machine_policy::Inspection {
            policy: Some(Default::default()),
            status: crate::machine_policy::Status::Unconfigured,
        };
        assert_eq!(classify_machine_posture(&unconfigured).0, "unconfigured");
        // Unreadable machine file without a snapshot → blocked, never open.
        let blocked = crate::machine_policy::Inspection {
            policy: None,
            status: crate::machine_policy::Status::Blocked {
                source_error: "boom".into(),
                snapshot_error: "missing".into(),
            },
        };
        assert_eq!(classify_machine_posture(&blocked).0, "blocked");
        // Present but empty [policy] → open.
        assert_eq!(classify_machine_posture(&current(policy(""))).0, "open");
        // Only a named-server rule → mixed (a repo can rename its server).
        assert_eq!(
            classify_machine_posture(&current(policy(
                "[policy.tools]\ngithub = [\"!delete_*\"]\n"
            )))
            .0,
            "mixed"
        );
        // A rename-proof "*" rule → restrictive.
        assert_eq!(
            classify_machine_posture(&current(policy("[policy.egress]\n\"*\" = [\"!*\"]\n"))).0,
            "restrictive"
        );
        // A filesystem scope alone → restrictive (bundle-global, no server key).
        assert_eq!(
            classify_machine_posture(&current(policy(
                "[policy.filesystem]\nwrite = [\"./**\"]\n"
            )))
            .0,
            "restrictive"
        );
    }

    /// Flatten a `Report`'s lines (across every section) into `(tag, msg)`
    /// pairs for assertions — the sections themselves aren't the point in
    /// these unit tests, just what got reported.
    fn report_lines(report: &Report) -> Vec<(&str, &str)> {
        report
            .sections
            .iter()
            .flat_map(|s| s.lines.iter().map(|(l, m)| (*l, m.as_str())))
            .collect()
    }

    /// [policy.egress] and [policy.secrets] keys are checked the same way as
    /// [policy.tools]: a key must be `"*"` or a real server, else it's a
    /// typo that silently firewalls nothing.
    #[test]
    fn check_named_policy_keys_flags_unknown_server() {
        let manifest: Manifest = toml::from_str(
            "version = 1\n[servers.known]\ntype = \"http\"\nurl = \"https://example.com\"\n",
        )
        .unwrap();
        let mut report = Report::new();
        check_named_policy_keys(
            "egress",
            &rules_map(&[("known", &["api.example"]), ("ghost", &["!evil.example"])]),
            &manifest,
            &mut report,
        );
        let lines = report_lines(&report);
        // A valid key is silent now — only the typo'd key earns a line.
        assert!(!lines.iter().any(|(_, m)| m.contains("known")));
        assert!(lines.iter().any(|(l, m)| *l == "error"
            && m.contains("[policy.egress]")
            && m.contains("ghost")
            && m.contains("no such server")));
    }

    /// [policy.filesystem] scopes are surfaced with honest enforcement
    /// labels: the write scope is enforced by the sandbox's workspace mount
    /// (advisory in host mode); read scopes are informational while the only
    /// mount is the whole workspace.
    #[test]
    fn filesystem_scopes_reported_with_honest_enforcement_labels() {
        let manifest: Manifest = toml::from_str(
            "version = 1\n[policy.filesystem]\nread = [\"/tmp/**\"]\nwrite = [\"/tmp/out/**\"]\n",
        )
        .unwrap();
        let mut report = Report::new();
        check_policy(&manifest, &mut report);
        let lines = report_lines(&report);
        assert!(lines
            .iter()
            .any(|(l, m)| *l == "ok" && m.contains("read") && m.contains("informational")));
        assert!(lines.iter().any(|(l, m)| *l == "ok"
            && m.contains("write")
            && m.contains("enforced in sandbox mode")
            && m.contains("advisory in host mode")));
    }

    /// The EFFECTIVE (machine ∩ project) ruleset cross-check: a server's own
    /// `${REF}` is flagged when the compiled ruleset would deny it for THAT
    /// server — the same decision `apply`/the gateway make, surfaced before
    /// either runs. AGENTSTACK_HOME points at an empty dir so no ambient
    /// machine policy on the test machine leaks in.
    #[test]
    fn effective_policy_flags_secret_ref_denied_by_project_policy() {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", home.path());
        let manifest: Manifest = toml::from_str(
            "version = 1\n[servers.figma]\ntype = \"stdio\"\ncommand = \"figma-mcp\"\n\
             env = { TOKEN = \"${FIGMA_TOKEN}\" }\n\
             [policy.secrets]\nfigma = [\"!FIGMA_TOKEN\"]\n",
        )
        .unwrap();
        let mut report = Report::new();
        check_effective_policy(&manifest, &mut report);
        std::env::remove_var("AGENTSTACK_HOME");
        let lines = report_lines(&report);
        assert!(
            lines.iter().any(|(l, m)| *l == "error"
                && m.contains("figma")
                && m.contains("FIGMA_TOKEN")
                && m.contains("[policy.secrets]")),
            "{lines:?}"
        );
    }

    /// Same cross-check, the egress side: an HTTP server's declared host
    /// fails the effective [policy.egress].
    #[test]
    fn effective_policy_flags_denied_declared_host() {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", home.path());
        let manifest: Manifest = toml::from_str(
            "version = 1\n[servers.sneaky]\ntype = \"http\"\nurl = \"https://evil.example/mcp\"\n\
             [policy.egress]\nsneaky = [\"!evil.example\"]\n",
        )
        .unwrap();
        let mut report = Report::new();
        check_effective_policy(&manifest, &mut report);
        std::env::remove_var("AGENTSTACK_HOME");
        let lines = report_lines(&report);
        assert!(
            lines.iter().any(|(l, m)| *l == "error"
                && m.contains("sneaky")
                && m.contains("evil.example")
                && m.contains("[policy.egress]")),
            "{lines:?}"
        );
    }

    /// A declared URL host hidden behind a `${REF}` can't be verified
    /// statically. That's silent for a server no egress rule constrains
    /// (allow-by-default), but worth a Warn once a rule DOES name the
    /// server — the doctor run can't promise the host is fine either way.
    #[test]
    fn effective_policy_warns_on_unverifiable_host_only_when_constrained() {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", home.path());

        let constrained: Manifest = toml::from_str(
            "version = 1\n[servers.dyn]\ntype = \"http\"\nurl = \"https://${HOST_REF}/mcp\"\n\
             [policy.egress]\ndyn = [\"api.example\"]\n",
        )
        .unwrap();
        let mut report = Report::new();
        check_effective_policy(&constrained, &mut report);
        let lines = report_lines(&report);
        assert!(
            lines
                .iter()
                .any(|(l, m)| *l == "warn" && m.contains("dyn") && m.contains("${REF}")),
            "{lines:?}"
        );

        let unconstrained: Manifest = toml::from_str(
            "version = 1\n[servers.dyn]\ntype = \"http\"\nurl = \"https://${HOST_REF}/mcp\"\n",
        )
        .unwrap();
        let mut report2 = Report::new();
        check_effective_policy(&unconstrained, &mut report2);
        std::env::remove_var("AGENTSTACK_HOME");
        assert!(
            report_lines(&report2).is_empty(),
            "{:?}",
            report_lines(&report2)
        );
    }

    /// The one line under "Content scan" for a run with the given flags.
    fn scan_line(deep: bool, ci: bool, proj: &Path) -> String {
        let mut report = Report::new();
        run_checks(
            &DoctorArgs {
                ci,
                live: false,
                probe: false,
                fix: false,
                deep,
                all: false,
                json: false,
                skip_drift: false,
            },
            Some(proj),
            &mut report,
        )
        .unwrap();
        let section = report
            .sections
            .iter()
            .find(|s| s.title == "Content scan")
            .expect("content scan section present");
        section.lines[0].1.clone()
    }

    /// The three Codex diagnostics helpers, against a fenced HOME: the
    /// effective doc limit honors project config ONLY when trusted; the
    /// instruction chain prefers the override at BOTH levels; trust reads
    /// projects."<canonical>".trust_level.
    #[test]
    fn codex_helpers_match_codex_semantics() {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("HOME", home.path());
        std::env::set_var("AGENTSTACK_HOME", home.path().join(".agentstack"));
        let proj = assert_fs::TempDir::new().unwrap();
        let root = proj.path().canonicalize().unwrap();

        // --- codex_doc_limit ---
        assert_eq!(codex_doc_limit(&root), 32 * 1024, "default");
        home.child(".codex/config.toml")
            .write_str("project_doc_max_bytes = 16384\n")
            .unwrap();
        assert_eq!(codex_doc_limit(&root), 16384, "global layer applies");
        // An UNTRUSTED project's limit must NOT apply (Codex ignores the layer).
        proj.child(".codex/config.toml")
            .write_str("project_doc_max_bytes = 65536\n")
            .unwrap();
        assert!(!codex_project_trusted(&root));
        assert_eq!(codex_doc_limit(&root), 16384, "untrusted project ignored");
        // Trusted → the project layer wins.
        home.child(".codex/config.toml")
            .write_str(&format!(
                "project_doc_max_bytes = 16384\n[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
                root.display()
            ))
            .unwrap();
        assert!(codex_project_trusted(&root));
        assert_eq!(codex_doc_limit(&root), 65536, "trusted project wins");
        // Explicitly untrusted is not trusted.
        home.child(".codex/config.toml")
            .write_str(&format!(
                "[projects.\"{}\"]\ntrust_level = \"untrusted\"\n",
                root.display()
            ))
            .unwrap();
        assert!(!codex_project_trusted(&root));

        // --- codex_instruction_chain: override wins at BOTH levels ---
        home.child(".codex/AGENTS.md")
            .write_str("global\n")
            .unwrap();
        proj.child("AGENTS.md").write_str("project!\n").unwrap();
        let (bytes, files) = codex_instruction_chain(&root);
        assert_eq!(bytes, 7 + 9);
        assert_eq!(files, ["~/.codex/AGENTS.md", "AGENTS.md"]);
        // A global override shadows the global AGENTS.md…
        home.child(".codex/AGENTS.override.md")
            .write_str("G-OVERRIDE\n")
            .unwrap();
        let (bytes, files) = codex_instruction_chain(&root);
        assert_eq!(bytes, 11 + 9);
        assert_eq!(files[0], "~/.codex/AGENTS.override.md");
        // …and a project override shadows the project AGENTS.md.
        proj.child("AGENTS.override.md").write_str("P!\n").unwrap();
        let (_, files) = codex_instruction_chain(&root);
        assert_eq!(files, ["~/.codex/AGENTS.override.md", "AGENTS.override.md"]);
        // An EMPTY override falls back to AGENTS.md (first non-empty only).
        proj.child("AGENTS.override.md").write_str("").unwrap();
        let (_, files) = codex_instruction_chain(&root);
        assert_eq!(files, ["~/.codex/AGENTS.override.md", "AGENTS.md"]);

        std::env::remove_var("AGENTSTACK_HOME");
    }

    #[test]
    fn content_scan_runs_only_with_deep_or_ci() {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("HOME", home.path());
        std::env::set_var("AGENTSTACK_HOME", home.path().join(".agentstack"));
        let proj = assert_fs::TempDir::new().unwrap();
        proj.child("agentstack.toml")
            .write_str("version = 1\n[targets]\ndefault = [\"claude-code\"]\n")
            .unwrap();

        // Fast default does not scan (and says so honestly, not with a green
        // ✓); --deep and --ci both run the real scan.
        assert!(scan_line(false, false, proj.path()).contains("not scanned"));
        assert!(!scan_line(true, false, proj.path()).contains("not scanned"));
        assert!(!scan_line(false, true, proj.path()).contains("not scanned"));

        std::env::remove_var("AGENTSTACK_HOME");
        std::env::remove_var("HOME");
    }

    /// Build a one-server manifest from a TOML server body for quirk tests.
    fn manifest_with_server(toml_body: &str) -> Manifest {
        let src = format!("version = 1\n[servers.s]\n{toml_body}\n");
        toml::from_str(&src).expect("valid manifest toml")
    }

    fn quirks_for(toml_body: &str) -> Vec<Quirk> {
        check_quirks(&manifest_with_server(toml_body))
    }

    fn is_bare_launcher_warning(q: &Quirk) -> bool {
        q.msg.contains("bare launcher") && q.msg.contains("resolves via PATH")
    }

    #[test]
    fn bare_npx_launcher_is_flagged() {
        let quirks = quirks_for(
            "type = \"stdio\"\ncommand = \"npx\"\nargs = [\"chrome-devtools-mcp@latest\"]",
        );
        assert!(
            quirks.iter().any(is_bare_launcher_warning),
            "expected a bare-launcher warning, got {quirks:?}"
        );
    }

    #[test]
    fn bare_node_launcher_is_flagged() {
        let quirks = quirks_for("type = \"stdio\"\ncommand = \"node\"\nargs = [\"server.js\"]");
        assert!(quirks.iter().any(is_bare_launcher_warning));
    }

    #[test]
    fn absolute_path_command_is_not_flagged() {
        let quirks = quirks_for(
            "type = \"stdio\"\ncommand = \"/usr/local/bin/node\"\nargs = [\"server.js\"]",
        );
        assert!(!quirks.iter().any(is_bare_launcher_warning), "{quirks:?}");
    }

    #[test]
    fn login_shell_wrapper_is_not_flagged() {
        let quirks = quirks_for(
            "type = \"stdio\"\ncommand = \"zsh\"\nargs = [\"-lc\", \"exec npx chrome-devtools-mcp@latest\"]",
        );
        assert!(!quirks.iter().any(is_bare_launcher_warning), "{quirks:?}");
    }

    #[test]
    fn http_server_is_not_flagged() {
        let quirks = quirks_for("type = \"http\"\nurl = \"https://example.com/mcp\"");
        assert!(!quirks.iter().any(is_bare_launcher_warning), "{quirks:?}");
    }

    #[test]
    fn unknown_bare_command_is_not_flagged() {
        // A custom binary name outside the known launcher set is assumed to have
        // a stable install location; we don't want false positives on it.
        let quirks = quirks_for("type = \"stdio\"\ncommand = \"my-mcp-server\"\nargs = []");
        assert!(!quirks.iter().any(is_bare_launcher_warning), "{quirks:?}");
    }

    /// F12: the report is read at a glance, so paths in it are folded and
    /// `$HOME`-abbreviated. Purely lexical — a `..` with nothing to fold into
    /// still means something and survives.
    #[test]
    fn tidy_path_folds_and_abbreviates() {
        // The `$HOME` leg below reads a process-global that other tests in this
        // binary mutate, so it has to hold the same lock they do. Without it
        // this passes or fails on scheduling: it was green for as long as
        // nothing happened to run beside it, and adding unrelated tests
        // elsewhere in the crate was enough to make it flake. A test whose
        // result depends on how many other tests exist is not testing what it
        // claims to.
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        assert_eq!(
            tidy_path(Path::new("/srv/proj/../home/.claude.json")),
            "/srv/home/.claude.json"
        );
        assert_eq!(tidy_path(Path::new("/srv/./proj/x")), "/srv/proj/x");
        // Nothing above the root to climb into: `..` is dropped rather than
        // producing a path that escapes `/`.
        assert_eq!(tidy_path(Path::new("/../etc")), "/etc");
        // A relative path climbing out keeps its meaning.
        assert_eq!(tidy_path(Path::new("../sibling")), "../sibling");

        if let Some(home) = dirs::home_dir() {
            assert_eq!(
                tidy_path(&home.join("proj/.mcp.json")),
                "~/proj/.mcp.json",
                "$HOME is abbreviated"
            );
        }
    }

    /// F04: nearly every published MCP server ships as `npx -y …`, so the
    /// bare-launcher finding must be stated ONCE with a count, not once per
    /// server — otherwise the noise scales with the size of a normal setup.
    #[test]
    fn bare_launcher_finding_is_collapsed_and_advisory() {
        let manifest: Manifest = toml::from_str(
            "version = 1\n\
             [servers.a]\ntype = \"stdio\"\ncommand = \"npx\"\nargs = [\"a\"]\n\
             [servers.b]\ntype = \"stdio\"\ncommand = \"npx\"\nargs = [\"b\"]\n\
             [servers.c]\ntype = \"stdio\"\ncommand = \"node\"\nargs = [\"c.js\"]\n",
        )
        .expect("valid manifest toml");
        let quirks = check_quirks(&manifest);
        let bare: Vec<_> = quirks
            .iter()
            .filter(|q| is_bare_launcher_warning(q))
            .collect();
        assert_eq!(bare.len(), 1, "one line for all of them, got {quirks:?}");
        let q = bare[0];
        assert!(q.advisory, "an ecosystem caveat is not a defect to repair");
        assert!(q.msg.starts_with("3 servers"), "{}", q.msg);
        for name in ["a", "b", "c"] {
            assert!(q.msg.contains(name), "must name {name}: {}", q.msg);
        }
    }

    /// F01: the whole point of the advisory tier. A project whose only
    /// findings are advisories has nothing to repair, so it must read `ready`
    /// and must not have one of them chosen as its "start with" next action.
    #[test]
    fn advisories_leave_the_project_ready() {
        let mut report = Report::new();
        report.section("Quirks");
        report.line(Level::Advisory, "2 servers use a bare launcher ↳ pin them");
        report.line(Level::Ok, "no unsupported syntax for any target");

        assert_eq!(report.advisories, 1);
        assert_eq!(report.warnings, 0);
        assert_eq!(report.errors, 0);
        assert_eq!(report.state(), "ready");
        assert_eq!(
            report.first_fix(),
            None,
            "an advisory must never become the one recommended action"
        );

        // A real warning still moves it, and still wins the triage line.
        report.line(
            Level::Warn,
            "Codex will IGNORE this config ↳ open Codex once",
        );
        assert_eq!(report.state(), "needs_attention");
        assert_eq!(report.first_fix(), Some("open Codex once"));
    }

    /// `doctor-mode-v1`: the JSON body carries `mode` and `activation` as
    /// typed fields — null before a project context is checked, verbatim
    /// labels once set. The panel used to reverse these out of section prose
    /// ("Mode zero-files", "not locked (never activated)"), which any
    /// rewording silently broke.
    #[test]
    fn to_json_carries_mode_and_activation() {
        let mut report = Report::new();
        let body = report.to_json();
        assert_eq!(body["mode"], serde_json::Value::Null);
        assert_eq!(body["activation"], serde_json::Value::Null);

        report.mode = Some("zero-files");
        report.activation = Some("never_activated");
        let body = report.to_json();
        assert_eq!(body["mode"], "zero-files");
        assert_eq!(body["activation"], "never_activated");
    }

    /// N1: Codex is detected by binary-on-PATH and lands in `targets.default`,
    /// so a `.codex/config.toml` gets rendered for projects that never
    /// mentioned Codex. As a warning that pinned every such project at
    /// `needs_attention` on any machine with Codex installed — the primary
    /// persona. `codex_in_use` is the discriminator: having accepted Codex's
    /// own trust prompt for ANY project is a deliberate act, so it separates
    /// "really uses this tool" from "has the binary".
    #[test]
    fn codex_in_use_reads_a_deliberate_act_not_mere_installation() {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let orig_home = std::env::var_os("HOME");
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("HOME", home.path());
        let cfg = home.path().join(".codex/config.toml");
        std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();

        // Each case is (config contents or None, expected). Collected before
        // asserting so a failure still restores HOME for every other test.
        let cases: Vec<(Option<&str>, bool)> = vec![
            // No config at all → not in use.
            (None, false),
            // Config, but no project ever trusted → installed, not used. This
            // is the case that must NOT gate readiness.
            (Some("[mcp_servers.x]\ncommand = \"true\"\n"), false),
            // An empty `projects` table is still "never accepted a prompt".
            (Some("[projects]\n"), false),
            // One accepted project anywhere → the user really runs Codex, so
            // a render it will silently ignore is a real warning again.
            (
                Some("[projects.\"/some/repo\"]\ntrust_level = \"trusted\"\n"),
                true,
            ),
            // Unparseable config resolves to "not in use" — the quieter side,
            // so a broken file can never manufacture a warning.
            (Some("{{{ not toml"), false),
        ];
        let got: Vec<bool> = cases
            .iter()
            .map(|(contents, _)| {
                match contents {
                    Some(c) => std::fs::write(&cfg, c).unwrap(),
                    None => {
                        let _ = std::fs::remove_file(&cfg);
                    }
                }
                codex_in_use()
            })
            .collect();

        match orig_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }

        let want: Vec<bool> = cases.iter().map(|(_, w)| *w).collect();
        assert_eq!(got, want, "codex_in_use must key off an accepted prompt");
    }

    /// F03: the recommended action used to be whichever warning happened to
    /// sit in the earliest section. Rank instead — a fix AgentStack can run
    /// beats a hand-off to another tool, even when the hand-off is listed
    /// first.
    #[test]
    fn first_fix_prefers_a_command_agentstack_can_run() {
        let mut report = Report::new();
        report.section("Instructions");
        report.line(Level::Warn, "Codex will IGNORE this ↳ open Codex once");
        report.section("Drift");
        report.line(Level::Warn, "2 changes pending ↳ agentstack apply --write");
        assert_eq!(report.first_fix(), Some("agentstack apply --write"));

        // Errors still outrank warnings, and the same preference applies
        // within the error class.
        report.section("Manifest");
        report.line(Level::Error, "unreadable ↳ open it and fix the syntax");
        assert_eq!(report.first_fix(), Some("open it and fix the syntax"));
        report.line(Level::Error, "unpinned ↳ agentstack lock");
        assert_eq!(report.first_fix(), Some("agentstack lock"));

        // With no runnable command anywhere, the first candidate stands.
        let mut manual = Report::new();
        manual.section("Instructions");
        manual.line(Level::Warn, "a ↳ do this by hand");
        manual.line(Level::Warn, "b ↳ then this");
        assert_eq!(manual.first_fix(), Some("do this by hand"));
    }

    /// The node_repl lesson: an absolute stdio command that no longer exists
    /// (owning app relocated itself) errors with the owner hint; bare names,
    /// relative paths, `${REF}`s, http servers, and existing paths stay quiet.
    #[test]
    fn missing_absolute_command_errors_with_owner_hint() {
        let server = |body: &str| -> crate::manifest::Server { toml::from_str(body).unwrap() };
        let gone = server(
            "type = \"stdio\"\ncommand = \"/Applications/Codex.app/Contents/Resources/cua_node/bin/node_repl\"\nowner = \"codex\"",
        );
        let err = missing_command_error("node_repl", &gone).expect("must error");
        assert!(err.contains("does not exist on this machine"), "{err}");
        assert!(err.contains("owner ('codex')"), "{err}");
        assert!(err.contains("apply --write"), "{err}");

        let ownerless = server("type = \"stdio\"\ncommand = \"/definitely/not/here/anymore\"");
        let err = missing_command_error("x", &ownerless).expect("must error");
        assert!(err.contains("fix the path in the manifest"), "{err}");

        for quiet in [
            "type = \"stdio\"\ncommand = \"npx\"\nargs = [\"-y\", \"pkg\"]",
            "type = \"stdio\"\ncommand = \"./local/tool.sh\"",
            "type = \"stdio\"\ncommand = \"/${APP_HOME}/bin/tool\"",
            "type = \"stdio\"\ncommand = \"/bin/sh\"",
            "type = \"http\"\nurl = \"https://example.com/mcp\"",
        ] {
            assert_eq!(missing_command_error("s", &server(quiet)), None, "{quiet}");
        }
    }

    /// Review finding M10. `--probe` is the only doctor check that starts a
    /// process, so the two gates in front of it are security properties, not
    /// conveniences — and the witness for "did not spawn" has to be the
    /// absence of a side effect, not the presence of a message. Each server
    /// here touches a marker file on startup, so the marker is proof.
    #[test]
    fn probe_spawns_nothing_for_an_untrusted_project_or_an_unresolved_secret() {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("HOME", home.path());
        std::env::set_var("AGENTSTACK_HOME", home.path().join(".agentstack"));

        let proj = assert_fs::TempDir::new().unwrap();
        let clean_marker = proj.path().join("clean.ran");
        let gated_marker = proj.path().join("gated.ran");
        let dir = proj.path().join(".agentstack");
        std::fs::create_dir_all(&dir).unwrap();
        // Neither server speaks MCP — they touch a file and exit, which is all
        // this test needs to distinguish "was started" from "was not".
        std::fs::write(
            dir.join("agentstack.toml"),
            format!(
                "version = 1\n[targets]\ndefault = []\n\
                 [servers.clean]\ntype = \"stdio\"\ncommand = \"touch\"\nargs = [\"{}\"]\n\
                 [servers.gated]\ntype = \"stdio\"\ncommand = \"touch\"\nargs = [\"{}\"]\n\
                 [servers.gated.env]\nTOKEN = \"${{ABSENT_PROBE_TEST_REF}}\"\n",
                clean_marker.display(),
                gated_marker.display()
            ),
        )
        .unwrap();
        let ctx = crate::commands::load(Some(proj.path())).unwrap();

        // Gate 1 — untrusted. Untrusted repository content is inert, and
        // "start the command this repo's manifest names" is the most direct
        // way there is to break that, so NEITHER server may run.
        let mut untrusted = Report::new();
        probe_stdio_servers(
            &ctx.loaded.manifest,
            &ctx,
            crate::trust::TrustState::Untrusted,
            &mut untrusted,
        );
        let lines = report_lines(&untrusted);
        assert!(
            lines
                .iter()
                .any(|(l, m)| *l == "warn" && m.contains("refusing to probe")),
            "{lines:?}"
        );
        assert!(
            !clean_marker.exists() && !gated_marker.exists(),
            "an untrusted project started a server"
        );
        assert_eq!(
            untrusted.probe.as_ref().map(|p| p.ran),
            Some(false),
            "the JSON must say the probe did not run"
        );

        // Gate 2 — trusted, but one server's `${REF}` does not resolve. That
        // one is reported as not-probeable rather than started with a
        // half-substituted environment, which would blame the server for a
        // missing secret. The other server proves the probe really does spawn
        // when both gates pass — without it, gate 1 above proves nothing.
        let mut trusted = Report::new();
        probe_stdio_servers(
            &ctx.loaded.manifest,
            &ctx,
            crate::trust::TrustState::Trusted,
            &mut trusted,
        );
        let lines = report_lines(&trusted);
        assert!(
            clean_marker.exists(),
            "a trusted project with resolvable refs must actually start the server"
        );
        assert!(
            !gated_marker.exists(),
            "a server with an unresolved ${{REF}} was started anyway"
        );
        assert!(
            lines.iter().any(|(l, m)| *l == "warn"
                && m.contains("gated")
                && m.contains("ABSENT_PROBE_TEST_REF")),
            "{lines:?}"
        );
        let servers = &trusted.probe.as_ref().unwrap().servers;
        assert_eq!(servers[1]["status"], "not_probeable", "{servers:?}");

        std::env::remove_var("AGENTSTACK_HOME");
    }
}
