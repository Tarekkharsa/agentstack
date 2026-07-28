// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Docs-vs-CLI sync gate.
//!
//! Two checks:
//! 1. `all_commands_region_matches_generator` — docs/reference.md's "All
//!    commands" inventory is generated from the clap tree by `agentstack self
//!    docs --write`, spliced into a managed HTML-comment region. This asserts
//!    the on-disk region matches the generator byte-for-byte, so it can never
//!    drift (a new subcommand or flag fails CI until `self docs --write` is
//!    re-run). This subsumes the old hand-inventory roster checks: a generated
//!    region needs no separate "is every subcommand listed" test.
//! 2. `every_prose_command_is_real` — the inverse direction: every
//!    `agentstack <verb> [<subverb>]` invocation written in a code span or
//!    fenced block across the docs must name a command that actually exists
//!    on the clap tree. A second token is checked as a subcommand, or accepted
//!    only when Clap declares a positional argument; this catches shapes such
//!    as the retired `proxy start`, not just nonexistent top-level verbs. The
//!    generator check above only concerns the inventory region, not free prose.
//! 3. `visible_help_says_toolset` / `docs_prose_say_toolset` /
//!    `authored_html_pages_say_toolset` — the vocabulary gate, on all three
//!    surfaces a person reads: the visible clap tree, Markdown prose, and the
//!    hand-authored site pages no generator would ever rewrite. One concept,
//!    one word: a named subset of your setup is a **toolset**. "Profile"
//!    survives only as file format and wire contract (`[profiles.<name>]`, the
//!    JSON fields, the frozen panel argv), spelled that way on purpose.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use clap::CommandFactory;

/// The clap `Command` tree that `every_prose_command_is_real` walks to learn
/// "what does the CLI actually expose". (The inventory freshness test builds
/// its block through the cli crate's own generator instead.)
fn cli_command() -> clap::Command {
    agentstack::cli::Cli::command()
}

#[test]
fn all_commands_region_matches_generator() {
    use agentstack::commands::self_cmd::{
        commands_block, COMMANDS_MARKER_BEGIN, COMMANDS_MARKER_END,
    };

    let reference = include_str!("../../../docs/reference.md");
    let begin = reference
        .find(COMMANDS_MARKER_BEGIN)
        .expect("docs/reference.md must keep the generated-commands begin marker");
    let end = reference
        .find(COMMANDS_MARKER_END)
        .expect("docs/reference.md must keep the generated-commands end marker");
    // `splice_commands` writes `<begin>\n{block}\n<end>`; the region between the
    // markers is exactly that middle, newlines included.
    let region = &reference[begin + COMMANDS_MARKER_BEGIN.len()..end];
    assert_eq!(
        region,
        format!("\n{}\n", commands_block()),
        "the 'All commands' inventory in docs/reference.md is stale ↳ run \
         `agentstack self docs --write` (or `cargo run -p agentstack -- self docs --write`)"
    );
}

// ── Prose-command lint ─────────────────────────────────────────────────────
//
// Below is deliberately plain string/line scanning rather than a markdown or
// HTML parser (no new dependency is worth adding for this): a tiny state
// machine tracks whether we're inside a "code context" — a fenced block or
// inline backtick span in Markdown, a `<pre>`/`<code>` element in HTML — and
// only text inside those contexts is scanned for `agentstack <verb>` tokens.

/// Legitimate non-command tokens that follow "agentstack" inside a code
/// context. Each entry is a false positive the lint found on a real pass over
/// the docs, with a one-line note on why it's fine. If this grows past ~10
/// entries, the code-context extraction above is too loose — tighten that
/// instead of allowlisting more.
const ALLOWLIST: &[&str] = &[
    // docs/troubleshooting.md — doctor's drift finding reads "no longer matches
    // what agentstack last wrote". A troubleshooting page is worth exactly what
    // its strings are searchable for, so it must quote that verbatim; "last" is
    // an adverb in captured output, not an `agentstack last` subcommand.
    "last",
    // docs/examples.html:864 — "# agent → agentstack control plane", a
    // comment inside a fenced MCP-tool-call example labeling the diagram,
    // not a claim that `agentstack control` is a command.
    "control",
    // docs/start.html — the setup wizard's opening plan prints "write one
    // agentstack manifest"; "manifest" is a noun in captured output, not an
    // `agentstack manifest` subcommand.
    "manifest",
    // docs/start.html — the P2 secret-storage help prints "agentstack keeps
    // this file out of git"; "keeps" is a verb in captured output, not a
    // command.
    "keeps",
    // docs/start.html — the import plan prints "Files agentstack will manage:";
    // "will" is a modal verb in captured output, not an `agentstack will`
    // command.
    "will",
];

/// One `agentstack <verb> [<subverb>]` occurrence found in a code context.
struct Violation {
    file: String,
    line: usize,
    snippet: String,
}

/// A file to scan, tagged with which "is this text in a code context" state
/// machine applies.
enum Kind {
    Markdown,
    Html,
}

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is crates/cli at compile time; repo root is two
    // levels up. (Same anchor the existing test uses via
    // `include_str!("../../../docs/reference.md")`.)
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Recursively collect every `SKILL.md` under `dir` (the catalog nests
/// skills like `linear/breakdown/SKILL.md`, so a single-level glob would
/// miss most of them).
fn find_skill_mds(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let entry = entry.expect("readable dir entry");
        let path = entry.path();
        if path.is_dir() {
            find_skill_mds(&path, out);
        } else if path.file_name().is_some_and(|n| n == "SKILL.md") {
            out.push(path);
        }
    }
}

