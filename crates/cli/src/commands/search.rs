//! `agentstack search <query>` — discovery across all providers (PLAN §9g/§9h):
//! the embedded catalog and the official MCP Registry. Marks what's already in
//! the manifest and prints how to add the rest. The agent's discovery surface.

use std::path::Path;

use anyhow::Result;
use owo_colors::OwoColorize;

use crate::cli::SearchArgs;
use crate::provider::{self, CandidateKind};

pub fn run(args: &SearchArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let query = args.query.clone().unwrap_or_default();
    if query.trim().is_empty() {
        // An empty query is not an error, so `--json` answers it the same way
        // it answers "no matches": the echoed (empty) query and an empty result
        // set. A caller that wanted results can see it never asked for any,
        // without parsing a usage sentence written for a human.
        if args.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&search_json(&query, &[], &[]))?
            );
            return Ok(());
        }
        println!(
            "Usage: agentstack search <query>  (searches your central library + the catalog + official MCP Registry)"
        );
        return Ok(());
    }

    let mut results = provider::search_all(&query, 25);
    rank(&mut results, &query);

    // A capability is "installed" if its server is in the manifest, or — for a
    // pack — if its `[packs.<name>]` install ledger exists.
    let installed = super::load(manifest_dir)
        .ok()
        .map(|ctx| {
            let m = &ctx.loaded.manifest;
            m.servers
                .keys()
                .chain(m.skills.keys())
                .chain(m.packs.keys())
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&search_json(&query, &results, &installed))?
        );
        return Ok(());
    }

    if results.is_empty() {
        println!("No matches for '{query}' (library, catalog, or registry).");
        return Ok(());
    }

    let total = results.len();
    let shown: Vec<&provider::Candidate> = if args.all {
        results.iter().collect()
    } else {
        page(&results)
    };
    let hidden = total - shown.len();

    if hidden == 0 {
        println!("{total} result(s) for '{query}':\n");
    } else {
        println!(
            "{} of {total} result(s) for '{query}', most relevant first:\n",
            shown.len()
        );
    }

    let mut current_source = "";
    for c in shown.iter().copied() {
        // A source heading only earns its line when the source changes, so a
        // single-source result set reads exactly as it did before.
        if c.source != current_source {
            current_source = c.source;
            println!("{}", source_heading(c.source).dimmed());
        }

        let added = installed.contains(&c.name);
        let mut badge = String::new();
        match &c.kind {
            CandidateKind::Pack(_) => badge.push_str(&format!(" {}", "[pack]".magenta())),
            CandidateKind::Skill(_) => badge.push_str(&format!(" {}", "[skill]".cyan())),
            CandidateKind::Extension(_) => badge.push_str(&format!(" {}", "[extension]".red())),
            CandidateKind::Hook(_) => badge.push_str(&format!(" {}", "[hook]".blue())),
            CandidateKind::Server(_) => {}
        }
        if added {
            badge.push_str(&format!(" {}", "(in manifest)".green()));
        }
        println!(
            "  {} {}{badge}",
            c.name.bold(),
            truncate(&c.description, 70)
        );
        if c.id != c.name {
            println!("    {}", c.id.dimmed());
        }
        // Composition / source line per kind.
        match &c.kind {
            CandidateKind::Pack(spec) => {
                let mut parts = Vec::new();
                if spec.server.is_some() {
                    parts.push("1 server".to_string());
                }
                if !spec.skills.is_empty() {
                    parts.push(format!("{} skill", spec.skills.len()));
                }
                if !spec.instructions.is_empty() {
                    parts.push(format!("{} instruction", spec.instructions.len()));
                }
                if !parts.is_empty() {
                    println!("    {} {}", "contains:".dimmed(), parts.join(" · "));
                }
            }
            CandidateKind::Skill(skill) => {
                let source = skill
                    .path
                    .as_deref()
                    .map(|p| format!("path:{p}"))
                    .or_else(|| skill.git.as_deref().map(|g| format!("git:{g}")))
                    .unwrap_or_else(|| "—".into());
                println!("    {} {source}", "source:".dimmed());
            }
            CandidateKind::Extension(ext) => {
                println!("    {} {} extension", "target:".dimmed(), ext.target);
            }
            CandidateKind::Hook(h) => {
                let matcher = h
                    .hook
                    .matcher
                    .as_deref()
                    .map(|m| format!(" · matcher {m}"))
                    .unwrap_or_default();
                println!("    {} {}{matcher}", "event:".dimmed(), h.hook.event);
            }
            CandidateKind::Server(_) => {}
        }
        // Extensions carry the strongest, honest warning of any kind — their
        // code runs in-process, ungoverned at runtime (design doc §7) — rather
        // than the generic "runs code (npx)" line, which is stdio-shaped.
        if let CandidateKind::Extension(_) = &c.kind {
            println!(
                "    {} {}",
                "trust:".to_string().dimmed(),
                "⚠ executable, in-process, ungoverned at runtime (agentstack pins provenance only)"
                    .red()
            );
        } else {
            let t = c.trust();
            let mut signals = Vec::new();
            if t.namespaced {
                signals.push("✓ verified namespace".green().to_string());
            }
            if t.runs_code {
                signals.push("⚠ runs code (npx)".yellow().to_string());
            }
            if t.needs_secret {
                signals.push("needs secret".dimmed().to_string());
            }
            if !signals.is_empty() {
                println!("    trust: {}", signals.join(" · "));
            }
        }
        if let CandidateKind::Extension(ext) = &c.kind {
            // Extensions aren't installed via `add from`; they are referenced by
            // name in the manifest, which re-gates trust + lock on the code.
            println!(
                "    {} reference it in [extensions.{}] with target = \"{}\", then `agentstack lock`",
                "↳".cyan(),
                c.name,
                ext.target
            );
        } else {
            // Every hit gets an action, including the one already installed.
            // The best match for a query is frequently something you added
            // last week, and a result line with no next step reads as a dead
            // end — which is what sends people back to a browser tab.
            let cmd = if added {
                format!("agentstack explain {}", c.name)
            } else if c.source == "catalog" {
                format!("agentstack add from {}", c.name)
            } else {
                format!("agentstack add from {}", c.id)
            };
            println!("    {} {cmd}", "↳".cyan());
        }
        println!();
    }

    if hidden > 0 {
        println!(
            "{}",
            format!("{hidden} more — agentstack search '{query}' --all").dimmed()
        );
    }
    Ok(())
}

