#!/usr/bin/env python3
"""Render the source-of-truth Markdown docs pages into styled site pages.

The Markdown stays canonical — concepts.md, choose.md, reference.md,
ARCHITECTURE.md, ENFORCEMENT.md, and howto/*.md are what you edit,
review, and read on GitHub. This script compiles each of them (see PAGES
below) into a docs-site HTML page carrying the same shell (header, sidebar,
footer, CSS variables) as docs.html, so site visitors never leave the site for
any of them. Links that target some other repo file — one this script does
not compile into a page — are rewritten to GitHub blob/tree URLs instead.

Deliberately supports only the Markdown subset those pages use — ATX headings,
paragraphs, flat lists, pipe tables, fenced code, bold/italic/inline
code/links. Anything unrecognized is reported loudly rather than silently
mangled, so drift in the sources is visible at build time.

The sidebar it splices in is two-tier: everyday groups render inline while the
advanced ("deeper") groups collapse into <details>, auto-opened on the page
they contain — so this CSS must style <summary> to match the group label.

Usage: python3 tools/make-docs-pages.py       # rewrites docs/*.html pages
Run it after editing any source page, and together with make-docs-sidebar.py
after editing that script's TREE (this script imports the TREE live).
"""

import html
import importlib.util
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DOCS = ROOT / "docs"
GH = "https://github.com/Tarekkharsa/agentstack"
SITE = "https://tarekkharsa.github.io/agentstack"

# (markdown source relative to docs/, html output relative to docs/, sidebar key)
PAGES = [
    ("start.md", "start.html", "start"),
    ("library.md", "library.html", "library"),
    ("tutorial.md", "tutorial/index.html", "tutorial"),
    ("concepts.md", "concepts.html", "concepts"),
    ("choose.md", "choose.html", "choose"),
    ("migrations.md", "migrations.html", "migrations"),
    ("troubleshooting.md", "troubleshooting.html", "troubleshooting"),
    ("faq.md", "faq.html", "faq"),
    ("reference.md", "reference.html", "reference"),
    ("integrations.md", "integrations.html", "integrations"),
    ("adapters.md", "adapters.html", "adapters"),
    ("automation.md", "automation.html", "automation"),
    ("ARCHITECTURE.md", "architecture.html", "how-it-works"),
    ("workflows.md", "workflows.html", "workflows"),
    ("ENFORCEMENT.md", "enforcement.html", "matrix"),
    ("howto/add-a-server.md", "howto/add-a-server.html", "howto-server"),
    ("howto/add-a-skill.md", "howto/add-a-skill.html", "howto-skill"),
    ("howto/name-a-toolset.md", "howto/name-a-toolset.html", "howto-toolset"),
    ("howto/run-a-workflow.md", "howto/run-a-workflow.html", "howto-workflow"),
    ("howto/trust-a-repo.md", "howto/trust-a-repo.html", "howto-trust"),
    ("howto/lock-down-a-run.md", "howto/lock-down-a-run.html", "howto-lockdown"),
    ("howto/team-setup.md", "howto/team-setup.html", "howto-team"),
    ("howto/ci.md", "howto/ci.html", "howto-ci"),
    ("howto/undo.md", "howto/undo.html", "howto-undo"),
    ("howto/see-what-happened.md", "howto/see-what-happened.html", "howto-audit"),
]
MD_TO_HTML = {src: out for src, out, _ in PAGES}

# ---------------------------------------------------------------- sidebar --
# Import the sidebar tree/renderer from its dashed filename.
_spec = importlib.util.spec_from_file_location(
    "make_docs_sidebar", Path(__file__).resolve().parent / "make-docs-sidebar.py"
)
_sidebar = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_sidebar)


def esc(s):
    return html.escape(s, quote=False)


def slug(text, seen):
    """GitHub's heading slugger: lowercase, drop punctuation in place, spaces→'-'."""
    s = re.sub(r"[^\w\- ]", "", text.lower()).replace(" ", "-")
    base, n = s, 1
    while s in seen:
        s = f"{base}-{n}"
        n += 1
    seen.add(s)
    return s


# ------------------------------------------------------------ link rewrite --
def rewrite_href(href, src_rel, out_rel, warnings):
    """Map a Markdown link target onto the generated site.

    Same-page anchors and absolute URLs pass through. Links to pages this
    script generates become site-local .html links; every other repo file
    becomes a GitHub blob/tree URL (those pages are GitHub-canonical).
    """
    if href.startswith("#") or re.match(r"^[a-z][a-z0-9+.-]*:", href):
        return href
    path, _, frag = href.partition("#")
    frag = f"#{frag}" if frag else ""
    # Resolve the target relative to the source file, expressed docs/-relative.
    src_dir = Path(src_rel).parent
    target = (src_dir / path).as_posix()
    parts = []
    for seg in target.split("/"):
        if seg == "..":
            parts and parts.pop() or parts.append("..")
        elif seg not in (".", ""):
            parts.append(seg)
    target = "/".join(parts)

    depth = len(Path(out_rel).parent.parts)
    if target in MD_TO_HTML:
        return "../" * depth + MD_TO_HTML[target] + frag

    fs = (DOCS / target) if not target.startswith("..") else (ROOT / target[3:])
    repo_rel = fs.resolve().relative_to(ROOT).as_posix() if fs.exists() else None
    if repo_rel:
        # Site assets (images) stay site-local — GitHub Pages serves docs/.
        if not target.startswith("..") and fs.suffix in (".svg", ".png", ".gif", ".webp"):
            return "../" * depth + target + frag
        kind = "tree" if fs.is_dir() else "blob"
        return f"{GH}/{kind}/main/{repo_rel}{frag}"
    warnings.append(f"{src_rel}: unresolved link target '{href}'")
    return href


