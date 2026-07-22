# F2 spike — per-child MCP config injection, raw-binary evidence

Date: 2026-07-22 · claude 2.1.216 · codex-cli 0.144.6 · macOS.
Method: probe MCP servers are `/bin/sh -c "touch <marker>; exec cat"` — the marker
file is filesystem evidence the harness spawned that server, independent of any
model output. Scratch project `f2/proj` has `.mcp.json` with `projProbe` +
`.claude/settings.local.json` `{"enableAllProjectMcpServers": true}` so the
project server is approved and WOULD load by default.

## (a) claude: --mcp-config + --strict-mcp-config

| run | flags | markers spawned | .mcp.json sha256 |
|---|---|---|---|
| baseline | none | spawned-proj | unchanged (9308…ef6c) |
| strict | `--mcp-config launch-A.json --strict-mcp-config` | spawned-A ONLY | unchanged |
| merge control | `--mcp-config launch-A.json` (no strict) | spawned-A + spawned-proj | unchanged |

- Baseline proves `-p` mode does spawn approved project `.mcp.json` servers (8.3 s run).
- Strict run: launch-scoped file honored, project config NOT loaded, project file
  untouched byte-for-byte. ~4.9 s.
- Merge control proves `--strict-mcp-config` is load-bearing: without it the project
  config merges in.
- NOTE: `--strict-mcp-config` also excludes USER-scope servers — stronger isolation
  than the shipped park/swap, which only scopes project scope and lets user-scope
  servers load into locked children.

## (b) codex: -c overrides

User `~/.codex/config.toml` has 8+ real `mcp_servers` entries (miro, chrome-devtools,
node_repl w/ 120 s startup timeout, agentstack, tldraw, kibana_mcp, figma, gha-search).

Run: `codex exec --skip-git-repo-check -c "mcp_servers={probeC = {command=\"/bin/sh\",
args=[…touch marker…], startup_timeout_sec=3}}" "Reply with exactly: ok"` (stdin /dev/null)

- spawned-C marker created; reply `ok`; 12.2 s wall (no 20 s/120 s startup stalls).
- **CORRECTION (2026-07-22, W2.5 review — supersedes this section's original
  conclusion):** this spike originally concluded the whole-table
  `-c 'mcp_servers={…}'` REPLACES the user server table. That was WRONG — it
  **MERGES**. A later live decoy-marker test (a `touch <marker>` server added to
  `~/.codex/config.toml`, then `codex exec -c 'mcp_servers={probe}'`) showed the
  decoy STILL SPAWNS under `-c` alone; the original run's "zero connections in
  12.2 s" was a slow-user-server-startup artifact (the user servers hadn't
  connected yet when the short session ended), not replacement. The correct
  strict scope is **`codex exec --ignore-user-config -c 'mcp_servers={probe}'`**
  — live-witnessed to exclude the decoy (marker ABSENT) while loading only the
  injected entry. `--ignore-user-config` is codex's equivalent of claude's
  `--strict-mcp-config`; it also drops the user's model/approval config (auth
  still resolves via `CODEX_HOME`) — acceptable for a governed run. Per-child
  injection uses `["--ignore-user-config", "-c", "{mcp_servers_toml}"]`.
- `~/.codex/config.toml` sha256 unchanged before/after (b945…b593).
- One stderr line `rmcp … Auth(AuthorizationRequired)` — plugin-layer noise
  in the original run.

## (c) concurrency, same project dir, different server sets

claude: two simultaneous `claude -p … --mcp-config launch-{A,B}.json --strict-mcp-config`
in the SAME project cwd:

```
A start 1784691086.217  A end 1784691090.857  exit=0  reply alpha  spawned-A
B start 1784691086.220  B end 1784691090.737  exit=0  reply beta   spawned-B
```

Fully overlapping (~4.6 s each, ~4.6 s total wall — true parallelism), each child saw
only its own server set, `.mcp.json` hash unchanged.

codex: two simultaneous `codex exec -c "mcp_servers={probeD|probeE…}"`:

```
C1 start 1784691311  end 1784691327  exit=0  reply delta    spawned-D
C2 start 1784691311  end 1784691323  exit=0  reply epsilon  spawned-E
```

Both children ran concurrently, each spawned only its own probe, no shared config file
touched.

## Caveat

These are raw-binary mechanism proofs. The shipped `run --locked` still park/swaps the
project config and serializes (by design, per W2); the design change is to make headless
children use this injection mechanism instead — see workflows-capability.md §12.

Artifacts: launch configs, marker dirs, per-run stdout/stderr and timing files in this
directory; codex session rollouts under ~/.codex/sessions/2026/07/22/.
