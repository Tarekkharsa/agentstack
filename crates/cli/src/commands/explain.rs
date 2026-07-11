//! `agentstack explain <capability>` — the trust lens. For a server or skill in
//! the manifest: where it came from, what secrets it needs (and whether they
//! resolve here), which tools get it and the files that get written, and the
//! safety signals (runs code? network egress? needs a secret?). Read-only.

use std::path::Path;

use anyhow::Result;

use crate::cli::ExplainArgs;
use crate::manifest::{ServerType, SkillSource};
use crate::scope::Scope;
use crate::secret::refs_in;
use crate::state::{target_key, State};
use crate::store::{local_source_dir, Store};

pub fn run(args: &ExplainArgs, manifest_dir: Option<&Path>) -> Result<()> {
    print!("{}", explain_text(&args.name, manifest_dir)?);
    Ok(())
}

/// Render the explanation as plain text (shared by the CLI and the MCP tool).
pub fn explain_text(name: &str, manifest_dir: Option<&Path>) -> Result<String> {
    let ctx = crate::commands::load(manifest_dir)?;
    let manifest = &ctx.loaded.manifest;
    let library = crate::library::Library::load_default().unwrap_or_default();
    let in_library_skill = library.get(name).is_some();
    let in_library_server = library.get_server(name).is_some();
    if manifest.servers.contains_key(name) || in_library_server {
        Ok(explain_server(name, &ctx))
    } else if manifest.skills.contains_key(name) || in_library_skill {
        Ok(explain_skill(name, &ctx))
    } else if manifest.instructions.contains_key(name) {
        Ok(explain_instruction(name, &ctx))
    } else {
        anyhow::bail!(
            "no server, skill, or instruction '{name}' in the manifest or central library. Try `agentstack search {name}` to find one to add."
        )
    }
}

/// Instruction fragments are simpler than servers/skills: what matters is
/// where the fragment comes from (this project vs the machine layer), where
/// its source lives, and which harnesses it compiles into.
fn explain_instruction(name: &str, ctx: &crate::commands::Context) -> String {
    let instr = &ctx.loaded.manifest.instructions[name];
    let mut out = format!("# {name} (instruction fragment)\n\n");
    if instr.from_user_layer {
        let layer = ctx
            .loaded
            .user_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "machine manifest".into());
        out.push_str(&format!(
            "Origin: machine layer ({layer}) — merged beneath this project; compiles at GLOBAL scope only.\n"
        ));
    } else {
        out.push_str("Origin: this project's manifest.\n");
    }
    out.push_str(&format!("Source: {}\n", instr.path));
    let src = Path::new(&instr.path);
    let resolved = if src.is_absolute() {
        src.to_path_buf()
    } else {
        ctx.dir.join(src)
    };
    if !resolved.exists() {
        out.push_str("  ✗ source file missing\n");
    }
    out.push_str(&format!(
        "Targets: {} — compiled into each one's CLAUDE.md / AGENTS.md managed region by `agentstack instructions --write` (or `apply`).\n",
        instr.targets.join(", ")
    ));
    out
}

