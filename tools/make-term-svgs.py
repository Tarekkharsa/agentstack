#!/usr/bin/env python3
"""Generate the animated terminal SVGs embedded by README.md (docs/*.svg).

GitHub renders README images through its camo proxy as plain <img>, which
blocks JS but NOT CSS animations inside an SVG — so these give the README
line-by-line terminal replays with crisp selectable-quality text at any DPI
instead of a fuzzy GIF. (The docs site's own pages simulate terminals in
plain JS; these SVGs exist for contexts that only allow an image.)

Design constraints:
- No SMIL, no JS: one CSS keyframe timeline per row, all sharing one master
  duration, looping like the GIFs they replace.
- Command rows "type" via a clipPath rect whose width animates in char steps.
  The rect's *attribute* width is the full width, so any renderer that
  ignores CSS geometry animation degrades to a static, fully-visible line.
- Every tspan is pinned with textLength so column alignment survives whatever
  monospace font the viewer has.

Transcripts are condensed from real runs (see the demo scripts referenced in
README) — keep them honest when editing.

Usage: python3 tools/make-term-svgs.py   # writes docs/*.svg
"""

import html
from pathlib import Path

# ---------------------------------------------------------------- palette --
# Matches the docs site's dark terminal (docs/index.html :root --term-*).
BG = "#0E1114"
BORDER = "rgba(255,255,255,0.09)"
HEAD_BG = "rgba(255,255,255,0.02)"
COLORS = {
    "i": "#D7DEE4",   # ink (default)
    "p": "#F0975A",   # prompt $
    "c": "#97A1AD",   # dim / comments
    "ok": "#7CC08B",
    "no": "#E5786B",
    "am": "#E0B04A",
}
DOTS = ["#E5786B", "#E0B04A", "#7CC08B"]

FS = 14            # font size
CW = 8.4           # forced char width (textLength pins it)
LH = 22            # line height
PAD = 20           # inner left/right padding
HEAD_H = 38        # title-bar height
TYPE_CPS = 0.028   # seconds per typed character
CMD_PAUSE = 0.40   # pause after a command finishes typing
OUT_STEP = 0.34    # default delay before the next row after an output row
HOLD = 3.6         # hold the finished frame before looping

MONO = "ui-monospace,SFMono-Regular,Menlo,Consolas,monospace"


def esc(s):
    return html.escape(s, quote=True)


class Row:
    def __init__(self, kind, spans, delay=None):
        self.kind = kind          # 'cmd' | 'out' | 'gap'
        self.spans = spans        # [(cls, text)]
        self.delay = delay        # override: seconds before NEXT row

    @property
    def text_len(self):
        return sum(len(t) for _, t in self.spans)


def cmd(command, comment=None):
    spans = [("p", "$ "), ("t", command)]
    if comment:
        spans.append(("c", comment))
    return Row("cmd", spans)


def out(*spans, d=None):
    return Row("out", list(spans), delay=d)


def gap():
    return Row("gap", [("i", " ")], delay=0.5)


