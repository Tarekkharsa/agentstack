# agentstack

> **Portable agent runtime config.** One reviewed, version-controlled setup for
> MCP servers, skills, instructions, settings, hooks, and profiles — runnable
> across coding agents, repos, machines, and teammates.

agentstack turns AI-agent setup into a reproducible repo artifact. You declare
capabilities once in a commit-safe `.agentstack/agentstack.toml`; agentstack
**compiles** that intent into each agent CLI's native config — Claude Code,
Claude Desktop, Codex, Cursor, Windsurf, Gemini CLI, VS Code, GitHub Copilot CLI,
OpenCode, Antigravity, Junie, Kiro, and Pi — resolving secrets locally on each
machine.

The goal is not just config sync. agentstack is the control layer for portable
agent environments: bootstrap a new laptop, share a team setup through git,
launch an agent with a known profile, audit what it can access, and remove or
upgrade capabilities without hand-editing every harness.

The core loop is intentionally small:

```sh
agentstack init       # import the setup you already have
agentstack bootstrap  # preflight: CLIs, skills, secrets, diff, next action
agentstack doctor     # prove the manifest is valid and reproducible
agentstack apply      # preview native config changes
agentstack apply --write
```

Each project then picks one of [three modes](#the-three-modes--where-rendered-files-live)
for the rendered files: **static** (on disk, gitignored), **clean-at-rest**
(sessions inject and revert — nothing between sessions), or **zero-files**
(the agent loads skills from the central library over MCP).

![agentstack first run: init → bootstrap → doctor --ci → apply --write, fenced](docs/firstrun.gif)

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/tarekkh/agentstack/main/install.sh | sh
# or: brew install tarekkh/tap/agentstack   ·   cargo install agentstack
```

Single static binary, zero runtime dependencies. (Releases are wired up in CI —
see [RELEASING.md](RELEASING.md).)

## Quick start

Start with the setup already on your machine:

```bash
agentstack init
agentstack bootstrap
agentstack secret set GH_PAT       # only when bootstrap reports a missing ref
agentstack doctor
agentstack apply                   # dry-run diff
agentstack apply --write
```

For a team repo, commit `.agentstack/agentstack.toml` and
`.agentstack/agentstack.lock`, but never commit local secrets. In CI, the trust
gate is:

```bash
agentstack install --locked
agentstack doctor --ci
```

or, as a one-line GitHub Action:

```yaml
jobs:
  agent-setup:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: tarekkh/agentstack@main   # or pin a release tag
```

`install --locked` proves checked-in skill sources still resolve to the pinned
lockfile entries; it fails instead of rewriting the lock. `doctor --ci` then
fails on structural manifest errors (unknown refs, missing transport fields),
unresolved required secrets, invalid target configs, lock drift, policy
violations, and high-severity content-scan findings (hidden Unicode in skill or
instruction content). Warnings still print for advisory issues that do not make
the setup unsafe to render.

## Why it exists

Managing AI-agent setup today is three tangled pains:

1. **Format fragmentation** — the *same* MCP server is spelled differently per
   CLI (Codex TOML `[mcp_servers.x]`, Cursor `url`, Windsurf `serverUrl`, Gemini
   `httpUrl`, VS Code's `servers` key, Claude's `type:"http"`).
2. **Reproducibility & drift** — a new laptop, a teammate, a fresh devcontainer:
   everyone re-does setup by hand, and configs drift apart.
3. **Secrets** — real tokens differ per machine and must never land in git.

The durable value is not only format translation (the ecosystem is slowly
converging on `mcp.json`). The value is the layer above it:
**secrets-by-reference, profiles/selective loading, reproducible lockfiles,
cross-source discovery, runtime launch control, and trust/governance gates** in
one auditable binary across the CLIs your team actually uses.

## Who it is for

agentstack is most useful when you use more than one agent CLI, share agent
setup with teammates, switch machines often, or need MCP servers, skills,
instructions, settings, and secrets to be reviewed in git.

It is probably overkill if you use one agent with one or two hand-managed MCP
servers and do not care about reproducibility, profiles, drift, or team
onboarding yet.

## Portable team workflow

Commit the shared intent — agentstack keeps it in a single `.agentstack/` folder
at the repo root:

```text
.agentstack/agentstack.toml
.agentstack/agentstack.lock
.agentstack/agentstack.md
.agentstack/skills/
.agentstack/instructions/
```

`agentstack init` creates this `.agentstack/` layout. Repos that still keep
`agentstack.toml`, `agentstack.lock`, `skills/`, and `instructions/` at the root
are discovered automatically (legacy layout), so existing setups keep working
without migration.

Then a new teammate or a new computer follows the same path:

```bash
git clone <repo>
agentstack bootstrap          # preflight: installed CLIs, skills, secrets, diff
agentstack secret set GH_PAT  # local only; never committed
agentstack doctor --live
agentstack apply --write
agentstack run codex --profile backend
```

`agentstack.toml` is portable. Secrets are not. They resolve per machine from
env, varlock, OS keychain, or `.env`, and unresolved secrets block writes by
default so placeholders do not leak into live harness config.

## The three modes — where rendered files live

You always commit the intent (`.agentstack/agentstack.toml` + `.lock`). What
happens to the *rendered* project artifacts (`.mcp.json`, `.claude/skills/`
symlinks) is a per-project choice:

1. **Static (default)** — artifacts sit on disk, kept out of git automatically
   via a managed `.gitignore` block (they're machine-local: absolute-path
   symlinks, resolved values). Works with harnesses launched any way. Prefer
   committing them for your team? `--no-gitignore` — files you already track
   are never affected either way.
2. **Clean-at-rest** — nothing generated exists between sessions. Add an empty
   profile (`[profiles.off]`), run `use off --scope project --write` once, then
   work through `agentstack run <cli> --profile <p>` (injects on launch,
   reverts on exit) or `session start <p> --scope project` … `session end`.
   Pruning-to-zero even deletes the empty config file, so `git status` stays
   silent. **Trade-off:** launching the harness *directly* (not through
   agentstack) sees no project servers/skills while at rest — and one command
   (`use <p> --scope project --write`) flips the project back to static.
3. **Zero files, model-driven** — register `agentstack mcp` once (global) and
   the agent itself lists and loads skills from the central library at runtime
   (`agentstack_list_loadable` / `agentstack_load`, session-fenced and logged).
   No skill files in the repo at all; instructions travel over MCP.

## What works today

- **13 agent CLIs** — Claude Code, Claude Desktop, Codex, Cursor, Windsurf,
  Gemini CLI, VS Code, GitHub Copilot CLI, OpenCode, Antigravity, Junie, Kiro,
  and Pi (each a data descriptor; add more without touching core code).
- **Cross-source discovery** — `search` and `add from` pull from the embedded
  catalog **and the official MCP Registry**, rendering to every CLI at once.
- **Crash-safe** — config writes are atomic with pre-write backups; never
  corrupts your real `~/.claude.json`.
- **Trust gate** — `[policy]` (require/forbid/`allowed_sources`) enforced by
  `doctor --ci`, plus provenance hints at the point of choosing.
- **Live runs** — launch any harness as a tracked process (`agentstack run`),
  optionally with a profile applied just for its lifetime; see and kill every run
  (and its whole process tree) from the CLI or dashboard — no Activity Monitor.
- **Supply-chain scanning** — every skill install is scanned for hidden Unicode
  (blocks) and prompt-injection heuristics (warns); `agentstack audit` and
  `doctor --ci` keep it enforced.
- **Central capability library** — one managed home (`~/.agentstack/lib/`);
  projects reference skills and servers **by name**, digest-pinned in the lock.
- **Local dashboard** — `agentstack dashboard`: a cross-harness matrix for
  servers, skills, secrets, settings, profiles, and runs. 127.0.0.1, token-gated,
  never exposes secret values; `--read-only` to browse without write access.
- **Agent-operable** — `agentstack mcp` runs as an MCP server (agent proposes,
  human applies) and proxies project servers behind a compact search + typed
  code-mode bindings surface.

The complete implemented-and-tested inventory — engine internals, plugin
recipes, live runs, code mode, the full command list — lives in
[`docs/reference.md`](docs/reference.md).

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

# Central library: put skills/servers in one home, reference them by name
agentstack consolidate --write               # sweep scattered skills into lib/skills
agentstack lib add-server kibana --file kibana.toml --write
agentstack lib list                          # skills + servers, grouped
# then reference by name in a profile: servers = ["kibana"], skills = ["sql-review"]

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
url = "https://kibana-mcp.example.com/mcp"
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

Create or adopt plugin recipes without hand-editing TOML — `agentstack
plugins create|adopt|sync|status|install` (all preview-first); the full tour
is in [`docs/reference.md`](docs/reference.md).

## Vendor packs

agentstack can also package a vendor or internal team's **MCP + skills + docs**
as one installable unit. That gives users one command to pull the server, the
skills that know how to drive it, and optional house rules into their own agent
CLIs without committing secrets.

```sh
agentstack add from linear-pack --write                      # server + skills
agentstack add from linear-pack --with-instructions --write  # also merge house rules
agentstack apply --write                                     # render to native configs
agentstack upgrade linear-pack --write                       # re-resolve and re-pin
```

Starter packs today: **`linear-pack`**, **`cloudflare-pack`**, **`posthog-pack`**
(plus the standalone **`pr-triage`** skill). Instruction prose is opt-in,
previewed, and merged with visible provenance. Bundled starter skills are
**agentstack-authored and unofficial** examples, not endorsed vendor content.

For the broader product direction, see
[`docs/plans/portable-agent-runtime-vision.md`](docs/plans/portable-agent-runtime-vision.md).

## Docs

- [Feature reference](docs/reference.md) — the complete tested inventory and
  full command list.
- [Central library guide](docs/central-library-guide.html) — visual walkthrough
  of referencing skills/servers by name from `~/.agentstack/lib/`; its flows are
  verified by the runnable sandbox (`agentstack-test/demo-central-library.sh`).
- [Design docs](docs/plans/) — vision, spec, and per-feature plans.

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

**Done:** the full shipped inventory lives in [`docs/reference.md`](docs/reference.md).

**Next:** central library for hooks (`lib/hooks/`) · one-command provider import
(sweep every CLI's skills + MCP entries into `~/.agentstack`, leaving generated
views/symlinks behind — see [`docs/plans/provider-import.md`](docs/plans/provider-import.md)) ·
harden pack remove/upgrade ownership · add golden coverage for every adapter ·
polish the new-machine/team bootstrap path · publish releases + a real demo ·
dogfood on a team · dashboard trust-footprint views for live runs · marketplace
providers (skills.sh-style) + optional audit enrichment · reconsider a JSON /
`mcp.json`-aligned manifest · install/remove flows for native plugin runtimes ·
discover stray unmanaged agent processes as an advisory view.

See [`docs/plans/original-spec.md`](docs/plans/original-spec.md) for the full spec and design
decisions (D1–D22),
[`docs/plans/central-store.md`](docs/plans/central-store.md) +
[`docs/plans/provider-import.md`](docs/plans/provider-import.md) for the central-library
design, [`docs/central-library-guide.html`](docs/central-library-guide.html) for
a visual guide covering existing projects, new central-library projects, and
generated provider views, and
[`docs/plans/portable-agent-runtime-vision.md`](docs/plans/portable-agent-runtime-vision.md)
for the current product vision.

## License

Dual-licensed under either [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at
your option.