/// Where a candidate came from, as a heading rather than a per-line tag.
/// Grouping matters because the three sources answer different questions: your
/// own library is what you already curated, the catalog is what ships reviewed,
/// and the registry is the open ecosystem.
fn source_heading(source: &str) -> &'static str {
    match source {
        "library" => "your library",
        "catalog" => "catalog",
        _ => "MCP registry",
    }
}

/// Relevance score for one candidate against the query. Higher is better.
///
/// The point is that an exact name and a substring buried in a description are
/// not the same answer. Before this, `search github` put the thing actually
/// called `github` above an Obsidian-vault server only by accident of provider
/// order, and often did not.
///
/// The ladder, most to least specific: the name IS the query · the name starts
/// with it · the last segment of a reverse-DNS id is the query (so
/// `io.github.github/github-mcp-server` scores as `github-mcp-server`) · the
/// name contains it · only the description mentions it. A verified namespace
/// adds a point as a tiebreak, never as a way to outrank a better name match —
/// which is why it is worth one and the name tiers are worth ten or more.
fn score(c: &provider::Candidate, query: &str) -> u32 {
    let q = query.trim().to_ascii_lowercase();
    let name = c.name.to_ascii_lowercase();
    let id = c.id.to_ascii_lowercase();
    let last = id.rsplit('/').next().unwrap_or(&id);

    let mut s = if name == q {
        100
    } else if last == q {
        80
    } else if name.starts_with(&q) {
        60
    } else if last.starts_with(&q) {
        50
    } else if name.contains(&q) {
        30
    } else if id.contains(&q) {
        20
    } else {
        // Description-only: it matched something, but nothing you named.
        10
    };
    if c.trust().namespaced {
        s += 1;
    }
    s
}

/// Sort in place by descending relevance, then by name so equal scores are
/// stable across runs — a search whose order changes between two identical
/// invocations is not a search anyone can build a habit on.
fn rank(results: &mut [provider::Candidate], query: &str) {
    results.sort_by(|a, b| {
        score(b, query)
            .cmp(&score(a, query))
            .then_with(|| a.name.cmp(&b.name))
    });
}

/// The default page: the most relevant few, at most three from any one source.
///
/// Both caps exist for the same reason. The total keeps the first screen
/// readable; the per-source cap keeps one deep provider from filling it, so a
/// good catalog hit is never pushed off by three mediocre registry ones. Order
/// within the page is still pure relevance — grouping is how it is printed, not
/// how it is chosen.
fn page(results: &[provider::Candidate]) -> Vec<&provider::Candidate> {
    const PAGE: usize = 5;
    const PER_SOURCE: usize = 3;

    let mut taken: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    let mut out = Vec::new();
    for c in results {
        if out.len() == PAGE {
            break;
        }
        let n = taken.entry(c.source).or_insert(0);
        if *n < PER_SOURCE {
            *n += 1;
            out.push(c);
        }
    }
    // Print grouped: relevance decided membership, the source decides layout.
    out.sort_by_key(|c| source_rank(c.source));
    out
}

/// Group order on screen. Your own library first — it is the one source whose
/// contents you already vetted.
fn source_rank(source: &str) -> u8 {
    match source {
        "library" => 0,
        "catalog" => 1,
        _ => 2,
    }
}