fn explain_server(name: &str, ctx: &crate::commands::Context) -> String {
    let manifest = &ctx.loaded.manifest;
    let library = crate::library::Library::load_default().unwrap_or_default();
    let lib_home = crate::util::paths::lib_home();

    // The definition is inline, or (failing that) from the central library.
    let server = match crate::resolve::resolve_server(manifest, &library, &lib_home, name) {
        Ok(r) => r.server,
        Err(e) => {
            let mut o = String::new();
            line(&mut o, &format!("{name}  (MCP server)\n"));
            kv(&mut o, "Source", "central library");
            kv(&mut o, "Status", &format!("⚠ unresolved — {e}"));
            return o;
        }
    };

    let mut o = String::new();
    let transport = match server.server_type {
        ServerType::Http => "http",
        ServerType::Stdio => "stdio",
    };
    line(&mut o, &format!("{name}  (MCP server · {transport})\n"));

    // Where the definition resolves from + its lockfile status.
    explain_server_lock(&mut o, name, manifest, ctx);

    // Provenance.
    match crate::provider::resolve(name) {
        Some(c) => kv(
            &mut o,
            "Source",
            &format!("known capability \"{}\" ({})", c.name, c.source),
        ),
        None => kv(
            &mut o,
            "Source",
            "custom — added by hand (not in the catalog)",
        ),
    }

    // Endpoint or command.
    match server.server_type {
        ServerType::Http => {
            if let Some(url) = &server.url {
                kv(&mut o, "Endpoint", &format!("{url}   → {}", host_of(url)));
            }
        }
        ServerType::Stdio => {
            let cmd = server.command.clone().unwrap_or_default();
            let args = server.args.join(" ");
            kv(&mut o, "Runs", format!("{cmd} {args}").trim());
        }
    }

    // Secrets it references and whether they resolve on this machine.
    let mut refs: Vec<String> = Vec::new();
    if let Some(u) = &server.url {
        refs.extend(refs_in(u));
    }
    for v in server.headers.values().chain(server.env.values()) {
        refs.extend(refs_in(v));
    }
    refs.sort();
    refs.dedup();
    let sources = crate::secret::SecretSources::detect(&ctx.dir);
    if refs.is_empty() {
        kv(&mut o, "Secrets", "none");
    } else {
        kv(&mut o, "Secrets", &format!("{} referenced", refs.len()));
        for r in &refs {
            let status = match sources.source_of(r) {
                Some(src) => format!("✓ resolves here (from {src})"),
                None => format!("✗ not set — run `agentstack secret set {r}`"),
            };
            o.push_str(&format!("                ${{{r}}}  {status}\n"));
        }
    }

    // Where it writes + which tools get it.
    let state = State::load().unwrap_or_default();
    let default: Vec<&str> = manifest
        .targets
        .default
        .iter()
        .map(String::as_str)
        .collect();
    o.push_str("  Writes to     the MCP config of each tool it's enabled for:\n");
    let mut enabled_summary: Vec<String> = Vec::new();
    for d in ctx.registry.iter() {
        if d.mcp.is_none() {
            continue;
        }
        let g_on = state
            .managed_servers(&target_key(&d.id, Scope::Global, &ctx.dir))
            .iter()
            .any(|s| s == name);
        let p_on = state
            .managed_servers(&target_key(&d.id, Scope::Project, &ctx.dir))
            .iter()
            .any(|s| s == name);
        let default_for = default.iter().any(|t| *t == d.id);
        if let Some(cfg) = &d.config {
            let scope = if g_on {
                " (enabled, global)"
            } else if default_for {
                " (default target)"
            } else {
                ""
            };
            o.push_str(&format!(
                "                {:<14} {}{}\n",
                d.display, cfg.path, scope
            ));
        }
        if let Some(proj) = &d.project {
            let scope = if p_on { " (enabled, project)" } else { "" };
            o.push_str(&format!(
                "                {:<14} <repo>/{}{}\n",
                "", proj.config, scope
            ));
        }
        if g_on {
            enabled_summary.push(format!("{} (global)", d.id));
        }
        if p_on {
            enabled_summary.push(format!("{} (project)", d.id));
        }
    }
    if enabled_summary.is_empty() {
        kv(
            &mut o,
            "Enabled for",
            &format!(
                "not applied yet — would apply to default targets: {}",
                if default.is_empty() {
                    "(none set)".into()
                } else {
                    default.join(", ")
                }
            ),
        );
    } else {
        kv(&mut o, "Enabled for", &enabled_summary.join(", "));
    }

    // Context cost: what this server's tools/list payload taxes every session.
    match crate::footprint::Footprints::load()
        .unwrap_or_default()
        .get(name)
    {
        Some(f) => kv(
            &mut o,
            "Context cost",
            &format!(
                "~{} per session across {} tool(s) ({})",
                crate::footprint::fmt_tokens(f.est_tokens),
                f.tools,
                crate::footprint::fmt_age(f.measured_at)
            ),
        ),
        None => kv(
            &mut o,
            "Context cost",
            "unmeasured — run `agentstack stats --live`",
        ),
    }

    // Tool firewall: what [policy.tools] does to this server at the gateway —
    // BOTH layers, or the view would understate enforcement. The machine layer
    // (checked first, deny precedence) matches the exact name and the `"*"`
    // wildcard key.
    let rule_summary = |rules: &[String]| {
        let denies: Vec<&str> = rules.iter().filter_map(|r| r.strip_prefix('!')).collect();
        let allows: Vec<&str> = rules
            .iter()
            .filter(|r| !r.starts_with('!'))
            .map(String::as_str)
            .collect();
        let mut parts = Vec::new();
        if !allows.is_empty() {
            parts.push(format!("allow only [{}]", allows.join(", ")));
        }
        if !denies.is_empty() {
            parts.push(format!("deny [{}]", denies.join(", ")));
        }
        parts.join("; ")
    };
    if let Some(rules) = manifest.policy.tools.get(name) {
        kv(
            &mut o,
            "Tool policy",
            &format!(
                "{} — enforced at the gateway; denied tools are invisible to agents and refused if called",
                rule_summary(rules)
            ),
        );
    }
    let machine = crate::manifest::machine_policy();
    for key in [name, "*"] {
        if let Some(rules) = machine.tools.get(key) {
            let scope = if key == "*" { " (via \"*\")" } else { "" };
            kv(
                &mut o,
                "Tool policy (machine)",
                &format!(
                    "{}{scope} — from ~/.agentstack/agentstack.toml, checked before project policy; this project cannot loosen it",
                    rule_summary(rules)
                ),
            );
        }
    }
    // The sibling per-server dimensions, same two-layer display. Egress is
    // checked against the DECLARED URL host at write/spawn time (runtime
    // filtering is Phase 2); secrets are enforced fail-closed at render and
    // gateway substitution.
    if let Some(rules) = manifest.policy.egress.get(name) {
        kv(
            &mut o,
            "Egress (policy)",
            &format!(
                "{} — declared URL host checked at write/spawn time; runtime filtering is Phase 2",
                rule_summary(rules)
            ),
        );
    }
    if let Some(rules) = manifest.policy.secrets.get(name) {
        kv(
            &mut o,
            "Secret access (policy)",
            &format!(
                "{} — refs outside this set never resolve for this server",
                rule_summary(rules)
            ),
        );
    }
    for key in [name, "*"] {
        let scope = if key == "*" { " (via \"*\")" } else { "" };
        if let Some(rules) = machine.egress.get(key) {
            kv(
                &mut o,
                "Egress (machine)",
                &format!(
                    "{}{scope} — this project cannot loosen it",
                    rule_summary(rules)
                ),
            );
        }
        if let Some(rules) = machine.secrets.get(key) {
            kv(
                &mut o,
                "Secret access (machine)",
                &format!(
                    "{}{scope} — this project cannot loosen it",
                    rule_summary(rules)
                ),
            );
        }
    }

    // Safety signals.
    o.push_str("  Safety\n");
    match server.server_type {
        ServerType::Stdio => bullet(
            &mut o,
            "⚠ runs a local process on your machine — review the command/package",
        ),
        ServerType::Http => {
            let host = host_of(server.url.as_deref().unwrap_or(""));
            let ruleset = crate::render::ruleset_for(manifest);
            // Annotate the declared host against the effective egress policy
            // when one constrains this server — the same check apply/gateway
            // enforce at write/spawn time.
            let verdict = if ruleset.egress_constrained(name) {
                match crate::render::declared_host(server.url.as_deref().unwrap_or("")) {
                    Some(h) => match ruleset.egress_decision(name, &h, None) {
                        Ok(()) => " — passes [policy.egress]",
                        Err(_) => " — ✗ BLOCKED by [policy.egress] at write/spawn time",
                    },
                    None => " — host not statically verifiable; fails closed at write/spawn time",
                }
            } else {
                ""
            };
            bullet(
                &mut o,
                &format!("connects out to {host} (network egress){verdict}"),
            );
        }
    }
    if refs.is_empty() {
        bullet(&mut o, "needs no secrets");
    } else {
        let resolved = refs
            .iter()
            .filter(|r| sources.source_of(r).is_some())
            .count();
        bullet(&mut o, &format!("needs {} secret(s) ({resolved} resolve here) — kept as ${{REF}}, never written as plaintext", refs.len()));
    }
    if crate::provider::resolve(name)
        .map(|c| c.trust().namespaced)
        .unwrap_or(false)
    {
        bullet(&mut o, "from a verified, namespaced catalog entry");
    }
    o
}

