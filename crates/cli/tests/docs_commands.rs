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
