// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Docs-vs-CLI sync gate.
//!
//! Two checks:
//! 1. `every_subcommand_is_documented_in_the_reference` — every top-level
//!    subcommand (hidden ones included — `trust`, `connect`, `mcp` are
//!    hidden but documented) must appear in docs/reference.md's "All
//!    commands" inventory. The command surface grows fast; a hand-maintained
//!    list silently rots without this.
//! 2. `every_prose_command_is_real` — the inverse direction: every
//!    `agentstack <verb> [<subverb>]` invocation written in a code span or
//!    fenced block across the docs must name a command that actually exists
//!    on the clap tree. This is how `agentstack stats` and a bare
//!    `agentstack connect` (neither ever existed / needed an argument)
//!    survived review — the roster check above only verifies the inventory
//!    list, not free prose elsewhere in the docs.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use clap::CommandFactory;

/// The clap `Command` tree, as `every_subcommand_is_documented_in_the_reference`
/// already builds it. Shared by both tests so there's one source of truth for
/// "what does the CLI actually expose".
fn cli_command() -> clap::Command {
    agentstack::cli::Cli::command()
}

#[test]
fn every_subcommand_is_documented_in_the_reference() {
    let reference = include_str!("../../../docs/reference.md");
    let section = reference
        .split("## All commands")
        .nth(1)
        .expect("docs/reference.md must keep an '## All commands' section")
        .split("\n## ")
        .next()
        .unwrap();

    let cmd = cli_command();
    let mut missing: Vec<String> = Vec::new();
    for sc in cmd.get_subcommands() {
        let name = sc.get_name();
        if name == "help" {
            continue;
        }
        if !section.contains(name) {
            missing.push(name.to_string());
        }
    }
    assert!(
        missing.is_empty(),
        "subcommand(s) missing from the 'All commands' inventory in docs/reference.md: {missing:?}"
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

/// The full scan set: README, top-level docs (minus the historical record
/// and dated design docs), redirect-stub-free HTML docs, CONTRIBUTING, and
/// every catalog skill.
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
            Some("md") => {
                if path.file_name().is_some_and(|n| n == "HISTORY.md") {
                    continue; // dated historical record, not current surface
                }
                files.push((path, Kind::Markdown));
            }
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
        // Word boundary before the match: skip "myagentstack" etc.
        if let Some(prev) = content[..match_pos].chars().next_back() {
            if prev.is_ascii_alphanumeric() || prev == '-' || prev == '_' {
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
        // Top-level verb is real. Only a real command with actual
        // sub-subcommands gets its second token checked — otherwise a
        // positional argument (`agentstack use backend`) would be
        // misread as a subcommand attempt.
        let Some(sub_names) = subs.get(tok1).filter(|s| !s.is_empty()) else {
            continue;
        };
        let Some((tok2_pos, tok2)) = second else {
            continue;
        };
        if !looks_like_command_token(tok2) {
            continue;
        }
        if ALLOWLIST.contains(&tok2) {
            continue;
        }
        if !sub_names.contains(tok2) {
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
        subs.insert(name.to_string(), sub_names);
        top.insert(name.to_string());
    }

    let root = repo_root();
    let mut violations = Vec::new();
    for (path, kind) in files_to_scan(&root) {
        scan_file(&path, &kind, &top, &subs, &mut violations);
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
