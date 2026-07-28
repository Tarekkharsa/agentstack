# First-value demo — import once, use it across every coding CLI

The fenced, reproducible proof of AgentStack's core promise (TODO §1.5):

1. **Start** from two real native configs — Claude Code (`~/.claude.json`)
   knows a `github` MCP server with an inline token; Codex
   (`~/.codex/config.toml`) knows a `tldraw` server. Neither knows the other's.
2. **Import** with `agentstack init --yes --secrets env`: one manifest, the
   token lifted to `${GITHUB_TOKEN}` with its value in a gitignored `.env`.
3. **Render** with `agentstack apply --scope global --write`: both native
   configs now carry both servers, each in its own format.
4. **Verify** with `agentstack doctor`: 0 errors, 0 warnings.
5. **Undo** with `agentstack restore --last --write` (twice): every file is
   byte-identical to where it started.

## Run it

```sh
./run-demo.sh
```

Self-contained: an isolated temp `HOME` and `AGENTSTACK_HOME`, stub `claude`/
`codex` binaries on a controlled `PATH`, nothing touches your real
configuration, and the sandbox is deleted on exit. Every step is asserted —
the script exits nonzero on any mismatch, so it is also a CI-runnable witness
that the journey's expected output stays accurate against the current binary.

## Record it

vhs stalls on this machine; use asciinema:

```sh
DEMO_PAUSE=2.5 asciinema rec first-value.cast --window-size 108x30 -c ./run-demo.sh
```

`DEMO_PAUSE` paces the narration lines for a watchable recording; the default
(0.6s) is for humans running it live, and `DEMO_PAUSE=0` for CI. The fixed
108×30 window keeps the longest output lines unwrapped (the sandbox already
pins its temp dir to a short `/tmp` path for the same reason).

The full recording is kept at `docs/demos/first-value.cast`. What the README,
the website landing page, the getting-started guide and the demos page embed
is `docs/demos/first-value.svg` — an animated terminal SVG condensed from that
cast, and the `FIRST_VALUE` scene in `tools/make-term-svgs.py` is its source.
It replaced a 664 KB GIF: 11 KB, crisp at any DPI, and it animates inside
GitHub's image proxy (which blocks JS but not CSS). After any change to this
script's output, re-record the cast, update the scene, and re-run:

```sh
python3 tools/make-term-svgs.py
```