# --------------------------------------------------------------- md → html --
INLINE_CODE = re.compile(r"`([^`]+)`")
BOLD = re.compile(r"\*\*(.+?)\*\*")
ITALIC = re.compile(r"(?<![*\w])\*([^*]+)\*(?![*\w])")
LINK = re.compile(r"\[([^\]]+)\]\(([^)\s]+)\)")
# A list item that is a bare link and nothing else. A list built ONLY of these
# is a table of contents — navigation — not running prose, so it must not carry
# the prose underline. That property is decided here, from the source, and
# recorded as a class; the stylesheet never has to guess it back out of the
# rendered markup with a selector that also catches real prose links.
LINK_ONLY = re.compile(r"\[[^\]]+\]\([^)\s]+\)")


def link_only(text):
    """True when this list item is one bare link and nothing else.

    Code spans are lifted into placeholders first, exactly as inline() does, so
    a label that contains brackets of its own — ``[Governance (`[policy]`)]`` —
    still reads as a single link instead of terminating the label early.
    """
    count = [0]

    def lift(_m):
        count[0] += 1
        return f"\x00{count[0]}\x00"

    return bool(LINK_ONLY.fullmatch(INLINE_CODE.sub(lift, text.strip())))


def inline(text, src_rel, out_rel, warnings):
    """Inline markdown → HTML. Code spans are lifted out into placeholders
    first so markup characters inside them are never interpreted — but bold,
    italic, and links still match across them (e.g. **bold with `code`**)."""
    spans = []

    def lift(m):
        spans.append(f"<code>{esc(m.group(1))}</code>")
        return f"\x00{len(spans) - 1}\x00"

    chunk = INLINE_CODE.sub(lift, text)
    chunk = esc(chunk)
    chunk = LINK.sub(
        lambda m: '<a href="%s">%s</a>'
        % (rewrite_href(html.unescape(m.group(2)), src_rel, out_rel, warnings), m.group(1)),
        chunk,
    )
    chunk = BOLD.sub(r"<strong>\1</strong>", chunk)
    chunk = ITALIC.sub(r"<em>\1</em>", chunk)
    # Markdown backslash-escapes, resolved after formatting so an escaped
    # character can never pair into bold/italic/link syntax.
    chunk = re.sub(r"\\(&lt;|&gt;|[*_`\[\]])", r"\1", chunk)
    return re.sub(r"\x00(\d+)\x00", lambda m: spans[int(m.group(1))], chunk)


# ------------------------------------------------------------- code blocks --
# A fenced block renders as a card (`.cb` in theme/organic.css): hairline
# border, clipped corners, a header strip naming the language, and a <pre> that
# is the only thing allowed to scroll. Shell blocks additionally get terminal
# chrome and per-line roles.
#
# HOW A COMMAND LINE IS TOLD FROM AN OUTPUT LINE
# ----------------------------------------------
# No new syntax is invented. The rule below was chosen by reading every fenced
# block under docs/**.md and matching what is already written there:
#
#   bash | sh | shell | zsh   "commands to run". Across all 84 such blocks in
#                             docs/ there is not one `$` prompt and not one
#                             line of program output — they are copy-paste
#                             command lists. So every line is a command, and a
#                             `$` prompt is DRAWN in front of each line that
#                             starts one: not a `\`-continuation, not a
#                             comment, not a blank line. The prompt is
#                             user-select:none, so copying still yields
#                             runnable text.
#
#   console | terminal |      "a recorded session" — and all three such blocks
#   session, plus any block   in docs/ do carry `$ ` prompts. A line beginning
#   whose first non-blank     `$ ` is a command: its own prompt is kept and
#   line begins with "$ "     coloured, the rest is at full brightness. EVERY
#                             OTHER LINE IS PROGRAM OUTPUT and is dimmed to
#                             --term-muted. That luminance split is what makes
#                             a transcript readable at a glance. The
#                             "first line starts with $" clause is what catches
#                             the ten ```text blocks that are really
#                             transcripts (tutorial.md, troubleshooting.md).
#
#   any block whose every     also a command list. 24 of the 102 ```text blocks
#   non-blank, non-comment    in docs/ are exactly this — reference.md writes
#   line begins with the      its per-verb recipes that way — and a block in
#   CLI name                  which EVERY line is literally an invocation of
#                             this CLI cannot be anything else. It is a
#                             mechanical test over content that already exists,
#                             not a syntax authors have to learn.
#
#   anything else             toml, json, yaml, js, ts, jsonc, prose samples,
#                             ASCII trees, captured output — a plain code card.
#                             No prompts, no dimming, no guessed roles. Where a
#                             block cannot be classified from what is written,
#                             it is styled well rather than labelled wrongly.
#
# Comments are dimmed inside terminal blocks either way. A `#` opens a comment
# only when it starts the line or follows whitespace AND is outside any quoted
# string, so `docs.html#start` and `--header "a#b"` stay code.
#
# One markup constraint, deliberately honoured below: a command's text is never
# split across elements. crates/cli/tests/docs_commands.rs parses every command
# shown in the docs out of the rendered HTML by finding "agentstack" inside a
# <pre>/<code> and reading the next two whitespace-delimited tokens. Wrapping a
# whole line is invisible to it; splitting `agentstack` from its verb would
# silently stop it checking that command.
CLI = "agentstack"
RUN_LANGS = {"bash", "sh", "shell", "zsh"}
SESSION_LANGS = {"console", "terminal", "session"}
# Languages worth naming in the header strip. An unlabelled fence and ```text
# name nothing useful, so those cards get no strip.
LABELLED_LANGS = RUN_LANGS | SESSION_LANGS | {
    "toml", "json", "jsonc", "yaml", "yml", "js", "ts", "tsx", "rust", "python",
    "diff", "ini", "xml", "html", "css", "sql", "make", "dockerfile",
}


