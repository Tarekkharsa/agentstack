//! `agentstack doctor` — the trust layer. Static, offline checks across five
//! categories: adapters/CLIs, secrets, drift, quirks, and skills. `--ci` exits
//! nonzero on any error (team gate); `--live` adds MCP `initialize` handshakes;
//! `--fix` re-applies drifted target configs (safe class). Drift/fix operate on
//! global scope.

use std::path::Path;

use anyhow::Result;
use owo_colors::OwoColorize;

use crate::cli::DoctorArgs;
use crate::manifest::{validate, Manifest, ServerType};
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

struct Report {
    errors: usize,
    warnings: usize,
}

impl Report {
    fn new() -> Self {
        Report {
            errors: 0,
            warnings: 0,
        }
    }

    fn line(&mut self, level: Level, msg: impl AsRef<str>) {
        let mark = match level {
            Level::Ok => "✓".green().to_string(),
            Level::Warn => {
                self.warnings += 1;
                "⚠".yellow().to_string()
            }
            Level::Error => {
                self.errors += 1;
                "✗".red().to_string()
            }
        };
        println!("  {mark} {}", msg.as_ref());
    }
}

pub fn run(args: &DoctorArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let ctx = super::load(manifest_dir)?;
    let manifest = &ctx.loaded.manifest;
    let mut report = Report::new();

    // Manifest-level validation first.
    for issue in validate(manifest) {
        report.line(Level::Warn, issue.message);
    }

    let target_ids = resolve_targets(manifest, &ctx.registry, &[]);
    let mut state = State::load()?;
    let mut fixed = 0;

    println!("{}", "Adapters & CLIs".bold());
    for id in &target_ids {
        match ctx.registry.get(id) {
            None => report.line(Level::Error, format!("{id}: unknown adapter")),
            Some(desc) => {
                let path = paths::expand_tilde(&desc.config.path);
                if desc.is_installed() {
                    match desc.read_config_value() {
                        Ok(_) => report.line(
                            Level::Ok,
                            format!("{:<14} installed · {} parses", desc.display, path.display()),
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

    println!("{}", "Secrets".bold());
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

    println!("{}", "Drift".bold());
    let mut any_drift = false;
    for id in &target_ids {
        let Some(desc) = ctx.registry.get(id) else {
            continue;
        };
        let key = target_key(id, Scope::Global);
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
            if args.fix {
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

    println!("{}", "Quirks".bold());
    let quirks = check_quirks(manifest);
    if quirks.is_empty() {
        report.line(Level::Ok, "no unsupported syntax for any target");
    }
    for q in quirks {
        report.line(Level::Warn, q);
    }

    println!("{}", "Skills".bold());
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

    if !manifest.policy.is_empty() {
        println!("{}", "Policy".bold());
        check_policy(manifest, &mut report);
    }

    if args.live {
        println!("{}", "MCP connectivity (--live)".bold());
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

    println!();
    if fixed > 0 {
        println!("{} re-applied {fixed} drifted target(s).", "✓".green());
    }
    println!(
        "{} error(s), {} warning(s).",
        report.errors, report.warnings
    );

    if args.ci && report.errors > 0 {
        std::process::exit(1);
    }
    Ok(())
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
