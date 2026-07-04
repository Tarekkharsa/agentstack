//! `agentstack doctor` — the trust layer. Static, offline checks across five
//! categories: adapters/CLIs, secrets, drift, quirks, and skills. `--ci` exits
//! nonzero on any error (team gate); `--live` adds MCP `initialize` handshakes;
//! `--fix` re-applies drifted target configs (safe class). Drift/fix operate on
//! global scope.

use std::path::Path;

use anyhow::Result;
use owo_colors::OwoColorize;

use crate::cli::DoctorArgs;
use crate::manifest::{validate_with_context, Manifest, ServerType};
use crate::render::{plan_target, resolve_targets, Selection};
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

/// Accumulates every check result (grouped by section) while printing the
/// familiar terminal report as it goes — unless `quiet`, which is how the
/// dashboard runs the same checks and renders them itself.
struct Report {
    errors: usize,
    warnings: usize,
    quiet: bool,
    sections: Vec<Section>,
}

struct Section {
    title: String,
    /// (level, message) — level is `ok` / `warn` / `error`.
    lines: Vec<(&'static str, String)>,
}

impl Report {
    fn new() -> Self {
        Report {
            errors: 0,
            warnings: 0,
            quiet: false,
            sections: Vec::new(),
        }
    }

    fn quiet() -> Self {
        Report {
            quiet: true,
            ..Report::new()
        }
    }

    fn section(&mut self, title: &str) {
        if !self.quiet {
            println!("{}", title.bold());
        }
        self.sections.push(Section {
            title: title.to_string(),
            lines: Vec::new(),
        });
    }

