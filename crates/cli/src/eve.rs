//! Governed intake from external ecosystems: turning someone else's bytes into
//! our types, or into a refusal.
//!
//! Phase 4. External ecosystems (vercel/eve's format is the first) publish
//! skills, MCP "connections", and registry indexes. We want their content
//! without inheriting their trust model, so intake is split in two and this
//! module is the half with no power:
//!
//! - **Here**: bytes → validated, bounded, typed values. Nothing else.
//! - **The caller**: fetching, [`crate::quarantine`] staging, the review card,
//!   the yes, and every write.
//!
//! That split is the point. A parser that also fetched or wrote would be a
//! second path to activation, and "intake never becomes activation" would stop
//! being a property of one directory (see `quarantine`'s module docs) and start
//! being a promise repeated at each source.
//!
//! # What "all input is hostile" means concretely here (invariant 7)
//!
//! Every function in this module treats its argument as an attack:
//!
//! - **Bounded before believed.** Counts and byte totals are checked against the
//!   consts below *before* any per-item work, so a hostile input cannot buy
//!   unbounded work by being large.
//! - **Nothing reaches a terminal unsanitized.** Any string headed for display
//!   or agent context goes through [`crate::text::sanitize_line`] and a length
//!   cap — the same choke point every other ingestion boundary uses.
//! - **Nothing reaches a path unvalidated.** Every relative path goes through
//!   [`crate::quarantine::check_relative`]. We do not add a second, kinder path
//!   check; a second check is how the first one stops being the choke point.
//! - **Nothing reaches a shell.** This module never builds a command line. A
//!   parsed `command` is stored as a field of [`Server`], which the runtime
//!   spawns argv-style; it is never interpolated into a string.
//! - **Nothing reaches the manifest as a credential.** See
//!   [`parse_connection`] — fetched secret values become `${REF}` on the way in.
//!
//! # Errors are refusals, not partial successes
//!
//! With one deliberate exception (a registry index, see [`parse_registry`]),
//! every failure here rejects the *whole* input and says so. Half-accepted
//! content is content a reviewer cannot reason about.

use anyhow::{bail, Context, Result};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use agentstack_core::manifest::{Server, ServerType};

use crate::commands::share::Entry;
use crate::quarantine;
use crate::text;

// ---------------------------------------------------------------------------
// Shared bounds
// ---------------------------------------------------------------------------
//
// These are refusal thresholds, not tuning knobs. Each one names a way a remote
// source could turn "parse this" into "spend the machine": too many files, one
// enormous file, many merely-large files, an endless index, a name or
// description long enough to wreck a terminal or a lookup table.

/// Most files accepted in one skill package.
pub const MAX_FILES: usize = 500;
/// Largest single file accepted, in bytes.
pub const MAX_FILE_BYTES: usize = 1024 * 1024;
/// Largest total payload accepted in one call, in bytes. Also bounds the raw
/// JSON handed to [`parse_connection`] and [`parse_registry`], so a 500 MB
/// document is refused before serde walks it.
pub const MAX_TOTAL_BYTES: usize = 16 * 1024 * 1024;
/// Most entries accepted from a registry index.
pub const MAX_REGISTRY_ITEMS: usize = 5000;
/// Longest accepted capability name. Same value the shared name contract uses
/// ([`text::NAME_MAX`]); restated here because it is part of *this* module's
/// documented bounds, and asserted equal in the tests.
pub const MAX_NAME: usize = 64;
/// Longest description kept, in characters. Longer text is truncated for
/// display rather than refused — a verbose description is sloppy, not hostile.
pub const MAX_DESCRIPTION: usize = 300;
/// Most `env` (or header) entries accepted on one connection.
pub const MAX_ENV_ENTRIES: usize = 100;

/// The shape of a breach message: name the limit, and state plainly that
/// nothing was accepted, so a user never has to wonder whether a partial import
/// landed.
fn over_limit(what: &str, saw: usize, limit: usize) -> anyhow::Error {
    anyhow::anyhow!(
        "refusing this import: {what} is {saw}, over the limit of {limit}. Nothing was accepted."
    )
}

// ---------------------------------------------------------------------------
// Skill packages
// ---------------------------------------------------------------------------

