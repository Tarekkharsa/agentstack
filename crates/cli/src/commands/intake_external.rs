//! Governed intake from external ecosystems.
//!
//! Phase 4. `agentstack add from <url-or-path>` reads supply that was never
//! designed for us — eve-format `SKILL.md` packages, MCP connection
//! definitions, registry JSON — and puts every byte of it through the funnel
//! everything else goes through: **fetch → bound → quarantine → card → yes.**
//!
//! # Their distribution, our governance
//!
//! The point is not to build a registry. It is that anything can flow in,
//! because nothing activates without the yes. An ungated ecosystem where
//! installing means running a setup command behind one y/N prompt is exactly
//! the supply this funnel is for: we consume it, and the consent gate does not
//! move.
//!
//! # Intake never becomes activation
//!
//! Fetching writes nothing a user did not ask for. The bytes land in
//! [`crate::quarantine`], which is inert by construction, and the card is
//! rendered from what is on disk rather than from what was in memory. Declining
//! removes the staging directory, and a witness asserts the Phase 1 property:
//! fetched-then-declined leaves the project byte-identical with nothing to find
//! later.
//!
//! # Everything here is hostile input
//!
//! Parsing lives in [`crate::eve`] and is bounded there — size, count, name
//! grammar, path shape, control characters. This module adds the two bounds
//! that only exist once there is a network: a response size cap enforced while
//! reading rather than after, and a refusal to follow a redirect to a scheme we
//! did not ask for. Nothing fetched is interpolated into a command.

use anyhow::{bail, Context, Result};
use owo_colors::OwoColorize;

use std::path::Path;

use crate::commands::share::Entry;

/// Cap on a fetched body. Enforced while reading, not after: "download it all,
/// then check the size" is not a bound, it is a report on how much memory an
/// attacker was allowed to take.
const MAX_FETCH_BYTES: u64 = 16 * 1024 * 1024;
const FETCH_TIMEOUT_SECS: u64 = 30;

/// Does this look like a source this module handles, rather than a catalog id?
///
/// Deliberately narrow. `add from` already has git-pack and catalog paths, and
/// a loose test here would swallow ids that belong to them — so this claims a
/// source only when it is unambiguously a URL or a filesystem path that exists.
pub fn claims(id: &str) -> bool {
    id.starts_with("https://")
        || id.starts_with("http://")
        || ((id.starts_with("./") || id.starts_with("../") || id.starts_with('/'))
            && Path::new(id).exists())
}

/// The whole funnel for one external source.
pub fn run(id: &str, dir: &Path, assume_yes: bool) -> Result<()> {
    // ── fetch ────────────────────────────────────────────────────────────
    let fetched = fetch(id)?;

    // ── parse, bounded ───────────────────────────────────────────────────
    let parsed = interpret(&fetched, id)?;

    match parsed {
        Parsed::Registry(items) => {
            // A registry is a CATALOG, not content. Listing it installs
            // nothing, and stops here on purpose: browsing and taking are
            // different acts, and collapsing them is how an ungated ecosystem
            // gets you.
            crate::outln!(
                "{} {} from {}",
                "found".green(),
                format!("{} item(s)", items.len()).bold(),
                crate::text::sanitize_line(id).dimmed()
            );
            for item in items.iter().take(50) {
                let lic = item
                    .license
                    .as_deref()
                    .map(|l| format!(" · {l}"))
                    .unwrap_or_default();
                crate::outln!(
                    "  {:<24}{}{}",
                    crate::text::sanitize_line(&item.name).bold(),
                    item.description.as_deref().unwrap_or("").dimmed(),
                    lic.dimmed()
                );
            }
            if items.len() > 50 {
                crate::outln!("  {}", format!("… and {} more", items.len() - 50).dimmed());
            }
            crate::outln!(
                "\n{}",
                "nothing was fetched or staged — add one by name or URL to review it".dimmed()
            );
            Ok(())
        }
        Parsed::Skill(entries) => stage_review_and_decide(entries, dir, id, assume_yes),
        Parsed::Connection(c) => {
            // A server is a DECLARATION, not content: there are no bytes to
            // quarantine, and it activates through the manifest plus the trust
            // gate rather than through this funnel. So the honest thing is to
            // show what was found — with the credentials already turned into
            // `${REF}` — and hand the user the declaration to make.
            report_connection(&c.name, &c.server, &c.refs, id)
        }
    }
}

enum Parsed {
    Skill(Vec<Entry>),
    /// Boxed because `Server` is much larger than the other variants, and an
    /// enum is sized for its biggest one — every `Parsed` on the stack would
    /// otherwise pay for the connection case, which is the rarest of the three.
    Connection(Box<Connection>),
    Registry(Vec<crate::eve::RegistryItem>),
}

struct Connection {
    name: String,
    server: agentstack_core::manifest::Server,
    refs: Vec<String>,
}

/// What was fetched, and from where.
struct Fetched {
    /// (relative path, contents) — one entry for a single document, many for a
    /// directory package.
    files: Vec<(String, String)>,
    /// Package name inferred from the source.
    name: String,
}