    fn line(&mut self, level: Level, msg: impl AsRef<str>) {
        let (mark, tag) = match level {
            Level::Ok => ("✓".green().to_string(), "ok"),
            Level::Warn => {
                self.warnings += 1;
                ("⚠".yellow().to_string(), "warn")
            }
            Level::Error => {
                self.errors += 1;
                ("✗".red().to_string(), "error")
            }
        };
        if !self.quiet {
            println!("  {mark} {}", msg.as_ref());
        }
        if self.sections.is_empty() {
            // Validation issues land before the first titled section.
            self.sections.push(Section {
                title: "Manifest".to_string(),
                lines: Vec::new(),
            });
        }
        self.sections
            .last_mut()
            .expect("section exists")
            .lines
            .push((tag, msg.as_ref().to_string()));
    }

    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "errors": self.errors,
            "warnings": self.warnings,
            "sections": self.sections.iter().map(|s| serde_json::json!({
                "title": s.title,
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
    let mut report = Report::quiet();
    run_checks(
        &DoctorArgs {
            ci: false,
            live: false,
            fix: false,
            deep: true,
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
            "no harness connected — optional ↳ agentstack connect --all",
        );
    }
    let base = crate::manifest::project_root_of(&ctx.dir);
    match crate::trust::check(&base) {
        crate::trust::TrustState::Trusted => {
            report.line(Level::Ok, "this project is trusted for auto mode")
        }
        crate::trust::TrustState::Changed => report.line(
            Level::Warn,
            "trusted, but the manifest changed since ↳ review + agentstack trust",
        ),
        // Untrusted is a choice, not a fault (Ok) — unless a harness actually
        // uses the bridge AND the manifest declares servers: then every session
        // here silently gets control-plane tools only, which is worth a warning.
        crate::trust::TrustState::Untrusted => {
            if connected > 0 && !manifest.servers.is_empty() {
                report.line(
                    Level::Warn,
                    format!(
                        "not trusted — {connected} harness(es) use the bridge, but this project's {} server(s) are not proxied ↳ agentstack trust {}",
                        manifest.servers.len(),
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

    report.section("Secrets");
    let refs = manifest.referenced_secrets();
    if refs.is_empty() {
        report.line(Level::Ok, "no secrets referenced");
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
    for id in &target_ids {
        let Some(desc) = ctx.registry.get(id) else {
            continue;
        };
        let key = target_key(id, Scope::Global, &ctx.dir);
        let previously = state.managed_servers(&key);
        let Some(plan) = plan_target(
            manifest,
            desc,
            &ctx.resolver,
            &Selection::All,
            &previously,
            Scope::Global,
            &ctx.dir,
        )?
        else {
            continue;
        };

        // Hand-edit since our last write?
        if let Some(ts) = state.targets.get(&key) {
            if !ts.last_hash.is_empty() {
                let on_disk = state::hash(&plan.existing);
                if on_disk != ts.last_hash {
                    any_drift = true;
                    report.line(
                        Level::Warn,
                        format!("{:<14} edited on disk since last apply", desc.display),
                    );
                }
            }
        }
        // Pending manifest changes?
        if plan.changed() {
            // An unresolved `${REF}` must never reach a live config — same gate
            // as `apply`/`toggle`. `doctor --fix` has no override, so we refuse.
            if args.fix && !plan.unresolved.is_empty() {
                any_drift = true;
                report.line(
                    Level::Error,
                    format!(
                        "{:<14} not fixed — unresolved secret(s): {}",
                        desc.display,
                        plan.unresolved.join(", ")
                    ),
                );
            } else if args.fix {
                plan.write()?;
                state.record(&key, plan.managed.clone(), &plan.proposed);
                fixed += 1;
                report.line(
                    Level::Ok,
                    format!(
                        "{:<14} re-applied {} change(s)",
                        desc.display,
                        plan.managed.len()
                    ),
                );
            } else {
                any_drift = true;
                report.line(
                    Level::Warn,
                    format!(
                        "{:<14} {} change(s) pending ↳ agentstack apply --write",
                        desc.display,
                        plan.managed.len().max(plan.removed.len())
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

    report.section("Quirks");
    let quirks = check_quirks(manifest);
    if quirks.is_empty() {
        report.line(Level::Ok, "no unsupported syntax for any target");
    }
    for q in quirks {
        report.line(Level::Warn, q);
    }

    report.section("Skills");
    if manifest.skills.is_empty() {
        report.line(Level::Ok, "no skills defined");
    }
    let store = crate::store::Store::default_store();
    for (name, skill) in &manifest.skills {
        match crate::store::local_source_dir(&store, skill, &ctx.dir) {
            None => report.line(
                Level::Warn,
                format!("{name:<20} not installed ↳ agentstack install"),
            ),
            Some(dir) if !dir.join("SKILL.md").exists() => report.line(
                Level::Warn,
                format!("{name:<20} no SKILL.md in {}", dir.display()),
            ),
            Some(_) => report.line(Level::Ok, format!("{name:<20} present · SKILL.md ok")),
        }
    }

    // Supply-chain content scan (same detectors as `agentstack audit`): hidden
    // Unicode is an error so `--ci` gates it; injection heuristics only warn.
    // It reads every skill body, so the everyday run skips it — `--deep` opts
    // in and `--ci` (the trust gate) always includes it.
    report.section("Content scan");
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
    check_reproducibility(manifest, &ctx.dir, &store, report);
    check_server_reproducibility(manifest, &ctx.dir, report);

    report.section("Plugin recipes");
    let recipe_statuses = crate::plugin_recipes::statuses(manifest, &ctx.registry, &ctx.dir);
    if recipe_statuses.is_empty() {
        report.line(Level::Ok, "no plugin recipes defined");
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
        if !recipe.generated {
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

    if !manifest.policy.is_empty() {
        report.section("Policy");
        check_policy(manifest, report);
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
                        format!(
                            "{name:<20} content drifted from lock ↳ agentstack use <profile> --write"
                        ),
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
                            format!(
                                "{name:<20} from library, not locked ↳ agentstack use <profile> --write"
                            ),
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
                    report.line(Level::Error, format!("{name:<20} broken server ref — {error}"));
                }
                ServerLockStatus::ChecksumDrift { .. } => report.line(
                    Level::Error,
                    format!("{name:<20} server definition drifted from lock ↳ agentstack use <profile> --write"),
                ),
                ServerLockStatus::MissingLockEntry => {
                    if r.origin == Some(ServerOrigin::Library) {
                        report.line(
                            Level::Warn,
                            format!("{name:<20} library server, not locked ↳ agentstack use <profile> --write"),
                        );
                    }
                }
                ServerLockStatus::Matches => {
                    if r.origin == Some(ServerOrigin::Library) {
                        report.line(Level::Ok, format!("{name:<20} library server · matches lock"));
                    }
                }
            }
        }
    }
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
    // [policy.tools] rules must name real servers — a typo'd server name would
    // silently firewall nothing.
    for (server, rules) in &manifest.policy.tools {
        if manifest.servers.contains_key(server) {
            let denies = rules.iter().filter(|r| r.starts_with('!')).count();
            let allows = rules.len() - denies;
            report.line(
                Level::Ok,
                format!("tools '{server}' — {allows} allow / {denies} deny rule(s), enforced at the gateway"),
            );
        } else {
            report.line(
                Level::Error,
                format!("[policy.tools] '{server}' — no such server in the manifest"),
            );
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

/// Detect per-target syntax a CLI can't handle, before it breaks at runtime.
fn check_quirks(manifest: &Manifest) -> Vec<String> {
    let mut out = Vec::new();
    for (name, server) in &manifest.servers {
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

    /// The one line under "Content scan" for a run with the given flags.
    fn scan_line(deep: bool, ci: bool, proj: &Path) -> String {
        let mut report = Report::quiet();
        run_checks(
            &DoctorArgs {
                ci,
                live: false,
                fix: false,
                deep,
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
}