/// Parse a fetched skill package into the same [`Entry`] values a shared bundle
/// produces.
///
/// `files` is `(relative path, UTF-8 content)` — already read by the caller,
/// because reading is I/O and this module does none. Returning `Entry` rather
/// than an eve-shaped type is the whole design: from here on, an eve skill and a
/// teammate's skill are literally the same value, staged by the same
/// `quarantine::stage`, reviewed by the same card, pinned by the same lock.
/// There is no second funnel to keep in sync, and therefore no second funnel to
/// forget to harden.
///
/// `origin` (a URL or registry id) is copied onto every entry rather than
/// recorded once for the package: entries travel individually through staging,
/// and provenance that can be separated from content is provenance that will be.
pub fn parse_skill_package(
    name: &str,
    files: &[(String, String)],
    origin: &str,
) -> Result<Vec<Entry>> {
    text::validate_name(name).context("the package name is not usable as a capability name")?;

    if files.len() > MAX_FILES {
        return Err(over_limit("the file count", files.len(), MAX_FILES));
    }
    let mut total = 0usize;
    for (path, body) in files {
        if body.len() > MAX_FILE_BYTES {
            return Err(over_limit(
                &format!("file '{}'", text::sanitize_line(path)),
                body.len(),
                MAX_FILE_BYTES,
            ));
        }
        // Saturating so the running total cannot itself overflow on a hostile
        // input; the check below catches it either way.
        total = total.saturating_add(body.len());
        if total > MAX_TOTAL_BYTES {
            return Err(over_limit("the total payload", total, MAX_TOTAL_BYTES));
        }
    }

    // The path choke point, before anything else looks at a path. `check_relative`
    // is `quarantine`'s function on purpose — one implementation of "cannot
    // escape", called from every intake source.
    for (path, _) in files {
        quarantine::check_relative(path)
            .with_context(|| format!("in skill package '{}'", text::sanitize_line(name)))?;
    }

    let Some((_, skill_md)) = files.iter().find(|(p, _)| is_root_skill_md(p)) else {
        // Name what we DID find. "SKILL.md not found" sends a user hunting; a
        // list of what arrived usually shows the problem (a wrapping directory,
        // a `skill.md`, a bare README) at a glance.
        let found = files
            .iter()
            .take(10)
            .map(|(p, _)| text::sanitize_line(p))
            .collect::<Vec<_>>()
            .join(", ");
        let found = if found.is_empty() {
            "nothing".to_string()
        } else {
            found
        };
        bail!(
            "'{}' has no SKILL.md at its root, so it is not a skill — found: {found}. \
             Nothing was accepted.",
            text::sanitize_line(name)
        );
    };

    let license = frontmatter_value(skill_md, "license");

    // Attribution travels WITH the content. An SPDX tag alone does not satisfy
    // most attribution obligations, and a notice that lives only in the fetch
    // log is a notice that is gone the moment the content is copied onward. If
    // several notice files arrive, all of them are carried — deciding which one
    // "counts" is a legal judgement, not a parser's.
    let notices: Vec<String> = files
        .iter()
        .filter(|(p, _)| is_notice_file(p))
        .map(|(p, body)| format!("--- {} ---\n{}", text::sanitize_line(p), body))
        .collect();
    let notice = if notices.is_empty() {
        None
    } else {
        Some(notices.join("\n\n"))
    };

    Ok(files
        .iter()
        .map(|(path, body)| Entry {
            name: name.to_string(),
            kind: "skill".to_string(),
            // Prefixed with the package name, matching what `share` produces:
            // an `Entry.path` is relative to the KIND's root
            // (`.agentstack/skills/`), not to the package. Without the prefix a
            // package's `SKILL.md` adopts to `skills/SKILL.md` — the skill's own
            // directory vanishes, and the second package imported collides with
            // the first. Found by the end-to-end witness, which checks where
            // the file actually lands rather than that the command said a
            // number.
            path: format!("{name}/{path}"),
            body: body.clone(),
            license: license.clone(),
            origin: Some(text::sanitize_line(origin)),
            notice: notice.clone(),
        })
        .collect())
}

/// Whether `path` is the package's own `SKILL.md` — at the root, not nested.
/// A nested `docs/SKILL.md` is a document about a skill, not the skill.
fn is_root_skill_md(path: &str) -> bool {
    path.trim_start_matches("./") == "SKILL.md"
}

/// Attribution files, matched case-insensitively on the file name only (a
/// `vendor/LICENSE` is still a licence someone is owed).
fn is_notice_file(path: &str) -> bool {
    let name = path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
        .to_ascii_uppercase();
    matches!(
        name.as_str(),
        "LICENSE" | "LICENSE.MD" | "LICENSE.TXT" | "NOTICE" | "NOTICE.MD"
    )
}

