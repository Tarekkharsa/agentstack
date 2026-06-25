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
- Full CLI + an optional local **dashboard** (below).

The dashboard is an embedded localhost server + a self-contained UI (shadcn
aesthetic, hand-written CSS — no Node, no framework, still one `cargo build`):
`agentstack dashboard` opens a cross-harness matrix with secrets, skills,
profiles, and usage panels, and can **set secrets, apply, activate a profile, and
install** right from the UI (`--read-only` disables writes). Bound to 127.0.0.1,
token-gated, and it never exposes secret values.

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
- **Global vs project scope** (`--scope`): write to each CLI's global locations
  (`~/.claude.json`, `~/.claude/skills`) or its project locations (`.mcp.json`,
  `.claude/skills/`) so any agent opening the repo inherits the setup.
- **Selective skills** via profiles — `use <profile>` materializes only that
  profile's skills (symlink, with copy fallback), pruning the rest it owns and
  never clobbering hand-made skill dirs.
- **Instruction files** — compile shared + harness-specific fragments into each
  CLI's `CLAUDE.md` / `AGENTS.md`, inside a managed `<!-- agentstack -->` region
  that preserves all surrounding hand-written prose.
- **`adopt`** — the reverse of `apply`: pull a hand-added server from a target
  config back into the manifest, lifting its inline secret, preserving comments.
- **`add`** — flag-driven (scriptable / agent-operable) add of a server or skill
  to the manifest, optionally into a profile; comments preserved.
- **`doctor --live`** — real MCP `initialize` handshake over HTTP; reports
  server name + tool count, or classifies the error (auth / http / connect).
- **Package manager** — skills declare a source (`path` or `git`);
  `install` fetches them into `~/.agentstack/store/` and writes a checksum-pinned
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
  `apply` (`--scope`, `--write`), `diff`, `use <profile>`, `instructions`,
  `adopt`, `doctor` (`--ci`, `--live`, `--fix`), `search`, `stats`,
  `secret set|get|rm|list`, `export`/`import`, `adapters`, `dashboard`, `mcp`,
  `hook`.

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

[targets]
default = ["claude-code", "codex"]
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
atomic writes + backups · `export`/`import` · `hook` · agent-operable `mcp`
server · local dashboard (matrix, Discover, per-CLI toggle).

**Next:** publish releases + a real demo · dogfood on a team · marketplace
providers (skills.sh-style) + optional audit enrichment · reconsider a JSON /
`mcp.json`-aligned manifest · plugins as a managed capability.

See [`agentstack-PLAN.md`](agentstack-PLAN.md) for the full spec and design
decisions (D1–D22).

## License

Dual-licensed under either [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at
your option.
