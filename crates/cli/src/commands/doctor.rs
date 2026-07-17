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
    Warn,
    Error,
}

/// Accumulates every check result (grouped by section). Nothing prints while
/// checks run — `print` renders the terminal report at the end, filtered by
/// per-section relevance, and the dashboard renders `to_json` itself. The
/// error/warning counters are display-independent: every check always runs and
/// always counts, whether or not its section is shown.
struct Report {
    errors: usize,
    warnings: usize,
    sections: Vec<Section>,
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
            sections: Vec::new(),
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
    /// prints everything, matching the JSON the dashboard gets.
    fn print(&self, show_all: bool) {
        let mut hidden = 0;
        for s in &self.sections {
            let flagged = s.lines.iter().any(|(tag, _)| *tag != "ok");
            if !(show_all || s.relevant || flagged) {
                hidden += 1;
                continue;
            }
            println!("{}", s.title.bold());
            for (tag, msg) in &s.lines {
                let mark = match *tag {
                    "warn" => "⚠".yellow().to_string(),
                    "error" => "✗".red().to_string(),
                    _ => "✓".green().to_string(),
                };
                println!("  {mark} {msg}");
            }
        }
        if hidden > 0 {
            println!(
                "{} {hidden} section(s) for features this project doesn't use are hidden — {} shows everything.",
                "·".dimmed(),
                "agentstack doctor --all".bold()
            );
        }
    }

    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "errors": self.errors,
            "warnings": self.warnings,
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
    let mut report = Report::new();
    let fixed = run_checks(args, manifest_dir, &mut report)?;

    // `--ci` always shows the full report: a team gate should print exactly
    // what it evaluated, not a per-project selection of it.
    report.print(args.all || args.ci);

    println!();
    if fixed > 0 {
        println!("{} re-applied {fixed} drifted target(s).", "✓".green());
    }
    println!(
        "{} error(s), {} warning(s).",
        report.errors, report.warnings
    );

    // In CI mode any error fails the trust gate. Return an error rather than
    // exiting inline so `main` owns the single exit point and this path stays
    // testable.
    if args.ci && report.errors > 0 {
        anyhow::bail!("doctor found {} error(s) — see report above", report.errors);
    }
    Ok(())
}