def render(title, rows, out_path):
    max_chars = max(r.text_len for r in rows)
    width = int(PAD * 2 + max_chars * CW + 12)
    height = int(HEAD_H + PAD * 0.6 + len(rows) * LH + PAD)

    # -- timeline ------------------------------------------------------------
    t = 0.6  # small lead-in
    starts, type_durs = [], []
    for r in rows:
        starts.append(t)
        if r.kind == "cmd":
            typed = next(txt for cls, txt in r.spans if cls == "t")
            dur = len(typed) * TYPE_CPS
            type_durs.append(dur)
            t += dur + (r.delay if r.delay is not None else CMD_PAUSE)
        else:
            type_durs.append(0.0)
            t += r.delay if r.delay is not None else OUT_STEP
    total = t + HOLD

    def pct(sec):
        return round(sec / total * 100, 3)

    css = [
        f"text{{font-family:{MONO};font-size:{FS}px;white-space:pre}}",
        ".p{font-weight:700}",
        "@keyframes blink{0%,50%{opacity:1}50.01%,100%{opacity:0}}",
    ]
    body = [
        f'<rect x="0.5" y="0.5" width="{width-1}" height="{height-1}" rx="12" '
        f'fill="{BG}" stroke="{BORDER}"/>',
        f'<rect x="0.5" y="0.5" width="{width-1}" height="{HEAD_H}" rx="12" fill="{HEAD_BG}"/>',
        f'<rect x="0.5" y="{HEAD_H-10}" width="{width-1}" height="10" fill="{BG}"/>',
        f'<line x1="0.5" y1="{HEAD_H}" x2="{width-0.5}" y2="{HEAD_H}" '
        f'stroke="rgba(255,255,255,0.07)"/>',
    ]
    for i, color in enumerate(DOTS):
        body.append(f'<circle cx="{22 + i * 18}" cy="{HEAD_H // 2}" r="5.5" fill="{color}"/>')
    body.append(
        f'<text x="{width - PAD}" y="{HEAD_H // 2 + 4}" text-anchor="end" '
        f'fill="{COLORS["c"]}" style="font-size:12px;font-weight:700">{esc(title)}</text>'
    )

    defs = []
    for i, r in enumerate(rows):
        y = HEAD_H + int(PAD * 0.6) + (i + 1) * LH - 6
        on, off = pct(starts[i]), None
        # row visibility: hidden until its start, visible until the loop resets
        css.append(
            f".q{i}{{opacity:0;animation:q{i} {total}s linear infinite}}"
            f"@keyframes q{i}{{0%,{on}%{{opacity:0}}{min(on + 0.01, 99.98)}%,99.5%{{opacity:1}}100%{{opacity:0}}}}"
        )
        # spans, alignment pinned per-span
        x = float(PAD)
        tspans = []
        for cls, txt in r.spans:
            if not txt:
                continue
            w = len(txt) * CW
            fill = COLORS.get(cls, COLORS["i"])
            weight = ' class="p"' if cls == "p" else ""
            if cls == "t":
                # typed segment: clip it with an animated rect
                clip_id = f"tc{i}"
                s0, s1 = pct(starts[i]), pct(starts[i] + type_durs[i])
                n = max(1, len(txt))
                defs.append(
                    f'<clipPath id="{clip_id}"><rect x="{x}" y="{y - FS - 3}" '
                    f'width="{w:.1f}" height="{LH}" class="w{i}"/></clipPath>'
                )
                css.append(
                    f".w{i}{{animation:w{i} {total}s linear infinite}}"
                    f"@keyframes w{i}{{0%,{s0}%{{width:0;animation-timing-function:steps({n},end)}}"
                    f"{s1}%,99.5%{{width:{w:.1f}px}}100%{{width:0}}}}"
                )
                tspans.append(
                    f'<text x="{x:.1f}" y="{y}" fill="{fill}" clip-path="url(#{clip_id})" '
                    f'textLength="{w:.1f}" lengthAdjust="spacingAndGlyphs" xml:space="preserve">{esc(txt)}</text>'
                )
                # comment after a typed command only appears once typing ends
                comment_on = s1
            else:
                start_at = on
                if cls == "c" and r.kind == "cmd" and any(c == "t" for c, _ in r.spans):
                    start_at = None  # patched below via extra class
                tspans.append(
                    f'<text x="{x:.1f}" y="{y}" fill="{fill}"{weight} '
                    f'textLength="{w:.1f}" lengthAdjust="spacingAndGlyphs" xml:space="preserve" '
                    f'{"" if start_at is not None else f"class=~cc{i}~ "}>{esc(txt)}</text>'
                )
            x += w
        if r.kind == "cmd":
            s1 = pct(starts[i] + type_durs[i])
            css.append(
                f".cc{i}{{opacity:0;animation:cc{i} {total}s linear infinite}}"
                f"@keyframes cc{i}{{0%,{s1}%{{opacity:0}}{min(s1 + 0.01, 99.98)}%,99.5%{{opacity:1}}100%{{opacity:0}}}}"
            )
        body.append(f'<g class="q{i}">{"".join(tspans).replace("~", chr(34))}</g>')

    # blinking cursor parked after the last row's text, alive once it lands
    last = len(rows) - 1
    last_y = HEAD_H + int(PAD * 0.6) + (last + 1) * LH - 6
    cur_x = PAD + rows[last].text_len * CW + 4
    on_last = pct(starts[last])
    css.append(
        f".cur{{opacity:0;animation:curon {total}s linear infinite, blink 1.1s steps(2,start) infinite}}"
        f"@keyframes curon{{0%,{on_last}%{{opacity:0}}{min(on_last + 0.01, 99.98)}%,99.5%{{opacity:1}}100%{{opacity:0}}}}"
    )
    body.append(
        f'<rect class="cur" x="{cur_x:.1f}" y="{last_y - FS + 1}" width="7" height="{FS + 2}" '
        f'fill="{COLORS["ok"]}"/>'
    )

    svg = (
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" '
        f'viewBox="0 0 {width} {height}" role="img" aria-label="{esc(title)}">'
        f"<style>{''.join(css)}</style><defs>{''.join(defs)}</defs>{''.join(body)}</svg>"
    )
    out_path.write_text(svg)
    print(f"{out_path}  {width}x{height}  {len(svg)/1024:.1f} KB  loop {total:.1f}s")