fn truncate(s: &str, n: usize) -> String {
    crate::text::truncate_chars(s, n)
}

/// The kind-specific half of a result: whatever the text prints on the line
/// under the headline, as named fields. `null` for a plain server, which has
/// no such line. One key whose shape is decided by `kind` beats five
/// mostly-absent top-level keys — a consumer branches on `kind` anyway.
fn candidate_details(kind: &CandidateKind) -> serde_json::Value {
    match kind {
        CandidateKind::Server(_) => serde_json::Value::Null,
        CandidateKind::Pack(spec) => serde_json::json!({
            "server": spec.server.is_some(),
            "skills": spec.skills.len(),
            "instructions": spec.instructions.len(),
        }),
        CandidateKind::Skill(skill) => serde_json::json!({
            "path": skill.path.as_deref().map(crate::text::sanitize_line),
            "git": skill.git.as_deref().map(crate::text::sanitize_line),
        }),
        CandidateKind::Extension(ext) => serde_json::json!({
            "target": crate::text::sanitize_line(&ext.target),
        }),
        CandidateKind::Hook(h) => serde_json::json!({
            "event": crate::text::sanitize_line(&h.hook.event),
            "matcher": h.hook.matcher.as_deref().map(crate::text::sanitize_line),
        }),
    }
}