fn fetch(id: &str) -> Result<Fetched> {
    if id.starts_with("http://") || id.starts_with("https://") {
        // http:// is accepted but named: a plaintext fetch is a supply chain
        // anyone on the path can rewrite, and the user should know that is what
        // they asked for rather than discovering it later.
        if id.starts_with("http://") {
            crate::outln!(
                "{}",
                "note: this is a plaintext http:// source — anyone on the network path can \
                 change what you receive. The review still happens."
                    .yellow()
            );
        }
        return fetch_url(id);
    }
    fetch_path(Path::new(id))
}

fn fetch_url(url: &str) -> Result<Fetched> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(FETCH_TIMEOUT_SECS))
        // A redirect can change the scheme and the host under us. Following it
        // silently would mean the thing we bounded is not the thing we fetched.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("building the HTTP client")?;
    let resp = client
        .get(url)
        .send()
        .with_context(|| format!("fetching {}", crate::text::sanitize_line(url)))?;

    if resp.status().is_redirection() {
        bail!(
            "{} redirected, and redirects are not followed — the destination would not be the \
             source you reviewed. Fetch the final URL directly.",
            crate::text::sanitize_line(url)
        );
    }
    if !resp.status().is_success() {
        bail!(
            "{} returned {} — nothing was fetched",
            crate::text::sanitize_line(url),
            resp.status().as_u16()
        );
    }
    // Declared length is a hint, not a promise — check it to fail early, then
    // bound the actual read regardless.
    if let Some(len) = resp.content_length() {
        if len > MAX_FETCH_BYTES {
            bail!("the response declares {len} bytes, over the {MAX_FETCH_BYTES}-byte limit");
        }
    }
    let body = read_bounded(resp)?;
    let name = url
        .rsplit('/')
        .find(|s| !s.is_empty())
        .unwrap_or("imported")
        .trim_end_matches(".md")
        .trim_end_matches(".json")
        .to_string();
    Ok(Fetched {
        files: vec![("SKILL.md".to_string(), body)],
        name,
    })
}

/// Read at most [`MAX_FETCH_BYTES`], refusing rather than truncating.
///
/// Truncation would be worse than refusal here: a half-read document that
/// happens to still parse would put content in front of a reviewer that is not
/// what the source actually serves.
fn read_bounded(resp: reqwest::blocking::Response) -> Result<String> {
    use std::io::Read;
    let mut buf = Vec::new();
    let mut limited = resp.take(MAX_FETCH_BYTES + 1);
    limited
        .read_to_end(&mut buf)
        .context("reading the response body")?;
    if buf.len() as u64 > MAX_FETCH_BYTES {
        bail!("the response is over the {MAX_FETCH_BYTES}-byte limit — nothing was parsed");
    }
    String::from_utf8(buf).map_err(|_| {
        anyhow::anyhow!("the response is not UTF-8 text — refusing to treat it as a capability")
    })
}

fn fetch_path(path: &Path) -> Result<Fetched> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "imported".to_string());
    if path.is_file() {
        let body =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "SKILL.md".into());
        return Ok(Fetched {
            files: vec![(file_name, body)],
            name: name
                .trim_end_matches(".md")
                .trim_end_matches(".json")
                .into(),
        });
    }
    let mut files = Vec::new();
    collect(path, path, &mut files)?;
    Ok(Fetched { files, name })
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) -> Result<()> {
    for e in std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .flatten()
    {
        let p = e.path();
        // Never follow a symlink out of the package: a link to `~/.ssh/id_rsa`
        // would otherwise be read and staged as "content".
        if e.file_type().map(|t| t.is_symlink()).unwrap_or(false) {
            continue;
        }
        if p.is_dir() {
            collect(root, &p, out)?;
        } else if let Ok(body) = std::fs::read_to_string(&p) {
            let rel = p
                .strip_prefix(root)
                .unwrap_or(&p)
                .to_string_lossy()
                .to_string();
            out.push((rel, body));
        }
        // Unreadable / non-UTF-8 files are skipped rather than failing the
        // import: a package with a stray binary is common, and the parser
        // requires SKILL.md to exist regardless.
    }
    Ok(())
}

/// Decide what shape the fetched bytes are, and parse accordingly.
fn interpret(f: &Fetched, origin: &str) -> Result<Parsed> {
    let single = f.files.first();
    if let Some((path, body)) = single {
        let trimmed = body.trim_start();
        if f.files.len() == 1 && (path.ends_with(".json") || trimmed.starts_with('[')) {
            // JSON: a registry or a connection. Try registry first — it is the
            // shape with an unambiguous container — then a connection.
            if let Ok(items) = crate::eve::parse_registry(body, origin) {
                if !items.is_empty() {
                    return Ok(Parsed::Registry(items));
                }
            }
            let (name, server, refs) = crate::eve::parse_connection(body, origin)
                .context("this JSON is neither a registry listing nor an MCP connection")?;
            return Ok(Parsed::Connection(Box::new(Connection {
                name,
                server,
                refs,
            })));
        }
    }
    let entries = crate::eve::parse_skill_package(&f.name, &f.files, origin)?;
    Ok(Parsed::Skill(entries))
}