fn explain_skill(name: &str, ctx: &crate::commands::Context) -> String {
    let manifest = &ctx.loaded.manifest;
    let store = Store::default_store();
    let library = crate::library::Library::load_default().unwrap_or_default();
    let lib_home = crate::util::paths::lib_home();

    // A skill is defined inline in the project manifest, or (failing that) in the
    // central library. Compute its source + local dir from wherever it lives.
    let inline = manifest.skills.get(name);
    let from_library = inline.is_none();
    let source: Option<SkillSource> = if let Some(skill) = inline {
        skill.source().ok()
    } else if let Some(entry) = library.get(name) {
        crate::manifest::Skill {
            path: entry.path.clone(),
            git: entry.git.clone(),
            rev: entry.rev.clone(),
            subpath: entry.subpath.clone(),
        }
        .source()
        .ok()
    } else {
        None
    };

    let mut o = String::new();
    let kind = match &source {
        Some(SkillSource::Git { .. }) => "git",
        Some(SkillSource::Path(_)) => "path",
        None => "?",
    };
    line(&mut o, &format!("{name}  (skill · {kind})\n"));

    if from_library {
        kv(&mut o, "Source", "central library (`agentstack lib`)");
    } else {
        match &source {
            Some(SkillSource::Git { url, rev, subpath }) => kv(
                &mut o,
                "Source",
                &format!(
                    "git {url}{}{}",
                    rev.as_ref().map(|r| format!(" @ {r}")).unwrap_or_default(),
                    subpath
                        .as_ref()
                        .map(|s| format!(" (subpath {s})"))
                        .unwrap_or_default()
                ),
            ),
            Some(SkillSource::Path(p)) => kv(&mut o, "Source", &format!("local path {p}")),
            None => kv(&mut o, "Source", "unknown"),
        }
    }

    // Resolve the local dir offline (never fetch) for description + installed.
    let local: Option<std::path::PathBuf> = if let Some(skill) = inline {
        local_source_dir(&store, skill, &ctx.dir)
    } else {
        crate::resolve::resolve_skill(
            manifest,
            &ctx.dir,
            &library,
            &lib_home,
            &store,
            name,
            crate::resolve::ResolveMode::NoFetch,
        )
        .ok()
        .map(|r| r.path)
        .filter(|p| p.exists())
    };
    if let Some(dir) = &local {
        if let Some(desc) = skill_description(dir) {
            kv(&mut o, "Description", &desc);
        }
        kv(&mut o, "Installed", "yes — available locally");
    } else {
        kv(&mut o, "Installed", "no — run `agentstack install`");
    }

    // Reproducibility detail: provenance (if central-library) and how the
    // resolved content compares to agentstack.lock. Git-backed skills are shown
    // from the lock without resolving (keeps explain offline).
    explain_lock(&mut o, name, manifest, ctx);

    let state = State::load().unwrap_or_default();
    o.push_str("  Writes to     each tool's skills dir when enabled:\n");
    let mut enabled_summary: Vec<String> = Vec::new();
    for d in ctx.registry.iter() {
        let Some(sk) = &d.skills else { continue };
        let g_on = state
            .managed_skills(&target_key(&d.id, Scope::Global, &ctx.dir))
            .iter()
            .any(|s| s == name);
        let p_on = state
            .managed_skills(&target_key(&d.id, Scope::Project, &ctx.dir))
            .iter()
            .any(|s| s == name);
        let strat = format!("{:?}", sk.strategy).to_lowercase();
        o.push_str(&format!(
            "                {:<14} {}/{}  ({strat}{})\n",
            d.display,
            sk.dir,
            name,
            if g_on { ", enabled global" } else { "" }
        ));
        if let Some(pd) = &sk.project_dir {
            o.push_str(&format!(
                "                {:<14} <repo>/{}/{}{}\n",
                "",
                pd,
                name,
                if p_on { "  (enabled, project)" } else { "" }
            ));
        }
        if g_on {
            enabled_summary.push(format!("{} (global)", d.id));
        }
        if p_on {
            enabled_summary.push(format!("{} (project)", d.id));
        }
    }
    if !enabled_summary.is_empty() {
        kv(&mut o, "Enabled for", &enabled_summary.join(", "));
    }

    o.push_str("  Safety\n");
    bullet(
        &mut o,
        "instructions only — a skill is text, it runs no code and makes no network calls",
    );
    if from_library {
        bullet(
            &mut o,
            "comes from the central library — inspect it with `agentstack lib list`",
        );
    } else {
        match &source {
            Some(SkillSource::Git { url, .. }) => bullet(
                &mut o,
                &format!("comes from git {url} — review the source you trust it from"),
            ),
            Some(SkillSource::Path(p)) => bullet(
                &mut o,
                &format!("comes from a local path ({p}) in this repo"),
            ),
            None => {}
        }
    }
    o
}