/// Read one scalar top-level key out of a `SKILL.md` YAML frontmatter fence.
///
/// This deliberately mirrors [`crate::library::parse_frontmatter_description`]
/// rather than calling it: that function is description-specific (it also folds
/// YAML block scalars, which a `license:` value never is). The fence-finding and
/// top-level-key rules are kept identical on purpose — if one changes, both
/// should. The value is sanitized here because it is remote text that will be
/// shown on the review card.
fn frontmatter_value(md: &str, key: &str) -> Option<String> {
    let rest = md.trim_start().strip_prefix("---")?;
    let end = rest.find("\n---")?;
    for line in rest[..end].lines() {
        // Only top-level keys: an indented `license:` belongs to some nested
        // structure, not to the skill.
        if line.starts_with(char::is_whitespace) {
            continue;
        }
        let Some(v) = line.trim().strip_prefix(key) else {
            continue;
        };
        let Some(v) = v.strip_prefix(':') else {
            continue; // `licensed-under:` is not `license:`
        };
        let v = text::sanitize_line(v.trim().trim_matches('"').trim_matches('\''));
        if v.is_empty() {
            return None;
        }
        return Some(text::truncate_chars(&v, MAX_NAME));
    }
    None
}

// ---------------------------------------------------------------------------
// Connections (MCP servers)
// ---------------------------------------------------------------------------

/// The wire shape of a connection. `#[serde(default)]` throughout: a missing
/// field is a shape we decide about below, with a message, rather than a serde
/// error a user cannot act on. Unknown fields are ignored rather than rejected —
/// external ecosystems add keys, and refusing an unknown key would make every
/// upstream addition a breaking change for us.
#[derive(Debug, Default, Deserialize)]
struct RawConnection {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    env: IndexMap<String, String>,
    #[serde(default)]
    headers: IndexMap<String, String>,
}

/// Parse an eve-format MCP connection into our [`Server`], with every credential
/// turned into a `${REF}` placeholder.
///
/// Returns `(name, server, refs_created)`. `refs_created` is the list of
/// `${REF}` names the caller must ask the user to fill in (`agentstack secret
/// set <REF>`); it is returned rather than resolved because resolving is a
/// machine-local action and this module has no I/O.
///
/// # Why the credential rewrite lives in the parser
///
/// Invariant 5 is "secrets never serialize". The dangerous moment for a fetched
/// connection is not the manifest write — it is the instant a real key exists
/// inside a value we are about to hand to the write path. Doing the rewrite in
/// the constructor of the `Server` means no `Server` with a live credential in
/// it is ever *constructed*, so there is no window in which a later mistake
/// (logging it, echoing it onto the review card, serializing it for a diff)
/// could leak one. The test asserts the raw value appears nowhere in the
/// serialized result, which is the property, not the mechanism.
pub fn parse_connection(raw: &str, origin: &str) -> Result<(String, Server, Vec<String>)> {
    if raw.len() > MAX_TOTAL_BYTES {
        return Err(over_limit(
            "the connection document",
            raw.len(),
            MAX_TOTAL_BYTES,
        ));
    }
    let value: serde_json::Value =
        serde_json::from_str(raw).context("this connection is not valid JSON")?;

    // Two shapes in the wild: a bare connection, or the `mcpServers` wrapper
    // every harness config uses. Exactly one entry — a wrapper with several is
    // a config file, and importing several servers behind one yes would be one
    // yes covering decisions the user never saw.
    let (wrapper_name, body) = match value.get("mcpServers") {
        Some(serde_json::Value::Object(map)) => {
            if map.len() != 1 {
                bail!(
                    "this file defines {} servers under 'mcpServers'; import one at a time so \
                     each is reviewed on its own. Nothing was accepted.",
                    map.len()
                );
            }
            let (k, v) = map
                .iter()
                .next()
                .ok_or_else(|| anyhow::anyhow!("'mcpServers' is empty — nothing to import"))?;
            (Some(k.clone()), v.clone())
        }
        Some(_) => bail!("'mcpServers' is present but is not an object"),
        None => (None, value),
    };

    let conn: RawConnection =
        serde_json::from_value(body).context("this connection has an unexpected shape")?;

    // The wrapper key names the server when the body does not; the body wins
    // when both exist, because that is the value the publisher wrote explicitly.
    let name = conn
        .name
        .clone()
        .or(wrapper_name)
        .ok_or_else(|| anyhow::anyhow!("this connection has no name"))?;
    // The shared name contract. It is what makes an accepted name exactly one
    // safe path component and shell-metacharacter-free — `a; rm -rf /` and
    // `../evil` die here, not at some later call site that happens to remember.
    text::validate_name(&name).context("refusing this connection's name")?;

    if conn.env.len() > MAX_ENV_ENTRIES {
        return Err(over_limit(
            "the env entry count",
            conn.env.len(),
            MAX_ENV_ENTRIES,
        ));
    }
    if conn.headers.len() > MAX_ENV_ENTRIES {
        return Err(over_limit(
            "the header count",
            conn.headers.len(),
            MAX_ENV_ENTRIES,
        ));
    }

    let mut refs: Vec<String> = Vec::new();
    let env = redact_map(&conn.env, &mut refs);
    let headers = redact_map(&conn.headers, &mut refs);

    let (server_type, url, command) = match (conn.url.as_deref(), conn.command.as_deref()) {
        (Some(url), _) => {
            let scheme_ok = url.len() < 8 * 1024
                && (url.to_ascii_lowercase().starts_with("http://")
                    || url.to_ascii_lowercase().starts_with("https://"));
            if !scheme_ok {
                // `file:`, `javascript:`, `data:` and friends. Named as a scheme
                // problem so the refusal is understandable, but rejected by an
                // allow-list so a scheme nobody thought of fails closed too.
                bail!(
                    "'{}' is not an http(s) URL — refusing it. Nothing was accepted.",
                    text::truncate_chars(&text::sanitize_line(url), 120)
                );
            }
            (ServerType::Http, Some(url.to_string()), None)
        }
        (None, Some(command)) => {
            if command.trim().is_empty() {
                bail!("this connection's command is empty — refusing it");
            }
            (ServerType::Stdio, None, Some(command.to_string()))
        }
        (None, None) => bail!("this connection has neither a 'url' nor a 'command'"),
    };

    let server = Server {
        server_type,
        url,
        command,
        args: conn.args,
        cwd: None,
        integrity_roots: Vec::new(),
        // `["*"]` is the manifest's own default (render to every adapter). We
        // spell it out because `Server` has no `Default`, and silently narrowing
        // targets at intake would make an imported server mysteriously absent.
        targets: vec!["*".to_string()],
        // Deliberately not set from `origin`: `owner` means "some app rewrites
        // this entry's values on disk, follow it". A fetch URL is provenance,
        // not an owner, and conflating them would let a remote source nominate
        // itself as the authority over a local config.
        owner: None,
        headers,
        env,
        extra: IndexMap::new(),
    };
    let _ = origin; // provenance is the caller's to record alongside the yes.
    Ok((name, server, refs))
}

