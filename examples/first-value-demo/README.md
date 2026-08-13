# First-value demo — import once, use it across every coding CLI

The fenced, reproducible proof of AgentStack's core promise (TODO §1.5):

1. **Start** from two real native configs — Claude Code (`~/.claude.json`)
   knows a `github` MCP server with an inline token; Codex
   (`~/.codex/config.toml`) knows a `tldraw` server. Neither knows the other's.
2. **Import** with `agentstack init --yes --secrets env`: the server
   definitions land in the library, the manifest references them by name, the
   inline token becomes `${GITHUB_TOKEN}` in the library definition, and its
   value lands in a gitignored `.env`. The token is asserted in all three
   places at once — no secret material in the manifest, the placeholder (never
   the value) in the library entry, the value only in the ignored `.env`.
2b. **Review** with `agentstack trust .`: nothing a manifest declares is active
   until a human has read it, and a scripted `--yes` acknowledges the write
   rather than the servers — so the import leaves the project untrusted and the
   consent is its own step (headlessly: preview the surface, hand its
   `surface_digest` back). At a terminal the wizard asks this inside `init`.
3. **Connect** with `agentstack x gateway connect --all --write`: the live lane
   needs one bridge registered per MCP-capable tool. Before it runs, `delivery`
   and `doctor` say "planned live (not connected)" — the demo resolves that
   complaint rather than asserting less.
4. **Route** with `agentstack delivery`: both tools are MCP-capable, so the
   servers are served live and nothing is written for them. The project is
   asserted clean on the tree itself — it holds `.agentstack/` and the
   `.gitignore` that hides the lifted secret, and no native config at all.
5. **Verify** with `agentstack doctor`: 0 errors, 0 warnings.
6. **Render anyway** with `agentstack x delivery render-locally --write`, then
   `agentstack apply --toolset default --scope global --write` — the lane for
   when you want the files. Under the default routing, asking for files is an
   explicit opt-in; the rendered lane is routed, not removed. Both native
   configs then carry both servers, each in its own format. The toolset has to
   be named because the definitions live in the library, not the manifest's
   `[servers]`.
7. **Undo** with `agentstack restore --last --write` (four times: the render,
   the render-locally override, the bridge, the import): every file is
   byte-identical to where it started, and the library entries the import
   created are gone with it.

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
