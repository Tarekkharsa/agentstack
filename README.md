# agentstack

> **Dotfiles for your AI agents.** One portable, version-controlled setup —
> MCP servers, skills, and instructions — that follows you across every coding
> agent, machine, and teammate.

You configure MCP servers, skills, and house rules once in a commit-safe
`agentstack.toml`. agentstack **compiles** that into each agent CLI's native
config — Claude Code, Codex, Cursor, Windsurf, Gemini CLI, VS Code — resolving
secrets per machine, so the *same* setup works everywhere without copy-pasting
JSON or leaking tokens into git.

```sh
agentstack init      # reverse-engineer a manifest from configs you already have
agentstack bootstrap # guided preflight: skills, secrets, diff, next action
agentstack apply     # render it to every CLI you have installed
agentstack doctor    # prove everything is wired (secrets, drift, connectivity)
```

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/tarekkh/agentstack/main/install.sh | sh
# or: brew install tarekkh/tap/agentstack   ·   cargo install agentstack
```

Single static binary, zero runtime dependencies. (Releases are wired up in CI —
see [RELEASING.md](RELEASING.md).)

## Why it exists

Managing AI-agent setup today is three tangled pains:

1. **Format fragmentation** — the *same* MCP server is spelled differently per
   CLI (Codex TOML `[mcp_servers.x]`, Cursor `url`, Windsurf `serverUrl`, Gemini
   `httpUrl`, VS Code's `servers` key, Claude's `type:"http"`).
2. **Reproducibility & drift** — a new laptop, a teammate, a fresh devcontainer:
   everyone re-does setup by hand, and configs drift apart.
3. **Secrets** — real tokens differ per machine and must never land in git.

The durable value isn't the format translation (the ecosystem is slowly
converging on `mcp.json`) — it's the **layers that survive convergence**:
**secrets-by-reference, profiles/selective-loading, reproducible lockfile,
cross-source discovery (the official MCP Registry), and a trust/governance gate**
— in one auditable binary, across every CLI.

## What works today

- **6 agent CLIs** — Claude Code, Codex, Cursor, Windsurf, Gemini CLI, VS Code
  (each a data descriptor; add more without touching core code).
- **Cross-source discovery** — `search` and `add from` pull from the embedded
  catalog **and the official MCP Registry**, rendering to every CLI at once.
- **Crash-safe** — config writes are atomic with pre-write backups; never
  corrupts your real `~/.claude.json`.
- **Trust gate** — `[policy]` (require/forbid/`allowed_sources`) enforced by
  `doctor --ci`, plus provenance hints at the point of choosing.
- **Live runs** — launch any harness as a tracked process (`agentstack run`),
  optionally with a profile applied just for its lifetime; see and kill every run
  (and its whole process tree) from the CLI or dashboard — no Activity Monitor.
- Full CLI + an optional local **dashboard** (below).

The dashboard is an embedded localhost server + a self-contained UI (shadcn
aesthetic, hand-written CSS — no Node, no framework, still one `cargo build`):
`agentstack dashboard` opens a cross-harness matrix with secrets, skills,
settings, profiles, and usage panels. By default it **can write to disk** — set
secrets, apply to live configs, toggle servers/skills per CLI, edit settings,
consolidate skills, and install. Pass **`--read-only`** to refuse every mutation
(browse + preview diffs only). Bound to 127.0.0.1, token-gated, it never exposes
secret values, and the same unresolved-secret blocking applies to its writes.

Implemented and tested:

- **Manifest model** with layered load (`agentstack.toml` + a gitignored
  `agentstack.local.toml` overlay) and static validation.
- **Data-driven adapters** — Claude Code, Codex, **Cursor, Windsurf, Gemini CLI,
  VS Code** (one YAML descriptor each, embedded in the binary; user
  overrides/additions from `~/.agentstack/adapters/`). Each CLI's quirks are
  encoded in data, not code: Claude's `type:"http"`, Codex's `http_headers`
  subtable, Windsurf's `serverUrl`, Gemini's `httpUrl`, VS Code's `servers` key.
  Per-OS config paths (`{config}/…`) resolve correctly on macOS/Linux/Windows.
- **Generic renderer** that applies field renames, transport tags, header
  nesting, and secret substitution — and its **inverse** (`init` imports
  existing configs back into a manifest).
- **Non-destructive mergers** — JSON splices only the managed section (untouched
  bytes, including floats, preserved exactly); TOML uses `toml_edit` to keep
  comments and formatting.
- **Secret resolution** chain: process env → **varlock** → **OS keychain** →
  project `.env`. Unresolved `${REF}`s are reported, never silently blanked.
- **State tracking** (`~/.agentstack/state.json`) so `apply` prunes entries we
  own that left the manifest, and `doctor`/`diff` detect hand-edits.
- **Governance (`[policy]`)** — `require`/`forbid` capabilities and an
  `allowed_sources` glob allowlist (e.g. `git:github.com/acme/*`), enforced by
  `doctor --ci`. Cross-source trust gating for executable-intent skills/MCPs.
- **Global vs project scope** (`--scope`): writes default to **global** (each
  CLI's `~/.claude.json`, `~/.claude/skills`); pass `--scope project` to write a
  repo's project locations (`.mcp.json`, `.claude/skills/`) so any agent opening
  the repo inherits the setup.
- **Unresolved secrets block writes** — if a `${REF}` doesn't resolve on this
  machine, `apply`/`use`/dashboard writes are refused for that target (never a
  `${TOKEN}` placeholder in live config); override with `--allow-unresolved`.
  Structural manifest validation errors block `--write` too.
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
- **Managed plugin recipes** — declare `[plugins.*]` once and `agentstack
  plugins sync --write` generates repo-local Claude Code + Codex plugin packages
  and marketplaces (`plugins/agentstack/*`, `.agents/plugins/marketplace.json`,
  `.claude-plugin/marketplace.json`). Native installed plugins remain visible in
  the dashboard as a separate read-only inventory; managed recipes can be
  composed from existing servers, skills, and hooks in the Plugins pane.
- **`adopt`** — the reverse of `apply`: pull a hand-added server from a target
  config back into the manifest, lifting its inline secret, preserving comments.
- **`add`** — flag-driven (scriptable / agent-operable) add of a server or skill
  to the manifest, optionally into a profile; comments preserved.
- **`doctor --live`** — real MCP `initialize` handshake over HTTP; reports
  server name + tool count, or classifies the error (auth / http / connect).
- **Package manager** — skills declare a source (`path` or `git`);
  `install` fetches them into `~/.agentstack/store/` and writes a SHA-256
  `agentstack.lock`; `install --locked` is reproducible (CI-safe); `update`
  re-resolves git skills; `remove` drops a capability from manifest + lock.
- **`search` across providers** — the embedded catalog **and the official MCP
  Registry** (`registry.modelcontextprotocol.io`). `agentstack add from <id>`
  resolves a registry/catalog server, lifts its secrets to `${REF}`s, and (on
  `apply`) renders it to **all your CLIs at once**. agentstack is the cross-CLI
  *client* over the registry + marketplaces, not another registry.
- **`stats`** — local usage analytics: activation counts + per-capability
  footprint (which target/scope slots it's live in).
- **`export`/`import`** — age-encrypted bundle (manifest + lock + optionally
  secrets) for moving a setup to a new machine; passphrase-protected.
- Commands: `init`, `add`, `install` (`--locked`), `update`, `remove`,
  `bootstrap` (`--write`), `apply` (`--scope`, `--write`), `diff`,
  `use <profile>`, `instructions`, `adopt`, `restore`, `doctor` (`--ci`,
  `--live`, `--fix`), `search`, `stats`, `secret set|get|rm|list`,
  `export`/`import`, `adapters`, `plugins`, `dashboard`, `mcp`, `hook`,
  `run`/`runs`/`kill`.

### Live runs (`agentstack run`)

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
**Runs** panel is for observing and killing — each row shows the run's *trust
footprint* (the exact servers + skills that live process can reach). The registry
is self-healing: a run whose wrapper died is pruned on the next `runs`. A
profile-bound run uses the session engine, so one is allowed per directory at a
time. Unix only for now.

### Agent-operable (`agentstack mcp`)

agentstack can run as an MCP server over stdio, so the agent itself can discover
and propose capabilities — tools: `agentstack_search`, `agentstack_list`,
`agentstack_doctor`, `agentstack_add_server`. Writes go to the **manifest only**
(commit-safe `${REF}`s, nothing executed): the agent proposes, a human reviews
and runs `apply` (the §9g/D20 trust gate). Register it like any stdio MCP server,
e.g. Claude Code:

```json
{ "mcpServers": { "agentstack": { "type": "stdio", "command": "agentstack", "args": ["mcp"] } } }
```

### Per-directory auto-activation (`agentstack hook`)

direnv-style: drop a `.agentstack` file (first line = profile name) in a repo,
add the hook to your shell rc, and entering the repo activates that profile at
project scope across your CLIs:

```bash
eval "$(agentstack hook zsh)"   # or bash / fish
echo backend > .agentstack       # in a project
```

`apply`/`use`/`instructions` **never write** without an explicit `--write`.

### Secrets

Secrets live as `${NAME}` references in the manifest and resolve per-machine:

1. **process env** — explicit override (CI / one-offs)
2. **[varlock](https://varlock.dev)** — when a `.env.schema` is present and the
   `varlock` binary is installed; delegates 1Password / AWS / Azure / GCP /
   Bitwarden / device-local encryption to varlock's
   `varlock load --format json-full`.
3. **OS keychain** — agentstack's own store; `agentstack secret set NAME` writes
   here (macOS Keychain / Windows Credential Manager / Linux Secret Service).
4. **project `.env`** — plain-text fallback.

## Usage

```bash
# Never a blank page: reverse-engineer a manifest from configs already on disk,
# lifting inline secrets into ${REF}s stored in the keychain.
agentstack init               # add --dry-run to preview, --no-keychain to skip storing

# Store / audit secrets
agentstack secret set KIBANA_TOKEN     # hidden prompt; or --value
agentstack secret list                 # which refs resolve, and from where

# Verify everything is wired up
agentstack doctor             # add --ci to exit nonzero on error

# First-run/team setup funnel: read-only preflight + diff by default.
agentstack bootstrap
agentstack bootstrap --write  # install skills, apply configs, then doctor

# See what would change in your real configs (read-only)
agentstack diff

# Dry-run a render (shows the diff, writes nothing)
agentstack apply

# Only a profile's servers, to one target
agentstack apply --profile backend --target codex

# Actually write (non-destructively, tracked in state.json)
agentstack apply --write

# Activate a profile: render its servers + materialize only its skills
agentstack use focus --write                 # global scope
agentstack use focus --scope project --write # into .mcp.json + .claude/skills/

# Compile CLAUDE.md / AGENTS.md from shared + per-harness fragments
agentstack instructions --scope project --write

# Launch a harness as a tracked run with a profile, then see/kill it
agentstack run claude-code --profile design   # profile reverts on exit
agentstack runs                               # list live runs (--json to script)
agentstack kill <id>                          # SIGTERM→SIGKILL; --force for immediate
```

### Manifest example

```toml
version = 1

[servers.kibana_mcp]
type = "http"
url = "https://kibana-mcp.ghaloyalty.com/mcp"
headers = { Authorization = "Bearer ${KIBANA_TOKEN}" }   # resolved per machine

[servers.github]
type = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
env = { GITHUB_TOKEN = "${GH_PAT}" }

[profiles.backend]
servers = ["kibana_mcp", "github"]

# Each CLI's native settings file, managed from here. agentstack owns only
# the keys you declare and leaves the rest of settings.json untouched.
[settings.claude-code]
enableAllProjectMcpServers = true
[settings.claude-code.permissions]
allow = ["Bash(git diff:*)", "Bash(git log:*)"]

[hooks.notify]
event = "Stop"
command = "echo done"

[plugins.play]
version = "1.0.0"
description = "Shared play workflow for Claude Code and Codex."
targets = ["claude-code", "codex"]
servers = ["github"]
hooks = ["notify"]

[targets]
default = ["claude-code", "codex"]
```

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

## Adding a CLI

Supporting a new CLI is one YAML descriptor — see `adapters/codex.yaml`. Drop
your own into `~/.agentstack/adapters/` to override or add targets without
rebuilding.

## Develop

```bash
cargo test          # unit + golden (insta) + integration
cargo clippy --all-targets
cargo fmt --check
```

## Roadmap

**Done:** 6 adapters · `init`/`add`/`apply`/`diff`/`use`/`instructions`/`adopt` ·
package manager (`install`/`update`/`remove` + lockfile) · secrets (keychain +
varlock) · scopes (global/project) · `doctor` (`--live`/`--fix`/`--ci`) ·
official MCP Registry provider + `search`/`add from` · `[policy]` trust gate ·
native per-CLI settings (`[settings.*]` → settings.json) · managed plugin
recipes (`[plugins.*]` → native Claude Code/Codex packages + marketplaces) ·
atomic writes + backups · `export`/`import` · `hook` · agent-operable `mcp`
server · local dashboard (server/skill matrices, Discover, add-skill, settings
editor) · live runs (`run`/`runs`/`kill` + dashboard Runs panel).

**Next:** publish releases + a real demo · dogfood on a team · marketplace
providers (skills.sh-style) + optional audit enrichment · reconsider a JSON /
`mcp.json`-aligned manifest · install/remove flows for native plugin runtimes ·
runs phase 2: discover stray (unmanaged) agent processes as an advisory view.

See [`agentstack-PLAN.md`](agentstack-PLAN.md) for the full spec and design
decisions (D1–D22).

## License

Dual-licensed under either [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at
your option.