/// Replace anything that looks like a live credential with `${REF}`, collecting
/// the ref names.
///
/// `refs` is `&mut Vec` rather than a returned collection so both maps (env and
/// headers) accumulate into one list in encounter order — the order the review
/// card shows and the order the user will be asked to fill them in.
fn redact_map(map: &IndexMap<String, String>, refs: &mut Vec<String>) -> IndexMap<String, String> {
    let mut out = IndexMap::new();
    for (k, v) in map {
        if is_placeholder(v) {
            // Already a reference. Re-wrapping would produce `${${X}}` — a ref
            // that resolves to nothing and fails closed at a confusing moment.
            out.insert(k.clone(), v.clone());
            continue;
        }
        if looks_secret(k, v) {
            let name = ref_name(k);
            if !refs.contains(&name) {
                refs.push(name.clone());
            }
            out.insert(k.clone(), format!("${{{name}}}"));
        } else {
            out.insert(k.clone(), v.clone());
        }
    }
    out
}

/// `${...}` — the manifest's secret-reference spelling.
fn is_placeholder(v: &str) -> bool {
    let v = v.trim();
    v.starts_with("${") && v.ends_with('}') && v.len() > 3
}

/// Two independent signals, either of which is enough. The false positive
/// (redacting something harmless) costs a user one puzzled moment; the false
/// negative writes a live key into a file that gets committed. The asymmetry
/// decides the tuning.
fn looks_secret(key: &str, value: &str) -> bool {
    if value.trim().is_empty() {
        return false;
    }
    let k = key.to_ascii_uppercase();
    let by_name = ["KEY", "TOKEN", "SECRET", "PASSWORD", "CREDENTIAL", "AUTH"]
        .iter()
        .any(|needle| k.contains(needle));
    if by_name {
        return true;
    }
    // A long opaque blob: no whitespace, and made of the alphabet keys and
    // tokens use. Prose, URLs, and paths fall out (they carry spaces, `/`, or
    // are short).
    let v = value.trim();
    v.len() >= 24
        && !v.contains(char::is_whitespace)
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+' | '='))
        && v.chars().any(|c| c.is_ascii_digit())
}

