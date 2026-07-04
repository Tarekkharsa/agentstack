# Feature reference

The complete, implemented-and-tested feature inventory. The
[README](../README.md) is the tour; this is the map.

## Core engine

- **Manifest model** with layered load (preferred
  `.agentstack/agentstack.toml` + a gitignored `agentstack.local.toml` overlay;
  legacy root `agentstack.toml` remains supported) and static validation.
- **Data-driven adapters** — Claude Code, Claude Desktop, Codex, Cursor,
  Windsurf, Gemini CLI, VS Code, GitHub Copilot CLI, OpenCode, Antigravity,
  Junie, Kiro, and Pi (one YAML descriptor each, embedded in the binary; user
  overrides/additions from `~/.agentstack/adapters/`). Each CLI's quirks are
  encoded in data, not code: Claude's `type:"http"`, Codex's `http_headers`
  subtable, Windsurf's and Antigravity's `serverUrl`, Gemini's `httpUrl`, VS
  Code's `servers` key, OpenCode's combined-`command` array and `mcp` key, and
  Copilot CLI's `type:"local"` stdio tag.
  Per-OS config paths (`{config}/…`) resolve correctly on macOS/Linux/Windows.
- **Generic renderer** that applies field renames, transport tags, header
  nesting, and secret substitution — and its **inverse** (`init` imports
  existing configs back into a manifest).
- **Non-destructive mergers** — JSON splices only the managed section (untouched
  bytes, including floats, preserved exactly); TOML uses `toml_edit` to keep
  comments and formatting.
- **State tracking** (`~/.agentstack/state.json`) so `apply` prunes entries we
  own that left the manifest, and `doctor`/`diff` detect hand-edits.