/// Quarantine, card, decide. The heart of the funnel.
fn stage_review_and_decide(
    entries: Vec<Entry>,
    dir: &Path,
    origin: &str,
    assume_yes: bool,
) -> Result<()> {
    let staged = crate::quarantine::stage(dir, &entries)?;

    crate::outln!("{}", "Review — from an external source".bold());
    crate::outln!(
        "  {} {}",
        "origin".dimmed(),
        crate::text::sanitize_line(origin)
    );
    crate::outln!(
        "  {}",
        "unsigned source — nothing vouches for where these bytes came from; review carefully"
            .dimmed()
    );
    crate::outln!();
    crate::outln!(
        "  Adds {} to this project",
        format!("{} file(s)", entries.len()).bold()
    );
    // Attribution on the card, not buried in a file — "Apache-2.0, from <origin>".
    if let Some(line) = attribution(&entries) {
        crate::outln!("  {line}");
    } else {
        // Absence is stated rather than omitted. A source that declared no
        // licence is a fact worth knowing before you take its code.
        crate::outln!(
            "  {}",
            "No licence declared by this source — you are taking it on unknown terms".yellow()
        );
    }
    crate::outln!(
        "\n  {}",
        format!("staged at {} · nothing is active", staged.display()).dimmed()
    );

    if !decided(assume_yes)? {
        crate::quarantine::discard(&staged)?;
        crate::outln!(
            "\n{} nothing was added; the staged copy is gone.",
            "·".dimmed()
        );
        return Ok(());
    }

    let landed = crate::quarantine::adopt(&staged, dir)?;
    crate::outln!("\n{} {} file(s) into this project.", "✓".green(), landed);
    let report = super::doctor::collect(Some(dir))?;
    if let Some(next) = report["next_action"].as_str() {
        crate::outln!("{} {}", "next:".bold(), next.bold());
    }
    Ok(())
}

/// "Apache-2.0, from eve.dev/r/summarize" — the licence and where it came from,
/// on the card, in that order.
fn attribution(entries: &[Entry]) -> Option<String> {
    let mut seen: Vec<String> = Vec::new();
    for e in entries {
        if let Some(lic) = &e.license {
            let s = match &e.origin {
                Some(origin) => format!("{lic}, from {origin}"),
                None => lic.clone(),
            };
            if !seen.contains(&s) {
                seen.push(s);
            }
        }
    }
    if seen.is_empty() {
        return None;
    }
    let carried = entries.iter().filter(|e| e.notice.is_some()).count();
    let mut line = format!("Licensed: {}", seen.join(" · "));
    if carried > 0 {
        // Say that the NOTICE text travels with the content, because that is
        // the part of an attribution obligation a tag alone does not satisfy.
        line.push_str(" · LICENSE/NOTICE text comes with it");
    }
    Some(line)
}

fn decided(assume_yes: bool) -> Result<bool> {
    if assume_yes {
        return Ok(true);
    }
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        crate::outln!(
            "\n{} not a terminal — nothing was added. Re-run with {} to accept.",
            "·".dimmed(),
            "--yes".bold()
        );
        return Ok(false);
    }
    super::panel_edit::confirm("Add this to the project?")
}

fn report_connection(
    name: &str,
    server: &agentstack_core::manifest::Server,
    refs: &[String],
    origin: &str,
) -> Result<()> {
    crate::outln!("{}", "Review — an MCP server definition".bold());
    crate::outln!(
        "  {} {}",
        "origin".dimmed(),
        crate::text::sanitize_line(origin)
    );
    crate::outln!("  {} {}", "name".dimmed(), crate::text::sanitize_line(name));
    if let Some(cmd) = &server.command {
        crate::outln!(
            "  {} {}",
            "will run".dimmed(),
            crate::text::sanitize_line(cmd)
        );
    }
    if let Some(url) = &server.url {
        crate::outln!(
            "  {} {}",
            "will contact".dimmed(),
            crate::text::sanitize_line(url)
        );
    }
    if !refs.is_empty() {
        // The credentials were replaced on the way in. Saying so is the whole
        // value of having done it: the user learns the source shipped live
        // secrets, and that none of them were written down.
        crate::outln!(
            "  {} {} — the source shipped literal values; they were NOT kept",
            "secrets".dimmed(),
            refs.iter()
                .map(|r| format!("${{{r}}}"))
                .collect::<Vec<_>>()
                .join(" · ")
                .bold()
        );
    }
    crate::outln!(
        "\n{}",
        "nothing was written. A server is a declaration, so it activates through the \
         manifest and the trust gate, not through this funnel."
            .dimmed()
    );
    crate::outln!(
        "  add it with: {}",
        format!("agentstack add server {name} …").bold()
    );
    for r in refs {
        crate::outln!("  then: {}", format!("agentstack secret set {r}").bold());
    }
    Ok(())
}
