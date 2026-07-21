#!/usr/bin/env python3
"""Generate the unified docs sidebar into every docs-experience page.

One tree, defined once below, spliced into docs.html, start.html, and
examples.html between `<!-- sidebar:begin -->` / `<!-- sidebar:end -->`
markers. (how-it-works, primitives, library, and strategy were folded into the
Markdown source of truth; their old URLs are redirect stubs. The Markdown docs
now render as site pages via make-docs-pages.py, so the tree links those
locally — only repo-root and non-doc files still link out to GitHub.) Each
page gets the
IDENTICAL tree; the only per-page differences are (a) the current entry is
highlighted and (b) the current page's own sections expand inline beneath its
entry (the nub-docs pattern). Links that point at the page being generated
collapse to plain `#anchor`s so in-page navigation doesn't reload.

The tree is split into two tiers: "open" groups render inline for the everyday
path, while "deeper" groups render as collapsed <details> (auto-opened when
they hold the current page) so power-user surface stays one click away.

Usage: python3 tools/make-docs-sidebar.py   # rewrites docs/*.html

make-docs-pages.py imports this TREE live to build the generated Markdown
pages' sidebars — after editing the TREE, run that script too.
"""

import html
import re
from pathlib import Path

GH = "https://github.com/Tarekkharsa/agentstack"

# ---------------------------------------------------------------- the tree --
# (group, command-or-None, [(label, href, key)], tier)
# tier is "open" (tier-one — always visible, the everyday path) or "deeper"
# (rendered collapsed as <details>, the power-user surface). Open groups come
# first; the "deeper" groups follow, one click away.
TREE = [
    ("Start", None, [
        ("Get started", "start.html", "start"),
        ("Install", "index.html#install", "install"),
        ("Concepts", "concepts.html", "concepts"),
        ("Which mode do I need?", "choose.html", "choose"),
        ("Examples", "examples.html", "examples"),
    ], "open"),
    ("Everyday", "$ agentstack add · apply · doctor", [
        ("The manifest", "concepts.html#the-manifest-and-the-lockfile", "manifest"),
        ("Add a server", "howto/add-a-server.html", "howto-server"),
        ("Add a skill", "howto/add-a-skill.html", "howto-skill"),
        ("Undo anything", "howto/undo.html", "howto-undo"),
        ("See what happened", "howto/see-what-happened.html", "howto-audit"),
        ("Trust a cloned repo", "howto/trust-a-repo.html", "howto-trust"),
    ], "open"),
    ("Reference", None, [
        ("Every command", "reference.html", "reference"),
        ("Agent manual (skill)", f"{GH}/blob/main/crates/cli/catalog/skills/using-agentstack/SKILL.md", "manual"),
    ], "open"),
    ("Configure deeper", "$ agentstack use · session", [
        ("How it works", "architecture.html", "how-it-works"),
        ("Central library", "reference.html#the-central-library", "library"),
        ("Delivery modes", "concepts.html#delivery-modes", "modes"),
        ("Dashboard", "reference.html#dashboard", "dashboard"),
    ], "deeper"),
    ("Protect", "$ agentstack trust · guard", [
        ("What trust does & doesn't", "enforcement.html#what-trusted-does-and-does-not-mean", "trustlimits"),
        ("Guard demo", f"{GH}/tree/main/examples/guard-demo", "guard"),
        ("Policy presets", f"{GH}/tree/main/examples/policies", "presets"),
    ], "deeper"),
    ("Run confined", "$ agentstack run --lockdown", [
        ("Lock down a run", "howto/lock-down-a-run.html", "howto-lockdown"),
        ("Enforcement matrix", "enforcement.html", "matrix"),
    ], "deeper"),
    ("Team & CI", None, [
        ("Team setup", "howto/team-setup.html", "howto-team"),
        ("Use in CI", "howto/ci.html", "howto-ci"),
    ], "deeper"),
    ("Observe deeper", "$ agentstack report", [
        ("Reports & call audit", "examples.html#e20", "reports"),
        ("Wire-cost analysis", "examples.html#e21", "wirecost"),
    ], "deeper"),
    ("Project", None, [
        ("Security review", "security-review-2026-07-11.html", "secreview"),
        ("Strategy", f"{GH}/blob/main/STRATEGY.md", "strategy"),
        ("History", "history.html", "history"),
    ], "deeper"),
]