# ------------------------------------------------------------------- specs --
# Condensed from real runs: demo-firstrun.sh, trust-gate-demo.sh,
# demo-lockdown.sh, demo-closed-loop.sh. Keep lines <= ~92 chars.

FIRSTRUN = (
    "init → apply → every CLI in sync",
    [
        cmd("agentstack init"),
        out(("c", "  🔍 Detected 6 CLIs: Claude Code · Codex · Copilot · Gemini · OpenCode · Pi")),
        out(("c", "  📥 Imported 1 MCP server from existing configs")),
        out(("ok", "  ✅ Wrote .agentstack/agentstack.toml"), d=0.7),
        gap(),
        cmd("agentstack apply"),
        out(("ok", "  ✓"), ("c", " manifest validates · 6 adapters installed · no missing secrets"), d=0.7),
        gap(),
        cmd("agentstack apply --write"),
        out(("c", "  Claude Code   "), ("ok", "✓ up to date"), ("c", "                # it already had the server")),
        out(("c", "  Codex CLI     + [mcp_servers.filesystem]      "), ("ok", "✓ wrote 1 server")),
        out(("c", '  Gemini CLI    + "filesystem": { … }           '), ("ok", "✓ wrote 1 server")),
        out(("c", "  …Copilot, OpenCode, Pi — the same server, each in its own syntax"), d=0.8),
        gap(),
        cmd("agentstack apply"),
        out(("ok", "  ✓"), ("c", " nothing to write — every target in sync")),
    ],
)

TRUST_GATE = (
    "clone → inert → trust → firewalled → audited",
    [
        cmd("git clone acme/the-repo && cd the-repo", "   # declares MCP servers"),
        cmd("agentstack mcp --auto-project", "              # the agent asks for tools"),
        out(("no", "  ✗ not trusted — none of its servers are spawned or even contacted"), d=0.8),
        gap(),
        cmd("agentstack trust .", "                         # you review, then consent"),
        out(("ok", "  ✓"), ("i", " trusted at sha256:c7d5858e…"), d=0.7),
        gap(),
        out(("c", "  demo__echo \"hi\"      "), ("ok", "✓ ok"), ("c", "       brokered, audited")),
        out(("c", "  demo__secret_read    "), ("no", "✗ refused"), ("c", '  [policy.tools] rule "!secret_read"'), d=0.9),
        gap(),
        cmd("agentstack lib sync", "                        # library → git, machine to machine"),
        out(("no", "  ✗ refusing to sync — 'Authorization' looks like a literal secret"), d=0.7),
        out(("ok", "  ✓"), ("c", " make it a ${REF} and it travels safely — values never leave the keychain")),
    ],
)