- **Global vs project scope** (`--scope`): writes default to **global** (each
  CLI's `~/.claude.json`, `~/.claude/skills`); pass `--scope project` to write a
  repo's project locations (`.mcp.json`, `.claude/skills/`) so any agent opening
  the repo inherits the setup.

## Secrets and trust

- **Secret resolution** chain: process env → **varlock** → **OS keychain** →
  project `.env`. Unresolved `${REF}`s are reported, never silently blanked.
- **Unresolved secrets block writes** — if a `${REF}` doesn't resolve on this
  machine, `apply`/`use`/dashboard writes are refused for that target (never a
  `${TOKEN}` placeholder in live config); override with `--allow-unresolved`.
  Structural manifest validation errors block `--write` too.
- **Governance (`[policy]`)** — `require`/`forbid` capabilities and an
  `allowed_sources` glob allowlist (e.g. `git:github.com/acme/*`), enforced by
  `doctor --ci`. Cross-source trust gating for executable-intent skills/MCPs.
- **MCP firewall (`[policy.tools]`)** — per-server tool rules enforced at the
  runtime gateway: `github = ["get_*", "list_*", "!list_secrets"]` (plain
  globs allow, `!` denies; allow patterns make the list an allowlist). A
  denied tool is **invisible** — filtered from `tools_search` and code-mode
  bindings — and refused with the rule named if called anyway. `doctor`
  errors on rules naming unknown servers; `explain <server>` shows the
  effective policy.
- **Call audit log** — every tool call the gateway brokers (MCP proxy and
  code-mode alike) appends to `~/.agentstack/audit/calls.jsonl`: timestamp,
  run id (when launched via `agentstack run`), server, tool, **argument
  digest** (never values), outcome (`ok`/`error`/`denied`), latency.
  Summarize with `agentstack audit --calls [--since <days>] [--json]`; the
  dashboard's Runs panel shows each run's trust footprint (Calls button) and
  an all-runs view. Best-effort and size-rotated; logging can never fail a
  call.
- **Content scanning + `audit`** — every `install` scans skill content for
  hidden Unicode (zero-width characters, bidi overrides, tag characters) and
  prompt-injection heuristics. Hidden-Unicode findings **block the install**
  (override with `--allow-flagged`); injection heuristics warn. `agentstack
  audit` (`--json`) re-scans everything materialized — skills and instruction
  files — and `doctor --ci` fails on high-severity findings, so a poisoned
  skill can't slide into CI unnoticed.
- **`doctor --live`** — real MCP `initialize` handshake over HTTP; reports
  server name + tool count, or classifies the error (auth / http / connect).

## Capabilities

- **Package manager** — skills declare a source (`path` or `git`);
  `install` fetches them into `~/.agentstack/store/` and writes a SHA-256
  `agentstack.lock`; `install --locked` is reproducible (CI-safe); `update`
  re-resolves git skills; `remove` drops a capability from manifest + lock.
  Repeat digests are served from a stat-fingerprint cache
  (`~/.agentstack/digest-cache.json`: file count + size + mtime + path hash per
  dir) — any mismatch falls back to the full read+hash, so `doctor`/`use` over
  a large library cost stat calls, not a re-hash of every byte. Delete the file
  to force full re-hashing.
- **Central capability library (`agentstack lib`)** — one managed home
  (`~/.agentstack/lib/`) that projects reference **by name** instead of copying
  files. Skill dirs (`lib/skills/`) and MCP server definitions
  (`lib/servers/*.toml`) are indexed in `library.toml`; a profile's
  `skills = ["sql-review"]` / `servers = ["kibana"]` resolve from there, and an
  inline `[skills.*]` / `[servers.*]` always overrides the library. Name refs are
  pinned by digest in `agentstack.lock` (servers pin the **definition** digest
  only — secret values stay `${REF}` and resolve at render/gateway time, never in
  the library or lock), and `doctor`/`explain` flag drift and show each item's
  origin + provenance. Profile resolution is offline by default (dry-run `use`,
  `doctor`, `explain` never fetch); `use --write` fetches git-backed skills when
  activation needs them. Manage it with `lib add` / `add-server` / `list` /
  `remove` / `remove-server`;
  `consolidate` sweeps scattered skills from every CLI into the library and
  symlinks the originals back; `lib migrate` copies a legacy
  `~/.agentstack/skills/` home in, preview-first and reversible. Provider folders
  are never owned — only their skills and MCP entries are managed.
- **Selective skills** via profiles — `use <profile>` materializes only that
  profile's skills (symlink, with copy fallback), pruning the rest it owns and
  never clobbering hand-made skill dirs.
- **Instruction files** — compile shared + harness-specific fragments into each
  CLI's `CLAUDE.md` / `AGENTS.md`, inside a managed `<!-- agentstack -->` region
  that preserves all surrounding hand-written prose.
- **Native settings** — manage each CLI's own settings file (Claude Code
  `~/.claude/settings.json` permissions/feature flags, Codex `config.toml`) from
  one `[settings.<cli>]` block. `apply` merges only the keys you declare into the
  real file (top-level ownership), resolves `${REF}`s, preserves hand-set keys,
  and prunes keys that leave the manifest. Editable from the dashboard.
- **Lifecycle hooks** — declare `[hooks.*]` once (event + optional matcher +
  command) and `apply` compiles them into each harness's native hooks config
  (Claude Code `settings.json`, Codex `config.toml`), resolving secrets and
  pruning hooks that leave the manifest. Add/list from the dashboard Hooks pane.
- **`search` across providers** — the embedded catalog **and the official MCP
  Registry** (`registry.modelcontextprotocol.io`). `agentstack add from <id>`
  resolves a registry/catalog server, lifts its secrets to `${REF}`s, and (on
  `apply`) renders it to **all your CLIs at once**. agentstack is the cross-CLI
  *client* over the registry + marketplaces, not another registry.
- **Git-hosted versioned packs** — any repo with a `pack.toml` installs as a
  pack from any git host, pinned to a version tag:
  `add from git:<host>/<repo>[@<tag>][#subdir]` (no tag → newest
  version-shaped tag; a repo with no version tags is an error, never a
  floating install). The ledger records `source`/`version`/`rev` (the
  resolved commit); extracted skills are digest-pinned in the lock so
  `install --locked` reproduces. `upgrade <pack>` lists remote tags, resolves
  the newest (never downgrades), previews the member diff, and re-pins on
  `--write`. `[policy] allowed_sources` is enforced **before** any fetch, and
  the clone's content passes the install scan gate. `agentstack pack init`
  scaffolds a publishable pack; the dashboard's Discover pane installs from a
  git URL with the same gates. (Semver ranges and transitive pack
  dependencies are deliberately not in v1.)