/// The full scan set: README, top-level docs, redirect-stub-free HTML docs,
/// CONTRIBUTING, and every catalog skill.
fn files_to_scan(root: &Path) -> Vec<(PathBuf, Kind)> {
    let mut files: Vec<(PathBuf, Kind)> = vec![
        (root.join("README.md"), Kind::Markdown),
        (root.join("CONTRIBUTING.md"), Kind::Markdown),
    ];

    let docs_dir = root.join("docs");
    for entry in std::fs::read_dir(&docs_dir).expect("docs/ dir readable") {
        let path = entry.expect("readable dir entry").path();
        if !path.is_file() {
            continue; // skips docs/design/, docs/spikes/, docs/demos/ dirs
        }
        match path.extension().and_then(|e| e.to_str()) {
            Some("md") => files.push((path, Kind::Markdown)),
            Some("html") => {
                let content = std::fs::read_to_string(&path).expect("readable html doc");
                if content.contains(r#"http-equiv="refresh""#) {
                    continue; // redirect stub, nothing to lint
                }
                if content.lines().count() <= 100 {
                    continue; // small stub-shaped page, skip per spec
                }
                files.push((path, Kind::Html));
            }
            _ => {}
        }
    }

    // Example-project docs are prose-command surface too: the top-level
    // examples/projects/README.md plus one README.md per example dir.
    let examples_dir = root.join("examples/projects");
    let top_readme = examples_dir.join("README.md");
    if top_readme.is_file() {
        files.push((top_readme, Kind::Markdown));
    }
    if let Ok(entries) = std::fs::read_dir(&examples_dir) {
        for entry in entries {
            let path = entry.expect("readable dir entry").path();
            if path.is_dir() {
                let readme = path.join("README.md");
                if readme.is_file() {
                    files.push((readme, Kind::Markdown));
                }
            }
        }
    }

    let mut skills = Vec::new();
    find_skill_mds(&root.join("crates/cli/catalog/skills"), &mut skills);
    files.extend(skills.into_iter().map(|p| (p, Kind::Markdown)));

    files
}

/// Byte ranges of `content` that are inside a Markdown code context: fenced
/// blocks (``` or ~~~) get their whole line, and outside a fence, text
/// between a pair of backticks on the same line is an inline code span.
/// Fence delimiter lines themselves aren't scanned (they're just the
/// ```lang marker). Inline spans are matched per-line — a code span
/// spanning multiple lines is rare enough in these docs not to matter.
fn markdown_code_spans(content: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut in_fence = false;
    let mut offset = 0usize;
    for line in content.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let is_fence_delim = trimmed.starts_with("```") || trimmed.starts_with("~~~");
        if is_fence_delim {
            in_fence = !in_fence;
        } else if in_fence {
            spans.push((offset, offset + line.len()));
        } else {
            let mut code_start: Option<usize> = None;
            for (i, c) in line.char_indices() {
                if c != '`' {
                    continue;
                }
                match code_start {
                    Some(start) => {
                        spans.push((offset + start, offset + i));
                        code_start = None;
                    }
                    None => code_start = Some(i + 1),
                }
            }
            // An unterminated backtick on this line (no closing `) isn't
            // treated as code — matches how it'd render (or fail to).
        }
        offset += line.len();
    }
    spans
}

/// Byte ranges of `content` inside `<pre>`/`<code>` elements (nesting, e.g.
/// `<pre><code>...</code></pre>`, collapses to one outer span — everything
/// in between is code context either way). Tag bodies themselves (between
/// `<` and `>`) are never part of a span, so attribute text like
/// `<pre class="agentstack-block">` can't accidentally match.
fn html_code_spans(content: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let lower = content.to_ascii_lowercase();
    let len = content.len();
    let mut i = 0usize;
    let mut depth = 0usize;
    let mut code_start: Option<usize> = None;
    while i < len {
        if content.as_bytes()[i] != b'<' {
            i += 1;
            continue;
        }
        let Some(rel_end) = content[i..].find('>') else {
            break; // unterminated tag; nothing more to do
        };
        let tag_end = i + rel_end + 1;
        let tag = &lower[i..tag_end];
        if tag.starts_with("<pre") || tag.starts_with("<code") {
            if depth == 0 {
                code_start = Some(tag_end);
            }
            depth += 1;
        } else if (tag.starts_with("</pre") || tag.starts_with("</code")) && depth > 0 {
            depth -= 1;
            if depth == 0 {
                if let Some(start) = code_start.take() {
                    spans.push((start, i));
                }
            }
        }
        i = tag_end;
    }
    spans
}

fn in_any_span(pos: usize, spans: &[(usize, usize)]) -> bool {
    spans.iter().any(|&(start, end)| pos >= start && pos < end)
}