LOCKDOWN = (
    "no route out · denied host blocked · on the record",
    [
        cmd("cat ~/.agentstack/agentstack.toml", "   # YOUR machine firewall"),
        out(("c", "  [policy.egress]")),
        out(("c", '  "*" = ["!blocked.invalid"]'), d=0.7),
        gap(),
        cmd("agentstack run --lockdown shtest -- -c 'unset HTTPS_PROXY; curl example.com'"),
        out(("c", "  posture: "), ("am", "LOCKDOWN / ENFORCED · NO DIRECT ROUTE")),
        out(("c", "  🔒 no host route, no internet — the only peer is the egress sidecar")),
        out(("no", "  BLOCKED"), ("c", "        # ignoring the proxy reaches nothing"), d=0.9),
        gap(),
        cmd("agentstack run --lockdown shtest -- -c 'curl https://blocked.invalid/steal'"),
        out(("no", "  ✗ refused at the sidecar"), ("c", " — and recorded"), d=0.8),
        gap(),
        cmd("agentstack report run r-0859dcee73"),
        out(("c", "  Posture   "), ("am", "LOCKDOWN / ENFORCED · NO DIRECT ROUTE")),
        out(("no", "    ✗ shtest → blocked.invalid"), ("c", '   denied by rule "!blocked.invalid" (machine policy)')),
    ],
)

CLOSED_LOOP = (
    "pack @v1.0.0 → every CLI → firewalled → audited → @v1.1.0",
    [
        cmd("agentstack add from git:acme/pack@v1.0.0 --write"),
        out(("ok", "  ✓ installed pack 'acme'"), ("c", " — servers + skills, secrets stay ${REF}s")),
        cmd("agentstack apply --write"),
        out(("ok", "  ✓"), ("c", " the whole pack spreads to every CLI on this machine"), d=0.8),
        gap(),
        out(("c", "  acme__search_docs     "), ("ok", "✓ ok")),
        out(("c", "  acme__delete_index    "), ("no", "✗ refused"), ("c", '   [policy.tools] rule "!delete_*"'), d=0.8),
        cmd("agentstack report calls"),
        out(("c", "  2 calls · 2 tools · "), ("no", "1 denied by policy"), ("c", " — digests, never values"), d=0.9),
        gap(),
        cmd("agentstack lock --upgrade acme --yes --write", "   # vendor ships v1.1.0"),
        out(("ok", "  ✓ upgraded pack 'acme'"), ("c", " — previewed, re-pinned @v1.1.0")),
    ],
)

GUARD = (
    "rm -rf blocked · git reset blocked · .env denied · on the record",
    [
        cmd("agentstack guard install", "     # a pre-tool-use hook in 9 CLIs"),
        out(("ok", "  ✓"), ("c", " wired: Claude Code · Codex · Gemini · Cursor · Windsurf · +4"), d=0.8),
        gap(),
        out(("c", "  agent → "), ("i", "rm -rf /opt/acme/data")),
        out(("no", "    ✗ blocked"), ("c", "   destructive, outside the workspace"), d=0.8),
        # NOTE: '~' is reserved by render() as a quote placeholder — avoid it
        # in row text (the real demo uses HEAD~3; condensed here without it).
        out(("c", "  agent → "), ("i", "git reset --hard")),
        out(("no", "    ✗ blocked"), ("c", "   discards uncommitted work"), d=0.8),
        out(("c", "  agent → "), ("i", "cat .env")),
        out(("no", "    ✗ blocked"), ("c", "   [policy.filesystem] deny glob"), d=0.8),
        out(("c", "  agent → "), ("i", "ls -la")),
        out(("ok", "    ✓ allowed"), d=0.9),
        gap(),
        cmd("agentstack report calls"),
        out(("c", "  3 denials recorded (host-guard) — tool + outcome, never file contents")),
    ],
)