/// `api-key` → `API_KEY`. Derived from the key name so the ref reads like the
/// thing it holds; forced into `[A-Z0-9_]` so it is a legal ref everywhere and
/// cannot smuggle a separator or a control character into a lookup.
fn ref_name(key: &str) -> String {
    let mut out: String = key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    out.truncate(MAX_NAME);
    if out.is_empty() || out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

// ---------------------------------------------------------------------------
// Registry indexes
// ---------------------------------------------------------------------------

/// One entry in an external registry index: enough to *show* a user, never
/// enough to activate anything. There is no command, no URL to execute, and no
/// content — `fetch_url` is a place the caller may choose to fetch from after a
/// human has looked at the row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryItem {
    pub name: String,
    /// `"skill"` or `"server"`.
    pub kind: String,
    pub description: Option<String>,
    pub license: Option<String>,
    pub origin: String,
    pub fetch_url: Option<String>,
}

/// Parse an external registry index.
///
/// # The one place partial success is right
///
/// Everywhere else in this module a bad input rejects the whole thing. An index
/// is different: it is a catalogue written by many hands, and one malformed row
/// should not hide the other four thousand. So malformed items are skipped.
///
/// But skipping has a failure mode worse than erroring — a registry that parses
/// to silently empty looks exactly like a registry with nothing in it, and the
/// user concludes "no results" instead of "this feed is broken or is not what I
/// think it is". So: if there was at least one item and *every* one was
/// malformed, that is an error.
pub fn parse_registry(raw: &str, origin: &str) -> Result<Vec<RegistryItem>> {
    if raw.len() > MAX_TOTAL_BYTES {
        return Err(over_limit(
            "the registry document",
            raw.len(),
            MAX_TOTAL_BYTES,
        ));
    }
    let value: serde_json::Value =
        serde_json::from_str(raw).context("this registry index is not valid JSON")?;

    // `(item, kind hint from the bucket it came in)`. Borrowed from `value`, so
    // no clone of a potentially large document — `value` outlives this vector.
    let mut raw_items: Vec<(&serde_json::Value, Option<&str>)> = Vec::new();
    match &value {
        serde_json::Value::Array(items) => raw_items.extend(items.iter().map(|i| (i, None))),
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::Array(items)) = map.get("items") {
                raw_items.extend(items.iter().map(|i| (i, None)));
            }
            for (field, kind) in [("skills", "skill"), ("servers", "server")] {
                if let Some(serde_json::Value::Array(items)) = map.get(field) {
                    raw_items.extend(items.iter().map(|i| (i, Some(kind))));
                }
            }
            if raw_items.is_empty() {
                bail!(
                    "this registry index has no 'items', 'skills', or 'servers' list. \
                     Nothing was accepted."
                );
            }
        }
        _ => bail!("a registry index must be a JSON array or object"),
    }

    if raw_items.len() > MAX_REGISTRY_ITEMS {
        return Err(over_limit(
            "the registry item count",
            raw_items.len(),
            MAX_REGISTRY_ITEMS,
        ));
    }

    let origin = text::sanitize_line(origin);
    let total = raw_items.len();
    let items: Vec<RegistryItem> = raw_items
        .into_iter()
        .filter_map(|(item, hint)| registry_item(item, hint, &origin))
        .collect();

    if items.is_empty() && total > 0 {
        bail!(
            "all {total} entries in this registry index were unusable — refusing it rather than \
             showing you an empty list. Nothing was accepted."
        );
    }
    Ok(items)
}