- **`adopt`** — the reverse of `apply`: pull a hand-added server from a target
  config back into the manifest, lifting its inline secret, preserving comments.
- **`add`** — flag-driven (scriptable / agent-operable) add of a server or skill
  to the manifest, optionally into a profile; comments preserved.
- **`stats`** — local usage analytics: activation counts + per-capability
  footprint (which target/scope slots it's live in) + **context cost**.
  `stats --live` measures each server's `tools/list` token footprint through
  the gateway (HTTP + stdio) and caches it (`~/.agentstack/footprint.json`);
  `stats`, `explain`, and the dashboard's Servers matrix then show what each
  server taxes every session offline, and `stats` flags dead weight
  (high-cost, never-activated servers) with the exact `remove` command.
- **`export`/`import`** — age-encrypted bundle (manifest + lock + optionally
  secrets) for moving a setup to a new machine; passphrase-protected.

## Managed plugin recipes

Declare `[plugins.*]` once and `agentstack plugins sync --write` generates
repo-local Claude Code + Codex plugin packages and marketplaces
(`plugins/agentstack/*`, `.agents/plugins/marketplace.json`,
`.claude-plugin/marketplace.json`). Native installed plugins remain visible in
the dashboard as a separate read-only inventory; managed recipes can be
composed from existing servers, skills, and hooks in the Plugins pane.

Create or adopt recipes without hand-editing TOML:

```bash
agentstack plugins create play \
  --description "Shared play workflow" \
  --target claude-code --target codex \
  --server github --hook notify \
  --write

# Lift an installed native plugin into [plugins.*] plus any bundled MCP/skills/hooks.
agentstack plugins adopt playwright --harness claude-code --write

# Generate repo-local native plugin packages + marketplaces.
agentstack plugins sync --write

# Check generated/native marketplace state and exact next install handoff.
agentstack plugins status play

# Run the native handoff only after reviewing the dry-run command plan.
agentstack plugins install play --target codex
agentstack plugins install play --target codex --write
agentstack plugins remove play --target codex --write
```

## Dashboard

An embedded localhost server + a self-contained UI (shadcn aesthetic,
hand-written CSS — no Node, no framework, still one `cargo build`):
`agentstack dashboard` opens a cross-harness matrix with secrets, skills,
settings, profiles, and usage panels. By default it **can write to disk** — set
secrets, apply to live configs, toggle servers/skills per CLI, edit settings,
consolidate skills, install, **run doctor** (full check-up rendered in the
Health tab), and **remove** a capability from the manifest. Pass
**`--read-only`** to refuse every mutation (browse + preview diffs only) —
enforced centrally for all POST routes and pinned by a route-matrix test.
Bound to 127.0.0.1, token-gated, it never exposes secret values, and the same
unresolved-secret blocking applies to its writes. The complete UI-only
lifecycle — discover → add → secrets → enable → apply → verify → remove →
undo — is walked through in [dashboard.md](dashboard.md).

## Live runs (`agentstack run`)

Launch an agent CLI as a **tracked run** and control it without leaving
agentstack. A run is a real OS process agentstack owns: it's spawned in its own
process group (so a kill takes down the whole tree), recorded in
`~/.agentstack/runs.json`, and visible to any other agentstack process — so the
dashboard can see and stop runs it didn't start.

```bash
# Launch a harness, attached to your terminal, with a profile applied for the
# life of the run (its servers + skills are reverted automatically on exit).
agentstack run claude-code --profile design
agentstack run codex --profile backend --scope project
agentstack run claude-code --keep        # leave the profile applied after exit

# See and stop runs (from here or the dashboard's Runs panel).
agentstack runs                # table; add --json for scripting
agentstack kill <id>           # SIGTERM, then SIGKILL if it won't go
agentstack kill <id> --force   # SIGKILL immediately
```

Launching is a terminal act (the harnesses are interactive TUIs); the dashboard's
**Runs** panel is for observing and killing tracked runs. The registry is
self-healing: a run whose wrapper died is pruned on the next `runs`. A
profile-bound run uses the session engine, so one is allowed per directory at a
time. Unix only for now. Showing the full per-run trust footprint in the
dashboard is part of the portable-runtime roadmap.

## Agent-operable (`agentstack mcp`)

agentstack can run as an MCP server over stdio, so the agent itself can discover
and propose capabilities — tools: `agentstack_search`, `agentstack_list`,
`agentstack_doctor`, `agentstack_add_server`. Writes go to the **manifest only**
(commit-safe `${REF}`s, nothing executed): the agent proposes, a human reviews
and runs `apply` (the §9g/D20 trust gate). Register it once per harness:

```bash
agentstack connect claude-code codex   # dry-run: shows the config diff
agentstack connect --all --write       # every installed harness with MCP support
```

`connect` writes one small entry — `agentstack mcp --auto-project` — into the
harness's **global** MCP config (undo with `disconnect`, verify with `doctor`).
You can still register it by hand like any stdio MCP server if you prefer:

```json
{ "mcpServers": { "agentstack": { "type": "stdio", "command": "agentstack", "args": ["mcp", "--auto-project"] } } }
```

### The zero-copy bridge (`--auto-project` + `trust`)

With `--auto-project`, one global registration serves **every** repo: at session
start the gateway discovers the active project — MCP client roots → cwd walk-up →
`$AGENTSTACK_MANIFEST_DIR` — and exposes that repo's stack. Move to another repo,
open a new session, get that repo's stack. No `.mcp.json`, no rendered files; a
repo needs only its `.agentstack/agentstack.toml` (+ lock).

Auto-discovery is **trust-gated**, direnv-style. A manifest can declare stdio
servers (arbitrary local commands) and reference secrets, so a repo you just
cloned gets **control-plane tools only** — nothing spawned, nothing contacted,
no secrets resolved — until you review it and run:

```bash
agentstack trust .          # shows what the manifest runs/contacts, then pins its digest
agentstack trust --list     # every trusted project + whether its manifest still matches
agentstack trust --revoke   # withdraw
```

Trust is pinned to the manifest's content digest (including
`agentstack.local.toml`): any edit — a `git pull`, say — drops the project back
to control-plane-only until re-trusted. Explicit `--manifest-dir` skips the gate
(naming a directory is the consent), matching plain `agentstack mcp`.