ONE_MANIFEST = (
    "one manifest → .mcp.json · config.toml · mcp.json",
    [
        cmd("cat .agentstack/agentstack.toml", "   # one committed source of truth"),
        out(("c", "  [servers.github]")),
        out(("c", '  command = "npx"  args = ["-y", "@modelcontextprotocol/server-github"]')),
        out(("c", '  env = { GITHUB_PERSONAL_ACCESS_TOKEN = "${GITHUB_TOKEN}" }'), d=0.8),
        gap(),
        cmd("agentstack apply --write"),
        out(("c", "  Claude Code   + .mcp.json                "), ("ok", "✓ wrote 1 server")),
        out(("c", "  Codex CLI     + .codex/config.toml       "), ("ok", "✓ wrote 1 server")),
        out(("c", "  Cursor        + .cursor/mcp.json         "), ("ok", "✓ wrote 1 server")),
        out(("c", "  instructions  → CLAUDE.md + AGENTS.md    "), ("ok", "✓ compiled"), d=0.9),
        gap(),
        out(("ok", "  ✓"), ("c", " same server, three native shapes — the manifest never holds the token")),
    ],
)

# The interactive `init` wizard's whole arc (P29 item 3), condensed from the
# real prompts/output in crates/cli/src/commands/{setup.rs,init.rs}: the P1 plan
# → detection/import/lift → the P2 storage choice (.env selected) → the P28 mode
# fork (static selected, with the actual fork_plan step list it prints) → a
# couple of apply writes → the P7 machine-change summary, closing on the P29
# doorway line the wizard now ends with. Every line is real output or real prompt
# text; the two selectors are shown as their dialoguer prompt renders them
# (`? … › <selected>`), with the unchosen options named on the next line. The
# doorway sentence is one printed line, wrapped at its em-dash to fit the card.
WIZARD_REPLAY = (
    "the guided init: plan · storage · mode · apply",
    [
        cmd("agentstack init"),
        out(("i", "Setup will:")),
        out(("i", "  1. detect the agent CLIs on this machine")),
        out(("i", "  2. import their existing configs")),
        out(("i", "  3. lift any inline tokens to ${REF} placeholders")),
        out(("i", "  4. write one agentstack manifest")),
        out(("c", "· Nothing is written until you confirm. Your CLIs are not touched yet."), d=0.5),
        gap(),
        out(("c", "🔍  6 CLI binaries on PATH: Claude Code · Codex · Gemini · OpenCode · Pi")),
        out(("c", "📥  Imported 1 MCP server(s) from existing configs")),
        out(("c", "🔐  Found 1 plaintext token in your live CLI configs — lifted to "), ("ok", "${GH_PAT}"), d=0.55),
        out(("i", "? Where should these token values live? › "), ("ok", "Project .env (default)")),
        out(("c", "    macOS keychain · Skip / decide later"), d=0.5),
        out(("i", "? Pick a delivery mode › "), ("ok", "static"), ("c", " — rendered configs on disk, kept out of git")),
        out(("c", "    clean-at-rest · zero-files")),
        out(("c", "  → preview · confirm · install · apply · skills · doctor"), d=0.6),
        gap(),
        out(("c", "  Claude Code  + .mcp.json     "), ("ok", "✓ wrote 1 server")),
        out(("c", "  Codex CLI    + config.toml   "), ("ok", "✓ wrote 1 server"), d=0.55),
        out(("ok", "✓"), ("i", " Setup complete."), ("c", " · undo: agentstack restore --last --write"), d=0.45),
        out(("i", "Learn the rest: https://tarekkharsa.github.io/agentstack/start.html"), d=0.08),
        out(("i", "  — or run `agentstack` anytime for your next step.")),
    ],
)

if __name__ == "__main__":
    docs = Path(__file__).resolve().parent.parent / "docs"
    # Only the SVGs still embedded somewhere are rendered: firstrun.svg
    # (design docs, examples/sandbox) and trust-gate.svg (README). The other
    # scene specs above are kept as source material but not written out —
    # the old landing/docs pages that embedded them were replaced by the
    # design-system site (docs/theme/).
    for name, (title, rows) in {
        "firstrun": FIRSTRUN,
        "trust-gate": TRUST_GATE,
    }.items():
        render(title, rows, docs / f"{name}.svg")