def split_comment(line):
    """Split a shell line into (code, comment). `comment` keeps its leading
    `#` and is empty when the line has none."""
    quote = None
    for idx, ch in enumerate(line):
        if quote is not None:
            if ch == quote:
                quote = None
        elif ch in "'\"":
            quote = ch
        elif ch == "#" and (idx == 0 or line[idx - 1].isspace()):
            return line[:idx], line[idx:]
    return line, ""


def shell_code(text):
    """Escaped shell text with any trailing comment dimmed."""
    code, comment = split_comment(text)
    if not comment:
        return esc(text)
    return esc(code) + f'<span class="cm">{esc(comment)}</span>'


def all_lines_invoke_cli(lines):
    """True when every line that is neither blank nor a comment starts with the
    CLI name at column 0 — the shape of a hand-written command list."""
    real = [ln for ln in lines if ln.strip() and not ln.lstrip().startswith("#")]
    return bool(real) and all(ln == CLI or ln.startswith(CLI + " ") for ln in real)


def render_block(lang, lines):
    """One fenced block → the `.cb` card. See the rule block above."""
    key = lang.lower()
    first = next((ln for ln in lines if ln.strip()), "")
    session = key in SESSION_LANGS or first.lstrip().startswith("$ ")
    run = not session and (key in RUN_LANGS or all_lines_invoke_cli(lines))

    rendered = []
    if session:
        for raw in lines:
            stripped = raw.lstrip(" ")
            if stripped.startswith("$ "):
                indent = raw[: len(raw) - len(stripped)]
                rendered.append(
                    f'{esc(indent)}<span class="pr">$ </span>'
                    + shell_code(stripped[2:])
                )
            elif raw.strip():
                rendered.append(f'<span class="out">{esc(raw)}</span>')
            else:
                rendered.append(esc(raw))
    elif run:
        continued = False
        for raw in lines:
            body = raw.lstrip(" ")
            if not body:
                rendered.append(esc(raw))
                continue
            if body.startswith("#"):
                rendered.append(f'<span class="cm">{esc(raw)}</span>')
                continued = False
                continue
            # A prompt marks where a command STARTS, so a `\`-continuation of
            # the previous line keeps the source's own indentation instead.
            prompt = "" if continued else '<span class="pr">$ </span>'
            indent = raw[: len(raw) - len(body)]
            rendered.append(esc(indent) + prompt + shell_code(body))
            continued = split_comment(body)[0].rstrip().endswith("\\")
    else:
        rendered = [esc(raw) for raw in lines]

    label = key if key in LABELLED_LANGS else ""
    bar = ""
    if session or run:
        dots = '<span class="cb-dots" aria-hidden="true"><span></span><span></span><span></span></span>'
        bar = f'<div class="cb-bar">{dots}<span class="cb-lang">{esc(label or "shell")}</span></div>'
    elif label:
        bar = f'<div class="cb-bar"><span class="cb-lang">{esc(label)}</span></div>'

    cls = "cb cb-term" if (session or run) else "cb"
    attr = f' data-language="{esc(label)}"' if label else ""
    code_cls = f' class="language-{esc(label)}"' if label else ""
    return (
        f'<div class="{cls}"{attr}>{bar}'
        f'<pre class="block"><code{code_cls}>' + "\n".join(rendered) + "</code></pre></div>"
    )