/// Append provenance + lockfile-status detail for a skill. Neutral, explain-style
/// wording (drift is noted with ⚠, not treated as an error — that severity lives
/// in `doctor`).
fn explain_lock(
    o: &mut String,
    name: &str,
    manifest: &crate::manifest::Manifest,
    ctx: &crate::commands::Context,
) {
    use crate::resolve::{ResolveMode, SkillLockStatus as S};
    let lock = crate::lock::Lock::load(&ctx.dir).unwrap_or_default();
    let library = crate::library::Library::load_default().unwrap_or_default();

    // Offline (`NoFetch`): a git body that isn't cached surfaces as a status,
    // not a fetch. The resolver carries the case; no git pre-check here.
    let lib_home = crate::util::paths::lib_home();
    let store = Store::default_store();
    let r = crate::resolve::skill_lock_status(
        name,
        manifest,
        &ctx.dir,
        &library,
        &lib_home,
        &store,
        &lock,
        ResolveMode::NoFetch,
    );
    if let Some(origin) = r.origin {
        kv(o, "Resolves", origin_label(origin));
    }
    if let Some(p) = &r.provenance {
        kv(o, "Provenance", p);
    }
    let msg = match &r.status {
        S::Matches => "matches lock".to_string(),
        S::MissingLockEntry => "not locked ↳ agentstack use <profile> --write".to_string(),
        S::ChecksumDrift { .. } => "⚠ content drifted from lock".to_string(),
        S::RevDrift { locked, current } => format!("⚠ rev drifted: locked {locked}, now {current}"),
        S::NotAvailableOffline { .. } => {
            "git-backed · not cached (run `agentstack install`)".to_string()
        }
        S::ResolveFailed { error } => format!("⚠ unresolved — {error}"),
    };
    kv(o, "Lock", &msg);
}

