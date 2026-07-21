#!/usr/bin/env python3
"""Render the source-of-truth Markdown docs pages into styled site pages.

The Markdown stays canonical — concepts.md, choose.md, reference.md,
ARCHITECTURE.md, ENFORCEMENT.md, HISTORY.md, and howto/*.md are what you edit,
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
    ("concepts.md", "concepts.html", "concepts"),
    ("choose.md", "choose.html", "choose"),
    ("reference.md", "reference.html", "reference"),
    ("ARCHITECTURE.md", "architecture.html", "how-it-works"),
    ("ENFORCEMENT.md", "enforcement.html", "matrix"),
    ("HISTORY.md", "history.html", "history"),
    ("howto/add-a-server.md", "howto/add-a-server.html", "howto-server"),
    ("howto/add-a-skill.md", "howto/add-a-skill.html", "howto-skill"),
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


def convert(md, src_rel, out_rel, warnings):
    """The page-body converter: returns (article_html, title, first_paragraph)."""
    lines = md.split("\n")
    out, seen_slugs = [], set()
    title, first_para = None, None
    i, in_ul, in_ol = 0, False, False

    def close_lists():
        nonlocal in_ul, in_ol
        if in_ul:
            out.append("</ul>")
            in_ul = False
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
            code = esc("\n".join(block))
            if lang == "mermaid":
                gh_page = f"{GH}/blob/main/docs/{src_rel}"
                out.append(
                    f'<p class="gennote">The diagram below is Mermaid source — '
                    f'<a href="{gh_page}">view it rendered on GitHub</a>.</p>'
                )
            out.append(f'<pre class="block"><code>{code}</code></pre>')
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
                out.append("<ul>")
                in_ul = True
            item = [m.group(1)]
            sub = []
            i += 1
            # Continuation lines belong to the item; "  - " lines open a
            # nested list; deeper-indented continuations belong to the last
            # nested item.
            while i < len(lines):
                nm = re.match(r"^  [-*] +(.*)$", lines[i])
                if nm:
                    sub.append([nm.group(1)])
                    i += 1
                elif re.match(r"^    \S", lines[i]) and sub:
                    sub[-1].append(lines[i].strip())
                    i += 1
                elif re.match(r"^  \S", lines[i]) and not sub:
                    item.append(lines[i].strip())
                    i += 1
                else:
                    break
            li = inline(" ".join(item), src_rel, out_rel, warnings)
            if sub:
                inner = "".join(
                    f"<li>{inline(' '.join(s), src_rel, out_rel, warnings)}</li>" for s in sub
                )
                li += f"<ul>{inner}</ul>"
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
        out.append(f"<p>{inline(text, src_rel, out_rel, warnings)}</p>")

    close_lists()
    return "\n".join(out), title or Path(src_rel).stem, first_para or ""


# ----------------------------------------------------------------- template --
# The shell matches the design-system pages (index/docs/examples/cookbook):
# theme/organic.css supplies fonts and component tokens, theme/theme.js owns
# the light/dark toggle (data-theme on <html>, dark default), and the token
# blocks below pin the slate palette in both themes. Keep these overrides in
# step with docs.html when the site design changes.
CSS = """
  :root {
    --paper: #F5EAD8; --surface: #FFFDF6; --ink: #201E1D; --muted: #6E6559;
    --accent: #C67139; --accent-ink: #FFFFFF;
    --accent-soft: color-mix(in srgb, var(--accent) 10%, transparent);
    --accent-line: color-mix(in srgb, var(--accent) 28%, transparent);
    --line: #E2D5BD; --line-soft: #EADFC9; --code-bg: #ECE0C8;
    --radius-sm: 9px;
    --mono: ui-monospace, "SF Mono", SFMono-Regular, Menlo, Consolas, monospace;
    --sans: var(--font-body, Figtree, sans-serif);
  }
  [data-theme="dark"] {
    --paper: #211E1B; --surface: #2A2622; --ink: #F5EAD8; --muted: #A89B87;
    --accent: #D98A52; --accent-ink: #211406;
    --line: #3A342E; --line-soft: #332E29; --code-bg: #332E29;
  }
  [data-palette="slate"] {
    --paper: #FBFAF7; --surface: #FFFFFF; --ink: #1B1F24; --muted: #5B6570;
    --accent: #B4540A; --accent-ink: #FFFFFF;
    --line: #E7E2D9; --line-soft: #EFEBE3; --code-bg: #F3F0EA;
  }
  [data-palette="slate"][data-theme="dark"] {
    --paper: #14171B; --surface: #1A1E23; --ink: #E9E7E2; --muted: #9AA3AC;
    --accent: #F0975A; --accent-ink: #1A1206;
    --line: #292E34; --line-soft: #23282E; --code-bg: #1E2329;
  }
  * { box-sizing: border-box; }
  html { scroll-behavior: smooth; }
  body { margin: 0; background: var(--paper); color: var(--ink); font: 16px/1.6 var(--sans); }
  a { color: var(--accent); text-decoration: none; }
  a:hover { text-decoration: underline; }
  code { font-family: var(--mono); font-size: 0.9em; background: var(--code-bg); border-radius: 5px; padding: 0.1em 0.35em; }
  header { position: sticky; top: 0; z-index: 50; background: color-mix(in srgb, var(--surface) 96%, transparent); -webkit-backdrop-filter: blur(12px); backdrop-filter: blur(12px); border-bottom: 1px solid var(--line); }
  .bar { max-width: 74rem; margin: 0 auto; padding: 0.7rem 1.35rem; display: flex; align-items: center; gap: 1.1rem; }
  .wordmark { font-family: var(--font-heading, Caprasimo, serif); font-weight: 400; font-size: 1.15rem; color: var(--ink); display: inline-flex; align-items: center; gap: 0.6rem; }
  .wordmark:hover { text-decoration: none; }
  .wordmark .mark { height: 28px; width: auto; display: block; }
  .wordmark .wm2 { color: var(--accent); }
  nav.top { margin-left: auto; display: flex; align-items: center; gap: 1.05rem; flex-wrap: nowrap; white-space: nowrap; }
  nav.top a { font-weight: 600; font-size: 0.85rem; color: var(--ink); }
  nav.top a:hover { color: var(--accent); text-decoration: none; }
  nav.top .themebtn { font-family: inherit; font-size: 0.8rem; font-weight: 600; padding: 0.4rem 0.85rem; border-radius: 999px; border: 1px solid var(--line); background: var(--surface); color: var(--ink); cursor: pointer; }
  nav.top .themebtn:hover { border-color: var(--accent); }
  nav.top .ghost { padding: 0.4rem 0.95rem; border: 1px solid var(--accent); border-radius: 999px; color: var(--accent-ink); background: var(--accent); }
  nav.top .ghost:hover { filter: brightness(1.08); }
  .docwrap { max-width: 78rem; margin: 0 auto; padding: 0 1.35rem; display: grid; grid-template-columns: 15rem minmax(0, 1fr); gap: 2.75rem; align-items: start; }
  aside.side { position: sticky; top: 4.2rem; max-height: calc(100vh - 5rem); overflow-y: auto; padding: 1.6rem 0 2rem; font-size: 0.85rem; }
  aside.side .grp { margin-bottom: 1.15rem; }
  aside.side .grp > b { display: block; font-family: var(--mono); font-size: 0.66rem; font-weight: 600; letter-spacing: 0.13em; text-transform: uppercase; color: var(--muted); margin-bottom: 0.35rem; }
  aside.side details.grp > summary { display: block; font-family: var(--mono); font-size: 0.66rem; font-weight: 600; letter-spacing: 0.13em; text-transform: uppercase; color: var(--muted); margin-bottom: 0.35rem; cursor: pointer; list-style: none; }
  aside.side details.grp > summary::-webkit-details-marker { display: none; }
  aside.side details.grp > summary::after { content: "\\25B8"; float: right; font-size: 0.85em; opacity: 0.55; font-weight: 400; }
  aside.side details.grp[open] > summary::after { content: "\\25BE"; }
  aside.side .grp > code { display: block; font-family: var(--mono); font-size: 0.66rem; color: var(--accent); background: none; border: none; padding: 0; margin: -0.25rem 0 0.35rem; }
  aside.side ul { list-style: none; margin: 0; padding: 0; }
  aside.side li a { display: flex; gap: 0.5rem; align-items: baseline; padding: 0.14rem 0.5rem; border-left: 2px solid transparent; border-radius: 0 5px 5px 0; font-family: var(--mono); font-size: 0.76rem; color: var(--muted); }
  aside.side li a:hover { color: var(--accent); text-decoration: none; }
  aside.side li a.on-page { color: var(--ink); font-weight: 600; border-left-color: var(--accent); background: var(--accent-soft); }
  @media (max-width: 960px) { .docwrap { display: block; } aside.side { display: none; } .bar { flex-wrap: wrap; } nav.top { flex-wrap: wrap; white-space: normal; } }
  main { min-width: 0; padding: 1.6rem 0 4rem; }
  main h1 { font-family: var(--font-heading, Caprasimo, serif); font-weight: 400; font-size: 2.1rem; line-height: 1.15; letter-spacing: normal; margin: 0 0 0.6rem; }
  main h2 { font-family: var(--font-heading, Caprasimo, serif); font-weight: 400; font-size: 1.45rem; margin: 2.2rem 0 0.5rem; padding-top: 1.4rem; border-top: 1px solid var(--line-soft); }
  main h3 { font-family: var(--mono); font-size: 0.98rem; margin: 1.5rem 0 0.4rem; }
  main h4 { font-family: var(--mono); font-size: 0.9rem; margin: 1.2rem 0 0.3rem; }
  .hlink { margin-left: 0.45rem; opacity: 0; font-size: 0.85em; }
  h2:hover .hlink, h3:hover .hlink, h4:hover .hlink { opacity: 0.7; }
  main p, main li { max-width: 46rem; }
  main ul, main ol { padding-left: 1.4rem; }
  pre.block { background: var(--code-bg); border: 1px solid var(--line-soft); border-radius: var(--radius-sm); padding: 0.85rem 1rem; overflow-x: auto; font-size: 0.82rem; line-height: 1.55; }
  pre.block code { background: none; padding: 0; font-size: 1em; }
  .tblwrap { overflow-x: auto; margin: 0.8rem 0; }
  table { border-collapse: collapse; font-size: 0.88rem; min-width: 60%; }
  th, td { text-align: left; padding: 0.45rem 0.8rem; border-bottom: 1px solid var(--line-soft); vertical-align: top; }
  th { font-family: var(--mono); font-size: 0.68rem; font-weight: 600; letter-spacing: 0.1em; text-transform: uppercase; color: var(--muted); }
  blockquote { margin: 1rem 0; padding: 0.2rem 1rem; border-left: 3px solid var(--accent-line); color: var(--muted); }
  main img { max-width: 100%; height: auto; }
  .gennote { font-size: 0.82rem; color: var(--muted); margin-bottom: 0.3rem; }
  .srcline { margin-top: 2.6rem; padding-top: 1rem; border-top: 1px solid var(--line-soft); font-family: var(--mono); font-size: 0.74rem; color: var(--muted); }
  footer { border-top: 1px solid var(--line); margin-top: 3rem; }
  footer .bar { font-size: 0.85rem; color: var(--muted); }
"""


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

<header>
  <div class="bar">
    <a class="wordmark" href="{base}./"><img class="mark" src="{base}theme/logo-mark.svg" alt=""><span>agent<span class="wm2">Stack</span></span></a>
    <nav class="top" aria-label="Project links">
      <a href="{base}docs.html">Documentation</a>
      <a href="{base}cookbook.html">Cookbook</a>
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
  <main>
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