def convert(md, src_rel, out_rel, warnings):
    """The page-body converter: returns (article_html, title, first_paragraph)."""
    lines = md.split("\n")
    out, seen_slugs = [], set()
    title, first_para = None, None
    i, in_ul, in_ol = 0, False, False
    # Where the open <ul>'s opening tag sits in `out`, and whether every item
    # seen so far is a bare link. Both are settled only when the list closes,
    # so the tag is patched in place rather than guessed up front.
    ul_at, ul_links_only = None, True

    def close_lists():
        nonlocal in_ul, in_ol, ul_at, ul_links_only
        if in_ul:
            if ul_links_only and ul_at is not None:
                out[ul_at] = '<ul class="navlist">'
            out.append("</ul>")
            in_ul = False
            ul_at, ul_links_only = None, True
        if in_ol:
            out.append("</ol>")
            in_ol = False

    while i < len(lines):
        line = lines[i]

        # HTML comments (single- or multi-line) are source-only: consume the
        # whole block and emit nothing — converting their inner lines as
        # Markdown would leave the comment unclosed and swallow the page.
        if line.lstrip().startswith("<!--"):
            while i < len(lines) and not lines[i].rstrip().endswith("-->"):
                i += 1
            i += 1
            continue

        if line.startswith("```"):
            close_lists()
            lang = line[3:].strip()
            block = []
            i += 1
            while i < len(lines) and not lines[i].startswith("```"):
                block.append(lines[i])
                i += 1
            i += 1
            if lang == "mermaid":
                gh_page = f"{GH}/blob/main/docs/{src_rel}"
                out.append(
                    f'<p class="gennote">The diagram below is Mermaid source — '
                    f'<a href="{gh_page}">view it rendered on GitHub</a>.</p>'
                )
            out.append(render_block(lang, block))
            continue

        m = re.match(r"^(#{1,4}) +(.*)$", line)
        if m:
            close_lists()
            level, text = len(m.group(1)), m.group(2).strip()
            sid = slug(re.sub(r"[`*]", "", text), seen_slugs)
            if level == 1 and title is None:
                title = re.sub(r"[`*]", "", text)
                out.append(f"<h1 id=\"{sid}\">{inline(text, src_rel, out_rel, warnings)}</h1>")
            else:
                out.append(
                    f'<h{level} id="{sid}">{inline(text, src_rel, out_rel, warnings)}'
                    f'<a class="hlink" href="#{sid}" aria-label="Link to this section">#</a></h{level}>'
                )
            i += 1
            continue

        if line.startswith("|") and i + 1 < len(lines) and re.match(r"^\|[\s:|-]+\|?$", lines[i + 1]):
            close_lists()
            header = [c.strip() for c in line.strip().strip("|").split("|")]
            i += 2
            rows = []
            while i < len(lines) and lines[i].startswith("|"):
                rows.append([c.strip() for c in lines[i].strip().strip("|").split("|")])
                i += 1
            out.append('<div class="tblwrap"><table>')
            out.append(
                "<thead><tr>"
                + "".join(f"<th>{inline(c, src_rel, out_rel, warnings)}</th>" for c in header)
                + "</tr></thead><tbody>"
            )
            for r in rows:
                out.append(
                    "<tr>" + "".join(f"<td>{inline(c, src_rel, out_rel, warnings)}</td>" for c in r) + "</tr>"
                )
            out.append("</tbody></table></div>")
            continue

        m = re.match(r"^[-*] +(.*)$", line)
        if m:
            if in_ol:
                out.append("</ol>")
                in_ol = False
            if not in_ul:
                ul_at = len(out)
                out.append("<ul>")
                in_ul = True
            item = [m.group(1)]
            sub = []
            tail = []
            i += 1
            # Continuation lines belong to the item; "  - " lines open a
            # nested list; deeper-indented continuations belong to the last
            # nested item. A continuation that comes back to the item's own
            # indent AFTER a nested list resumes the parent item's prose, so it
            # becomes a trailing paragraph INSIDE the <li> — never a sibling of
            # the <li>, which would put a <p> straight inside the <ul> and is
            # the axe "list" (serious) violation.
            while i < len(lines):
                nm = re.match(r"^  [-*] +(.*)$", lines[i])
                if nm:
                    sub.append([nm.group(1)])
                    i += 1
                elif re.match(r"^    \S", lines[i]) and sub:
                    sub[-1].append(lines[i].strip())
                    i += 1
                elif re.match(r"^  \S", lines[i]):
                    (tail if sub else item).append(lines[i].strip())
                    i += 1
                else:
                    break
            ul_links_only = ul_links_only and not tail and all(
                link_only(" ".join(part)) for part in [item, *sub]
            )
            li = inline(" ".join(item), src_rel, out_rel, warnings)
            if sub:
                inner = "".join(
                    f"<li>{inline(' '.join(s), src_rel, out_rel, warnings)}</li>" for s in sub
                )
                li += f"<ul>{inner}</ul>"
            if tail:
                li += f"<p>{inline(' '.join(tail), src_rel, out_rel, warnings)}</p>"
            out.append(f"<li>{li}</li>")
            continue

        m = re.match(r"^\d+\. +(.*)$", line)
        if m:
            if in_ul:
                out.append("</ul>")
                in_ul = False
            if not in_ol:
                out.append("<ol>")
                in_ol = True
            item = [m.group(1)]
            i += 1
            while i < len(lines) and re.match(r"^   \S", lines[i]):
                item.append(lines[i].strip())
                i += 1
            out.append(f"<li>{inline(' '.join(item), src_rel, out_rel, warnings)}</li>")
            continue

        if line.startswith(("---", "***")) and set(line.strip()) <= set("-* "):
            close_lists()
            out.append("<hr>")
            i += 1
            continue

        if line.startswith(">"):
            close_lists()
            quote = []
            while i < len(lines) and lines[i].startswith(">"):
                quote.append(lines[i].lstrip("> "))
                i += 1
            out.append(
                f"<blockquote>{inline(' '.join(quote), src_rel, out_rel, warnings)}</blockquote>"
            )
            continue

        if not line.strip():
            close_lists()
            i += 1
            continue

        m = re.match(r"^!\[([^\]]*)\]\(([^)\s]+)\)\s*$", line)
        if m:
            close_lists()
            src = rewrite_href(m.group(2), src_rel, out_rel, warnings)
            out.append(f'<img src="{src}" alt="{html.escape(m.group(1), quote=True)}">')
            i += 1
            continue

        if line.lstrip().startswith("<"):
            # Explicit anchors are expected (the kept-anchor pattern);
            # anything else raw is worth a look.
            if not re.match(r"^\s*<a id=", line):
                warnings.append(f"{src_rel}:{i + 1}: raw HTML line passed through verbatim")
            out.append(line)
            i += 1
            continue

        para = [line.strip()]
        i += 1
        while i < len(lines) and lines[i].strip() and not re.match(
            r"^(#{1,4} |[-*] |\d+\. |```|\||>)", lines[i]
        ):
            para.append(lines[i].strip())
            i += 1
        text = " ".join(para)
        if first_para is None:
            first_para = re.sub(r"[`*\[\]]|\([^)]*\)", "", text)[:155].strip()
        # A paragraph that is one bare link and nothing else — the bold part
        # headings inside a Contents block are written that way — is navigation
        # by the same test the lists use, so it carries the same class.
        cls = ' class="navlist"' if link_only(text.strip().strip("*")) else ""
        out.append(f"<p{cls}>{inline(text, src_rel, out_rel, warnings)}</p>")

    close_lists()
    return "\n".join(out), title or Path(src_rel).stem, first_para or ""