fn origin_label(origin: crate::resolve::SkillOrigin) -> &'static str {
    match origin {
        crate::resolve::SkillOrigin::Inline => "inline (this project)",
        crate::resolve::SkillOrigin::Library => "central library",
    }
}

/// Append where a server resolves from + its lockfile status. Neutral wording;
/// drift is noted with ⚠, not treated as an error (severity lives in `doctor`).
/// Only the definition digest is compared — `${REF}`s stay placeholders.
fn explain_server_lock(
    o: &mut String,
    name: &str,
    manifest: &crate::manifest::Manifest,
    ctx: &crate::commands::Context,
) {
    use crate::resolve::{ServerLockStatus as S, ServerOrigin};
    let lock = crate::lock::Lock::load(&ctx.dir).unwrap_or_default();
    let library = crate::library::Library::load_default().unwrap_or_default();
    let lib_home = crate::util::paths::lib_home();
    let r = crate::resolve::server_lock_status(name, manifest, &library, &lib_home, &lock);
    if let Some(origin) = r.origin {
        kv(
            o,
            "Resolves",
            match origin {
                ServerOrigin::Inline => "inline (this project)",
                ServerOrigin::Library => "central library",
            },
        );
    }
    if let Some(p) = &r.provenance {
        kv(o, "Provenance", p);
    }
    let msg = match &r.status {
        S::Matches => "matches lock".to_string(),
        S::MissingLockEntry => "not locked ↳ agentstack use <profile> --write".to_string(),
        S::ChecksumDrift { .. } => "⚠ definition drifted from lock".to_string(),
        S::ResolveFailed { error } => format!("⚠ unresolved — {error}"),
    };
    kv(o, "Lock", &msg);
}

/* ---------- helpers ---------- */

fn line(o: &mut String, s: &str) {
    o.push('\n');
    o.push_str(s);
    o.push('\n');
}
fn kv(o: &mut String, k: &str, v: &str) {
    o.push_str(&format!("  {:<12}  {v}\n", k));
}
fn bullet(o: &mut String, s: &str) {
    o.push_str(&format!("                • {s}\n"));
}

/// Host portion of a URL, for a glanceable "where does this connect" signal.
fn host_of(url: &str) -> String {
    let after = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    after.split(['/', '?']).next().unwrap_or(after).to_string()
}

/// The `description:` from a skill's `SKILL.md` frontmatter, if present.
fn skill_description(source: &Path) -> Option<String> {
    let text = std::fs::read_to_string(source.join("SKILL.md")).ok()?;
    let rest = text.trim_start().strip_prefix("---")?;
    let end = rest.find("\n---")?;
    rest[..end]
        .lines()
        .find_map(|l| l.trim().strip_prefix("description:"))
        .map(|v| v.trim().trim_matches('"').trim_matches('\'').to_string())
}
