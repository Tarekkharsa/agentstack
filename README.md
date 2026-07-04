# agentstack

> **Portable agent runtime config.** One reviewed, version-controlled setup for
> MCP servers, skills, instructions, settings, hooks, and profiles — runnable
> across coding agents, repos, machines, and teammates.

You set up MCP servers, skills, and instructions **once** in a single
`.agentstack/agentstack.toml`, and agentstack writes them into every agent CLI's
own config format for you — Claude Code, Claude Desktop, Codex, Cursor, Windsurf,
Gemini CLI, VS Code, GitHub Copilot CLI, OpenCode, Antigravity, Junie, Kiro, and
Pi. Secrets stay as references and resolve locally on each machine, so the file
is safe to commit and share.

## Install

```sh
# After the first GitHub release is published (see RELEASING.md):
curl -fsSL https://raw.githubusercontent.com/Tarekkharsa/agentstack/main/install.sh | sh
# or: brew install Tarekkharsa/tap/agentstack   ·   cargo install agentstack
```

Until then, install from source — `cargo install --path .`, or link a release
build with `agentstack self link` (see [Develop](#develop)).

Single static binary, zero runtime dependencies.

## Quick start — from your current setup to every CLI

You don't start from a blank page. `init` reads the agent config already on your
machine and turns it into a manifest; `bootstrap` walks you through the rest.

```bash
agentstack init         # import the servers + skills you already have
agentstack bootstrap    # checks CLIs, skills, and secrets; shows what's missing
agentstack apply        # preview each CLI's config changes, then confirm to write
                        # (in scripts/CI, `apply --write` skips the prompt)
```

![agentstack first run: init → bootstrap → apply](docs/firstrun.gif)

If `bootstrap` reports a secret it couldn't find (say a GitHub token), store it
once — it goes in your OS keychain, never the manifest:

```bash
agentstack secret set GH_PAT
```

That's the whole everyday loop. Everything below is for when you want more:
sharing a setup with a teammate, launching agents with a profile, or auditing
what an agent can touch. Run `agentstack` with no arguments any time and it tells
you the one next step for the directory you're in.

## Why agentstack

Setting up AI agents by hand has three problems:

1. **Every CLI spells the same thing differently** — one MCP server needs a
   different config syntax in Codex, Cursor, Windsurf, Gemini, VS Code, and
   Claude Code.
2. **Setups drift and don't travel** — a new laptop, a teammate, or a fresh
   devcontainer means redoing everything by hand, slightly differently.
3. **Secrets end up in the wrong places** — real tokens pasted into config
   files that were never meant to be shared.

agentstack solves all three with one reviewed file: secrets stay references,
lockfiles make setups reproducible, and one `apply` renders everything to every
CLI. It shines when you use more than one agent CLI, share setup with
teammates, or switch machines often. If you use a single agent with one
hand-managed server and don't care about any of that yet, you probably don't
need it.

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

In CI, the trust gate is two commands — or the one-line GitHub Action:

```bash
agentstack install --locked   # fail if sources no longer match the pinned lock
agentstack doctor --ci        # fail on manifest errors, drift, policy, unsafe content
```

```yaml
jobs:
  agent-setup:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: Tarekkharsa/agentstack@main   # or pin a release tag
```

`install --locked` proves checked-in skill sources still resolve to the pinned
lockfile entries instead of rewriting the lock. `doctor --ci` then fails on
structural manifest errors, unresolved required secrets, invalid target configs,
lock drift, policy violations, and high-severity content-scan findings (hidden
Unicode in skill or instruction content). Advisory issues stay warnings.

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
3. **Zero files, model-driven** — `agentstack connect --all --write` registers
   the gateway once, globally, per harness. After that every **trusted** repo
   brings its own stack automatically: `agentstack mcp --auto-project` discovers
   the active project at session start (client roots → cwd walk-up →
   `$AGENTSTACK_MANIFEST_DIR`) and proxies its servers — firewall, audit log,
   and skills over MCP (`agentstack_list_loadable` / `agentstack_load`,
   session-fenced and logged) included. No per-repo files at all. Safety:
   auto-discovery is direnv-style trust-gated — a freshly cloned repo gets
   control-plane tools only (nothing spawned, no secrets resolved) until you
   review it and run `agentstack trust .`; any manifest edit re-requires trust.

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
- **MCP firewall + audit** — `[policy.tools]` allow/deny globs per server,
  enforced at the runtime gateway (denied tools are invisible to agents and
  refused if called), and an append-only call log — server, tool, argument
  *digest*, outcome — behind `agentstack audit --calls` and the dashboard's
  per-run trust footprint.
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
- **Zero-copy bridge** — `agentstack connect --all --write` registers the
  gateway once per harness; `mcp --auto-project` then discovers each repo at
  session start (client roots → cwd walk-up → env) and proxies its stack with
  no per-repo files. direnv-style safety: repos are inert until
  `agentstack trust .` pins their manifest digest; any edit re-requires trust.
- **Optimize** — `agentstack optimize` turns the collected signals (usage,
  call audit log, context costs, trust ledger) into recommendations — inert
  servers, firewall allowlists, denied/erroring tools — each with evidence,
  the exact command or TOML, and a safety rationale; `--write` applies only
  the provably-safe class.

The complete implemented-and-tested inventory — engine internals, plugin
recipes, live runs, code mode, the full command list — lives in
[`docs/reference.md`](docs/reference.md).

The closed loop in under a minute — a vendor publishes a versioned pack, a
fresh machine installs it at its tag, spreads it to every CLI, firewalls one
of its tools, watches a live call get refused, reads the audit receipts, and
picks up the vendor's next tag with one `upgrade`
(`agentstack-test/demo-closed-loop.sh`, fully sandboxed):

![agentstack closed loop: git pack install → apply → firewall → audited calls → upgrade](docs/closed-loop.gif)

### Per-directory auto-activation (`agentstack hook`)

direnv-style: drop a `.agentstack` file (first line = profile name) in a repo,
add the hook to your shell rc, and entering the repo activates that profile at
project scope across your CLIs:

```bash
eval "$(agentstack hook zsh)"   # or bash / fish
echo backend > .agentstack       # in a project
```

`apply` previews first and asks before writing in an interactive terminal; in
scripts and CI it stays read-only unless you pass `--write`. `use` and
`instructions` still require an explicit `--write`.

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

# Preview a render. In a terminal, agentstack asks before writing; in scripts,
# this stays read-only unless you pass --write.
agentstack apply

# Only a profile's servers, to one target
agentstack apply --profile backend --target codex

# Write directly without prompting (non-destructive, tracked in state.json)
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

# Zero-copy bridge: register the gateway once per harness, trust repos one by one
agentstack connect --all --write              # one global entry per harness (undo: disconnect)
agentstack trust .                            # review what THIS repo would run, then pin it
agentstack trust --list                       # audit every grant; edits invalidate digests

# Tune with evidence once runtime data has accumulated
agentstack audit --calls                      # the gateway call log (digests, outcomes)
agentstack optimize                           # recommendations with evidence + exact actions
agentstack optimize --write                   # apply only the provably-safe class
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

Packs also install from **any git host, versioned by tags** — a repo with a
`pack.toml` is a publishable pack (`agentstack pack init` scaffolds one):

```sh
agentstack add from git:github.com/acme/agent-pack@v1.2.0 --write
agentstack upgrade acme --write     # finds the newest version tag, previews, re-pins
```

The source is policy-gated **before** anything is fetched (`[policy]
allowed_sources`), and the clone's skill content passes the same
hidden-Unicode/injection scan as `install` before anything is written.

Starter packs today: **`linear-pack`**, **`cloudflare-pack`**, **`posthog-pack`**
(plus the standalone **`pr-triage`** and **`using-agentstack`** skills).
Instruction prose is opt-in, previewed, and merged with visible provenance.
Bundled starter skills are **agentstack-authored and unofficial** examples, not
endorsed vendor content.

## Docs

- [Central library guide](https://tarekkharsa.github.io/agentstack/) — the
  visual walkthrough: one library, projects that select by name, generated
  views for every CLI. Its flows are verified by the runnable sandbox
  (`agentstack-test/demo-central-library.sh`).
- [Feature reference](docs/reference.md) — the complete tested inventory and
  full command list.
- [The no-terminal path](docs/dashboard.md) — the full capability lifecycle
  (discover → add → secrets → enable → apply → verify → remove → undo) done
  entirely from the dashboard UI.

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

### Install from source

```bash
cargo install --path .                  # `agentstack` on PATH via ~/.cargo/bin

# or link a release build directly:
cargo build --release
./target/release/agentstack self link   # symlink into /usr/local/bin or ~/.local/bin
agentstack self which                   # verify what a bare `agentstack` runs
```

Don't wrap the binary in a shell function or alias: those exist only in
interactive shells, so agent harnesses and scripts — which spawn
non-interactive shells — won't see them. A real file on PATH (what `self
link` creates) works everywhere, and `connect` registers that stable path in
harness configs so they survive rebuilds.

## Roadmap

**Done:** the full shipped inventory lives in [`docs/reference.md`](docs/reference.md).

**Next:** central library for hooks (`lib/hooks/`) · `watch` auto-sync ·
transitive pack dependencies + semver ranges · team library sync (git-backed
`~/.agentstack/lib`) · SBOM export from the lockfile · marketplace providers ·
Windows support for live runs · session auto-start/end from the directory
hook.

## License

Dual-licensed under either [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at
your option.