# ----------------------------------------------------------------- template --
# The shell matches the design-system pages (index/docs/examples):
# theme/organic.css supplies fonts, the whole two-axis palette
# (data-palette × data-theme), the link treatment, and the .cb code/terminal
# component; theme/theme.js owns the light/dark toggle (data-theme on <html>,
# dark default). What is left below is layout and typography for the docs
# shell only — NO colour definitions. The short token names it uses
# (--paper/--ink/--line/--code-bg/--mono/…) are aliases of the --color-* ramp,
# declared once in organic.css, so a colour changes in exactly one place.
CSS = """
  * { box-sizing: border-box; }
  html { scroll-behavior: smooth; }
  body { margin: 0; background: var(--paper); color: var(--ink); font: 16px/1.5 var(--sans); }
  a code { color: var(--ink); }
  code { font-family: var(--mono); font-size: 0.86em; background: var(--code-bg); border-radius: 5px; padding: 0.1em 0.35em; }
  header { position: sticky; top: 0; z-index: 50; background: color-mix(in srgb, var(--surface) 96%, transparent); -webkit-backdrop-filter: blur(12px); backdrop-filter: blur(12px); border-bottom: 1px solid var(--line); }
  .bar { max-width: 74rem; margin: 0 auto; padding: 0.7rem 1.35rem; display: flex; align-items: center; gap: 1.1rem; }
  .wordmark { font-family: var(--sans); font-weight: 650; font-size: 1.05rem; letter-spacing: -0.025em; color: var(--ink); display: inline-flex; align-items: center; gap: 0.6rem; }
  .wordmark:hover { text-decoration: none; }
  .wordmark .mark { height: 28px; width: auto; display: block; }
  .wordmark .wm2 { color: var(--ink); }
  nav.top { margin-left: auto; display: flex; align-items: center; gap: 1.05rem; flex-wrap: nowrap; white-space: nowrap; }
  nav.top a { font-weight: 500; font-size: 0.85rem; letter-spacing: -0.01em; color: var(--muted); }
  nav.top a:hover { color: var(--ink); text-decoration: none; }
  /* The theme control is a quiet bordered control; the one call to action is
     the inverted slab. Neither is the accent — see the button note in
     theme/organic.css. */
  nav.top .themebtn { font-family: var(--sans); font-size: 0.8rem; font-weight: 500; letter-spacing: -0.01em; padding: 0.42rem 0.85rem; border-radius: 8px; border: 1px solid var(--line); background: transparent; color: var(--muted); cursor: pointer; transition: color 0.18s ease, border-color 0.18s ease, background 0.18s ease; }
  nav.top .themebtn:hover { color: var(--ink); border-color: var(--line-strong); background: var(--quiet-hover); }
  nav.top .ghost { padding: 0.44rem 0.95rem; border: 1px solid transparent; border-radius: 8px; font-weight: 600; color: var(--paper); background: var(--ink); transition: transform 0.18s ease, background 0.18s ease; }
  nav.top .ghost:hover { transform: translateY(-1px); background: var(--btn-primary-hover); color: var(--paper); }
  nav.top .ghost:active { transform: translateY(0); }
  @media (prefers-reduced-motion: reduce) { nav.top .ghost:hover { transform: none; } }
  .docwrap { max-width: 78rem; margin: 0 auto; padding: 0 1.35rem; display: grid; grid-template-columns: 15rem minmax(0, 1fr); gap: 2.75rem; align-items: start; }
  aside.side { position: sticky; top: 4.2rem; max-height: calc(100vh - 5.5rem); overflow-y: auto; scrollbar-width: thin; padding: 1.5rem 0.15rem 2rem; font-size: 0.85rem; }
  aside.side .grp { margin-bottom: 0.6rem; }
  /* One group treatment for both tiers. An always-open group is a <div> and a
     deeper group is a <details>, but the label row — face, size, tracking,
     colour, padding and left edge — is declared once for both, so the rail
     reads as one system. The chevron is the only thing that distinguishes
     them, because it is the only thing that behaves differently. */
  aside.side .grp > b,
  aside.side details.grp > summary { display: flex; align-items: center; gap: 0.4rem; font-family: var(--mono); font-size: 0.64rem; font-weight: 600; letter-spacing: 0.13em; text-transform: uppercase; color: var(--muted); padding: 0.28rem 0.5rem; margin: 0 0 0.05rem; border-radius: 6px; list-style: none; }
  aside.side details.grp > summary { cursor: pointer; }
  aside.side details.grp > summary:hover { color: var(--ink); background: var(--quiet-hover); }
  aside.side details.grp > summary::-webkit-details-marker { display: none; }
  /* One glyph, rotated when open — rather than two different characters, which
     is what made the collapsed and open groups look unrelated. */
  aside.side details.grp > summary::after { content: "\\25B8"; margin-left: auto; font-size: 0.85em; font-weight: 400; opacity: 0.6; transition: transform 0.16s ease; }
  aside.side details.grp[open] > summary::after { transform: rotate(90deg); }
  /* The command hint orients a group ("this is the status/doctor corner") but
     it is not navigation, so it is quiet mono at body-muted colour. The accent
     is reserved for state — in this rail, the current page and nothing else. */
  aside.side .grp > code { display: block; font-family: var(--mono); font-size: 0.62rem; line-height: 1.5; color: var(--muted); background: none; border: none; padding: 0 0.5rem; margin: 0 0 0.2rem; }
  aside.side ul { list-style: none; margin: 0; padding: 0; }
  aside.side li a { display: block; padding: 0.24rem 0.5rem; border-radius: 6px; font-family: var(--sans); font-size: 0.8rem; line-height: 1.35; letter-spacing: -0.01em; color: var(--muted); transition: color 0.16s ease, background-color 0.16s ease, box-shadow 0.16s ease; }
  aside.side li a:hover { color: var(--ink); text-decoration: none; background: var(--quiet-hover); }
  /* Current page: a 2px accent bar drawn as an INSET shadow — no border, so
     nothing reflows — over an 8% accent wash. A mark in the margin, not a
     filled block. It hangs off aria-current so the visible state and the
     announced state cannot drift apart. */
  aside.side li a[aria-current="page"] { color: var(--ink); background: color-mix(in srgb, var(--accent) 8%, transparent); box-shadow: inset 2px 0 var(--accent); }
  @media (max-width: 960px) { .docwrap { display: block; } aside.side { display: none; } .bar { flex-wrap: wrap; } nav.top { flex-wrap: wrap; white-space: normal; } }
  main { min-width: 0; padding: 1.6rem 0 4rem; }
  /* One sans, weight 500, negative tracking — no display serif, no bold
     headings. The smaller headings (h3/h4) step up to 600 at body size rather
     than growing, so a subsection reads as a label and not as another title.
     Sizes are fluid: the clamp floor is what has to fit a 390px screen. */
  main h1 { font-family: var(--sans); font-weight: 500; font-size: clamp(2rem, 5.4vw, 2.7rem); line-height: 1.05; letter-spacing: -0.035em; margin: 0 0 0.7rem; }
  main h2 { font-family: var(--sans); font-weight: 500; font-size: clamp(1.5rem, 3.2vw, 1.9rem); line-height: 1.12; letter-spacing: -0.035em; margin: 2.4rem 0 0.6rem; padding-top: 1.5rem; border-top: 1px solid var(--line-soft); }
  main h3 { font-family: var(--sans); font-weight: 600; font-size: 1rem; letter-spacing: -0.015em; margin: 1.9rem 0 0.4rem; }
  main h4 { font-family: var(--sans); font-weight: 600; font-size: 0.94rem; letter-spacing: -0.015em; margin: 1.4rem 0 0.3rem; }
  .hlink { margin-left: 0.45rem; opacity: 0; font-size: 0.85em; font-weight: 400; color: var(--accent); }
  h2:hover .hlink, h3:hover .hlink, h4:hover .hlink { opacity: 0.9; }
  /* Running prose sits one step back from the page foreground; <strong> is
     promoted to full foreground, which is what makes a scanned paragraph give
     up its point. Both ends clear 4.5:1 in both themes. */
  main p, main li, main dd, main blockquote { color: var(--prose); }
  main strong, main b { color: var(--ink); font-weight: 600; }
  main p, main li { max-width: 46rem; line-height: 1.78; }
  main ul, main ol { padding-left: 1.4rem; }
  main li::marker { color: var(--accent); }
  main li + li { margin-top: 0.35rem; }
  /* A contents list is navigation: no accent markers, no prose underlines
     (theme/organic.css owns the underline exception), and set tighter and
     smaller than prose so a long table of contents scans as an index rather
     than as another section of the page. */
  main .navlist { list-style: none; padding-left: 0; margin: 0.5rem 0 1.4rem; }
  main .navlist li { line-height: 1.5; font-size: 0.94rem; }
  main .navlist li::marker { content: ""; }
  main .navlist li + li { margin-top: 0.1rem; }
  main .navlist a { color: var(--ink); }
  main .navlist ul { list-style: none; padding-left: 0.95rem; margin: 0.1rem 0 0.45rem; }
  main .navlist ul li { font-size: 0.88rem; }
  main .navlist ul a { color: var(--prose); }
  /* A paragraph that is nothing but a link is a contents heading, not prose. */
  main p.navlist { margin: 1.6rem 0 0.2rem; font-size: 0.94rem; }
  main p.navlist strong { letter-spacing: -0.01em; }
  /* Code blocks and terminals are the .cb component in theme/organic.css. */
  main .cb { margin: 1.1rem 0; }
  /* Long inline code (paths, digests, command lines) wraps inside its own
     container instead of widening the page on narrow viewports. */
  main p code, main li code, main td code, main h2 code, main h3 code { overflow-wrap: anywhere; }
  .tblwrap { overflow-x: auto; margin: 0.8rem 0; }
  table { border-collapse: collapse; font-size: 0.88rem; min-width: 60%; }
  th, td { text-align: left; padding: 0.45rem 0.8rem; border-bottom: 1px solid var(--line-soft); vertical-align: top; }
  th { font-family: var(--mono); font-size: 0.68rem; font-weight: 600; letter-spacing: 0.1em; text-transform: uppercase; color: var(--muted); }
  blockquote { margin: 1rem 0; padding: 0.2rem 1rem; border-left: 3px solid var(--accent-line); color: var(--muted); }
  main img { max-width: 100%; height: auto; }
  .gennote { font-size: 0.82rem; color: var(--muted); margin-bottom: 0.3rem; }
  .srcline { margin-top: 2.6rem; padding-top: 1rem; border-top: 1px solid var(--line-soft); font-family: var(--mono); font-size: 0.74rem; color: var(--muted); }
  .versionbar { margin: 0 0 1.8rem; padding: 0.7rem 0.95rem; border: 1px solid var(--line-soft); border-left: 3px solid var(--accent); border-radius: 0 8px 8px 0; background: var(--surface); font-size: 0.82rem; color: var(--muted); }
  .versionbar b { color: var(--ink); font-weight: 650; }
  footer { border-top: 1px solid var(--line); margin-top: 3rem; }
  footer .bar { font-size: 0.85rem; color: var(--muted); }
"""