/// A whitespace-delimited token that looks command-shaped, per the spec's
/// skip rules (flags, variables, paths) plus the `^[a-z][a-z0-9-]+$` shape.
fn looks_like_command_token(tok: &str) -> bool {
    if tok.is_empty() || tok.starts_with('-') {
        return false; // flag
    }
    if tok.contains(['$', '{', '}', '<', '>']) {
        return false; // variable/placeholder
    }
    if tok.contains(['/', '.']) {
        return false; // path
    }
    let mut chars = tok.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() {
        return false;
    }
    let rest_ok = chars
        .clone()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    rest_ok && chars.count() >= 1 // total length >= 2, matching `[a-z0-9-]+`
}

/// A token's byte offset (relative to the enclosing `content`) paired with
/// its text.
type Token<'a> = (usize, &'a str);

/// Split the text right after an "agentstack" match into up to two
/// whitespace-delimited tokens (verb, subverb), returning byte offsets
/// (relative to `content`) alongside each token so callers can report a line
/// number for it. A token also ends at `<` or `` ` `` — a closing HTML tag
/// like `</span>` or a closing Markdown backtick is routinely glued directly
/// onto the last word with no intervening whitespace
/// (`` `agentstack stats` `` / `agentstack report r-0859dcee73</span>`), and
/// without this, the glued-on character would make the token look like a
/// path/variable (see `looks_like_command_token`) and hide a real violation
/// instead of flagging it.
fn next_two_tokens(content: &str, after: usize) -> (Option<Token<'_>>, Option<Token<'_>>) {
    if !content[after..].starts_with(char::is_whitespace) {
        return (None, None); // "agentstack" wasn't followed by whitespace
    }
    let is_boundary = |c: char| c.is_whitespace() || c == '<' || c == '`';
    let mut tokens: Vec<(usize, &str)> = Vec::new();
    let mut cursor = &content[after..];
    let mut cursor_offset = after;
    for _ in 0..2 {
        let skip = cursor.len() - cursor.trim_start().len();
        cursor = cursor.trim_start();
        cursor_offset += skip;
        if cursor.is_empty() {
            break;
        }
        let tok_len = cursor.find(is_boundary).unwrap_or(cursor.len());
        tokens.push((cursor_offset, &cursor[..tok_len]));
        cursor_offset += tok_len;
        cursor = &cursor[tok_len..];
    }
    let mut it = tokens.into_iter();
    (it.next(), it.next())
}

fn line_number(content: &str, pos: usize) -> usize {
    content[..pos].bytes().filter(|&b| b == b'\n').count() + 1
}

fn snippet_for_line(content: &str, pos: usize) -> String {
    let start = content[..pos].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let end = content[pos..]
        .find('\n')
        .map(|i| pos + i)
        .unwrap_or(content.len());
    content[start..end].trim().to_string()
}

fn scan_file(
    path: &Path,
    kind: &Kind,
    top: &HashSet<String>,
    subs: &HashMap<String, HashSet<String>>,
    positional: &HashSet<String>,
    violations: &mut Vec<Violation>,
) {
    let content = std::fs::read_to_string(path).expect("readable scan-set file");
    let spans = match kind {
        Kind::Markdown => markdown_code_spans(&content),
        Kind::Html => html_code_spans(&content),
    };

    let display_path = path
        .strip_prefix(repo_root())
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned();

    for (match_pos, _) in content.match_indices("agentstack") {
        // Word boundary before the match: skip "myagentstack" etc., and skip
        // path segments like "/path/to/agentstack" — a binary path is not a
        // prose invocation, and whatever follows it is not our subcommand.
        if let Some(prev) = content[..match_pos].chars().next_back() {
            if prev.is_ascii_alphanumeric() || prev == '-' || prev == '_' || prev == '/' {
                continue;
            }
        }
        if !in_any_span(match_pos, &spans) {
            continue;
        }
        let after = match_pos + "agentstack".len();
        let (first, second) = next_two_tokens(&content, after);
        let Some((tok1_pos, tok1)) = first else {
            continue;
        };
        if !looks_like_command_token(tok1) {
            continue;
        }
        if ALLOWLIST.contains(&tok1) {
            continue;
        }
        if !top.contains(tok1) {
            violations.push(Violation {
                file: display_path.clone(),
                line: line_number(&content, tok1_pos),
                snippet: snippet_for_line(&content, tok1_pos),
            });
            continue;
        }
        let Some((tok2_pos, tok2)) = second else {
            continue;
        };
        // A `<placeholder>` argument after a leaf command that accepts no
        // positional documents an argument the CLI doesn't take (the shipped
        // `adopt <name>` bug). Markdown carries a raw `<name>` (the token
        // splitter stops at `<`, leaving an empty token there); HTML escapes
        // it as `&lt;name&gt;`. A raw `<` in HTML is always a real tag, so
        // only the escaped form counts there.
        let placeholder_arg = match kind {
            Kind::Markdown => {
                tok2.is_empty() && content[tok2_pos..].starts_with('<') && {
                    let inner: String = content[tok2_pos + 1..]
                        .chars()
                        .take_while(|&c| c != '>')
                        .collect();
                    !inner.is_empty()
                        && inner.chars().all(|c| {
                            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_'
                        })
                }
            }
            Kind::Html => tok2.starts_with("&lt;"),
        };
        if placeholder_arg {
            // (`is_none_or` needs Rust 1.82; the workspace MSRV is 1.80.)
            let is_leaf = subs.get(tok1).map(|s| s.is_empty()).unwrap_or(true);
            if is_leaf && !positional.contains(tok1) {
                violations.push(Violation {
                    file: display_path.clone(),
                    line: line_number(&content, tok2_pos),
                    snippet: snippet_for_line(&content, tok2_pos),
                });
            }
            continue;
        }
        if !looks_like_command_token(tok2) {
            continue;
        }
        if ALLOWLIST.contains(&tok2) {
            continue;
        }
        let valid_second = match subs.get(tok1).filter(|names| !names.is_empty()) {
            Some(sub_names) => sub_names.contains(tok2),
            None => positional.contains(tok1),
        };
        if !valid_second {
            violations.push(Violation {
                file: display_path.clone(),
                line: line_number(&content, tok2_pos),
                snippet: snippet_for_line(&content, tok2_pos),
            });
        }
    }
}