/// The enveloped `search --json` body (contract `json-reads-v1`): the same
/// candidates the text prints, keyed rather than laid out.
///
/// Two deliberate differences from the screen, both rendering concessions
/// rather than different data. Descriptions ship whole — the 70-column
/// truncation exists for a terminal, not for a consumer. And the trust signals
/// ship as three booleans instead of the sentence the screen assembles, so a
/// caller filters on `runs_code` rather than matching on a warning glyph. The
/// extension warning is not a fourth signal: it is what `kind == "extension"`
/// plus `runs_code` already means, spelled out for a human.
///
/// Results arrive ranked, most relevant first — the same order the screen
/// prints, before the screen's page cap. `--json` is never paginated: the cap
/// is a terminal concession like the 70-column truncation, and a consumer that
/// wanted five results can take five.
///
/// `add_command` is null when there is nothing to *add* — either the capability
/// is already in the manifest (`in_manifest`), or it is an extension, which is
/// referenced by name in `[extensions.*]`. The screen offers `explain` for the
/// installed case instead of leaving the line without a next step; that stays a
/// human affordance rather than a second command field, because a consumer with
/// `name` and `in_manifest` can already offer whatever its own UI should.
///
/// Every string here crosses from a registry response or the central library
/// into a caller's UI, so all of it goes through `sanitize_line` (rule 7).
fn search_json(
    query: &str,
    results: &[provider::Candidate],
    installed: &[String],
) -> serde_json::Value {
    let out: Vec<serde_json::Value> = results
        .iter()
        .map(|c| {
            let in_manifest = installed.contains(&c.name);
            let t = c.trust();
            let add_command = match (&c.kind, in_manifest) {
                (CandidateKind::Extension(_), _) | (_, true) => None,
                (_, false) => Some(format!(
                    "agentstack add from {}",
                    crate::text::sanitize_line(if c.source == "catalog" {
                        &c.name
                    } else {
                        &c.id
                    })
                )),
            };
            serde_json::json!({
                "name": crate::text::sanitize_line(&c.name),
                "id": crate::text::sanitize_line(&c.id),
                "description": crate::text::sanitize_line(&c.description),
                "source": c.source,
                "kind": match &c.kind {
                    CandidateKind::Server(_) => "server",
                    CandidateKind::Skill(_) => "skill",
                    CandidateKind::Pack(_) => "pack",
                    CandidateKind::Extension(_) => "extension",
                    CandidateKind::Hook(_) => "hook",
                },
                "details": candidate_details(&c.kind),
                "in_manifest": in_manifest,
                "trust": {
                    "namespaced": t.namespaced,
                    "runs_code": t.runs_code,
                    "needs_secret": t.needs_secret,
                },
                "add_command": add_command,
            })
        })
        .collect();
    crate::ui_contract::envelope(serde_json::json!({
        "query": crate::text::sanitize_line(query),
        "results": out,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{Candidate, Install};

    fn server(id: &str, name: &str, description: &str) -> Candidate {
        Candidate {
            id: id.into(),
            name: name.into(),
            description: description.into(),
            source: "registry",
            kind: CandidateKind::Server(Install::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "thing".into()],
                secret_env: vec!["TOKEN".into()],
            }),
        }
    }

    /// `json-reads-v1`: every fact the search screen prints per result is a
    /// named field — including the trust signals, which the text renders as a
    /// glyph sentence a consumer would otherwise have to parse.
    #[test]
    fn search_json_names_the_facts_the_screen_prints() {
        let results = [
            server("io.github.a/thing", "thing", "Does a thing"),
            server("io.github.b/other", "other", "Something else"),
        ];
        let out = search_json("thing", &results, &["other".to_string()]);
        assert_eq!(out["schema_version"], crate::ui_contract::SCHEMA_VERSION);
        assert_eq!(out["query"], "thing");

        let first = &out["results"][0];
        assert_eq!(first["name"], "thing");
        assert_eq!(first["id"], "io.github.a/thing");
        assert_eq!(first["kind"], "server");
        assert_eq!(first["details"], serde_json::Value::Null);
        assert_eq!(first["in_manifest"], false);
        assert_eq!(first["trust"]["namespaced"], true);
        assert_eq!(first["trust"]["runs_code"], true);
        assert_eq!(first["trust"]["needs_secret"], true);
        assert_eq!(
            first["add_command"],
            "agentstack add from io.github.a/thing"
        );

        // Already in the manifest → nothing to offer, exactly as the screen
        // prints no `↳` line for it.
        assert_eq!(out["results"][1]["in_manifest"], true);
        assert_eq!(out["results"][1]["add_command"], serde_json::Value::Null);

        // An empty query is answered, not refused: the same envelope with the
        // query echoed back and no results.
        let empty = search_json("", &[], &[]);
        assert_eq!(empty["query"], "");
        assert_eq!(empty["results"].as_array().unwrap().len(), 0);
    }

    /// The finding this ranking exists to close: `search github` returned the
    /// thing actually called `github` alongside an Obsidian-vault server and a
    /// PR analyser, ordered by which provider answered first. An exact name
    /// must beat a reverse-DNS tail, which must beat a prefix, which must beat
    /// a description-only mention.
    #[test]
    fn exact_name_outranks_substring_and_description_matches() {
        let mut results = vec![
            server(
                "io.github.x/vault",
                "obsidian-vault",
                "Notes, with github sync",
            ),
            server(
                "io.github.y/pr",
                "pr-analyser",
                "Reviews github pull requests",
            ),
            server("io.github.github/github", "github", "The GitHub MCP server"),
            server("io.github.z/gh", "github-projects", "Boards and issues"),
        ];
        rank(&mut results, "github");
        let order: Vec<&str> = results.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(order[0], "github", "exact name first: {order:?}");
        assert_eq!(order[1], "github-projects", "prefix beats description");
        // The two that only mention it in prose come last, in a stable order.
        assert_eq!(&order[2..], &["obsidian-vault", "pr-analyser"]);

        // Same input, same order — twice. A search that reshuffles between
        // identical runs is one nobody can build a habit on.
        let mut again = results.clone();
        rank(&mut again, "github");
        assert_eq!(
            again.iter().map(|c| &c.name).collect::<Vec<_>>(),
            results.iter().map(|c| &c.name).collect::<Vec<_>>()
        );
    }

    /// A reverse-DNS id whose last segment is the query is an exact answer
    /// wearing a namespace, and must not be demoted to a substring match.
    #[test]
    fn reverse_dns_tail_counts_as_a_name_match() {
        let namespaced = server("io.github.acme/thing", "acme-thing", "A thing");
        let prose = server("io.github.b/other", "other", "mentions thing in passing");
        assert!(score(&namespaced, "thing") > score(&prose, "thing"));
    }

    /// The page caps exist so one deep provider cannot fill the first screen.
    /// Membership is relevance; layout is source.
    #[test]
    fn page_caps_total_and_per_source_then_groups() {
        let mut results: Vec<Candidate> = (0..6)
            .map(|i| {
                let mut c = server(&format!("io.github.r/s{i}"), &format!("thing{i}"), "reg");
                c.source = "registry";
                c
            })
            .collect();
        let mut lib = server("lib/thing", "thing", "the exact one");
        lib.source = "library";
        results.push(lib);
        rank(&mut results, "thing");

        let shown = page(&results);
        assert_eq!(shown.len(), 4, "3 registry + 1 library, under the 5 cap");
        assert_eq!(
            shown.iter().filter(|c| c.source == "registry").count(),
            3,
            "no single source fills the page"
        );
        // Grouped for print: the library the user curated comes first.
        assert_eq!(shown[0].source, "library");
        assert_eq!(shown[0].name, "thing");
    }

    /// Rule 7: registry responses are hostile input. A description carrying
    /// terminal escapes must not reach a consumer's UI with them intact.
    #[test]
    fn search_json_sanitizes_registry_supplied_text() {
        let results = [server(
            "io.github.a/thing",
            "thing",
            "harmless\u{1b}[2Joverwrite",
        )];
        let raw = search_json("thing", &results, &[]).to_string();
        assert!(!raw.contains('\u{1b}'), "escapes stripped: {raw}");
    }
}