def version_banner():
    """Say which build these pages describe.

    The docs are compiled from `main`, but the installer serves the latest
    release, so a page can document a command the reader's binary does not
    have — with nothing on the page admitting it (review finding F01).

    Derived from CHANGELOG.md, deliberately, and NOT from git. Generation must
    be a pure function of committed files: the docs CI gate regenerates and
    diffs against what is checked in, so anything environment-dependent turns
    an ordinary build into a failure. An earlier version of this read
    `git describe`, which emitted nothing at all in a tarball or a clone
    without tag history — changing every generated page.

    CHANGELOG.md carries both facts already, and the release process updates
    it: the newest `## vX.Y.Z` heading is the newest tagged build, and a
    non-empty `## Unreleased` section above it means `main` is ahead of it.

    TWO TAGS, NOT ONE. "the newest tag" and "what the installer gives you" are
    different builds whenever the newest tag is a pre-release: install.sh
    fetches `/releases/latest/download`, and GitHub's `latest` never points at
    a prerelease. So the newest heading WITHOUT a prerelease suffix is read out
    separately — that is the release a reader actually gets — and the bar names
    both. Reading only the first heading (and, before the fix, truncating it at
    the first `-`) told every reader of an RC's docs that `v0.18.0-rc.3` was
    `v0.18.0`, "the current release", when the installer was serving v0.17.1.
    """
    text = (ROOT / "CHANGELOG.md").read_text()

    # `-rc.3` is PART of the version, not trailing noise: stopping at the first
    # `-` turns a candidate into the release it is only a candidate for. Match
    # the whole semver and stop there, so the ` — <date>` tail is left behind
    # without the prerelease going with it.
    tags = re.findall(r"^## +(v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?)", text, re.M)
    if not tags:
        # No release recorded yet. "Ahead of the latest release" is not a
        # meaningful thing to say, so say the part that is still true.
        return (
            '<p class="versionbar">These pages are generated from '
            "<code>main</code> and may describe unreleased behavior. "
            "<code>agentstack --version</code> says which build you have.</p>"
        )
    newest = tags[0]
    stable = next((t for t in tags if "-" not in t), None)

    unreleased = re.search(
        r"^## +Unreleased\s*(.*?)(?=^## +v)", text, re.M | re.S
    )
    has_unreleased = bool(unreleased and unreleased.group(1).strip())

    href = f"{GH}/releases/latest"
    installer = f'<a href="{href}">the installer</a>'
    Installer = f'<a href="{href}">The installer</a>'
    if has_unreleased:
        # Naming the newest tag only says something when it is NOT the release
        # the next sentence already names.
        ahead = (
            "the current release"
            if newest == stable
            else f"the newest tagged build (<b>{esc(newest)}</b>)"
        )
        lead = (
            "These pages describe <b>unreleased <code>main</code></b>, which "
            f"is ahead of {ahead}."
        )
    elif newest == stable:
        lead = f"These pages describe <b>{esc(newest)}</b>, the current release."
    else:
        lead = f"These pages describe <b>{esc(newest)}</b>, a pre-release."

    if stable is None:
        gets = f"There is no published release yet, so {installer} has none to give you."
    elif has_unreleased:
        gets = (
            f"{Installer} serves <b>{esc(stable)}</b>, so commands and flags "
            "documented here may not exist in your build."
        )
    elif newest == stable:
        gets = f"That is the build {installer} gives you."
    else:
        gets = (
            f"{Installer} serves the current release, "
            f"<b>{esc(stable)}</b> — <code>/releases/latest</code> never points "
            "at a pre-release — so this is not the build you get unless you ask "
            f"for it by name: <code>AGENTSTACK_VERSION={esc(newest)}</code>."
        )

    return (
        f'<p class="versionbar">{lead} {gets} '
        "<code>agentstack --version</code> says which build you have.</p>"
    )