#[test]
fn every_prose_command_is_real() {
    let cmd = cli_command();
    let mut top: HashSet<String> = HashSet::new();
    let mut subs: HashMap<String, HashSet<String>> = HashMap::new();
    let mut positional: HashSet<String> = HashSet::new();
    for sc in cmd.get_subcommands() {
        let name = sc.get_name();
        if name == "help" {
            continue;
        }
        let sub_names: HashSet<String> = sc
            .get_subcommands()
            .map(|s| s.get_name().to_string())
            .filter(|n| n != "help")
            .collect();
        if sc.get_positionals().next().is_some() {
            positional.insert(name.to_string());
        }
        subs.insert(name.to_string(), sub_names);
        top.insert(name.to_string());
    }

    let root = repo_root();
    let mut violations = Vec::new();
    for (path, kind) in files_to_scan(&root) {
        scan_file(&path, &kind, &top, &subs, &positional, &mut violations);
    }

    assert!(
        violations.is_empty(),
        "prose `agentstack <verb>` invocation(s) that don't name a real subcommand:\n{}",
        violations
            .iter()
            .map(|v| format!("  {}:{}: {}", v.file, v.line, v.snippet))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

// ── Dynamic-snippet parser gate ────────────────────────────────────────────
//
// The prose lint above covers Markdown/HTML code contexts, but the site also
// carries commands in places no code-span scan reaches: copy-button
// `data-copy` attributes, the tutorial's JavaScript `{cmd:'…'}` objects, and
// terminal-simulation line arrays (`['$ agentstack …','g']`). Those are the
// strings a reader actually copies, so each one must parse against the real
// clap tree, not just name a real verb.

/// Full commands that are intentionally shown but stop before being
/// executable (e.g. deliberately partial pipelines). Keep entries rare and
/// justified — an unrecognized shape should fail the test, not slip in here.
const DYNAMIC_ALLOWLIST: &[&str] = &[];

/// Scan `content` for every occurrence of `start_pat` and return the text up
/// to (not including) the next `end` character, with its byte offset.
fn extract_after(content: &str, start_pat: &str, end: char) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for (pos, _) in content.match_indices(start_pat) {
        let start = pos + start_pat.len();
        if let Some(rel) = content[start..].find(end) {
            out.push((start, content[start..start + rel].to_string()));
        }
    }
    out
}

fn html_unescape(s: &str) -> String {
    s.replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// Normalize one extracted snippet into argv tokens, or `None` when it is
/// out of scope by construction:
/// - not an `agentstack` invocation (config snippets, `git`/`curl`/guard
///   demo lines are legitimately copyable but not our parser's business);
/// - contains an explicit placeholder or shell construct (`<…>`, `…`, `*`,
///   `[…]` optional groups, `${REF}`, `$(…)`, pipes) — shown to be filled
///   in by the reader, never executed verbatim.
fn normalize_dynamic(raw: &str) -> Option<Vec<String>> {
    let s = html_unescape(raw);
    let s = s.strip_prefix("$ ").unwrap_or(&s);
    // Trailing inline comment on a simulated line: `agentstack init   # once`.
    let s = match s.find(" #") {
        Some(i) => &s[..i],
        None => s,
    };
    // Transcript annotations: `doctor --ci  →  exit 0` shows an outcome,
    // `trust . · guard install` decoratively joins two commands in a header.
    // The text before the separator is the command; the rest is narration.
    let s = match s.find(" → ") {
        Some(i) => &s[..i],
        None => s,
    };
    let s = match s.find(" · ") {
        Some(i) => &s[..i],
        None => s,
    }
    .trim();
    // The demos page abbreviates the binary as `as` in its transcripts.
    let s = match s.strip_prefix("as ") {
        Some(rest) => format!("agentstack {rest}"),
        None => s.to_string(),
    };
    if s != "agentstack" && !s.starts_with("agentstack ") {
        return None;
    }
    if s.contains(['<', '>', '…', '*', '[', ']', '|']) || s.contains("${") || s.contains("$(") {
        return None;
    }
    if DYNAMIC_ALLOWLIST.contains(&s.as_str()) {
        return None;
    }
    Some(s.split_whitespace().map(String::from).collect())
}

#[test]
fn every_dynamic_command_parses() {
    let root = repo_root();
    // (file, patterns) — each pattern is (start marker, terminator).
    let sources: &[(&str, &[(&str, char)])] = &[
        // Copy buttons put the exact copied string in `data-copy`.
        (
            "docs/cookbook.html",
            &[("data-copy=\"", '"'), (">$ agentstack", '<')],
        ),
        (
            "docs/index.html",
            &[("data-copy=\"", '"'), (">$ agentstack", '<')],
        ),
        ("docs/start.html", &[("data-copy=\"", '"')]),
        // Terminal-simulation arrays: `['$ agentstack …', 'g']` / `['$ as …', 'y']`.
        (
            "docs/examples.html",
            &[("data-copy=\"", '"'), ("['$ ", '\'')],
        ),
        // Tutorial: command buttons `{cmd:'…'}` and drift-resolver lines.
        (
            "docs/tutorial/index.html",
            &[("cmd:'", '\''), ("['$ ", '\'')],
        ),
    ];

    let mut checked = 0usize;
    let mut failures: Vec<String> = Vec::new();
    for (rel, patterns) in sources {
        let path = root.join(rel);
        let content = std::fs::read_to_string(&path).expect("readable dynamic-snippet source");
        for (start_pat, end) in *patterns {
            for (pos, raw) in extract_after(&content, start_pat, *end) {
                // `>$ agentstack` extraction drops the matched prefix; put the
                // command head back before normalizing.
                let raw = if *start_pat == ">$ agentstack" {
                    format!("agentstack{raw}")
                } else {
                    raw
                };
                let Some(tokens) = normalize_dynamic(&raw) else {
                    continue;
                };
                checked += 1;
                if let Err(err) = cli_command().try_get_matches_from(&tokens) {
                    failures.push(format!(
                        "  {rel}:{}: `{}` → {}",
                        line_number(&content, pos),
                        tokens.join(" "),
                        err.kind()
                    ));
                }
            }
        }
    }

    assert!(
        failures.is_empty(),
        "displayed/copied command(s) that don't parse on the real CLI:\n{}",
        failures.join("\n")
    );
    // Extraction floor: if a markup refactor silently empties the scan, this
    // trips before the gate quietly stops guarding anything.
    assert!(
        checked >= 30,
        "dynamic-command extraction found only {checked} commands — the \
         extractor patterns no longer match the site markup"
    );
}

// ── Vocabulary gate: one concept, one word ─────────────────────────────────
//
// The product name for "a named subset of your setup" is **toolset**. It used
// to be three names for one thing — "profile" in the CLI, "toolset" on the
// site, `[profiles.*]` in the file — and a reader had to learn all three. These
// two tests hold the line on the two surfaces a person actually reads: the
// `--help` of a command they can see, and the docs prose.
//
// What deliberately keeps the old spelling, and is therefore NOT a violation:
//   * the manifest key `[profiles.<name>]` — the file format; renaming it would
//     break every manifest on every machine for no user benefit;
//   * the JSON contract fields and feature names (`profiles`, `profile`,
//     `profiles-v1`, `profiles-edit-v1`) — versioned wire, consumed by t3code;
//   * the hidden panel verbs (`create-profile`, `use-profile`,
//     `add-*-to-profile`) and their `--profile` flags — fixed argv, not prose;
//   * internal Rust identifiers — no user reads them.
// The first three are all spelled inside backticks in the docs, so the code-span
// rule below exempts them structurally rather than by allowlist.

/// The one-line fix every failure message ends with, so a contributor does not
/// have to go find this file to learn the rule.
const VOCAB_FIX: &str = "say \"toolset\" — \"profile\" is kept only for the manifest key \
     `[profiles.<name>]`, the JSON/wire contract, and the frozen panel argv";

/// Walk the visible clap tree, collecting `(path, what, text)` for every piece
/// of help a person can reach without already knowing a hidden command's name.
/// Hidden subcommands are not descended into: their help is machine surface,
/// and the panel verbs there are *named* `…-profile` on purpose.
fn visible_help_texts(
    cmd: &clap::Command,
    path: &str,
    out: &mut Vec<(String, &'static str, String)>,
) {
    let mut push = |what: &'static str, text: Option<String>| {
        if let Some(t) = text {
            out.push((path.to_string(), what, t));
        }
    };
    push("about", cmd.get_about().map(ToString::to_string));
    push("long_about", cmd.get_long_about().map(ToString::to_string));
    push("after_help", cmd.get_after_help().map(ToString::to_string));
    push(
        "before_help",
        cmd.get_before_help().map(ToString::to_string),
    );
    for arg in cmd.get_arguments() {
        if arg.is_hide_set() {
            continue;
        }
        let id = arg.get_id().to_string();
        // The long name and its *visible* aliases are read off the help screen
        // too; a hidden `alias = "profile"` is back-compat plumbing, not prose.
        if let Some(long) = arg.get_long() {
            out.push((path.to_string(), "flag name", format!("--{long}")));
        }
        if let Some(vn) = arg.get_value_names() {
            for name in vn {
                out.push((path.to_string(), "value name", name.to_string()));
            }
        }
        let arg_path = format!("{path} {id}");
        if let Some(h) = arg.get_help() {
            out.push((arg_path.clone(), "arg help", h.to_string()));
        }
        if let Some(h) = arg.get_long_help() {
            out.push((arg_path, "arg long help", h.to_string()));
        }
    }
    for sub in cmd.get_subcommands() {
        if sub.is_hide_set() || sub.get_name() == "help" {
            continue;
        }
        visible_help_texts(sub, &format!("{path} {}", sub.get_name()), out);
    }
}

#[test]
fn visible_help_says_toolset() {
    let mut texts = Vec::new();
    visible_help_texts(&cli_command(), "agentstack", &mut texts);
    // Floor: if a refactor empties the walk, fail loudly instead of passing.
    assert!(
        texts.len() > 50,
        "the visible-help walk found only {} strings — it stopped descending the clap tree",
        texts.len()
    );

    let offenders: Vec<String> = texts
        .iter()
        .filter(|(_, _, text)| text.to_ascii_lowercase().contains("profile"))
        .map(|(path, what, text)| format!("  `{path}` {what}: {text}"))
        .collect();

    assert!(
        offenders.is_empty(),
        "visible command help says \"profile\" — {VOCAB_FIX}:\n{}",
        offenders.join("\n")
    );
}

/// Legitimate "profile" spellings in docs prose (outside every code span).
/// Each entry is a full line substring the scan may skip, with why. Keep this
/// list tiny: a growing allowlist is this lint failing silently, and the right
/// fix for a new entry is almost always to put the identifier in backticks —
/// which exempts it structurally, because it is then code, not prose.
const VOCAB_ALLOWLIST: &[&str] = &[
    // docs/reference.md — the link target of the runnable lease example. It is
    // a real directory on disk (`examples/mcp-profile-lease/`), so the path
    // cannot be reworded without moving the example.
    "examples/mcp-profile-lease",
];

/// README + every Markdown page under `docs/`, recursively.
///
/// `docs/design/` is excluded: those files document the wire contract and the
/// storage format by their real names (`profiles-edit-v1`, `UseArgs.profile`),
/// they are internal engineering records rather than reader-facing prose, and
/// the sibling `every_prose_command_is_real` scan skips them for the same
/// reason. Anything a reader reaches from the docs site is in scope.
fn vocab_doc_files(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries {
            let path = entry.expect("readable dir entry").path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n == "design") {
                    continue;
                }
                walk(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
                out.push(path);
            }
        }
    }
    let mut files = vec![root.join("README.md")];
    walk(&root.join("docs"), &mut files);
    files.sort();
    files
}

