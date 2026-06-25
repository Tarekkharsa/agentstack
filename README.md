# agentstack

> One portable manifest, every agent CLI.

`agentstack` is a single-binary CLI that manages MCP servers (and, soon, skills)
across multiple AI coding agents — Claude Code, Codex, and any CLI you can
describe with a small YAML adapter — from **one** commit-safe source of truth.

Describe your agent capabilities once in `agentstack.toml`; agentstack compiles
that manifest into each CLI's native config (JSON for Claude Code, TOML for
Codex, …), resolves secrets per-machine, scopes capabilities per profile, and
proves the result with `diff`.

## Why

Configuring AI agents today tangles three pains together:

1. **Format fragmentation** — same MCP server, different syntax per CLI
   (Codex `[mcp_servers.x]` TOML, Claude Code `mcpServers` JSON with
   `"type":"http"`, headers named `http_headers` vs `headers`, …).
2. **Selective loading** — sometimes all capabilities, sometimes a curated few.
3. **Secrets & per-machine drift** — real tokens differ per machine and must
   never be committed.

agentstack's wedge: MCP **+** profiles **+** selective loading **+**
secrets-by-reference **+** cross-machine migration **+** a trust layer, in one
binary, extensible to any CLI via data descriptors.

## Status — CLI complete + local dashboard (Phases 0–4)

The full command-line tool plus a local web **dashboard** are built and tested.
The dashboard is an embedded localhost server + a self-contained UI (shadcn
aesthetic, hand-written CSS — no Node, no framework, still one `cargo build`):
`agentstack dashboard` opens a cross-harness matrix with secrets, skills,
profiles, and usage panels, and can **set secrets, apply, activate a profile, and
install** right from the UI (`--read-only` disables writes). Bound to 127.0.0.1,
token-gated, and it never exposes secret values.

Implemented and tested:

- **Manifest model** with layered load (`agentstack.toml` + a gitignored
  `agentstack.local.toml` overlay) and static validation.
- **Data-driven adapters** (`adapters/claude-code.yaml`, `adapters/codex.yaml`,
  embedded in the binary; user overrides from `~/.agentstack/adapters/`).
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
- **`search`** — find capabilities in an embedded starter catalog (registry v0),
  printing copy-pasteable `add` commands; the discovery surface the agent uses
  to provision itself.
- **`stats`** — local usage analytics: activation counts + per-capability
  footprint (which target/scope slots it's live in).
- **`export`/`import`** — age-encrypted bundle (manifest + lock + optionally
  secrets) for moving a setup to a new machine; passphrase-protected.
- Commands: `init`, `add`, `install` (`--locked`), `update`, `remove`,
  `apply` (`--scope`, `--write`), `diff`, `use <profile>`, `instructions`,
  `adopt`, `doctor` (`--ci`, `--live`), `search`, `stats`,
  `secret set|get|rm|list`, `export`/`import`, `adapters`, `dashboard`.

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

- **Phase 1 (done)** — `init` (discover + import + lift secrets), state-tracked
  writes, OS-keychain + varlock `secret set`, static `doctor`.
- **Phase 2** — `doctor --live` (MCP handshake) / `--fix`, interactive `add`,
  profiles + skill symlinking, `adopt`.
- **Phase 3** — encrypted `export`/`import`, `init --from <git>`, more adapters,
  per-directory auto-activation.

See [`agentstack-PLAN.md`](agentstack-PLAN.md) for the full spec.

## License

Dual-licensed under either [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at
your option.