VERSION_BANNER = version_banner()


def build_page(src_rel, out_rel, key):
    warnings = []
    md = (DOCS / src_rel).read_text()
    body, title, desc = convert(md, src_rel, out_rel, warnings)

    depth = len(Path(out_rel).parent.parts)
    base = "../" * depth
    aside = _sidebar.render(out_rel, key)
    # Sidebar hrefs are docs/-relative; reroot them for pages in subdirectories.
    if base:
        aside = re.sub(
            r'href="(?!https?:|#|\.\./)', f'href="{base}', aside
        )

    gh_src = f"{GH}/blob/main/docs/{src_rel}"
    page = f"""<!doctype html>
<!-- GENERATED by tools/make-docs-pages.py from docs/{src_rel} — edit the Markdown, not this file. -->
<html lang="en" data-palette="slate">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{esc(title)} — agentstack</title>
<meta name="description" content="{html.escape(desc, quote=True)}">
<meta name="color-scheme" content="light dark">
<link rel="icon" href="{base}favicon.svg" type="image/svg+xml">
<link rel="canonical" href="{SITE}/{out_rel}">
<script src="{base}theme/theme.js"></script>
<link rel="stylesheet" href="{base}theme/organic.css">
<style>{CSS}</style>
</head>
<body>
<a class="skip-link" href="#main-content">Skip to content</a>

<header>
  <div class="bar">
    <a class="wordmark" href="{base}./"><img class="mark" src="{base}theme/logo-mark.svg" alt=""><span>agent<span class="wm2">Stack</span></span></a>
    <nav class="top" aria-label="Project links">
      <a href="{base}docs.html">Documentation</a>
      <a href="{base}examples.html">Demos</a>
      <a href="{base}tutorial/">Tutorial</a>
      <a href="https://github.com/Tarekkharsa/agentstack">GitHub</a>
      <button class="themebtn" data-theme-toggle onclick="toggleTheme()">Light mode</button>
      <a class="ghost" href="{base}start.html">Get&nbsp;started</a>
    </nav>
  </div>
</header>

<div class="docwrap">
{aside}
  <main id="main-content" tabindex="-1">
{VERSION_BANNER}
{body}
  <p class="srcline">Source of truth: <a href="{gh_src}">docs/{src_rel}</a> — this page is generated from it.</p>
  </main>
</div>

<footer>
  <div class="bar">
    <span>MIT or Apache-2.0, at your option.</span>
  </div>
</footer>

</body>
</html>
"""
    out_path = DOCS / out_rel
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(page)
    return warnings


if __name__ == "__main__":
    all_warnings = []
    for src, out, key in PAGES:
        all_warnings += build_page(src, out, key)
        print(f"{out}: generated from {src}")
    for w in all_warnings:
        print(f"warning: {w}", file=sys.stderr)
    sys.exit(1 if all_warnings else 0)