/// One index row, or `None` if it is unusable. Returning `Option` rather than
/// `Result` is the honest signature: the caller has already decided (in
/// [`parse_registry`]) that a single bad row is skipped, so there is no error
/// here anyone would act on.
fn registry_item(
    item: &serde_json::Value,
    kind_hint: Option<&str>,
    origin: &str,
) -> Option<RegistryItem> {
    let obj = item.as_object()?;
    let name = obj.get("name")?.as_str()?;
    // The same name contract as everywhere else. A registry row whose name we
    // would refuse later is a row we must not show as installable now.
    text::validate_name(name).ok()?;

    let kind = obj
        .get("kind")
        .or_else(|| obj.get("type"))
        .and_then(|k| k.as_str())
        .or(kind_hint)
        .unwrap_or("skill");
    let kind = match kind {
        "skill" => "skill",
        "server" | "connection" | "mcp" => "server",
        _ => return None,
    };

    let description = obj
        .get("description")
        .and_then(|d| d.as_str())
        .map(|d| text::truncate_chars(&text::sanitize_line(d), MAX_DESCRIPTION))
        .filter(|d| !d.is_empty());
    let license = obj
        .get("license")
        .and_then(|l| l.as_str())
        .map(|l| text::truncate_chars(&text::sanitize_line(l), MAX_NAME))
        .filter(|l| !l.is_empty());
    // A non-http(s) fetch target drops the field rather than the row: the row is
    // still worth showing, it just is not fetchable from here.
    let fetch_url = ["fetch_url", "url", "source"]
        .iter()
        .find_map(|k| obj.get(*k).and_then(|u| u.as_str()))
        .filter(|u| {
            let lower = u.to_ascii_lowercase();
            lower.starts_with("http://") || lower.starts_with("https://")
        })
        .map(|u| text::truncate_chars(&text::sanitize_line(u), 2048));

    Some(RegistryItem {
        name: name.to_string(),
        kind: kind.to_string(),
        description,
        license,
        origin: origin.to_string(),
        fetch_url,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(path: &str, body: &str) -> (String, String) {
        (path.to_string(), body.to_string())
    }

    #[test]
    fn bounds_match_the_shared_name_contract() {
        assert_eq!(MAX_NAME, text::NAME_MAX);
    }

    // -- skill packages ----------------------------------------------------

    #[test]
    fn a_package_without_skill_md_is_refused() {
        let files = vec![f("README.md", "hi"), f("docs/SKILL.md", "nested")];
        let err = parse_skill_package("thing", &files, "https://example.test/x")
            .expect_err("must be refused");
        let msg = err.to_string();
        assert!(msg.contains("no SKILL.md"), "{msg}");
        // The message names what arrived, so the user can see the problem.
        assert!(msg.contains("README.md"), "{msg}");
    }

    #[test]
    fn a_traversing_path_is_refused() {
        for hostile in ["../../x", "/etc/passwd", "a/../../out", "ok\0/x"] {
            let files = vec![f("SKILL.md", "---\n---\n"), f(hostile, "payload")];
            assert!(
                parse_skill_package("thing", &files, "o").is_err(),
                "{hostile:?} must be refused"
            );
        }
    }

    #[test]
    fn license_and_notice_travel_with_the_content() {
        let files = vec![
            f(
                "SKILL.md",
                "---\nname: thing\nlicense: Apache-2.0\ndescription: does a thing\n---\nbody",
            ),
            f("LICENSE", "Copyright someone else. All rights reserved."),
            f("reference/notes.md", "notes"),
        ];
        let entries = parse_skill_package("thing", &files, "https://example.test/x")
            .expect("well-formed package");
        assert_eq!(entries.len(), 3);
        for e in &entries {
            assert_eq!(e.kind, "skill");
            assert_eq!(e.license.as_deref(), Some("Apache-2.0"));
            assert_eq!(e.origin.as_deref(), Some("https://example.test/x"));
            // Attribution rides on EVERY entry — an entry that gets separated
            // from its siblings must still carry the notice.
            assert!(
                e.notice
                    .as_deref()
                    .is_some_and(|n| n.contains("All rights reserved")),
                "notice missing from {}",
                e.path
            );
        }
    }

    #[test]
    fn a_case_odd_notice_file_still_counts() {
        let files = vec![f("SKILL.md", "x"), f("Notice.md", "attribution here")];
        let entries = parse_skill_package("thing", &files, "o").expect("ok");
        assert!(entries[0]
            .notice
            .as_deref()
            .is_some_and(|n| n.contains("attribution here")));
    }

    #[test]
    fn frontmatter_license_is_not_confused_by_lookalikes() {
        // Indented (belongs to something nested) and a different key entirely.
        let md = "---\nmeta:\n  license: GPL-3.0\nlicensed-under: MIT\n---\n";
        assert_eq!(frontmatter_value(md, "license"), None);
    }

    #[test]
    fn oversized_input_hits_the_bound() {
        let big = "x".repeat(MAX_FILE_BYTES + 1);
        let files = vec![f("SKILL.md", "x"), f("big.txt", &big)];
        let err = parse_skill_package("thing", &files, "o").expect_err("must be refused");
        assert!(err.to_string().contains("Nothing was accepted"));

        let many: Vec<(String, String)> = (0..MAX_FILES + 1)
            .map(|i| f(&format!("f{i}.txt"), "x"))
            .collect();
        let err = parse_skill_package("thing", &many, "o").expect_err("must be refused");
        assert!(err.to_string().contains(&MAX_FILES.to_string()));

        // Many merely-large files: each under the per-file cap, the total over.
        let chunk = "y".repeat(MAX_FILE_BYTES);
        let heavy: Vec<(String, String)> = (0..20).map(|i| f(&format!("f{i}"), &chunk)).collect();
        let err = parse_skill_package("thing", &heavy, "o").expect_err("must be refused");
        assert!(err.to_string().contains("total payload"), "{err}");
    }

    // -- connections -------------------------------------------------------

    /// The property, stated as a search: after parsing, the raw credential
    /// exists nowhere in the value we are about to hand onward.
    #[test]
    fn a_literal_credential_never_reaches_the_server() {
        const SECRET: &str = "sk-live-9f3c2a77bb5140d0aeef1122334455";
        let raw = format!(
            r#"{{"name":"acme","command":"acme-mcp","args":["serve"],
                 "env":{{"ACME_API_KEY":"{SECRET}","ACME_REGION":"eu-west-1"}}}}"#
        );
        let (name, server, refs) = parse_connection(&raw, "https://example.test").expect("parses");
        assert_eq!(name, "acme");
        assert_eq!(refs, vec!["ACME_API_KEY".to_string()]);
        assert_eq!(
            server.env.get("ACME_API_KEY").map(String::as_str),
            Some("${ACME_API_KEY}")
        );
        // Non-secret values pass through untouched — over-redacting would make
        // the imported server unusable for a different reason.
        assert_eq!(
            server.env.get("ACME_REGION").map(String::as_str),
            Some("eu-west-1")
        );

        let serialized = serde_json::to_string(&server).expect("serializes");
        assert!(
            !serialized.contains(SECRET),
            "the raw credential survived into {serialized}"
        );
        assert!(!serialized.contains("sk-live"), "{serialized}");
    }

    #[test]
    fn a_header_credential_is_redacted_too() {
        let raw = r#"{"name":"acme","url":"https://mcp.example.test/v1",
                      "headers":{"Authorization":"Bearer abcdefghijklmnop","X-Trace":"on"}}"#;
        let (_, server, refs) = parse_connection(raw, "o").expect("parses");
        assert_eq!(refs, vec!["AUTHORIZATION".to_string()]);
        assert_eq!(
            server.headers.get("Authorization").map(String::as_str),
            Some("${AUTHORIZATION}")
        );
        assert_eq!(
            server.headers.get("X-Trace").map(String::as_str),
            Some("on")
        );
        assert_eq!(server.server_type, ServerType::Http);
    }

    #[test]
    fn an_existing_ref_is_not_double_wrapped() {
        let raw = r#"{"name":"acme","command":"acme-mcp","env":{"ACME_TOKEN":"${ACME_TOKEN}"}}"#;
        let (_, server, refs) = parse_connection(raw, "o").expect("parses");
        assert_eq!(
            server.env.get("ACME_TOKEN").map(String::as_str),
            Some("${ACME_TOKEN}")
        );
        // Nothing to ask the user for: the ref was already declared.
        assert!(refs.is_empty(), "{refs:?}");
    }

    #[test]
    fn a_long_opaque_value_is_redacted_even_without_a_telltale_key() {
        let raw = r#"{"name":"acme","command":"acme-mcp",
                      "env":{"ACME_ID":"a7Kq2Zx9Lm4Rt8Vb1Nc6Wd3Ye5Uf0","ACME_HOME":"/opt/acme"}}"#;
        let (_, server, refs) = parse_connection(raw, "o").expect("parses");
        assert_eq!(refs, vec!["ACME_ID".to_string()]);
        assert_eq!(
            server.env.get("ACME_HOME").map(String::as_str),
            Some("/opt/acme")
        );
    }

    #[test]
    fn a_dangerous_url_scheme_is_refused() {
        for url in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,<script>",
            "ftp://example.test",
            "HTTPS_evil://x",
            "//example.test",
        ] {
            let raw = format!(r#"{{"name":"acme","url":"{url}"}}"#);
            assert!(
                parse_connection(&raw, "o").is_err(),
                "{url} must be refused"
            );
        }
        let raw = r#"{"name":"acme","url":"HTTPS://Example.test/v1"}"#;
        assert!(
            parse_connection(raw, "o").is_ok(),
            "scheme match is case-insensitive"
        );
    }

    #[test]
    fn a_hostile_name_is_refused() {
        for name in [
            "../evil",
            "a; rm -rf /",
            "",
            "-flag",
            "a/b",
            "A",
            "x".repeat(65).as_str(),
        ] {
            let raw = serde_json::json!({"name": name, "command": "ok"}).to_string();
            assert!(
                parse_connection(&raw, "o").is_err(),
                "{name:?} must be refused"
            );
        }
    }

    #[test]
    fn an_empty_command_and_a_missing_transport_are_refused() {
        assert!(parse_connection(r#"{"name":"acme","command":"   "}"#, "o").is_err());
        assert!(parse_connection(r#"{"name":"acme"}"#, "o").is_err());
    }

    #[test]
    fn a_wrapper_names_the_server_and_only_one_is_accepted() {
        let one = r#"{"mcpServers":{"acme":{"command":"acme-mcp"}}}"#;
        let (name, server, _) = parse_connection(one, "o").expect("parses");
        assert_eq!(name, "acme");
        assert_eq!(server.server_type, ServerType::Stdio);

        let two = r#"{"mcpServers":{"acme":{"command":"a"},"other":{"command":"b"}}}"#;
        let err = parse_connection(two, "o").expect_err("must be refused");
        assert!(err.to_string().contains("one at a time"), "{err}");
    }

    #[test]
    fn too_many_env_entries_hit_the_bound() {
        let env: serde_json::Map<String, serde_json::Value> = (0..MAX_ENV_ENTRIES + 1)
            .map(|i| (format!("K{i}"), serde_json::json!("v")))
            .collect();
        let raw = serde_json::json!({"name":"acme","command":"x","env":env}).to_string();
        let err = parse_connection(&raw, "o").expect_err("must be refused");
        assert!(err.to_string().contains("Nothing was accepted"), "{err}");
    }

    // -- registries --------------------------------------------------------

    #[test]
    fn malformed_rows_are_skipped_but_an_all_bad_index_errors() {
        let raw = r#"[
            {"name":"alpha","kind":"skill"},
            {"name":"../evil","kind":"skill"},
            {"name":"beta","kind":"server"},
            {"kind":"skill"},
            {"name":"gamma"}
        ]"#;
        let items = parse_registry(raw, "https://reg.test").expect("three good rows");
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].kind, "skill");
        assert_eq!(items[1].kind, "server");
        // No `kind` at all defaults to a skill rather than dropping the row.
        assert_eq!(items[2].name, "gamma");
        assert!(items.iter().all(|i| i.origin == "https://reg.test"));

        let all_bad = r#"[{"name":"../evil"},{"nope":1},{"name":"x","kind":"wat"}]"#;
        let err = parse_registry(all_bad, "https://reg.test").expect_err("must error");
        assert!(err.to_string().contains("unusable"), "{err}");

        // An index that is genuinely empty is NOT an error — there is nothing
        // being hidden from the user.
        assert!(parse_registry("[]", "o").expect("empty is fine").is_empty());
    }

    #[test]
    fn both_wrapper_shapes_are_accepted() {
        let items = parse_registry(r#"{"items":[{"name":"a"}]}"#, "o").expect("items");
        assert_eq!(items.len(), 1);
        let items = parse_registry(r#"{"skills":[{"name":"a"}],"servers":[{"name":"b"}]}"#, "o")
            .expect("buckets");
        assert_eq!(items.len(), 2);
        // The bucket supplies the kind when the row does not say.
        assert_eq!(items[0].kind, "skill");
        assert_eq!(items[1].kind, "server");
    }

    #[test]
    fn hostile_display_text_does_not_survive() {
        let raw = serde_json::json!([{
            "name": "alpha",
            "description": "safe\u{1b}]0;EVIL\u{07} \u{1b}[2Jtext\u{202e}here\nsecond line",
            "license": "MIT\u{1b}[31m",
            "url": "javascript:alert(1)"
        }])
        .to_string();
        let items = parse_registry(&raw, "reg\u{1b}[2J.test").expect("one row");
        let d = items[0].description.as_deref().unwrap_or_default();
        assert!(
            !d.contains('\u{1b}') && !d.contains('\u{202e}') && !d.contains('\n'),
            "hostile chars survived in {d:?}"
        );
        // The readable text survives, on one line, with the escapes gone.
        assert_eq!(d, "safe texthere second line", "{d:?}");
        assert_eq!(items[0].license.as_deref(), Some("MIT"));
        assert_eq!(items[0].origin, "reg.test");
        // Non-http(s) fetch targets are dropped, not shown as clickable.
        assert_eq!(items[0].fetch_url, None);
    }

    #[test]
    fn a_long_description_is_capped() {
        let raw = serde_json::json!([{"name":"alpha","description":"a".repeat(5000)}]).to_string();
        let items = parse_registry(&raw, "o").expect("one row");
        let d = items[0].description.as_deref().unwrap_or_default();
        // `truncate_chars` appends an ellipsis, so the cap is chars + 1.
        assert!(
            d.chars().count() <= MAX_DESCRIPTION + 1,
            "{}",
            d.chars().count()
        );
    }

    #[test]
    fn an_oversized_registry_hits_the_bound() {
        let items: Vec<serde_json::Value> = (0..MAX_REGISTRY_ITEMS + 1)
            .map(|i| serde_json::json!({"name": format!("s{i}")}))
            .collect();
        let raw = serde_json::Value::Array(items).to_string();
        let err = parse_registry(&raw, "o").expect_err("must be refused");
        assert!(err.to_string().contains("Nothing was accepted"), "{err}");
    }
}
