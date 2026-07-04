# agentstack

> **One reviewed, version-controlled setup for your AI agents** — MCP servers,
> skills, instructions, settings, and profiles, rendered into every agent CLI
> you use.

Define your stack once in `.agentstack/agentstack.toml`. agentstack writes it
into the native config of 13 agent CLIs — Claude Code, Claude Desktop, Codex,
Cursor, Windsurf, Gemini CLI, VS Code, GitHub Copilot CLI, OpenCode,
Antigravity, Junie, Kiro, and Pi. Secrets stay `${REFERENCES}` that resolve
per machine, so the file is safe to commit and share.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/Tarekkharsa/agentstack/main/install.sh | sh
```

Or from a checkout:

```sh
cargo build --release
./target/release/agentstack self link   # symlink onto your PATH
```

One static binary, no runtime dependencies.

## Start in 60 seconds

You don't start from a blank page — `init` imports the agent config already on
your machine:

```bash
agentstack init         # turn your existing CLI configs into one manifest
agentstack bootstrap    # check CLIs, skills, secrets; see what's missing
agentstack apply        # preview every CLI's changes, confirm to write
```

![agentstack first run: init → bootstrap → apply](docs/firstrun.gif)

If `bootstrap` reports a missing secret, store it once — it goes in your OS
keychain, never in the manifest:

```bash
agentstack secret set GH_PAT
```

That's the whole everyday loop. Two habits worth keeping:

- `agentstack` with no arguments tells you the one next step for the directory
  you're in.
- `agentstack doctor` verifies everything is wired up and names the exact fix
  for anything that isn't.

## Why

Setting up AI agents by hand has three problems:

1. **Every CLI spells the same thing differently** — one MCP server, six
   config syntaxes.
2. **Setups drift and don't travel** — a new laptop or teammate means redoing
   everything, slightly differently.
3. **Secrets end up in the wrong places** — real tokens pasted into files that
   were never meant to be shared.

One reviewed file fixes all three: secrets stay references, a lockfile makes
setups reproducible, and one `apply` renders everything everywhere. If you use
a single agent with one hand-managed server, you probably don't need this yet.

## A manifest at a glance

```toml
version = 1

[servers.github]
type = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
env = { GITHUB_TOKEN = "${GH_PAT}" }        # resolved per machine, never stored

[servers.kibana]
type = "http"
url = "https://kibana-mcp.example.com/mcp"
headers = { Authorization = "Bearer ${KIBANA_TOKEN}" }

[profiles.backend]
servers = ["kibana", "github"]
skills = ["sql-review"]                      # resolves from your central library

[targets]
default = ["claude-code", "codex"]
```

## Everyday commands

| Command | What it does |
| --- | --- |
| `agentstack init` | Reverse-engineer a manifest from the configs you already have |
| `agentstack bootstrap` | Preflight: installed CLIs, skills, secrets, pending diff |
| `agentstack apply` | Preview each CLI's config changes; confirm (or `--write`) to render |
| `agentstack doctor` | Verify wiring; every warning comes with the exact fix command |
| `agentstack diff` | What would change, read-only |
| `agentstack secret set NAME` | Store a secret in the OS keychain |
| `agentstack use <profile> --write` | Activate one profile's servers + skills |
| `agentstack run <cli> --profile <p>` | Launch a harness with a profile for its lifetime |
| `agentstack lock` | Pin profile refs in the lockfile without rendering anything |
| `agentstack dashboard` | The same lifecycle in a local web UI |

The [feature reference](docs/reference.md) has the complete command list.

## Share it with a team

Commit `.agentstack/` (manifest + lock). A teammate — or your CI — then runs:

```bash
git clone <repo>
agentstack bootstrap
agentstack secret set GH_PAT   # local only; never committed
agentstack apply --write
```

In CI, the trust gate is two commands — or the one-line GitHub Action:

```bash
agentstack install --locked   # fail if sources drifted from the pinned lock
agentstack doctor --ci        # fail on errors, drift, policy, unsafe content
```

```yaml
steps:
  - uses: actions/checkout@v4
  - uses: Tarekkharsa/agentstack@main   # or pin a release tag
```

## Where rendered files live — pick a mode

You always commit the *intent* (`agentstack.toml` + `.lock`). The rendered
artifacts (`.mcp.json`, `.claude/skills/`) are a per-project choice:

- **Static** (default) — artifacts sit on disk, kept out of git by a managed
  `.gitignore` block. Works however you launch your tools.
- **Clean-at-rest** — nothing generated exists between sessions; profiles are
  injected by `agentstack run` / `session start` and reverted on exit.
  `git status` stays silent.
- **Zero files** — `agentstack connect` registers the gateway once per
  harness; every **trusted** repo then brings its own servers through
  `agentstack mcp --auto-project`, with a tool firewall and call audit log
  included. Untrusted repos are inert until you review and `agentstack trust .`

Details and trade-offs: [feature reference → three modes](docs/reference.md).

## Going further

- **[Docs site](https://tarekkharsa.github.io/agentstack/)** — the visual
  getting-started walkthrough.
- **[Feature reference](docs/reference.md)** — the complete tested inventory:
  central library, vendor packs, MCP firewall, call audit log, `optimize`,
  plugin recipes, live runs, code mode, every command and flag.
- **[The no-terminal path](docs/dashboard.md)** — the full lifecycle done
  entirely from the dashboard UI.
- **Vendor packs** — `agentstack add from git:github.com/acme/pack@v1.2.0`
  installs a versioned MCP + skills + house-rules bundle, policy-gated and
  content-scanned before anything is written.
- **Personal layer** — `agentstack init --global` gives your machine-wide
  instructions a home; they merge beneath every project without ever landing
  in a repo's committed files.

The closed loop in under a minute — install a versioned pack, spread it to
every CLI, firewall a tool, watch the refusal in the audit log, upgrade to the
vendor's next tag:

![agentstack closed loop](docs/closed-loop.gif)

## Develop

```bash
cargo test              # unit + golden + integration
cargo clippy --all-targets
cargo fmt --check
```

Install your build with `agentstack self link` (symlinks the binary onto your
PATH; `self which` verifies what a bare `agentstack` runs). Don't wrap the
binary in a shell function or alias — those exist only in interactive shells,
so agent harnesses and scripts won't see them.

Adding a CLI is one YAML descriptor — see `adapters/codex.yaml`; drop your own
into `~/.agentstack/adapters/` without rebuilding.

## License

Dual-licensed under either [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE)
at your option.