The gate is visible from inside the session, not just on stderr: when the
project is untrusted (or its manifest changed since it was trusted),
`tools_search` says so and names the exact `agentstack trust <dir>` command,
and `agentstack_doctor` includes a `Trust (auto mode):` line.

Honest limits: MCP servers, secrets, the tool firewall, the call audit log, and
skills-over-MCP (`agentstack_list_loadable`/`agentstack_load`) are fully
zero-copy. Native skill folders and instruction files (`CLAUDE.md`/`AGENTS.md`)
are read from disk by the harnesses themselves and still need render mode
(`apply`/`use`) — `connect` prints this per harness.

### Compact proxied surface + code mode

`agentstack mcp` also **proxies** the project's MCP servers — HTTP and stdio
(stdio children spawn lazily in their own process group, get `${REF}`s resolved
into their env per session, and are tree-killed when the session ends). Instead of
dumping every upstream tool into `tools/list` (context bloat that grows with each
server you add), the proxied surface collapses behind two stable tools:

- **`tools_search`** — ranked discovery. `tools_search({ query })` returns compact
  cards (one line per matching upstream tool, with an entity ref); a second call
  `tools_search({ entity: "server__tool:tool" })` returns that one tool's input
  schema and a ready-to-run code-mode snippet. Deterministic substring ranking, no
  embeddings. Read-only. (Distinct from `agentstack_search`, which searches the
  *catalog* for servers to install.)
- **`tools_bindings`** — code mode via **typed bindings for harness-run code**.
  Generates a typed, **secret-free** TypeScript client
  (`codemode.<server>.<tool>(input)`) plus a runtime shim, so the agent writes
  **one** small program that calls several upstream tools and runs it with its own
  code/bash tool — one program instead of many tool round-trips.