#[test]
fn docs_prose_say_toolset() {
    let root = repo_root();
    let files = vocab_doc_files(&root);
    // Floor: the scan set must not silently collapse to nothing.
    assert!(
        files.len() >= 10,
        "the docs vocabulary scan found only {} file(s)",
        files.len()
    );

    let mut violations = Vec::new();
    for path in files {
        let content = std::fs::read_to_string(&path).expect("readable docs page");
        let spans = markdown_code_spans(&content);
        let display_path = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        let lower = content.to_ascii_lowercase();
        for (pos, _) in lower.match_indices("profile") {
            if in_any_span(pos, &spans) {
                continue; // an identifier, a manifest key, a flag, a command
            }
            let snippet = snippet_for_line(&content, pos);
            if VOCAB_ALLOWLIST.iter().any(|a| snippet.contains(a)) {
                continue;
            }
            violations.push(Violation {
                file: display_path.clone(),
                line: line_number(&content, pos),
                snippet,
            });
        }
    }

    assert!(
        violations.is_empty(),
        "docs prose says \"profile\" outside a code span — {VOCAB_FIX}:\n{}",
        violations
            .iter()
            .map(|v| format!("  {}:{}: {}", v.file, v.line, v.snippet))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn action_default_binary_matches_this_release() {
    let action = include_str!("../../../action.yml");
    let version_input = action
        .split("  version:\n")
        .nth(1)
        .and_then(|tail| tail.split("  working-directory:\n").next())
        .expect("action.yml must keep inputs.version before working-directory");
    let default = version_input
        .lines()
        .find_map(|line| line.trim().strip_prefix("default:"))
        .map(str::trim)
        .expect("action.yml inputs.version must have a default");
    assert_eq!(
        default,
        format!("v{}", env!("CARGO_PKG_VERSION")),
        "a pinned action release must install its matching binary by default"
    );
}

// ------------------------------------------------- authored-page vocabulary --

/// Output paths (relative to `docs/`) of every page `tools/make-docs-pages.py`
/// compiles from Markdown.
///
/// Parsed from the script's `PAGES` list at test time rather than duplicated
/// here. That inversion is the robust direction: the authored set is then
/// *everything else* under `docs/`, so a brand-new hand-written page is
/// covered by the vocabulary gate the moment it lands, with nobody having to
/// remember to enroll it. Hardcoding the authored six would silently fail open
/// on the seventh. Compiled pages are excluded because editing them is a
/// no-op — the generator overwrites them — so their vocabulary has to be
/// fixed in the `.md` source instead, where the existing Markdown prose gate
/// already applies.
fn compiled_html_pages(root: &Path) -> HashSet<String> {
    let script = std::fs::read_to_string(root.join("tools/make-docs-pages.py"))
        .expect("tools/make-docs-pages.py readable");
    let list = script
        .split_once("PAGES = [")
        .map(|(_, tail)| tail)
        .and_then(|tail| tail.split_once("\n]"))
        .map(|(body, _)| body)
        .expect("make-docs-pages.py must keep its `PAGES = [` ... `\n]` literal");

    // Each entry is ("<src>.md", "<out>.html", "<sidebar key>"); take the
    // middle field. A plain quote-split keeps this free of a regex dependency.
    let mut out = HashSet::new();
    for line in list.lines() {
        let fields: Vec<&str> = line.split('"').collect();
        // ["    (", src, ", ", out, ", ", key, "),"] → quoted fields are odd indices.
        if fields.len() >= 4 {
            out.insert(fields[3].to_string());
        }
    }
    assert!(
        out.contains("concepts.html") && out.contains("howto/undo.html"),
        "PAGES parse produced an implausible set — did the literal's shape change? got {out:?}"
    );
    out
}

/// Every hand-authored HTML page under `docs/`: the whole tree minus the
/// compiled outputs, minus pure design assets, minus redirect stubs.
fn authored_html_pages(root: &Path) -> Vec<PathBuf> {
    let compiled = compiled_html_pages(root);
    let docs = root.join("docs");
    let mut found = Vec::new();
    collect_html(&docs, &mut found);

    let mut pages: Vec<PathBuf> = found
        .into_iter()
        .filter(|p| {
            let rel = p.strip_prefix(&docs).unwrap().to_string_lossy().to_string();
            // `design/` and `theme/` hold brand mockups and the OG card, not
            // documentation prose.
            if rel.starts_with("design/") || rel.starts_with("theme/") {
                return false;
            }
            if compiled.contains(&rel) {
                return false;
            }
            let content = std::fs::read_to_string(p).expect("readable html doc");
            !content.contains(r#"http-equiv="refresh""#) // redirect stub
        })
        .collect();
    pages.sort();
    assert!(
        pages.iter().any(|p| p.ends_with("index.html")),
        "the landing page must be in the authored set"
    );
    pages
}

fn collect_html(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let path = entry.expect("readable dir entry").path();
        if path.is_dir() {
            collect_html(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("html") {
            out.push(path);
        }
    }
}

/// Byte ranges of every `style="..."` attribute value. These pages carry heavy
/// inline CSS; a property or custom-property name is not prose and must never
/// trip the vocabulary gate.
fn style_attr_spans(content: &str) -> Vec<(usize, usize)> {
    let lower = content.to_ascii_lowercase();
    let mut spans = Vec::new();
    let mut from = 0usize;
    while let Some(rel) = lower[from..].find("style=\"") {
        let open = from + rel + "style=\"".len();
        match content[open..].find('"') {
            Some(len) => {
                spans.push((open, open + len));
                from = open + len + 1;
            }
            None => break, // unterminated attribute; nothing sane to skip
        }
    }
    spans
}

/// Substrings in which "profile" is a frozen *identifier* rather than a word
/// of prose. These are contracts we do not own the spelling of, so the gate
/// must let them through wherever they appear.
const PROFILE_IDENTIFIERS: &[&str] = &[
    // The manifest table key. The concept was renamed; the TOML key was not.
    "[profiles.",
    // The MCP wire field, e.g. `agentstack_lease_open({ "profile": "backend" })`.
    "\"profile\"",
    // Panel/MCP verb names in the fixed-argv contract t3code drives.
    "create-profile",
    "use-profile",
    "_create_profile",
    // The flag as the fixed-argv panel verbs still spell it. On VISIBLE
    // commands the long form is now `--toolset`, with `--profile` kept as a
    // working alias — so prose showing a visible command should say
    // `--toolset`, and this entry exists for the hidden panel contract only.
    "--profile",
    // A real directory (`examples/mcp-profile-lease/`), wired into CI.
    "mcp-profile-lease",
];

/// Byte ranges covered by any [`PROFILE_IDENTIFIERS`] occurrence.
fn identifier_spans(content: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    for needle in PROFILE_IDENTIFIERS {
        let mut from = 0usize;
        while let Some(rel) = content[from..].find(needle) {
            let start = from + rel;
            spans.push((start, start + needle.len()));
            from = start + needle.len();
        }
    }
    spans
}

#[test]
fn authored_html_pages_say_toolset() {
    let root = repo_root();
    let mut violations: Vec<String> = Vec::new();

    for path in authored_html_pages(&root) {
        let content = std::fs::read_to_string(&path).expect("readable html doc");
        let lower = content.to_ascii_lowercase();

        // Regions where "profile" is never prose: rendered code, inline CSS,
        // and the frozen identifiers above.
        let mut skip = html_code_spans(&content);
        skip.extend(style_attr_spans(&content));
        skip.extend(identifier_spans(&content));

        let display = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();

        for (pos, _) in lower.match_indices("profile") {
            if in_any_span(pos, &skip) {
                continue;
            }
            violations.push(format!(
                "{display}:{} — {}",
                line_number(&content, pos),
                snippet_for_line(&content, pos)
            ));
        }
    }

    assert!(
        violations.is_empty(),
        "hand-authored site pages must say \"toolset\", not \"profile\" — one \
         concept, one word. Rewrite the prose below (a frozen identifier such \
         as the `[profiles.*]` TOML key, the `\"profile\"` MCP wire field, the \
         `--profile` flag, or a `*-profile` panel verb belongs in \
         PROFILE_IDENTIFIERS instead):\n{}",
        violations.join("\n")
    );
}

/// Claim phrasings that overstate what any mode enforces. Each is banned
/// because `docs/ENFORCEMENT.md` explicitly says the opposite, and marketing
/// copy is exactly where the caveat gets dropped for rhythm.
///
/// The rule these encode is the one in ENFORCEMENT.md's own claim discipline:
/// AgentStack **restricts destinations and records decisions**. It does not
/// inspect payloads, so nothing it does can be described as stopping
/// exfiltration, and no mode is "secure by default" — the enforcement matrix
/// has four postures precisely because they differ.
///
/// The fix is never to soften the product. It is to name the mode and the
/// mechanism: "blocks connections to hosts you did not approve, on the
/// enforced paths" says more than "prevents exfiltration" and is true.
const OVERSTATED_CLAIMS: &[(&str, &str)] = &[
    (
        "block exfiltration",
        "destinations are restricted; payloads are never inspected",
    ),
    (
        "blocks exfiltration",
        "destinations are restricted; payloads are never inspected",
    ),
    (
        "prevents exfiltration",
        "destinations are restricted; payloads are never inspected",
    ),
    (
        "prevent exfiltration",
        "destinations are restricted; payloads are never inspected",
    ),
    (
        "stops exfiltration",
        "destinations are restricted; payloads are never inspected",
    ),
    (
        "exfiltration is impossible",
        "ENFORCEMENT.md names this exact sentence as the claim never to make",
    ),
    (
        "secure by default",
        "name the posture — host, gateway, sandbox, or lockdown — and what it enforces",
    ),
    (
        "fully secure",
        "no mode claims this; cite the enforcement cell instead",
    ),
    ("bank-grade", "not a claim this project can substantiate"),
    (
        "military-grade",
        "not a claim this project can substantiate",
    ),
];

/// F06: every public safety claim must survive a reading of
/// `docs/ENFORCEMENT.md`.
///
/// The technical caveats in this repository are unusually careful; the risk was
/// always that the marketing copy would not inherit them. It already had not:
/// the examples index promised to "block exfiltration" three clicks from a
/// page stating that an allowed destination can still receive anything.
///
/// This is a lint over phrasings rather than a judgement of meaning, so it
/// cannot catch every overstatement. It catches the ones that are cheap to
/// write by accident, which is the failure mode that actually occurred.
///
/// Two exemptions, both for the same reason: the careful docs say these
/// sentences in order to *deny* them. `ENFORCEMENT.md` and its rendered page
/// are skipped wholesale, because stating the ceiling is their entire job. And
/// anywhere else, a match preceded by a negation is the good case — "never
/// means exfiltration is impossible" is the sentence we want, and a lint that
/// failed it would teach the next person to delete the caveat.
#[test]
fn public_docs_make_no_claim_enforcement_md_denies() {
    let root = repo_root();
    let mut files = vocab_doc_files(&root);
    files.extend(authored_html_pages(&root));

    assert!(
        files.len() >= 10,
        "the claim scan found only {} file(s) — the discovery broke, not the docs",
        files.len()
    );

    let mut violations: Vec<String> = Vec::new();
    for path in files {
        let display = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        // The two pages whose job is to state the ceiling quote the banned
        // phrases to rule them out.
        if display.ends_with("ENFORCEMENT.md") || display.ends_with("enforcement.html") {
            continue;
        }
        let content = std::fs::read_to_string(&path).expect("readable docs page");
        let lower = content.to_ascii_lowercase();
        for (claim, why) in OVERSTATED_CLAIMS {
            for (pos, _) in lower.match_indices(claim) {
                if negated_before(&lower, pos) {
                    continue;
                }
                violations.push(format!(
                    "{display}:{} — \"{claim}\": {why}\n      {}",
                    line_number(&content, pos),
                    snippet_for_line(&content, pos)
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "public copy makes a claim docs/ENFORCEMENT.md denies. Name the mode and \
         the mechanism instead of the outcome:\n{}",
        violations.join("\n")
    );
}

/// Is this claim being denied rather than made?
///
/// Looks back over the same sentence for a negation. The window stops at a
/// sentence boundary so a denial in one sentence cannot excuse an assertion in
/// the next — "we cannot inspect payloads. AgentStack blocks exfiltration."
/// must still fail, which is exactly the shape a careless edit produces.
fn negated_before(lower: &str, pos: usize) -> bool {
    const NEGATIONS: &[&str] = &[
        "never",
        "not ",
        "n't",
        "cannot",
        "no claim",
        "rather than",
        "instead of",
        "does not mean",
        "is not",
    ];
    let window_start = pos.saturating_sub(160);
    // Respect char boundaries: these files are UTF-8 and full of em dashes.
    let start = (window_start..=pos)
        .find(|i| lower.is_char_boundary(*i))
        .unwrap_or(pos);
    let window = &lower[start..pos];
    // Only the current sentence counts.
    let sentence = window
        .rfind(['.', '!', '?'])
        .map(|i| &window[i + 1..])
        .unwrap_or(window);
    // Prose wraps, so "not\nthat exfiltration is impossible" must read the
    // same as "not that …". Collapse runs of whitespace to one space before
    // matching, or the lint fails a sentence purely for where the line broke.
    let sentence: String = {
        let mut s = String::with_capacity(sentence.len());
        let mut in_ws = false;
        for c in sentence.chars() {
            if c.is_whitespace() {
                if !in_ws {
                    s.push(' ');
                }
                in_ws = true;
            } else {
                s.push(c);
                in_ws = false;
            }
        }
        s
    };
    NEGATIONS.iter().any(|n| sentence.contains(n))
}
