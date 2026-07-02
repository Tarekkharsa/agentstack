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
    } else {
        anyhow::bail!(
            "no server or skill '{name}' in the manifest or central library. Try `agentstack search {name}` to find one to add."
        )
    }
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

    // Safety signals.
    o.push_str("  Safety\n");
    match server.server_type {
        ServerType::Stdio => bullet(
            &mut o,
            "⚠ runs a local process on your machine — review the command/package",
        ),
        ServerType::Http => bullet(
            &mut o,
            &format!(
                "connects out to {} (network egress)",
                host_of(server.url.as_deref().unwrap_or(""))
            ),
        ),
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
            Some(SkillSource::Git { url, rev }) => kv(
                &mut o,
                "Source",
                &format!(
                    "git {url}{}",
                    rev.as_ref().map(|r| format!(" @ {r}")).unwrap_or_default()
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