agentstack emits the bindings and brokers the real MCP calls over a loopback,
token-gated endpoint (`${REF}`s are resolved once per gateway session, at
launch — never emitted into bindings or logs); the agent's code runs in the
**harness's** own sandbox — never inside agentstack, which stays a compiler, not a
runtime. (A full Code Mode in the [TanStack](https://tanstack.com/ai/latest/docs/code-mode/code-mode)
sense — a sandboxed `execute_typescript` executor — is reserved for a future
hosted `tools_execute`.) Materialize the client to `.agentstack/codemode/` with:

```bash
agentstack codemode            # dry-run: what would be generated
agentstack codemode --write    # write client.ts + agentstack-runtime.ts (+ .gitignore)
```

## Optimize (`agentstack optimize`)

Turns the signals agentstack already collects — activation counts, the gateway
call audit log, per-server context costs (`stats --live`), the trust ledger —
into concrete recommendations: inert servers to remove, `[policy.tools]`
allowlists to narrow high-cost servers to the tools you actually use, denied
and erroring calls to review, stale trust grants to refresh or revoke.

The contract: **every recommendation carries its evidence** (numbers, window,
data source), **the exact command or TOML** to act on it, and **why it is safe
or why it needs review**. One stated limit: the audit log only sees
gateway-brokered calls — a server rendered into a native config is called
directly by the harness, so such servers are never auto-removed on "no calls"
evidence alone.

```bash
agentstack optimize              # read-only report
agentstack optimize --json       # machine-readable
agentstack optimize --since 30   # only the last 30 days of runtime evidence
agentstack optimize --write      # apply ONLY the safe class: provably-inert
                                 # manifest entries (no calls, no activations,
                                 # no profile, not rendered anywhere, ≥14d of
                                 # history) and trust grants for deleted dirs
```

## All commands

`init`, `add`, `install` (`--locked`, `--allow-flagged`), `update`, `remove`,
`upgrade`, `bootstrap` (`--write`), `apply` (`--scope`, `--write`), `diff`,
`explain`, `use <profile>`, `session`, `instructions`, `adopt`, `consolidate`,
`lib add|add-server|list|remove|remove-server|migrate`, `restore`,
`doctor` (`--ci`, `--live`, `--fix`), `audit` (`--json`, `--calls`,
`--since`), `optimize` (`--json`, `--write`, `--since`), `search`,
`stats` (`--live`),
`secret set|get|rm|list`, `export`/`import`, `adapters`, `pack init`, `plugins`,
`dashboard`, `mcp` (`--auto-project`), `connect`/`disconnect`,
`trust` (`--list`, `--revoke`), `codemode`, `hook`, `run`/`runs`/`kill`.

## Everything shipped so far

13 adapters · `init`/`add`/`apply`/`diff`/`use`/`instructions`/`adopt` ·
package manager (`install`/`update`/`remove` + lockfile) · central capability
library (`lib` skills + servers referenced by name, digest-pinned in the lock,
drift in `doctor`/`explain`, `consolidate` into `lib/skills`) · secrets (keychain +
varlock) · scopes (global/project) · `doctor` (`--live`/`--fix`/`--ci`) ·
content scanning on install + `audit` · official MCP Registry provider +
`search`/`add from` · `[policy]` trust gate · native per-CLI settings
(`[settings.*]` → settings.json) · managed plugin recipes (`[plugins.*]` →
native Claude Code/Codex packages + marketplaces) · atomic writes + backups ·
`export`/`import` · `hook` · agent-operable `mcp` server · local dashboard
(server/skill matrices, Discover, add-skill, settings editor) · live runs
(`run`/`runs`/`kill` + dashboard Runs panel) · GitHub Action trust gate ·
nightly adapter-conformance CI · zero-copy bridge (`connect` + `mcp
--auto-project` + digest-pinned `trust`) · `optimize` (evidence-backed
recommendations from usage/audit/cost signals, safe-class `--write`).