# ------------------------------------------- per-page inline expansions -----
# key -> [(num-or-None, label, anchor)]; None entries render as sub-headers.
EXPANSIONS = {
    "start": [
        ("1", "Install the binary", "#s-install"),
        (None, "Track A — unify your setup", None),
        ("A1", "Import what's there", "#s-import"),
        ("A2", "Render into every CLI", "#s-render"),
        ("A3", "Wire the guardrails", "#s-guard"),
        ("A4", "Verify the loop", "#s-verify"),
        (None, "Track B — govern a repo", None),
        ("B1", "Register the gateway", "#s-connect"),
        ("B2", "Clone — it stays inert", "#s-clone"),
        ("B3", "Review, then trust", "#s-trust"),
        ("B4", "Secrets in the keychain", "#s-secret"),
        ("B5", "Run it confined", "#s-run"),
        ("B6", "Read the flight recorder", "#s-report"),
    ],
    "examples": [
        (None, "Configure", None),
        ("1", "The smallest manifest", "#e1"),
        ("2", "Fan out to several CLIs", "#e2"),
        ("3", "Secrets out of the file", "#e3"),
        ("4", "HTTP server with auth", "#e4"),
        ("5", "A native key for one CLI", "#e5"),
        ("6", "Add skills", "#e6"),
        ("7", "Task-specific profiles", "#e7"),
        ("8", "Share house rules across CLIs", "#e8"),
        ("26", "Native extensions", "#e26"),
        ("9", "The everyday loop", "#e9"),
        ("14", "The central library", "#e14"),
        ("15", "Sync across machines", "#e15"),
        ("16", "Versioned vendor packs", "#e16"),
        ("24", "Add a CLI in one file", "#e24"),
        ("25", "The personal layer", "#e25"),
        (None, "Protect", None),
        ("10", "The MCP tool firewall", "#e10"),
        ("11", "The machine layer", "#e11"),
        ("12", "Governance", "#e12"),
        ("13", "The trust gate", "#e13"),
        ("19", "Policy dimensions", "#e19"),
        ("23", "The CI trust gate", "#e23"),
        (None, "Run confined", None),
        ("18", "Sandboxed runs", "#e18"),
        ("22", "Governed TypeScript", "#e22"),
        (None, "Observe", None),
        ("20", "Audit, analyze, report", "#e20"),
        ("21", "What your tools cost", "#e21"),
    ],
}

# page file -> current tree key (docs.html is the hub: tree, nothing current).
# Only the retained docs-experience pages are generated; the folded pages
# (primitives/how-it-works/library/strategy) are now redirect stubs.
PAGES = {
    "docs.html": None,
    "start.html": "start",
    "examples.html": "examples",
}

BEGIN, END = "<!-- sidebar:begin (generated — edit tools/make-docs-sidebar.py) -->", "<!-- sidebar:end -->"


def esc(s):
    return html.escape(s, quote=False)


def render(page_file, current):
    out = [f'<aside class="side" aria-label="Documentation">']
    for group, cmd, entries, tier in TREE:
        # Tier-one groups render inline as a <div>; "deeper" groups collapse
        # into a <details> that auto-opens when it holds the current page, so
        # the reader always sees where they are without expanding by hand.
        collapsed = tier == "deeper"
        if collapsed:
            open_attr = ' open' if any(key == current for _, _, key in entries) else ''
            out.append(f'  <details class="grp"{open_attr}>')
            out.append(f'    <summary>{esc(group)}</summary>')
        else:
            out.append('  <div class="grp">')
            out.append(f'    <b>{esc(group)}</b>')
        if cmd:
            out.append(f'    <code>{esc(cmd)}</code>')
        out.append('    <ul>')
        for label, href, key in entries:
            # Self-links collapse to in-page anchors (or a dead-center '#').
            h = href
            base = href.split('#')[0]
            if base == page_file:
                h = '#' + href.split('#')[1] if '#' in href else href
            on = ' class="on-page"' if key == current else ''
            out.append(f'      <li><a{on} href="{h}">{esc(label)}</a>')
            if key == current and key in EXPANSIONS:
                out.append('        <ul class="sub">')
                for num, sublabel, anchor in EXPANSIONS[key]:
                    if anchor is None:
                        out.append(f'          <li class="subhead">{esc(sublabel)}</li>')
                    else:
                        out.append(
                            f'          <li><a href="{anchor}"><span class="n">{num}</span>{esc(sublabel)}</a></li>')
                out.append('        </ul>')
            out.append('      </li>')
        out.append('    </ul>')
        out.append('  </details>' if collapsed else '  </div>')
    out.append('</aside>')
    return '\n'.join(out)


def splice(path, aside):
    s = path.read_text()
    block = f'{BEGIN}\n{aside}\n{END}'
    if BEGIN in s:
        s = re.sub(re.escape(BEGIN) + r'[\s\S]*?' + re.escape(END), block, s, count=1)
    else:
        # First run: replace the existing hand-written aside.
        s, n = re.subn(r'<aside class="side"[\s\S]*?</aside>', block, s, count=1)
        if n != 1:
            raise SystemExit(f'{path}: no aside found to replace')
    path.write_text(s)


if __name__ == '__main__':
    docs = Path(__file__).resolve().parent.parent / 'docs'
    for page, current in PAGES.items():
        splice(docs / page, render(page, current))
        print(f'{page}: sidebar generated (current={current})')