/// The same checks `doctor` runs, with fix/live off and nothing printed —
/// the dashboard's Doctor pane. Deep stays on: the pane is an explicit
/// "run the check-up" surface, so it keeps the content scan's findings.
pub fn collect(manifest_dir: Option<&Path>) -> Result<serde_json::Value> {
    let mut report = Report::new();
    run_checks(
        &DoctorArgs {
            ci: false,
            live: false,
            fix: false,
            deep: true,
            all: true,
        },
        manifest_dir,
        &mut report,
    )?;
    Ok(report.to_json())
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
        report.line(level, issue.message);
    }

    let target_ids = resolve_targets(manifest, &ctx.registry, &[]);
    let mut state = State::load()?;
    let mut fixed = 0;

    report.section("Adapters & CLIs");
    for id in &target_ids {
        match ctx.registry.get(id) {
            None => report.line(Level::Error, format!("{id}: unknown adapter")),
            Some(desc) => {
                let path_label = desc
                    .config
                    .as_ref()
                    .map(|c| paths::expand_tilde(&c.path).display().to_string())
                    .unwrap_or_else(|| "no MCP config".to_string());
                if desc.is_installed() {
                    match desc.read_config_value() {
                        Ok(_) => report.line(
                            Level::Ok,
                            format!("{:<14} installed · {} parses", desc.display, path_label),
                        ),
                        Err(e) => report.line(
                            Level::Error,
                            format!("{}: config invalid · {e}", desc.display),
                        ),
                    }
                } else if desc.config_present() {
                    report.line(
                        Level::Warn,
                        format!("{:<14} config present but binary not on PATH", desc.display),
                    );
                } else {
                    report.line(Level::Warn, format!("{:<14} not detected", desc.display));
                }
            }
        }
    }

    // Zero-files bridge: which harnesses have the global `agentstack mcp
    // --auto-project` gateway registered, and where this project stands with
    // the trust gate. Not being connected is a choice, not a fault — only a
    // stale trust digest warns.
    report.section("Zero-files bridge");
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
                format!("{:<14} bridge registered (agentstack mcp)", desc.display),
            );
        }
    }
    if connected == 0 {
        report.line(
            Level::Ok,
            "no harness connected — optional ↳ agentstack gateway connect --all",
        );
    }
    let base = crate::manifest::project_root_of(&ctx.dir);
    let trust_state = crate::trust::check(&base);
    match trust_state {
        crate::trust::TrustState::Trusted => {
            report.line(Level::Ok, "this project is trusted for auto mode")
        }
        crate::trust::TrustState::Changed => report.line(
            Level::Warn,
            "trusted, but the manifest or lockfile changed since ↳ review + agentstack trust",
        ),
        // Untrusted is a choice, not a fault (Ok) — unless a harness actually
        // uses the bridge AND the project declares a runtime surface (inline
        // servers or profile/library name refs): then every session here
        // silently gets control-plane tools only, which is worth a warning.
        crate::trust::TrustState::Untrusted => {
            let runtime = crate::resolve::runtime_server_names(manifest, None);
            if connected > 0 && !runtime.is_empty() {
                report.line(
                    Level::Warn,
                    format!(
                        "not trusted — {connected} harness(es) use the bridge, but this project's {} server(s) are not proxied ↳ agentstack trust {}",
                        runtime.len(),
                        base.display()
                    ),
                );
            } else {
                report.line(
                    Level::Ok,
                    "not trusted for auto mode — untrusted repos get control-plane tools only ↳ agentstack trust",
                );
            }
        }
    }
    // Relevant once the bridge is in play on either side: a harness registered
    // it, or this project entered the trust lifecycle.
    if connected == 0 && trust_state == crate::trust::TrustState::Untrusted {
        report.mark_irrelevant();
    }

    report.section("Secrets");
    let refs = manifest.referenced_secrets();
    if refs.is_empty() {
        report.line(Level::Ok, "no secrets referenced");
        report.mark_irrelevant();
    }
    for name in &refs {
        if ctx.resolver.resolve(name).is_some() {
            report.line(Level::Ok, format!("{name:<20} resolved"));
        } else {
            report.line(
                Level::Error,
                format!("{name:<20} not found ↳ agentstack secret set {name}"),
            );
        }
    }

    report.section("Drift");
    let mut any_drift = false;
    let identity = state::manifest_identity(&ctx.dir);
    // Owner-refreshed servers: the drift check renders with the owning app's
    // on-disk values, so an owned server that changed on disk is reported as
    // "refresh the manifest", never as a pending revert of what the app wrote
    // (see render::owned).
    let mut server_map: indexmap::IndexMap<String, crate::manifest::Server> =
        manifest.servers.clone();
    let owned = crate::render::refresh_owned_servers(
        &mut server_map,
        &ctx.registry,
        Scope::Global,
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
        let key = target_key(id, Scope::Global, &ctx.dir);
        // Was this key's managed set recorded by a different manifest? Its
        // leftover entries are then not ours to prune (see
        // State::foreign_prunes): `--fix` keeps them, and the report points at
        // `apply --prune-foreign` instead of `apply --write`.
        let foreign_key = state
            .manifest_source(&key)
            .is_some_and(|src| src != identity);
        let mut previously = state.managed_servers(&key);
        let kept = if args.fix {
            state.foreign_prunes(&key, Scope::Global, &ctx.dir, &mut previously, |n| {
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
            &server_map,
            &previously,
            Scope::Global,
            &ctx.dir,
        )?
        else {
            continue;
        };

        if !kept_report.is_empty() {
            any_drift = true;
            report.line(
                Level::Warn,
                format!(
                    "{:<14} kept {} — applied by another manifest ↳ keep them: \
                     agentstack adopt · prune them: agentstack apply --prune-foreign",
                    desc.display,
                    kept_report.join(", ")
                ),
            );
        }
        // Hand-edit since our last write?
        if let Some(ts) = state.targets.get(&key) {
            if !ts.last_hash.is_empty() {
                let on_disk = state::hash(&plan.existing);
                if on_disk != ts.last_hash {
                    // The owner of an owned server rewrites its own config by
                    // design. When the mismatch is fully explained — this
                    // target owns a refreshed server and the owner-refreshed
                    // render proposes no change — it's app churn, not a
                    // hand-edit to review.
                    let owner_churn =
                        !plan.changed() && owned.iter().any(|o| o.stale && o.owner == **id);
                    if owner_churn {
                        report.line(
                            Level::Ok,
                            format!(
                                "{:<14} rewritten by the app itself (owned server) — \
                                 refresh the manifest: agentstack apply --write",
                                desc.display
                            ),
                        );
                    } else {
                        any_drift = true;
                        report.line(
                            Level::Warn,
                            format!(
                                "{:<14} edited on disk since last apply ↳ review: agentstack diff · \
                                 keep the hand-edit: agentstack adopt",
                                desc.display
                            ),
                        );
                    }
                }
            }
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
                            "{:<14} not fixed — unresolved secret(s): {}",
                            desc.display,
                            plan.unresolved.join(", ")
                        ),
                    );
                }
                if !plan.failed.is_empty() {
                    report.line(
                        Level::Error,
                        format!(
                            "{:<14} not fixed — secret read failure(s): {}",
                            desc.display,
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
                        "{:<14} re-applied {} change(s)",
                        desc.display,
                        plan.managed.len()
                    ),
                );
            } else if plan.removed.is_empty() {
                any_drift = true;
                report.line(
                    Level::Warn,
                    format!(
                        "{:<14} {} change(s) pending ↳ agentstack apply --write",
                        desc.display,
                        plan.managed.len()
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
                    "agentstack apply --prune-foreign"
                } else {
                    "agentstack apply --write"
                };
                report.line(
                    Level::Warn,
                    format!(
                        "{:<14} would REMOVE {} ↳ keep them: agentstack adopt · \
                         prune them: {prune_cmd}",
                        desc.display,
                        plan.removed.join(", ")
                    ),
                );
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

    // Instruction fragments: the managed region of each CLAUDE.md / AGENTS.md
    // must match what the manifest would compile at SOME scope the project
    // actually uses (apply picks the scope per invocation), and every declared
    // fragment source must exist. A missing source is an error (`--ci` gates
    // it); a stale region is drift, so it warns.
    report.section("Instructions");
    if manifest.instructions.is_empty() {
        report.line(Level::Ok, "no instruction fragments defined");
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
                    "{inherited} fragment(s) inherited from the machine manifest ({})",
                    up.display()
                ),
            );
        }
        let mut instr_issues = 0;
        for id in &target_ids {
            let Some(desc) = ctx.registry.get(id) else {
                continue;
            };
            let global = crate::render::instructions::plan_instructions(
                manifest,
                desc,
                Scope::Global,
                &ctx.dir,
            );
            let project = crate::render::instructions::plan_instructions(
                manifest,
                desc,
                Scope::Project,
                &ctx.dir,
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
        if root.join(".codex/config.toml").exists() && !codex_project_trusted(&root) {
            report.line(
                Level::Warn,
                format!(
                    "Codex will IGNORE {}/.codex/config.toml — the project is not trusted in ~/.codex/config.toml (projects.\"{}\".trust_level) ↳ open Codex in this folder once and accept the trust prompt",
                    root.display(),
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
        report.line(Level::Warn, q);
    }

    // Lifecycle hooks: the same staleness contract as instructions — the
    // rendered hooks key of each hook-capable target must match what the
    // manifest would compile (global scope, mirroring drift/fix).
    report.section("Hooks");
    if manifest.hooks.is_empty() {
        report.line(Level::Ok, "no lifecycle hooks defined");
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
                    "{} hook(s) defined but no selected target supports hooks",
                    manifest.hooks.len()
                ),
            );
        } else if hook_issues == 0 {
            report.line(
                Level::Ok,
                format!(
                    "{} hook(s) in sync across {hook_capable} hook-capable target(s)",
                    manifest.hooks.len()
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
        report.line(Level::Ok, "no skills defined");
        // The broken-symlink sweep below still appends warn lines when a
        // detected adapter's skills dir is unhealthy — those keep it visible.
        report.mark_irrelevant();
    }
    let store = crate::store::Store::default_store();
    let skills_library = crate::library::Library::load_default().unwrap_or_default();
    let skills_lib_home = paths::lib_home();
    for name in &skill_names {
        // Inline definitions resolve straight to their local dir; a
        // profile-only name resolves through the central library (offline —
        // NoFetch, so a git body that isn't cached reports as not installed
        // rather than triggering a fetch).
        let dir = if let Some(skill) = manifest.skills.get(name) {
            crate::store::local_source_dir(&store, skill, &ctx.dir)
        } else {
            crate::resolve::resolve_skill(
                manifest,
                &ctx.dir,
                &skills_library,
                &skills_lib_home,
                &store,
                name,
                crate::resolve::ResolveMode::NoFetch,
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
            Some(_) => report.line(Level::Ok, format!("{name:<20} present · SKILL.md ok")),
        }
    }
    // Broken skill links on disk: a symlink in a detected CLI's skills dir
    // whose target is gone loads nothing — and consolidate skips it — so name
    // it here with the fix instead of leaving the skill silently dead. Every
    // detected adapter is walked, not just the manifest's targets: the dead
    // link breaks that CLI regardless of what this project fans out to.
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
    // consolidated/pre-existing skills have no other surface that tells the
    // user they're undiscoverable. A skill without a frontmatter description
    // only matches search by name and shows as a bare name in every loadable
    // index — warn with the fix, don't block (it still works by name).
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
                    .map_or(true, |d| d.trim().is_empty())
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

    // Supply-chain content scan (same detectors as `agentstack audit`): hidden
    // Unicode is an error so `--ci` gates it; injection heuristics only warn.
    // It reads every skill body, so the everyday run skips it — `--deep` opts
    // in and `--ci` (the trust gate) always includes it.
    report.section("Content scan");
    // Nothing with scannable content declared → nothing this could ever find.
    if skill_names.is_empty() && manifest.servers.is_empty() {
        report.mark_irrelevant();
    }
    if args.ci || args.deep {
        let mut flagged = 0usize;
        for unit in crate::commands::audit::collect(manifest, &ctx.dir, &store) {
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
        report.line(
            Level::Ok,
            "skipped (reads every skill body) ↳ agentstack doctor --deep — always on in --ci",
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

    report.section("Plugin recipes");
    let recipe_statuses = crate::plugin_recipes::statuses(manifest, &ctx.registry, &ctx.dir);
    if recipe_statuses.is_empty() {
        report.line(Level::Ok, "no plugin recipes defined");
        report.mark_irrelevant();
    }
    for recipe in recipe_statuses {
        if let Some(conflict) = &recipe.conflict {
            report.line(Level::Error, format!("{:<20} {conflict}", recipe.name));
            continue;
        }
        if !recipe.missing_skills.is_empty() {
            report.line(
                Level::Warn,
                format!(
                    "{:<20} missing skill source(s): {}",
                    recipe.name,
                    recipe.missing_skills.join(", ")
                ),
            );
            continue;
        }
        // Targets whose adopted-from native plugin is still installed need no
        // generated package/marketplace/install — nagging sync+install there
        // would install the same plugin a second time.
        let native_targets: Vec<&str> = recipe
            .installs
            .iter()
            .filter(|i| i.native.is_some())
            .map(|i| i.target.as_str())
            .collect();
        let all_native = !recipe.targets.is_empty()
            && recipe
                .targets
                .iter()
                .all(|t| native_targets.contains(&t.as_str()));
        if all_native {
            report.line(
                Level::Ok,
                format!(
                    "{:<20} satisfied natively ({}) — no generated package needed",
                    recipe.name,
                    native_targets.join(", ")
                ),
            );
        } else if !recipe.generated {
            report.line(
                Level::Warn,
                format!(
                    "{:<20} not generated ↳ agentstack plugins sync --write",
                    recipe.name
                ),
            );
        } else if recipe.stale {
            report.line(
                Level::Warn,
                format!(
                    "{:<20} generated package stale ↳ agentstack plugins sync --write",
                    recipe.name
                ),
            );
        } else {
            report.line(Level::Ok, format!("{:<20} package generated", recipe.name));
        }
        for market in &recipe.marketplaces {
            if native_targets.contains(&market.target.as_str()) {
                continue;
            }
            if !market.present {
                report.line(
                    Level::Warn,
                    format!(
                        "{:<20} {} marketplace missing ↳ agentstack plugins sync --write",
                        recipe.name, market.target
                    ),
                );
            } else if market.stale {
                report.line(
                    Level::Warn,
                    format!(
                        "{:<20} {} marketplace stale ↳ agentstack plugins sync --write",
                        recipe.name, market.target
                    ),
                );
            }
        }
        for install in &recipe.installs {
            if let Some(native) = &install.native {
                if let Some(drift) = &native.drift {
                    report.line(
                        Level::Warn,
                        format!(
                            "{:<20} {}: native {}@{} moved since adoption ({drift}) ↳ re-adopt to refresh",
                            recipe.name, install.target, native.plugin, native.marketplace
                        ),
                    );
                } else if native.enabled == Some(false) {
                    report.line(
                        Level::Warn,
                        format!(
                            "{:<20} {}: native install {}@{} is disabled ↳ enable it in the harness",
                            recipe.name, install.target, native.plugin, native.marketplace
                        ),
                    );
                } else {
                    let at = match (&native.version, &native.rev) {
                        (Some(v), Some(r)) => format!(", up to date @ {v}+{r}"),
                        (Some(v), None) => format!(", up to date @ {v}"),
                        (None, Some(r)) => format!(", up to date @ rev {r}"),
                        (None, None) => String::new(),
                    };
                    report.line(
                        Level::Ok,
                        format!(
                            "{:<20} {}: native install {} ✓{at}",
                            recipe.name, install.target, native.marketplace
                        ),
                    );
                }
                continue;
            }
            if !install.installed {
                report.line(
                    Level::Warn,
                    format!("{:<20} not installed in {}", recipe.name, install.target),
                );
            } else {
                let enabled = match install.enabled {
                    Some(true) => "enabled",
                    Some(false) => "disabled",
                    None => install.status.as_deref().unwrap_or("installed"),
                };
                report.line(
                    Level::Ok,
                    format!(
                        "{:<20} installed in {} ({enabled})",
                        recipe.name, install.target
                    ),
                );
            }
        }
    }

    // Project policy is optional; the machine layer applies either way, so the
    // "Policy" section shows whenever EITHER has something to report.
    let machine_policy = crate::machine_policy::inspect();
    // Machine-policy posture: one honest word (open / mixed / restrictive) for
    // how locked-down THIS machine's own firewall layer is — shown even when
    // there's no machine policy at all, because "open" is the case most worth
    // stating out loud. Borrows `machine_policy`; the Policy section below still
    // moves it into `check_machine_policy`.
    report.section("Machine policy posture");
    let (posture, why) = classify_machine_posture(&machine_policy);
    report.line(Level::Ok, format!("{posture} — {why}"));
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

    if args.live {
        report.section("MCP connectivity (--live)");
        let http: Vec<_> = manifest
            .servers
            .iter()
            .filter(|(_, s)| s.server_type == ServerType::Http)
            .collect();
        if http.is_empty() {
            report.line(Level::Ok, "no HTTP servers to probe");
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
        report.line(Level::Ok, "no library-backed profile skills to verify");
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
                    Ok(current) if current.hex() == m.checksum => report.line(
                        Level::Ok,
                        format!("{:<20} rendered copy matches pin ({})", m.name, desc.id),
                    ),
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
    check_named_policy_keys(
        "tools",
        "enforced at the gateway",
        &manifest.policy.tools,
        manifest,
        report,
    );
    check_named_policy_keys(
        "egress",
        "checked against the declared host at write/spawn time",
        &manifest.policy.egress,
        manifest,
        report,
    );
    check_named_policy_keys(
        "secrets",
        "enforced fail-closed at render + gateway",
        &manifest.policy.secrets,
        manifest,
        report,
    );
    // [policy.filesystem] scopes are bundle-global (not per-server). The
    // write scope is enforced in sandbox mode (the workspace mounts
    // read-only unless it covers the workspace root — deny-by-default);
    // host mode remains advisory, and read scopes stay informational while
    // the only mount is the whole workspace.
    if !manifest.policy.filesystem.read.is_empty() {
        report.line(
            Level::Ok,
            format!(
                "[policy.filesystem] read — {} scope(s) — informational (the sandbox mounts one whole workspace)",
                manifest.policy.filesystem.read.len()
            ),
        );
    }
    if !manifest.policy.filesystem.write.is_empty() {
        report.line(
            Level::Ok,
            format!(
                "[policy.filesystem] write — {} scope(s) — enforced in sandbox mode (workspace mounts read-only unless covered); advisory in host mode",
                manifest.policy.filesystem.write.len()
            ),
        );
    }
}

/// One per-server-keyed policy dimension's key-validation: every key in
/// `map` must be `"*"` or a real server in the manifest, else it silently
/// firewalls nothing (a typo'd server name). `dimension` is the bare name
/// (`"tools"`, `"egress"`, `"secrets"`) used in the Ok summary; the Error
/// line always names the bracketed `[policy.<dimension>]` form to match
/// how a maintainer would grep the manifest for it.
fn check_named_policy_keys(
    dimension: &str,
    enforced_where: &str,
    map: &indexmap::IndexMap<String, Vec<String>>,
    manifest: &Manifest,
    report: &mut Report,
) {
    for (server, rules) in map {
        if server == "*" || manifest.servers.contains_key(server) {
            let denies = rules.iter().filter(|r| r.starts_with('!')).count();
            let allows = rules.len() - denies;
            let label = if server == "*" {
                "every server"
            } else {
                server
            };
            report.line(
                Level::Ok,
                format!("{dimension} '{label}' — {allows} allow / {denies} deny rule(s), {enforced_where}"),
            );
        } else {
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

/// One machine-layer dimension's summary + rename-dodge lint: an Ok line
/// with the rule-set count (silent when the dimension is unused), then one
/// Warn per named deny not mirrored under `"*"`.
fn report_machine_dimension(
    dimension: &str,
    map: &indexmap::IndexMap<String, Vec<String>>,
    report: &mut Report,
) {
    if map.is_empty() {
        return;
    }
    report.line(
        Level::Ok,
        format!(
            "machine [policy.{dimension}] — {} server rule set(s), checked before project policy on every call",
            map.len()
        ),
    );
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
        crate::machine_policy::Status::Current {
            source_digest,
            snapshot_synced,
            ..
        } => report.line(
            Level::Ok,
            format!(
                "machine policy CURRENT — source {source_digest}; snapshot {}",
                if *snapshot_synced { "in sync" } else { "not in sync" }
            ),
        ),
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

/// Flag a stdio server whose `command` is a bare, `PATH`-dependent launcher — no
/// path separator, not a tilde path, and in [`PATH_DEPENDENT_LAUNCHERS`]. We
/// only warn for that known set (not every bare command) so intentional `PATH`
/// binaries with a stable install location don't produce false positives.
fn bare_launcher_quirk(name: &str, server: &Server) -> Option<String> {
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
    Some(format!(
        "server '{name}': bare launcher `{cmd}` resolves via PATH; a GUI-launched harness \
         (Claude Code.app, Claude Desktop, VS Code) may inherit a minimal PATH and fail to spawn \
         it. Use an absolute path (e.g. an absolute {cmd} under the intended version) or a \
         login-shell wrapper: command = \"zsh\", args = [\"-lc\", \"exec {cmd} …\"]"
    ))
}

/// Detect per-target syntax a CLI can't handle, before it breaks at runtime.
fn check_quirks(manifest: &Manifest) -> Vec<String> {
    let mut out = Vec::new();
    for (name, server) in &manifest.servers {
        if let Some(msg) = bare_launcher_quirk(name, server) {
            out.push(msg);
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
                out.push(format!(
                    "server '{name}': ${{VAR:-default}} syntax is unsupported by Codex"
                ));
                break;
            }
        }
        // stdio servers with http-only fields, or vice versa.
        if server.server_type == ServerType::Stdio && !server.headers.is_empty() {
            out.push(format!(
                "server '{name}': stdio transport ignores `headers`"
            ));
        }
        if server.server_type == ServerType::Http && server.command.is_some() {
            out.push(format!("server '{name}': http transport ignores `command`"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::prelude::*;

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
        crate::trust::trust(proj.path()).unwrap();
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
            "checked against the declared host at write/spawn time",
            &rules_map(&[("known", &["api.example"]), ("ghost", &["!evil.example"])]),
            &manifest,
            &mut report,
        );
        let lines = report_lines(&report);
        assert!(lines.iter().any(|(l, m)| *l == "ok" && m.contains("known")));
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
                fix: false,
                deep,
                all: false,
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

        // Fast default skips; --deep and --ci both run the real scan.
        assert!(scan_line(false, false, proj.path()).contains("skipped"));
        assert!(!scan_line(true, false, proj.path()).contains("skipped"));
        assert!(!scan_line(false, true, proj.path()).contains("skipped"));

        std::env::remove_var("AGENTSTACK_HOME");
        std::env::remove_var("HOME");
    }

    /// Build a one-server manifest from a TOML server body for quirk tests.
    fn manifest_with_server(toml_body: &str) -> Manifest {
        let src = format!("version = 1\n[servers.s]\n{toml_body}\n");
        toml::from_str(&src).expect("valid manifest toml")
    }

    fn quirks_for(toml_body: &str) -> Vec<String> {
        check_quirks(&manifest_with_server(toml_body))
    }

    fn is_bare_launcher_warning(q: &str) -> bool {
        q.contains("bare launcher") && q.contains("resolves via PATH")
    }

    #[test]
    fn bare_npx_launcher_is_flagged() {
        let quirks = quirks_for(
            "type = \"stdio\"\ncommand = \"npx\"\nargs = [\"chrome-devtools-mcp@latest\"]",
        );
        assert!(
            quirks.iter().any(|q| is_bare_launcher_warning(q)),
            "expected a bare-launcher warning, got {quirks:?}"
        );
    }

    #[test]
    fn bare_node_launcher_is_flagged() {
        let quirks = quirks_for("type = \"stdio\"\ncommand = \"node\"\nargs = [\"server.js\"]");
        assert!(quirks.iter().any(|q| is_bare_launcher_warning(q)));
    }

    #[test]
    fn absolute_path_command_is_not_flagged() {
        let quirks = quirks_for(
            "type = \"stdio\"\ncommand = \"/usr/local/bin/node\"\nargs = [\"server.js\"]",
        );
        assert!(
            !quirks.iter().any(|q| is_bare_launcher_warning(q)),
            "{quirks:?}"
        );
    }

    #[test]
    fn login_shell_wrapper_is_not_flagged() {
        let quirks = quirks_for(
            "type = \"stdio\"\ncommand = \"zsh\"\nargs = [\"-lc\", \"exec npx chrome-devtools-mcp@latest\"]",
        );
        assert!(
            !quirks.iter().any(|q| is_bare_launcher_warning(q)),
            "{quirks:?}"
        );
    }

    #[test]
    fn http_server_is_not_flagged() {
        let quirks = quirks_for("type = \"http\"\nurl = \"https://example.com/mcp\"");
        assert!(
            !quirks.iter().any(|q| is_bare_launcher_warning(q)),
            "{quirks:?}"
        );
    }

    #[test]
    fn unknown_bare_command_is_not_flagged() {
        // A custom binary name outside the known launcher set is assumed to have
        // a stable install location; we don't want false positives on it.
        let quirks = quirks_for("type = \"stdio\"\ncommand = \"my-mcp-server\"\nargs = []");
        assert!(
            !quirks.iter().any(|q| is_bare_launcher_warning(q)),
            "{quirks:?}"
        );
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
}
