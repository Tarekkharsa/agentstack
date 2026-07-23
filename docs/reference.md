<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# Feature reference

The complete, implemented-and-tested feature inventory. The
[README](../README.md) is the tour; this is the map. Terms (CLI, adapter,
target, gateway, posture, trust, …) are defined once in
[concepts.md](concepts.md) — this page assumes them and stays operational.

Deeper rationale and field notes — edge cases, crate-level caveats, and
implementation internals — live in
[reference field notes](design/reference-field-notes.md).

**Contents**

**[Part I — The everyday loop](#part-i--the-everyday-loop)**

- [Secrets and trust](#secrets-and-trust)
  - [Secret resolution](#secret-resolution)
  - [Where lifted secrets go (`init`)](#where-lifted-secrets-go-init)
  - [Unresolved secrets block writes](#unresolved-secrets-block-writes)
  - [`doctor --live`](#doctor---live)
  - [One undo verb: `restore`](#one-undo-verb-restore)
  - [`doctor` shows what you use](#doctor-shows-what-you-use)
- [Drift: adopt or apply?](#drift-adopt-or-apply)
  - [`adopt` and `add`](#adopt-and-add)
  - [Search across providers](#search-across-providers)
  - [Selective skills via profiles](#selective-skills-via-profiles)
- [Live runs (`agentstack run`)](#live-runs-agentstack-run)

**[Part II — The power surface](#part-ii--the-power-surface)**

- [Core engine](#core-engine)
  - [The manifest](#the-manifest)
  - [Data-driven adapters](#data-driven-adapters)
  - [Rendering and merging](#rendering-and-merging)
  - [State tracking](#state-tracking)
  - [Scopes](#scopes)
- [Delivery modes — where rendered files live](#delivery-modes--where-rendered-files-live)
  - [Owned servers (`owner = "codex"`)](#owned-servers-owner--codex)
- [Agent-operable (`agentstack mcp`)](#agent-operable-agentstack-mcp)
  - [Transparent mode (`--transparent`)](#transparent-mode---transparent)
  - [The zero-files gateway (`--auto-project` + `trust`)](#the-zero-files-gateway---auto-project--trust)
  - [MCP profile leases](#mcp-profile-leases-one-connection-one-capability-fence)
  - [Compact proxied surface + code mode](#compact-proxied-surface--code-mode)
  - [Experimental `tools_execute`](#experimental-tools_execute)
- [Governance (`[policy]`)](#governance-policy)
- [MCP firewall (`[policy.tools]`)](#mcp-firewall-policytools)
- [Egress rules (`[policy.egress]`)](#egress-rules-policyegress)
- [Secret access (`[policy.secrets]`)](#secret-access-policysecrets)
- [Filesystem scopes (`[policy.filesystem]`)](#filesystem-scopes-policyfilesystem)
- [Call log](#call-log)
- [Content scanning](#content-scanning)
- [Ephemeral sessions (`agentstack session`)](#ephemeral-sessions-agentstack-session)
  - [Execution posture](#execution-posture)
  - [The Protected tier in detail (`run --locked`)](#the-protected-tier-in-detail-run---locked)
- [The central library](#the-central-library)
  - [Layout and name resolution](#layout-and-name-resolution)
  - [Pinning and provenance](#pinning-and-provenance)
  - [Adding capabilities](#adding-capabilities)
  - [Syncing across machines (`lib sync`)](#syncing-across-machines-lib-sync)
  - [The two mental models](#the-two-mental-models)
- [Capabilities](#capabilities)
  - [Package manager](#package-manager)
  - [Git-hosted versioned packs](#git-hosted-versioned-packs)
  - [`add skill <source>`](#add-skill-source--install-from-any-skills-repo)
  - [`try` — run a skill without installing anything](#try--run-a-skill-without-installing-anything)
  - [Instruction files](#instruction-files)
  - [The machine layer](#the-machine-layer)
  - [Native settings](#native-settings)
  - [Lifecycle hooks](#lifecycle-hooks)
  - [Native extensions](#native-extensions)
  - [`report usage` (usage analytics)](#report-usage-usage-analytics)
  - [Wire proxy (`proxy`)](#wire-proxy-proxy)
  - [`export` / `import`](#export--import)
- [Optimize (`agentstack optimize`)](#optimize-agentstack-optimize)

**[Part III — Full command reference](#part-iii--full-command-reference)**

- [All commands](#all-commands)
- [Everything shipped so far](#everything-shipped-so-far)

## Part I — The everyday loop

This part is everything `agentstack --help` shows by default: the nine everyday commands — `init`, `status`, `add`, `search`, `apply`, `doctor`, `use`, `run`, `trust` — and the machinery directly behind them. Read only what is here and you can reach a working setup: one manifest rendered into every CLI's native config, credentials kept out of that config, hand-edits caught, and a profile activated. The power surface in Part II is entirely opt-in — nothing in the everyday loop requires it.

## Secrets and trust

The enforcement core: how a secret resolves, where a policy narrows what a
server may do, and what every brokered call records. Read it if you run
untrusted repos, resolve credentials on this machine, or want a machine ceiling
no project can loosen.

### Secret resolution

The chain — process env → **varlock** → **OS keychain** → project `.env` — and
the `${REF}` rules live in [concepts.md — secrets](concepts.md#secrets);
unresolved refs are reported, never blanked. Operational specifics: the varlock
link activates only when the project opts in (a `.env.schema` present) and the
`varlock` binary is on PATH, otherwise the chain silently skips it. When active,
agentstack shells out to `varlock load --format json-full --compact` and
delegates the whole provider matrix (1Password, AWS/Azure/GCP, Bitwarden,
device-local stores) to it — see [varlock.dev](https://varlock.dev). Each ref
resolves **once per run**; a transient keychain read is retried, a persistent
failure is reported as *keychain read failed* (distinct from *not found*), so a
flaky keychain daemon never blocks a write by claiming a stored secret is missing.

### Where lifted secrets go (`init`)

```text
init --secrets env|keychain|skip
```

When `init` finds inline tokens in an imported config it lifts each to a `${REF}`
and picks where the value lands: a gitignored project `.env` (**the default**),
the OS keychain (service `agentstack`), or skip and write only the placeholder.
Interactive prompts for the three; non-interactive takes `--secrets` and defaults
to `keychain` when absent, so CI never starts writing plaintext by surprise.
`--no-keychain` is the deprecated alias for `--secrets skip`; a skip prints every
unstored `${REF}` with the command to store it. The `.env` writer places values
next to the manifest with a managed `.gitignore` entry, and `secret set
--env-file` targets that same `.env`. The manifest itself only ever holds
`${REF}` placeholders (rule 5).

### Unresolved secrets block writes

If a `${REF}` doesn't resolve on this machine, `apply`/`use` writes are
refused for that target — never a `${TOKEN}` placeholder in live config.
Override with `--allow-unresolved`. Structural manifest validation errors
block `--write` too.

### `doctor --live`

```text
agentstack doctor --live
```
Real MCP `initialize` handshake over HTTP; reports server name + tool count,
or classifies the error (auth / http / connect).

### One undo verb: `restore`

`restore` is the single undo verb for agentstack's recorded **writes** — servers,
settings, hooks, instructions, even the owned-server manifest refresh. Full
walkthrough and the table of the five actions undone by their own verb:
[undo anything](howto/undo.md).

```text
agentstack restore                 # list the recorded changes
agentstack restore <id> --write    # revert one (unique id prefix)
agentstack restore --last --write  # revert the most recent
agentstack restore <adapter>       # single-slot config restore (fallback)
```

Reverted files show up as pending again; `agentstack restore` lists the same
recorded writes and the identifier needed to roll each one back.

### `doctor` shows what you use

```text
agentstack doctor         # only the sections relevant to this project
agentstack doctor --all   # every section
agentstack doctor --ci    # the full report (a team gate)
```

Every check always runs, but the default report prints only the sections
relevant to this project — a feature you've never touched (the zero-files
gateway, native extensions, reproducibility pins…) stays out of the way until it
is used or produces a warning/error, which always shows. A closing line counts
what was hidden; `--all` and `--ci` print the full report, while `--json`
provides the complete machine-readable view for t3code and automation.

## Drift: adopt or apply?

```text
agentstack diff            # review the drift
agentstack adopt           # keep the hand-edit (pull it into the manifest)
agentstack apply --write   # keep the manifest (re-render over the edit)
```

`doctor` flags drift in both directions, and the fixes are opposites — pick
by which side holds the truth:

- **"edited on disk since last apply"** — the live config changed after our
  last write. Review with `agentstack diff`; if the hand-edit should stay,
  `agentstack adopt` pulls it into the manifest. If the manifest is right,
  `agentstack apply --write` re-renders over the edit.
- **"would REMOVE \<names\>"** — the manifest no longer selects entries we
  manage, so the next `apply --write` deletes them from the live config.
  `agentstack adopt` first if any of them should survive; apply only when
  the removal is intended. Both scopes are checked: entries a
  `--scope project` apply recorded (e.g. in `.mcp.json`) get their own line,
  labeled `(project)` and hinting `apply --scope project --write`.
- Entries recorded by a **different manifest** are never pruned implicitly
  (global scope is shared by every manifest on the machine): `apply` keeps
  them and says so, and `diff`/`doctor` keep surfacing them as kept — not as
  pending deletions — until you decide. Prune them with an explicit
  `apply --prune-foreign` (it still works after the guarded write recorded
  its own set), or `adopt` them into the current manifest.

### `adopt` and `add`

`adopt` is the keep-side of a [drift decision](#drift-adopt-or-apply) — it
imports native **server drift** (hand-added servers, hand-edited fields) from
target configs back into the manifest, lifting inline secrets and preserving
comments. It takes no positional name: it sweeps the drifted targets, scoped
with `--target <id>` if you want just one CLI's config. `add` is the
flag-driven (scriptable / agent-operable) way to add a server or skill,
optionally into a profile.

```text
agentstack adopt --write                  # import hand-added server drift into the manifest
agentstack adopt --target claude-code --write   # only that CLI's config
agentstack add ...                        # flag-driven add of a server or skill
```

### Search across providers

`search` queries **your central library first** (skill and library-server names,
labelled `[library]`), then the embedded catalog **and the official MCP
Registry**; `add from <id>` resolves a registry/catalog server, lifts its secrets
to `${REF}`s, and renders it to **all your CLIs at once**.

```text
agentstack search <query>
agentstack add from <id>
```

agentstack is the cross-CLI *client* over the registry + marketplaces, not
another registry.

### Selective skills via profiles

`use <profile>` materializes only that profile's skills, pruning the rest it
owns and never clobbering hand-made skill dirs.

```text
agentstack use <profile> --write   # materialize only that profile's skills
```

The profile is optional: one declared profile is chosen automatically, and a
manifest with **no** profiles activates its full inline set — `agentstack use
--write` just works; several profiles need a name. Materialization is
symlink-with-copy-fallback; when a prune empties the managed skills dir
(deactivation, `session end`) the dir is removed too, but rmdir semantics spare
any dir holding user content. Interactive `init` activates through this exact
`use` code path; plain `apply` never touches skills, it only names which profile
activates them.

## Live runs (`agentstack run`)

Launch an agent CLI as a **tracked run** and control it without leaving
agentstack. A run is a real OS process agentstack owns: spawned in its own
process group (so a kill takes down the whole tree), recorded in
`~/.agentstack/runs.json`, and visible to any other AgentStack process or
integrated supervisor.

```bash
# Launch a harness, attached to your terminal, with a profile applied for the
# life of the run (its servers + skills are reverted automatically on exit).
agentstack run claude-code --profile design
agentstack run codex --profile backend --scope project
agentstack run claude-code --keep        # leave the profile applied after exit

# See runs and stop them here.
agentstack report runs         # table; add --json for scripting
agentstack kill <id>           # SIGTERM, then SIGKILL if it won't go
agentstack kill <id> --force   # SIGKILL immediately
```

Launching is a terminal act (the CLIs are interactive TUIs). The registry is
self-healing: a run
whose wrapper died is pruned on the next `report runs`. A profile-bound run uses
the session engine, so one is allowed per directory at a time. Every tracked run
records a minimal lifecycle and prints `agentstack report run <id>` when it exits;
gateway-brokered tool calls join that report without recording argument values.
Unix only for now.

## Part II — The power surface

These are the twenty-nine commands `agentstack --help` keeps hidden as progressive disclosure, plus the advanced delivery and enforcement modes the everyday loop only points at. Hidden is not unsupported — every command here is fully maintained and carries its own `--help`, exactly as the [All commands](#all-commands) preamble spells out. Reach in when you need a machine-wide policy ceiling, the zero-files gateway, ephemeral sessions, locked runs, the central library, or the observability tooling.

## Core engine

The machinery every other section builds on: how one manifest is loaded,
validated, and rendered into native config for thirteen agent CLIs — and how a
later hand-edit is caught. Skip it unless you want the internals of how intent
becomes config.

### The manifest

Layered load: the preferred `.agentstack/agentstack.toml` plus a gitignored
`agentstack.local.toml` overlay (legacy root `agentstack.toml` remains
supported), with static validation before anything renders. Relative paths in
the manifest (skill `path`, instruction sources) anchor at the **manifest's
own directory** — `.agentstack/` in the preferred layout — so
`path = "./skills/x"` materializes at `.agentstack/skills/x`; a server's
`cwd` is the deliberate exception and anchors at the project root. The `version`
field is checked on load — a manifest (or lockfile, or library index)
written by a newer schema than the build supports errors with an "upgrade
agentstack" message instead of being misread silently.

### Data-driven adapters

Claude Code, Claude Desktop, Codex, Cursor, Windsurf, Gemini CLI, VS Code,
GitHub Copilot CLI, OpenCode, Antigravity, Junie, Kiro, and Pi — one YAML
descriptor each, embedded in the binary, with user overrides and additions
loaded from `~/.agentstack/adapters/`. Each CLI's quirks are encoded in data,
not code (Claude's `type:"http"`, Codex's `http_headers` subtable, Gemini's
`httpUrl`, VS Code's `servers` key, Copilot CLI's `type:"local"` stdio tag, …),
and per-OS config paths (`{config}/…`) resolve on macOS/Linux/Windows.
`agentstack adapters list` shows their ids.

### Rendering and merging

A generic renderer applies field renames, transport tags, header nesting, and
secret substitution; its **inverse** powers `init`, importing existing configs
back into a manifest. Merges are non-destructive — JSON splices only the managed
section (untouched bytes, floats included, preserved exactly); TOML uses
`toml_edit` to keep comments and formatting. Nothing drops silently: a server
whose transport a target can't express, or whose **name** the CLI would refuse at
startup (Codex validates against `^[a-zA-Z0-9_-]+$`), is skipped with a spoken
reason rather than written into a config that errors on launch.

Native keys with no transport-neutral equivalent live under a per-target `extra`
table, passed through verbatim by that one adapter (`${REF}` substitution still
applies); `init`/`adopt` lift unknown keys back into `extra.<adapter>`, and a
typo'd adapter id is a validation error. A stdio server can declare a `cwd` for
servers that only start from their own directory — it renders to each adapter's
native working-directory key (Codex, Cursor, Gemini CLI, OpenCode, Copilot CLI),
round-trips through `init`/`adopt`, warns where an adapter has no such key, and
the gateway honors it too (defaulting to the project root, never the client's cwd).

```toml
[servers.miro.extra.codex]
startup_timeout_sec = 20   # npx cold-cache fetch must not block CLI startup

[servers.tldraw]
cwd = "/path/to/tldraw-mcp-server"   # supports ${REF}/path expansion
```

A server can also scope which targets it renders to, mirroring instructions and
hooks: `targets = ["claude-code"]` fans out to that adapter only, `["*"]`
(default) means every target, `targets = []` opts out of the direct fan-out.
`apply`, `diff`, and `doctor` share the one filter; a typo'd id is a validation
error.

### State tracking

`~/.agentstack/state.json` records what agentstack manages per target, so
`apply` prunes entries we own that left the manifest and `doctor`/`diff`
detect hand-edits — see [drift: adopt or apply?](#drift-adopt-or-apply) for
which fix to run. `diff --json` emits the selected scope, profile, per-CLI
change/diff records, kept foreign entries, owner refreshes, and warnings for CI
or agent consumers.

### Scopes

Writes default to the **manifest's home**: a repo manifest writes **project**
locations (`.mcp.json`, `.claude/skills/` — repo-local, behind the managed
`.gitignore` block), while the machine manifest (`~/.agentstack/`) writes
**global** locations (each CLI's `~/.claude.json`, `~/.claude/skills`).
`--scope` overrides either way — e.g. `apply --scope global` in a repo puts
its servers in every project's config on this machine. `doctor` follows the
scope your writes actually recorded, so a deliberate `--scope` choice is
honored, not second-guessed.

<a id="where-rendered-files-live-three-modes"></a>
## Delivery modes — where rendered files live

You always commit the *intent* (`agentstack.toml` + `agentstack.lock`); the
rendered artifacts — `.mcp.json`, `.claude/skills/`, the compiled
`CLAUDE.md` / `AGENTS.md` — are a per-project choice of **static**,
**clean-at-rest**, or **zero-files**. What each mode *is*:
[concepts.md — delivery modes](concepts.md#delivery-modes); which to pick:
[which mode do I need?](choose.md). The operational levers:

- **static** (default) — artifacts on disk, kept out of git by a managed
  `.gitignore` block; pass `--no-gitignore` to commit them instead.
- **clean-at-rest** — `agentstack lock` pins name refs *without rendering*, so
  `git status` stays silent; a profile arrives via
  [`session start`](#ephemeral-sessions-agentstack-session) /
  [`run`](#live-runs-agentstack-run) and reverts on exit.
- **zero-files** — `agentstack gateway connect` registers the gateway once per
  CLI (one write to each CLI's global config) and every **trusted** repo serves
  its own stack live; `agentstack_lease_open(profile)` fences one MCP connection
  to a profile without rendering native files. A machine-local
  `codemode/endpoint.json` coordinate may exist for the connection's duration —
  see [the zero-files gateway](#the-zero-files-gateway---auto-project--trust).

**Recommendation:** prefer the zero-file lease path for interactive work when the
CLI supports MCP; use static or clean-at-rest when the CLI must read native
skill/instruction files. Add `--sandbox --lockdown` when the agent process
itself needs isolation — a lease is a capability fence, not a sandbox. See
[the primitives and decision table](ARCHITECTURE.md#operating-model--choose-the-boundary-you-need).

Interactive `init` presents the three as an arrow-key choice **before any
write**, and the selection **forks** the run: **static** takes the render path
(preview → confirm → `apply --write` → activate skills → doctor);
**clean-at-rest** renders nothing and pins the lockfile, teaching the
`session start`/`session end` rhythm; **zero-files** renders nothing and offers
to register the gateway (`gateway connect --all --write`), then points at
`agentstack trust .` (which the wizard never runs for you — trust is human
consent). Bare `agentstack` reports the project's current mode on its `Mode`
line, derived from what is on disk.

The managed `.gitignore` block is anchored to **outcomes, not declarations**: an
entry exists only for a file agentstack actually wrote or still manages, so a
blocked run (unresolved secrets) hides nothing and a hand-maintained
`.mcp.json` / `CLAUDE.md` is never ignored. `apply` and `use` derive the block
from the same records, so alternating them never churns a committed `.gitignore`.

### Owned servers (`owner = "codex"`)

Some CLIs rewrite their own server entries — the Codex desktop app refreshes
`node_repl` env on every self-update. Marking a server `owner = "codex"` flips
the source of truth to the owner's on-disk config, so a blind `apply` never
downgrades the app's fresh values:

```toml
[servers.node_repl]
type = "stdio"
command = "node"
owner = "codex"   # codex's own config is the source of truth
```

Every plan (`apply`, `diff`, `doctor`, `use`) refreshes the definition from the
owner's config, fans the fresh values out to every *other* target, and reports
drift as "refresh + re-fan out: `apply --write`", never a downgrade. Per key, a
manifest value carrying a `${REF}` stays manifest-canonical (copying the resolved
disk literal back would leak the secret); everything else follows the owner's
disk. An `owner` id that isn't a registered adapter is a validation error.
**Trust interaction:** the auto-refresh changes the manifest digest, so trust
that was **valid** immediately before the rewrite is re-pinned to the new digest
(a machine-derived change from a config the owner already executes); trust
already broken or absent is left untouched — the refresh never mints trust.

## Agent-operable (`agentstack mcp`)

agentstack runs as an MCP server over stdio, so the agent itself can discover and
propose capabilities. The control-plane tools it advertises are below; the
**propose** group writes the **manifest only** (commit-safe `${REF}`s, nothing
executed — the agent proposes, a human runs `apply`):

| Tool(s) | What it does | More |
|---|---|---|
| `agentstack_search` | search catalog + your library for capabilities to install | [search](#search-across-providers) |
| `agentstack_list` | list the manifest's capabilities | — |
| `agentstack_doctor` | run the doctor checks (adds a `Trust (auto mode):` line) | [doctor](#doctor-shows-what-you-use) |
| `agentstack_explain` | explain a server/skill before relying on it | — |
| `agentstack_diff` | show manifest ↔ on-disk drift | [drift](#drift-adopt-or-apply) |
| `agentstack_add_from` | *propose:* add a catalog/registry server | [search](#search-across-providers) |
| `agentstack_add_server` | *propose:* add a server | [`adopt`/`add`](#adopt-and-add) |
| `agentstack_add_skill` | *propose:* add a skill | [`add skill`](#add-skill-source--install-from-any-skills-repo) |
| `agentstack_create_profile` | *propose:* create a profile | [profiles](#selective-skills-via-profiles) |
| `agentstack_list_loadable`, `agentstack_load` | the two-step skill loader (below) | this section |
| `agentstack_lease_open` / `_status` / `_close` / `_freeze` | MCP profile lease lifecycle (below) | [leases](#mcp-profile-leases-one-connection-one-capability-fence) |
| `agentstack_session_start` / `_end` / `_list` / `_freeze` | render/revert a native session (`start` takes a `profile`) | [sessions](#ephemeral-sessions-agentstack-session) |
| `tools_search`, `tools_bindings` | the compact proxied tool surface + code mode (below) | [code mode](#compact-proxied-surface--code-mode) |
| `tools_execute` | *experimental, sandbox builds only* — host the code-mode program | [below](#experimental-tools_execute) |

Register it once per CLI:

```bash
agentstack gateway connect claude-code codex   # dry-run: shows the config diff
agentstack gateway connect --all --write       # every installed harness
```

`gateway connect` writes one small entry — `agentstack mcp --auto-project` — into
the CLI's **global** MCP config (undo with `gateway disconnect`, verify with
`doctor`). Register it by hand like any stdio MCP server if you prefer:

```json
{ "mcpServers": { "agentstack": { "type": "stdio", "command": "agentstack", "args": ["mcp", "--auto-project"] } } }
```

### Transparent mode (`--transparent`)

```text
agentstack mcp --transparent
agentstack gateway connect --transparent
```

Two ways to expose the proxied surface:

- **Compact (default)**: `tools/list` advertises agentstack's control-plane
  tools only; upstream tools collapse behind `tools_search` (and code mode), so
  the agent's tool context stays bounded however many tools the upstreams expose.
  Requires the agent to use `tools_search` → call by namespaced name.
- **Transparent**: `tools/list` additionally advertises every policy-filtered
  upstream tool as `<server>__<tool>` — a drop-in MCP proxy any standard client
  consumes with zero agentstack knowledge. The firewall, trust gate, and audit
  log apply identically; the first listing pays upstream discovery.

In auto-project mode the gateway builds lazily, so transparent mode declares the
`listChanged` capability and sends `notifications/tools/list_changed` once the
(trust-gated) gateway comes up — clients re-fetch `tools/list` and see the
upstream tools without ever calling a control-plane tool first.

### The zero-files gateway (`--auto-project` + `trust`)

With `--auto-project`, one global registration serves **every** repo: at session
start the gateway discovers the active project — MCP client roots → cwd walk-up →
`$AGENTSTACK_MANIFEST_DIR` — and exposes that repo's stack. No `.mcp.json`, no
rendered files; a repo needs only its `.agentstack/agentstack.toml` (+ lock,
pinned with `agentstack lock`, which renders nothing).

Discovery is **trust-gated**, direnv-style: a freshly cloned repo gets
**control-plane tools only** — nothing spawned, contacted, or resolved — until
you review and trust it. Trust is pinned to the consent digest (concept:
[concepts.md](concepts.md#trust-and-the-consent-digest); scope:
[ENFORCEMENT.md](ENFORCEMENT.md#what-trusted-does-and-does-not-mean)); any edit —
a `git pull`, a re-lock — drops the repo back to control-plane-only until
re-trusted.

```bash
agentstack trust .          # preview what the manifest runs/contacts, then pin its digest
agentstack trust --list     # every trusted project + whether its manifest still matches
agentstack trust --revoke   # withdraw
```

`trust .` previews the **effective runtime surface** — inline servers and library
refs alike, each ref labeled pinned/unpinned/drifted. Explicit `--manifest-dir`
skips the gate (naming a directory is the consent).

Library-referenced server definitions live outside the digest, so the gateway
integrity-checks them at launch against the lock's pinned digests: a drifted
definition is refused (`agentstack lock` to fix), an unpinned ref is served with
a warning, a **missing** lockfile is the zero-lock workflow (all unpinned,
warned), and a lockfile that exists but can't be read fails **closed** — pins
unknowable, so library servers are refused and `trust` errors rather than review
an unverifiable surface.

The digest does not cover arbitrary files the manifest references: trusting a
repo whose server runs `python3 ./server.py` authorizes *that command*, and a
later edit to `server.py` does not re-gate (an edit to the manifest does). Review
referenced local scripts as part of `trust .`, the way you'd review a `.envrc`.
The gate is visible in-session: `tools_search` and
`agentstack_doctor` (a `Trust (auto mode):` line) name the exact `agentstack
trust <dir>` command when the project is untrusted or changed. agentstack's own
manual (the bundled `using-agentstack` skill) is always loadable here, even
untrusted — see
[field notes](design/reference-field-notes.md#zero-files-gateway-always-on-manual).

Honest limits: MCP servers, secrets, the tool firewall, the call audit log, and
skills-over-MCP create no per-project native artifacts. Native skill folders and
instruction files (`CLAUDE.md`/`AGENTS.md`) are read from disk by the CLIs
themselves and still need render mode (`apply`/`use`) — `gateway connect` prints
this per CLI.

### MCP profile leases: one connection, one capability fence

An MCP profile lease is process-local state owned by one `agentstack mcp`
process — the zero-file counterpart of a native `session start`, but with no
cleanup contract: a lease never renders harness config, creates a native skill
folder, or writes `sessions.json`, so close/process exit has nothing to restore.
The normal agent-side sequence (these are MCP tool calls, not shell commands):

```text
agentstack_lease_open({ "profile": "backend" })
agentstack_list_loadable({})
agentstack_load({ "name": "sql-review", "reason": "review this migration" })
agentstack_lease_status({})
agentstack_lease_close({})
```

While the lease is active: the live gateway exposes only servers from the
selected profile; `agentstack_list_loadable`/`agentstack_load` expose only that
profile's skills (plus the embedded `using-agentstack` manual), with an optional
case-insensitive `query` that filters **within** the fence; the first load of
each skill is recorded with its reason; and trust, lock/digest verification,
machine and project policy, and call auditing all continue to apply.
`agentstack_lease_freeze({ "name": "backend-observed" })` converts the leased
server list plus the skills actually loaded into a new manifest profile — a
manifest-only proposal; review the edit, then `agentstack lock`.

The control plane refuses to place a lease over an active native session, or a
native session over an active lease. A lease is deliberately invisible to
separate processes — read `agentstack_lease_status` from the same connection;
opening a different valid profile replaces the current lease. See
[`examples/mcp-profile-lease`](../examples/mcp-profile-lease/) for a runnable
lifecycle, and
[field notes](design/reference-field-notes.md#lease-survival-across-a-mid-connection-change)
for lease survival across a mid-connection manifest change.

### Compact proxied surface + code mode

`agentstack mcp` proxies the project's MCP servers (HTTP and stdio) behind two
stable tools rather than dumping every upstream tool into `tools/list`, so tool
context stays bounded however many servers you add. Stdio children spawn lazily
in their own process group, get `${REF}`s resolved into their env per session,
and are tree-killed when the session ends.

- **`tools_search({ query })`** — ranked discovery (deterministic substring,
  read-only): compact cards, one per matching tool with an entity ref; a second
  call `tools_search({ entity: "server__tool:tool" })` returns that tool's input
  schema and a ready-to-run code-mode snippet. (Distinct from `agentstack_search`,
  which searches the *catalog*.)
- **`tools_bindings`** — code mode: a typed, **secret-free** TypeScript client
  (`codemode.<server>.<tool>(input)`) plus a runtime shim, so the agent writes
  **one** small program calling several upstream tools and runs it with its own
  code/bash tool.

agentstack brokers the real MCP calls over a loopback, token-gated endpoint
(`${REF}`s resolved once per gateway session, never emitted into bindings or
logs); the agent's code runs in the **harness's** own sandbox, and the client is
fetched through the same MCP surface — nothing to install on disk.

### Experimental `tools_execute`

Sandbox-enabled release builds can also host the program themselves. The MCP tool
is advertised only when the **machine** manifest — not a repository — contains:

```toml
[experimental]
tools_execute = true

# Optional machine-owned defaults; each must remain within the hard ceiling.
[experimental.tools_execute_limits]
timeout_ms = 30000
max_calls = 40
max_output_bytes = 131072
```

Request schema:

```json
{
  "code": "import { tools, input } from 'agentstack:runtime'; export default await tools.github.get_issue({ number: input.number });",
  "allowTools": ["github__get_issue"],
  "input": { "number": 42 },
  "limits": { "timeoutMs": 15000, "maxCalls": 20, "maxOutputBytes": 65536 }
}
```

`code` and `allowTools` are required. Grants are exact namespaced tool names;
wildcards, empty grants, unknown tools, and extra request fields fail closed.
`input` is JSON and defaults to `null`. Request limits can only narrow the
machine defaults:

| Limit | Default | Hard ceiling |
|---|---:|---:|
| source | — | 256 KiB |
| input JSON | — | 1 MiB |
| timeout | 15 s | 60 s |
| gateway calls | 20 | 100 |
| stdout + stderr | 64 KiB | 256 KiB |
| result JSON | — | 1 MiB |
| granted tools | — | 100 |

The default export becomes the JSON result. Imports are offline (no npm install
or module fetch). The guest runs in a hardened Docker container (pinned Node 22
slim image, non-root, read-only root, all capabilities dropped, its only network
peer the egress sidecar) with **no host fallback** — missing trust, the sandbox
build feature, Docker, the pinned image, the sidecar, relay auth, recording, or
teardown returns a stable non-sensitive error. Full isolation accounting is
[ENFORCEMENT.md](ENFORCEMENT.md#experimental-tools_execute). This surface remains
experimental (see [field notes](design/reference-field-notes.md#tools_execute-review-status));
cancellation kills the
[whole process tree](design/reference-field-notes.md#tools_execute-cancellation).

### Governance (`[policy]`)

`require`/`forbid` capabilities and an `allowed_sources` glob allowlist (e.g.
`git:github.com/acme/*`), enforced by `doctor --ci`. Cross-source trust gating
for executable-intent skills and MCPs.

### MCP firewall (`[policy.tools]`)

Per-server tool rules enforced at the runtime gateway:
`github = ["get_*", "list_*", "!list_secrets"]` — plain globs allow, `!` denies;
any allow pattern makes the list an allowlist. A denied tool is **invisible**
(filtered from `tools_search` and code-mode bindings) and refused with the rule
named if called anyway. `doctor` errors on rules naming unknown servers;
`explain <server>` shows the effective policy.
`explain <name> --json` exposes the capability kind, provenance, safety signals,
secret-resolution metadata, and relevant project policy as structured fields;
the full human explanation remains available in its `text` field. The MCP
`agentstack_explain` tool returns this same structured object.

**Machine layer with deny precedence.** The machine manifest may carry its own
`[policy.tools]`, checked **before** the project's on every brokered call, so a
repo can never loosen a machine rule (effective policy = machine ∩ project — see
[concepts.md — machine policy](concepts.md#machine-manifest-and-machine-policy)).
A machine refusal names its layer in the error and audit log. Policy is keyed on
the **manifest-chosen server name**, so a machine rule for `github` constrains a
server *named* `github`, not the GitHub MCP server under any name; use the `"*"`
wildcard key for rules that must survive renaming:

```toml
# ~/.agentstack/agentstack.toml — applies to every project on this machine
[policy.tools]
"*" = ["!delete_*"]                   # rename-proof: no server may delete_*
github = ["get_*", "list_*"]          # servers NAMED github are read-only
```

The layer loads once per gateway launch (tightening mid-session takes effect next
session). Each valid load stores a secret-free, digest-labelled last-known-good
snapshot: a later malformed edit is enforced from that snapshot as **DEGRADED**;
a malformed first load or unusable snapshot makes protected activation
**BLOCKED** rather than silently falling back to project-only policy; a genuinely
absent machine manifest is the benign **UNCONFIGURED** state. `doctor`
distinguishes all three.

### Egress rules (`[policy.egress]`)

Per-server outbound-host rules, keyed and evaluated exactly like `[policy.tools]`
(globs allow, `!` denies, `"*"` rename-proof, machine layer checked first and no
repo can loosen it) — the subject is the destination host instead of a tool name.
A pattern may pin a port with a `:port` suffix (`api.example.com:443`); a bare
host means any port. The write/spawn-time check matches the host and defers the
port; the sandbox egress proxy enforces the exact CONNECT port at runtime.

```toml
[policy.egress]
"*" = ["!169.254.169.254"]            # rename-proof: no server reaches metadata
kibana = ["*.example.com:443"]        # this server: only TLS to our domain
```

An unconstrained server is allow-by-default; a constrained server whose
declared URL host can't be resolved statically (it hides behind a `${REF}`)
fails closed at write time.

### Secret access (`[policy.secrets]`)

Per-server allowlists over `${REF}` names, same keyed grammar again (globs, `!`
denies, `"*"` rename-proof). Enforced **fail-closed at both substitution
sites**: a ref outside a server's effective set never resolves for it — not
into a rendered config, not into a gateway upstream.

```toml
[policy.secrets]
github = ["GH_*"]                     # this server may only read GH_* refs
"*" = ["!AWS_*"]                      # no server resolves an AWS_* secret
```

### Filesystem scopes (`[policy.filesystem]`)

Manifest-global path-glob scopes (not per-server) in three lists. `write` gates
the `run --sandbox` mount, `read` is informational, and `deny` is a pure
blocklist unioned across the machine and project layers — a repo can add denies
but never drop the machine's — matched against the workspace-relative path, the
absolute path, **and** the bare file name. What each list actually enforces at
runtime (the read-only mount is coarse/all-or-nothing; `deny` runs through the
cooperative host guard) is [the enforcement matrix](ENFORCEMENT.md#the-matrix).

```toml
[policy.filesystem]
write = ["./**"]                      # sandbox: workspace mounts read-write
deny  = [".env*", "**/*.pem"]         # no tool call may touch these, ever
```

### Call log

Every tool call the gateway brokers (MCP proxy and code-mode alike) appends to
`~/.agentstack/audit/calls.jsonl` (`0600`, dir `0700`): timestamp, run id (under
`agentstack run`), server, tool, **keyed argument digest** (never values — keyed
with a per-machine secret so an exfiltrated log can't confirm guessed arguments),
outcome (`ok`/`error`/`denied`), latency, and a detail that is either the policy
rule (denials) or a **fixed error class** (failures) — upstream error text is
never written, so a malicious server can't inject content into the log.
Summarize with `agentstack report calls [--since <days>] [--json]`; add
`--tail <n>` to also list the last n individual calls (`--project <path>`
scopes everything to one project root). With `--json`, `--tail` adds an
`events` array of raw records — the stable feed external UIs consume; the
default JSON shape is unchanged without it. Best-effort local
**diagnostics** (logging can never fail a call; size-rotated at ~5 MB × 2), not
tamper-evident — input to `report calls`/`optimize`, not forensic evidence.

### Content scanning

Every `install` scans skill content for hidden Unicode (zero-width
characters, bidi overrides, tag characters) and prompt-injection heuristics.
Hidden-Unicode findings **block the install** (override with
`--allow-flagged`); injection heuristics warn. `doctor --deep` is the on-demand
content re-scan of everything materialized (skills and instruction files), and
`doctor --ci` fails on high-severity findings, so a poisoned skill can't slide
into CI unnoticed. Everyday `doctor` skips this scan (it reads every skill body);
`--json` emits the whole report machine-readably for t3code and automation.
Interactive `init` offers the deep scan as an explicit yes/no at its
closing doctor step, but only when the project actually has skills.

## Ephemeral sessions (`agentstack session`)

A session loads a profile **for now** and reverts it on exit — the clean-at-rest
mode's native primitive, so nothing generated persists between sessions.

```bash
agentstack session start backend          # render backend's profile (project scope)
agentstack session start backend --scope global
agentstack session list                   # active sessions on this machine
agentstack session end                    # revert this directory's session
agentstack session end --all              # revert every active session
agentstack session freeze --name backend-ci   # pin the resolved set into a new profile
```

`start` renders the profile's servers, skills, instructions, settings, and
hooks, records the write, and reverts it on `end` (or `end --all`). `freeze`
captures the session's resolved set — the profile's servers plus the skills
actually loaded — into a new profile (default `<profile>-frozen`) so CI can
replay it deterministically; review the manifest edit, then `agentstack lock`.
The same start/end lifecycle backs the MCP `agentstack_session_*` tools and
t3code's toolset picker.

### Execution posture

Every run is labelled with its **enforcement posture** — one of
`HOST / ADVISORY`, `HOST / PROTECTED`,
`SANDBOX / PROXIED · DIRECT ROUTE OPEN`, or
`LOCKDOWN / ENFORCED · NO DIRECT ROUTE` — saying how strongly the effective
policy is actually enforced at runtime, not merely declared. The sandbox and
lockdown labels are emitted with those suffixes; the suffix is the honest half
of the claim, so it is quoted here as printed. What each label guarantees is
[the enforcement matrix](ENFORCEMENT.md#the-matrix); `ENFORCED` is reserved for
lockdown, and even there the honest claim is *unapproved egress is blocked*, not
that exfiltration is impossible.

The label appears on the run banner, in `agentstack run --sandbox --plan`, and in
`agentstack report run <id>` (`report --json` carries the `posture` slug); a
sandbox run records it beside the flight-recorder log, and a `--locked` run
carries it in its `attempt_started` event. `agentstack doctor` also prints a
one-word **machine-policy summary** — `open`, `restrictive`, or `mixed` —
describing the machine policy's shape (`restrictive` means a `"*"` rule or a
`[policy.filesystem]` scope binds every server, not that the policy is tight).
Ready-to-use machine policies for common setups live in
[`examples/policies/`](../examples/policies/) (`compatible`, `developer`,
`locked-down`, `ci`).

### The Protected tier in detail (`run --locked`)

```text
agentstack run <cli> --locked
agentstack run <cli> --locked --plan   # walk the gate sequence read-only
```

A locked run is a fail-closed **pre-launch gate sequence plus a frozen
capability surface** — every decision recorded, nothing re-derived mid-run:

1. **Gates, in order** (each records a `gate_decision` event; the first
   refusal stops the launch): enforced **trust** (explicit consent, current
   digest), strict **lock verification** including the D3 executable pins
   (a one-byte edit to a pinned local server executable refuses the run) and
   the `rendered-verify` re-check of delivered extension copies, then
   **policy admission** (every declared capability must fit under the machine
   ceiling — an unclassifiable host, e.g. a `${REF}` in a URL's host portion,
   refuses because it *cannot* be checked).
2. **Grant freeze.** The run's entire authority — compiled machine ∩ project
   ruleset, the resolved `${REF}`-only server set, project root + consent
   digest, the fencing profile — is frozen into an `AuthorityGrant` whose
   canonical digest is printed and recorded (`grant_frozen`).
3. **Bridge handoff.** A reviewed projection of the grant (never argv, never
   secret values) is sealed under a machine-local HMAC key into the run's
   private dir, and the **launch-scoped** project MCP config points the
   harness at `agentstack mcp --grant <artifact>`. The bridge consumes the
   artifact **verbatim** and fails closed (serving nothing, loudly) on a failed
   MAC, schema/version skew, a consent digest that no longer matches (any
   post-freeze manifest edit), lost trust, or a machine ceiling that changed
   since freeze. It never re-derives authority from disk.
4. **Frozen control plane.** Under `--grant`, control-plane tools that would
   swap the surface or mutate state mid-run — lease open/close/freeze,
   `session_start`, `session_end`/`freeze`, `add_skill`/`add_server`/`add_from`,
   `create_profile` — are refused for the run's duration. Read-only
   discovery and trust-gated skill loading still answer.
5. **`--profile <name>` is a fence**, not a session: gates, grant, artifact,
   and bridge all see only that profile's server subset; no native session
   state is applied or reverted.
6. **Hygiene.** The original project MCP config is parked in the run's
   private dir (never left in the repo) and restored byte-identical; a
   sentinel makes overlapping locked runs refuse instead of stacking; a
   crash leaves the more restrictive state.

`run --locked --plan` walks the whole sequence read-only, printing every decision
the live path would (plus the grant digest a live run would freeze) and mutating
nothing. What is and isn't claimed at this tier (pre-launch gating on the HOST
tier, not kernel isolation) is [ENFORCEMENT.md — the locked run's frozen
grant](ENFORCEMENT.md#the-locked-runs-frozen-grant-run---locked); the asserted
walkthrough is [`examples/projects/locked-run/`](../examples/projects/locked-run/)
and the full contract is
the [locked-run enforcement contract](ENFORCEMENT.md#the-locked-runs-frozen-grant-run---locked).

## The central library

One managed home — `~/.agentstack/lib/` — that projects reference **by name**
instead of copying files between repos.

### Layout and name resolution

Skill dirs (`lib/skills/`) and MCP server definitions (`lib/servers/*.toml`)
are indexed in `library.toml`. A profile's `skills = ["sql-review"]` /
`servers = ["kibana"]` resolve from there; an inline `[skills.*]` /
`[servers.*]` table always overrides the library. Provider folders are never
owned — only their skills and MCP entries are managed. The runtime gateway
resolves server name refs through the same inline-first/central-library path as
rendering, but where rendering hard-fails a run on a broken ref, the gateway
skips just that server (with a stderr report) and keeps the rest up.

### Pinning and provenance

Name refs are pinned by digest in `agentstack.lock` — servers pin the
**definition** digest only; secret values stay `${REF}` and resolve at
render/gateway time, never in the library or the lock. Native extensions pin
differently: a `[[extension]]` entry records `name`, `target`, and a `checksum`
from the **strict** integrity-root digest over the whole source tree, so
retargeting a byte-identical extension is drift and a one-byte source edit
re-gates trust (see [Native extensions](#native-extensions)). `doctor`/`explain`
flag drift and show each item's origin. Profile resolution is offline by default
(dry-run `use`, `doctor`, `explain` never fetch); `use --write` fetches
git-backed skills when activation needs them. `agentstack lock [--profile <name>]`
pins every profile's name refs **without** rendering — the lock-only path for
clean-at-rest repos. The lockfile is part of a project's consent surface, so when
a currently-trusted project's pins change, `lock` warns that its trust is now
stale and must be re-granted with `agentstack trust .` — new pins are new consent.

### Adding capabilities

```text
agentstack lib add ./<dir> --name <name>               # copy a local skill in
agentstack lib add owner/repo --skill <name>           # from any skills repo
agentstack lib add owner/repo --subpath <dir>          # from a repo subdirectory
agentstack lib add-server <name>                        # reusable server definition
agentstack lib new <name>                               # scaffold a new skill
```

`lib add ./<dir>` **copies** the source into `lib/skills/<name>` — the library
copy is canonical from then on (source edits have no effect), provenance records
the original path, and a temp-dir source gets a dangling-path warning. `lib add
owner/repo --subpath <dir>` (any git URL, `--skill <name>` selecting from a
multi-skill repo) installs from a repo subdirectory, staging the fetch so a dry
run never touches the store, recording truthful `git:<url>@<rev>#<dir>`
provenance. `lib add-server` stores a reusable definition with its `${REF}`s
intact. `lib new <name>` scaffolds `./<name>/SKILL.md` from the house template —
edit it, then register it with `agentstack add skill ./<name> --write` (this
project) or `lib add ./<name> --write` (every project). Every `lib add` runs the same hidden-unicode /
prompt-injection scan as `install`/`doctor --deep` before the copy becomes
canonical (high findings block unless `--allow-flagged`) and warns above ~10 MiB.

### Syncing across machines (`lib sync`)

```text
agentstack lib sync [--status]
agentstack lib sync --allow-secrets   # override the fail-closed secret gate
```

`lib sync` versions the library as a git repo (init/clone/pull/commit/push,
`--status` to preview); the content-store cache stays local. Its promise —
**secrets never travel** — is enforced by a gate that fails closed:

- Before any commit, every `lib/servers/*.toml` is scanned for literal
  (non-`${REF}`) secrets across **every field a credential could hide in** —
  headers, env, the `url` (userinfo passwords, secretish query params), `args`.
- A server file that can't be read or parsed **blocks the sync** rather than
  slipping through unscanned, naming any secret-looking line.
- Before pushing, the **outgoing commits** are scanned too, so a secret committed
  once and later edited out can't ride along in history (the message names the
  commit and file).
- `--allow-secrets` overrides all three, deliberately and loudly.

Pulled content passes the same supply-chain scan as `lib add` — warn-only (a
completed pull can't be blocked without stranding the tree) — and incrementally:
a no-op pull scans nothing, a real pull scans only the skills it changed.

### The two mental models

Three ways a skill or server reaches a profile; the manifest syntax alone picks
which:

- **By-name library reference** — `skills = ["greet"]` / `servers = ["kibana"]`
  with **no** matching `[skills.greet]` / `[servers.kibana]` table. Resolved
  fresh from `~/.agentstack/lib` on every lock, pinned there by `checksum`
  (skills) or definition digest (servers); nothing is copied into the repo. The
  cross-repo default.
- **Vendored pack copy** — installed with `add from git:<host>/<repo>`. Members
  are copied into the project and digest-pinned, and a `[packs.<name>]` ledger
  records `source`/`version`/`rev` so `lock --upgrade` re-resolves them — a
  self-contained snapshot that versions as one unit (see
  [Git-hosted versioned packs](#git-hosted-versioned-packs)).
- **Inline manifest** — a `[skills.greet]` / `[servers.*]` table with its own
  `path`/`git`/`command`. Lives in the repo and **always overrides** a same-named
  library reference.

The trap: a `[skills.greet]` block with **no** source is read as an inline skill
*missing* its source — it errors, it does not fall back to a library skill of the
same name. Drop the block and list `greet` in `skills = […]` to reference the
library copy; keep the block only for a distinct inline skill. `explain` prints
each capability's model on its `Model` line.

## Capabilities

The kinds of thing a profile can carry — skills, servers, instructions,
settings, hooks, extensions, packs — and the commands that add, search, and
account for them. It is a menu; jump to the capability you need.

### Package manager

Skills declare a source (`path` or `git`); the package manager fetches them into
`~/.agentstack/store/`, writes a SHA-256 `agentstack.lock`, and reproduces it
exactly under `--locked`.

```text
agentstack install            # fetch skill sources, write the lockfile
agentstack install --locked   # reproducible, CI-safe
agentstack lock --update      # re-resolve git skills
agentstack remove <name>      # drop a capability from manifest + lock
```

Profile-aware: skills a profile references by name (library-resolved, no inline
`[skills.*]`) keep their lock pins through the reconcile pass — pin or refresh
with `agentstack lock`. Content digests always hash current bytes; see
[field notes](design/reference-field-notes.md#orphaned-digest-cache) for the
harmless orphaned `digest-cache.json` older versions may leave.

### Git-hosted versioned packs

Any repo with a `pack.toml` installs as a version-pinned pack from any git host;
`lock --upgrade` resolves the newest tag (never downgrades), previews the member
diff, and re-pins.

```text
agentstack add from git:<host>/<repo>[@<tag>][#subdir]
agentstack lock --upgrade <pack> --yes --write
agentstack lib pack-init
```

No tag → the newest version-shaped tag; a repo with no version tags is an error,
never a floating install. The ledger records `source`/`version`/`rev`; extracted
skills are digest-pinned so `install --locked` reproduces. `[policy]
allowed_sources` is enforced **before** any fetch, and the clone passes the
install scan gate. `lib pack-init` scaffolds a publishable pack. (Semver ranges
and transitive pack dependencies are deliberately out of v1.)

### `add skill <source>` — install from any skills repo

```text
agentstack add skill anthropics/skills --skill pdf --write   # owner/repo (GitHub)
agentstack add skill anthropics/skills --list                # discover, inspect only
agentstack add skill https://github.com/o/r/tree/main/skills/pdf
agentstack add skill git@github.com:o/r.git --rev v1.2 --skill pdf
agentstack add skill ./my-skill --name code-review
```

Sources: `owner/repo` (always GitHub — a bare shorthand never touches your
filesystem), full GitHub/GitLab URLs including `/tree/<ref>/<subpath>`, generic
git remotes (`git@…`, `ssh://`, `file://`, `*.git`), or a spelled local path
(`./dir`, `../dir`, absolute, `~/dir`). `owner/repo@skill` and `#ref` alias
`--skill`/`--rev` (a flag disagreeing with its alias is an error);
credential-bearing URLs are rejected — use a git credential helper. Discovery
scans the ecosystem's conventional locations (see
[field notes](design/reference-field-notes.md#add-skill-discovery-and-staging)).

Everything runs preview-first: the dry run fetches into transient staging
(`~/.agentstack/stage/…`, removed on exit) and never touches the manifest, lock,
or store. `--write` promotes the staged clone into the store (rename-only — the
scanned bytes land verbatim), writes one `[skills.<name>]` entry per selected
skill, and records the lock pins (exact commit + content checksum). Content is
scan-gated before anything is offered; high-severity findings block unless
`--allow-flagged`. The manifest `rev` records your branch/tag intent; the lock
commit is authoritative until `agentstack lock --update` relocks.

**Activation is part of the same write, mode-aware** — detected from pre-write
disk state:

| Mode | `--write` does |
|---|---|
| static, unambiguous profile (none declared, or exactly one) | manifest + lock + **materialize** into the default targets (project scope for a project manifest), per-target `✓`/`⚠`/`✗` reporting |
| static, several profiles | manifest + lock; activate with `agentstack use <profile> --write` (profile fencing wins — which is live is unknowable) |
| clean-at-rest | manifest + lock; the next `agentstack session start <profile>` picks it up (an active session won't) |
| zero-files | manifest + lock, the current lease untouched; **trust re-gates on the edit** — run `agentstack trust .`, or the gateway serves control-plane-only next connection |

Profile membership: no declared profiles → the implicit default; exactly one →
added automatically; several → `--profile` (or an interactive pick). Naming a
nonexistent profile is an error, never a silent create.

### `try` — run a skill without installing anything

```text
agentstack try anthropics/skills --skill pdf | claude
```

Stages and scans exactly like `add skill`, materializes the one selected
skill under `~/.agentstack/try/`, and prints a wrapper prompt on stdout —
pipe it into any agent CLI. Nothing touches the manifest, lock, library, or
configs; status goes to stderr with a provenance line naming what loaded.
Skills containing symlinks are refused (the ephemeral copy must not
dereference one), and `doctor` names leftover try dirs with the remedy.

### Instruction files

Compile shared + harness-specific `[instructions.*]` fragments into each CLI's
`CLAUDE.md` / `AGENTS.md`, inside a managed `<!-- agentstack -->` region that
preserves surrounding hand-written prose.

```text
agentstack instructions --write   # compile [instructions.*] into CLAUDE.md / AGENTS.md
```

Dry-run by default. Part of the mainstream lifecycle: `apply` (so `init` too)
compiles the region alongside servers/settings/hooks behind the same `--write`
gate — a manifest with no `[instructions.*]` never touches a region another layer
owns — and `doctor` flags a stale managed region (warn ↳ `instructions --write`)
or a missing fragment source (error, gates `--ci`). Installing a pack's house
rules prints the exact compile command.

### The machine layer

The machine manifest is the personal, cross-project layer (concept:
[concepts.md](concepts.md#machine-manifest-and-machine-policy)).

```text
agentstack init --global                            # seed ~/.agentstack/agentstack.toml + instructions/
agentstack instructions --manifest-dir ~ --write    # compile personal fragments
```

`init --global` seeds `~/.agentstack/agentstack.toml`, an `instructions/` dir,
and the machine `[guard]` + `[policy.filesystem]` deny defaults (the same list
`guard install` writes), and offers to install the host guard into detected CLIs.
Inherited fragments compile at **global scope only** (personal rules never land
in a repo's committed `CLAUDE.md`); a same-named project fragment wins.
Provenance is visible everywhere: `instructions` labels inherited fragments
`(machine)`, `doctor` counts them, `explain <fragment>` names the layer. The
bundled **agentstack house rules** fragment (`[instructions.agentstack]`) teaches
every agent the manifest-first workflow and is offered opt-in by `init --global`
and the `init` wizard. The zero-files gateway never treats the machine layer as a
project: it cannot be `trust`ed or activated by `mcp --auto-project`.

### Native settings

Manage each CLI's own settings file (Claude Code `~/.claude/settings.json`, Codex
`config.toml`) from one `[settings.<cli>]` block; `apply` merges only the keys you
declare, resolves `${REF}`s, preserves hand-set keys, and prunes keys that leave
the manifest.

```text
agentstack settings set <target> <key> <value>
agentstack settings unset <target> <key>
```

Dry-run by default; `--write` applies.

### Lifecycle hooks

Declare `[hooks.*]` once (event + optional matcher + command) and `apply` renders
them into each harness's native hooks config (Claude Code `settings.json`, Codex
`config.toml`), resolving secrets and pruning hooks that leave the manifest.

```text
[hooks.<name>]             # event + optional matcher + command
agentstack apply --write   # render them into each harness's native hooks config
```

`doctor` verifies the rendered hooks.

### Native extensions

`[extensions.<name>]` manages a harness's native executable add-ons — pi's
TypeScript extensions, OpenCode's JS plugins. It is the **highest-risk**
capability agentstack delivers: the code runs inside the harness process at full
user permission, and agentstack governs only *pre-delivery* (provenance and
content binding), never runtime — see
[ENFORCEMENT.md](ENFORCEMENT.md#native-extensions).

```text
[extensions.<name>]                                                # path/git + exactly one target
agentstack lib add-extension <name> --target <adapter> --path <dir>
agentstack lock                                                    # pin (strict integrity-root digest)
```

```toml
[extensions.checkpoint]
description = "Git checkpoint on every agent turn"
path = "./extensions/checkpoint"   # or: git = "…", rev = "…", subpath = "…"
target = "pi"                      # exactly one adapter id
```

- **Source** — a local `path` (manifest-anchored), a `git` source (`subpath`
  required, `rev` optional), or a bare central-library name (`lib add-extension …
  --path <dir>` or `--git <url> --subpath <dir>`). A declaration with none of
  these is a validation error.
- **`target` is singular** — one CLI's API; no `targets` list, no `"*"` fan-out.
  An unknown target, or `"*"`, is a validation error.
- **Reserved names** — anything beginning `agentstack-guard` is rejected (those
  belong to the host guard).
- **Strict pinning** — each extension gets a `[[extension]]` lock entry
  (`name`/`target`/`checksum`) via the strict integrity-root digest (symlinks
  rejected, `.git` included). An unpinned extension blocks; `agentstack lock` pins.

`apply` renders by **copying** (never symlinking) the lock-pinned source into the
target's extension directory, tracked in a per-directory ownership ledger so a
re-render prunes exactly what agentstack placed. An untrusted or drifted project
renders **zero** extension bytes. Two adapters render today — **pi**
(`~/.pi/agent/extensions`, or `.pi/extensions` at project scope) and **OpenCode**
(`~/.config/opencode/plugins`, global only); any other target validates but
**warns and does not render**. Under `run --locked`, a `rendered-verify` gate
re-checks each delivered copy against its lock pin before launch.

### `report usage` (usage analytics)

Local usage analytics: activation counts + per-capability footprint (which
target/scope slots it's live in) + **context cost** — flagging high-cost,
never-activated servers with the exact `remove` command.

```text
agentstack report usage
agentstack report usage --live   # measure each server's tools/list token footprint
```

`report usage --live` measures each server's `tools/list` token footprint through
the gateway (HTTP + stdio) and caches it (`~/.agentstack/footprint.json`); `report
usage` and `explain` then show that cost offline.

### Wire proxy (`proxy`)

Where `report usage --live` gives a **static** estimate of a server's
`tools/list` cost, the wire proxy gives **runtime ground truth**: what the
`tools` block actually costs, in input tokens, on every real turn your harness
sends.

```bash
export ANTHROPIC_BASE_URL=http://127.0.0.1:8787
agentstack proxy             # loopback relay (default 127.0.0.1:8787; --port/--upstream)
# …drive Claude Code (or any Anthropic-API harness) as usual…
agentstack report wire       # --json for the raw aggregate
```

`agentstack proxy` relays every request **verbatim** to the Anthropic API; point
the harness's base URL at it and use it normally. Records append to
`~/.agentstack/proxy/requests.jsonl` (size-rotated, same contract as the call
log) and are **content-free by construction**: counts, capability/tool names,
token estimates, the model id, best-effort usage numbers — never prompt/message
bodies, tool arguments, secrets, or header values. `report wire` aggregates the
log into a ranked, per-capability table — `tools` (typical per-turn count),
`avg tokens/turn`, `calls`, and a loaded-vs-called `hint` (`keep` / `drop / lazy`
/ `watch`) — over the same servers and profiles agentstack manages, closing the
loop with the static `footprint` / `report usage` / `doctor` lenses. Bucketing
and SSE internals: [field notes](design/reference-field-notes.md#wire-proxy-internals).

### `export` / `import`

```text
agentstack export --output <file> [--secrets] [--passphrase <p>]
agentstack import <file> [--passphrase <p>]
```
An age-encrypted archive (manifest + lock + optionally secrets) for moving a
setup to a new machine; passphrase-protected.

## Optimize (`agentstack optimize`)

Turns the signals agentstack already collects — activation counts, the gateway
call audit log, per-server context costs (`report usage --live`), the trust
ledger — into concrete recommendations: inert servers to remove, `[policy.tools]`
allowlists to narrow high-cost servers, denied and erroring calls to review,
stale trust grants to refresh or revoke.

```bash
agentstack optimize              # read-only report
agentstack optimize --json       # machine-readable
agentstack optimize --since 30   # only the last 30 days of runtime evidence
agentstack optimize --write      # apply ONLY the safe class: provably-inert
                                 # manifest entries (no calls, no activations,
                                 # no profile, not rendered anywhere, ≥14d of
                                 # history) and trust grants for deleted dirs
```

The contract: **every recommendation carries its evidence** (numbers, window,
data source), **the exact command or TOML** to act on it, and **why it is safe
or why it needs review**. One stated limit: the audit log only sees
gateway-brokered calls — a server rendered into a native config is called
directly by the harness, so such servers are never auto-removed on "no calls"
evidence alone.

## t3code integration

t3code is AgentStack’s primary graphical surface. It consumes the same read-only JSON reports and invokes a closed set of CLI-owned actions; see [Use with t3code](howto/use-with-t3code.md). The AgentStack CLI remains the complete standalone and automation interface.

## Part III — Full command reference

The generated command tree and the one-glance census, unchanged. Everything above has a prose home; this is the exhaustive index behind it — the exact verb, flag, and subcommand surface, regenerated from the CLI itself.

## All commands

The full command surface, generated from the CLI's own command tree by
`agentstack self docs --write` (CI fails if this list goes stale). Bare
`agentstack --help` deliberately shows only the **9 everyday commands** — `init`,
`status`, `add`, `search`, `apply`, `doctor`, `use`, `run`, `trust`. The other 29
are hidden from `--help` as progressive disclosure but are **fully supported**,
each with its own `--help`; **hidden does not mean deprecated or unsupported**.
`agentstack --help --all` prints the entire tree, and each line below marks the
hidden ones: a hidden top-level command carries `_(hidden)_`, and a hidden
subcommand carries a trailing `*` (e.g. `guard`'s `check*`). Reach for it when
you need the exact verb, flag, or subcommand.

<!-- agentstack:generated commands -->
- **`setup`** _(hidden)_ — Hidden alias of interactive `init` — same guided wizard, older name — flags `--target/--profile/--scope`
- **`init`** — Set up everything in one command: detect, import, choose, apply, verify — flags `--global/--force/--dry-run/--plan/--secrets/--no-keychain/--yes/--consented-plan`
- **`status`** — Where this project stands, on one screen: detected CLIs, manifest, trust, secrets, and the one next step
- **`add`** — Add a server or skill to the manifest — subcommands `from/server/skill`
- **`set`** _(hidden)_ — Create or update a manifest entry in place (idempotent `add`) — subcommands `server`
- **`search`** — Search the capability catalog (and mark what's already added)
- **`apply`** — Render the manifest into each target's native config — flags `--target/--profile/--dry-run/--write/--scope/--allow-unresolved/--prune-foreign/--no-gitignore`
- **`instructions`** _(hidden)_ — Compile [instructions.*] into each CLI's CLAUDE.md / AGENTS.md — flags `--target/--scope/--write`
- **`doctor`** — Verify everything is wired up: adapters, secrets, drift, skills, per-CLI details — flags `--ci/--live/--fix/--deep/--all/--json`
- **`remove`** _(hidden)_ — Remove a server or skill from the manifest (and lockfile) — flags `--write`
- **`install`** _(hidden)_ — Fetch skill sources into the store and write the lockfile — flags `--locked/--allow-flagged`
- **`lock`** _(hidden)_ — Resolve each profile's skill + server refs and pin `agentstack.lock` — flags `--profile/--update/--upgrade/--all/--with-instructions/--yes/--write`
- **`try`** _(hidden)_ — Try a skill without installing anything: stage, scan, and emit a wrapper prompt on stdout for piping into any agent CLI — flags `--skill/--rev/--subpath/--allow-flagged`
- **`lib`** _(hidden)_ — Manage the central capability library — subcommands `new/add/add-server/add-extension/add-hook/list/remove/remove-server/remove-extension/remove-hook/sync/pack-init`
- **`adopt`** _(hidden)_ — Keep a hand-edit: pull drifted native config back into the manifest — flags `--target/--scope/--write/--no-keychain`
- **`use`** — Activate a profile: render its servers + materialize its skills — flags `--target/--scope/--write/--allow-unresolved/--prune-foreign/--no-gitignore/--list/--json`
- **`session`** _(hidden)_ — Manage ephemeral sessions: load a profile for now, then revert it — subcommands `start/end/list/freeze`
- **`run`** — Launch an agent CLI as a tracked run — flags `--locked/--prompt/--profile/--scope/--keep/--sandbox/--lockdown/--plan`
- **`kill`** _(hidden)_ — Kill a tracked run by id (and revert its profile if it owned one) — flags `--force`
- **`shim`** _(hidden)_ — Exec-through launcher shim for external supervisors (e.g. t3code) — subcommands `make/exec*`
- **`workflow`** _(hidden)_ — Governed workflows (preview): run a pinned workflow under full admission — trust gate, strict lock verification, machine-capped ceilings — with every `agent()` call becoming a locked child run. Hidden while the lane is preview (Stage E adds `workflow report`) — subcommands `run/report/list/runs`
- **`report`** _(hidden)_ — Every "what happened" view in one place — subcommands `run/runs/usage/calls/wire`
- **`sign`** _(hidden)_ — Sign this project's agentstack.lock with a fresh ed25519 key (writes a detached agentstack.lock.sig, prints the public key to publish) — flags `--print-key-only`
- **`verify`** _(hidden)_ — Verify agentstack.lock against a published ed25519 public key and its detached signature — flags `--pubkey/--signature`
- **`guard`** _(hidden)_ — Machine-level destructive-command guard — subcommands `check*/test/install/uninstall/status`
- **`gateway`** _(hidden)_ — The zero-files gateway: register it once per CLI (`connect`) and every trusted repo brings its own servers through `agentstack mcp --auto-project` with no per-project files — subcommands `connect/disconnect`
- **`trust`** — Trust a project's manifest for the zero-files gateway (direnv-style) — flags `--list/--revoke/--yes/--consented-digest/--preview`
- **`mcp`** _(hidden)_ — Run agentstack as an MCP server over stdio (for an agent to call) — flags `--auto-project/--transparent`
- **`diff`** _(hidden)_ — Show drift between the manifest and the on-disk configs — flags `--target/--profile/--scope/--json`
- **`explain`** _(hidden)_ — Explain a server, skill, or instruction before you rely on it — flags `--json`
- **`optimize`** _(hidden)_ — Turn agentstack's collected signals into concrete recommendations — flags `--json/--write/--since`
- **`proxy`** _(hidden)_ — Start the wire relay: a localhost proxy in front of the Anthropic API — flags `--port/--upstream`
- **`restore`** _(hidden)_ — Undo a recorded write: revert an apply/use/session history entry (servers, settings, hooks, instructions), or restore one adapter's config from its single-slot backup — flags `--last/--scope/--write/--json`
- **`secret`** _(hidden)_ — Manage secrets in the OS keychain — subcommands `set/get/rm/list`
- **`settings`** _(hidden)_ — Edit a target's native `[settings.<target>]` entries — subcommands `set/unset`
- **`export`** _(hidden)_ — Export the manifest (+ lock, + optionally secrets) as an encrypted bundle — flags `--output/--secrets/--passphrase`
- **`import`** _(hidden)_ — Import an encrypted bundle on a new machine — flags `--force/--no-keychain/--passphrase`
- **`adapters`** _(hidden)_ — Inspect the available CLI adapters — subcommands `list/show/validate`
- **`self`** _(hidden)_ — Manage this binary's own install: `self link` puts a stable `agentstack` on PATH (a symlink, no installer needed); `self which` shows which binary a bare `agentstack` runs and flags stale links — subcommands `link/which/docs*`
<!-- agentstack:end -->

## Everything shipped so far

A single-glance census of every capability that exists today — the fastest way
to confirm a feature is real before you go hunting for its section above.

13 adapters · `init`/`add`/`apply`/`diff`/`use`/`instructions`/`adopt` ·
package manager (`install`/`lock --update`/`remove` + lockfile) · central capability
library (`lib` skills + servers referenced by name, digest-pinned in the lock,
drift in `doctor`/`explain`) · secrets (keychain +
varlock) · scopes (global/project) · `doctor` (`--live`/`--fix`/`--ci`/`--deep`) ·
content scanning on install + `doctor --deep` · official MCP Registry provider +
`search`/`add from` · `[policy]` trust gate · native per-CLI settings
(`[settings.*]` → settings.json) · native extensions (`[extensions.*]` →
content-pinned harness add-ons, re-verified at `run --locked`) · atomic writes + backups ·
`export`/`import` · portable lifecycle hooks · agent-operable `mcp` server ·
t3code integration contracts · live runs (`run`/`report runs`/`kill`) ·
GitHub Action trust gate ·
nightly adapter-conformance CI · zero-files gateway (`gateway connect` + `mcp
--auto-project` + digest-pinned `trust`) · `optimize` (evidence-backed
recommendations from usage/audit/cost signals, safe-class `--write`) ·
fail-closed `lib sync` secret gate (all server fields + outgoing history) ·
machine-level destructive-command `guard` · Docker `run --sandbox` and
no-direct-route `--lockdown` with compiled egress/filesystem policy · per-run
`report` (lifecycle, limits, egress, tool calls, secret refs) · detached
`sign`/`verify` · experimental frozen-plan `tools_execute`.
