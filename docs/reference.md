# Feature reference

The complete, implemented-and-tested feature inventory. The
[README](../README.md) is the tour; this is the map.

**Contents:** [Core engine](#core-engine) ·
[Where rendered files live](#where-rendered-files-live-three-modes) ·
[Secrets and trust](#secrets-and-trust) ·
[The central library](#the-central-library) ·
[Capabilities](#capabilities) ·
[Drift](#drift-adopt-or-apply) ·
[Plugin recipes](#managed-plugin-recipes) · [Dashboard](#dashboard) ·
[Live runs](#live-runs-agentstack-run) ·
[Agent-operable](#agent-operable-agentstack-mcp) ·
[Optimize](#optimize-agentstack-optimize) · [All commands](#all-commands)

## Core engine

### The manifest

Layered load: the preferred `.agentstack/agentstack.toml` plus a gitignored
`agentstack.local.toml` overlay (legacy root `agentstack.toml` remains
supported), with static validation before anything renders. The `version`
field is checked on load — a manifest (or lockfile, or library index)
written by a newer schema than the build supports errors with an "upgrade
agentstack" message instead of being misread silently.

### Data-driven adapters

Claude Code, Claude Desktop, Codex, Cursor, Windsurf, Gemini CLI, VS Code,
GitHub Copilot CLI, OpenCode, Antigravity, Junie, Kiro, and Pi — one YAML
descriptor each, embedded in the binary, with user overrides and additions
loaded from `~/.agentstack/adapters/`. Each CLI's quirks are encoded in data,
not code: Claude's `type:"http"`, Codex's `http_headers` subtable, Windsurf's
and Antigravity's `serverUrl`, Gemini's `httpUrl`, VS Code's `servers` key,
OpenCode's combined-`command` array and `mcp` key, Copilot CLI's
`type:"local"` stdio tag. Per-OS config paths (`{config}/…`) resolve correctly
on macOS/Linux/Windows.

### Rendering and merging

A generic renderer applies field renames, transport tags, header nesting, and
secret substitution — and its **inverse** powers `init`, which imports
existing configs back into a manifest. Mergers are non-destructive: JSON
splices only the managed section (untouched bytes, including floats, preserved
exactly); TOML uses `toml_edit` to keep comments and formatting.

Native keys with no transport-neutral equivalent — Codex's
`startup_timeout_sec`, say — live under a per-target `extra` table and are
passed through verbatim by that one adapter (string values still get `${REF}`
substitution):

```toml
[servers.miro.extra.codex]
startup_timeout_sec = 20   # npx cold-cache fetch must not block CLI startup
```

`init` and `adopt` lift unknown config keys back into `extra.<adapter>`, so a
hand-tuned native key round-trips instead of being dropped by the next
`apply --write`. A typo'd adapter id under `extra.` is a validation error, not
a silent no-op.

A stdio server can declare a `cwd` — the working directory it launches from —
for servers that only start correctly when spawned from their own directory
(e.g. relative dynamic `import()`s that resolve against `process.cwd()`):

```toml
[servers.tldraw]
type = "stdio"
command = "node"
args = ["dist/index.js"]
cwd = "/path/to/tldraw-mcp-server"   # supports ${REF}/path expansion
```

It renders to each adapter's native working-directory key (`cwd` on Codex,
Cursor, Gemini CLI, OpenCode, and Copilot CLI) and round-trips through
`init`/`adopt`. Adapters whose config has no such key (Claude Code, VS Code,
Windsurf, Kiro, Claude Desktop, …) render the server without it and `apply`
prints a warning — the server may need a shell wrapper that `cd`s first on
those harnesses.

The gateway honors `cwd` too: stdio upstreams are spawned in the server's
`cwd` (relative paths anchor at the project root), defaulting to the project
root itself — never in whatever directory the client happened to launch
`agentstack mcp` from.

A server can also scope which targets it renders to at all, mirroring
instructions and hooks: `[servers.X] targets = ["claude-code"]` fans out to
that adapter only, the `["*"]` default means every target, and an explicit
`targets = []` opts out of the direct fan-out entirely (how adopted plugin
servers are stored — the owning plugin delivers them instead). `apply`,
`diff`, and `doctor` drift all share the one filter, and a typo'd id in
`targets` is a validation error.

### Owned servers (`owner = "codex"`)

Some harness apps rewrite their own server entries — the Codex desktop app,
for one, refreshes `node_repl` env values on every self-update. Left alone,
the manifest goes stale, `doctor` flags drift, and a blind `apply --write`
would *downgrade* the app's fresh values. Marking the server as owned flips
the source of truth:

```toml
[servers.node_repl]
type = "stdio"
command = "node"
owner = "codex"   # codex's own config is the source of truth
```

Every plan (`apply`, `diff`, `doctor`, `use`) refreshes the definition from
the owner's on-disk config before rendering, so the owner's config is never
reverted, and every *other* target fans out with the fresh values. Drift on an
owned server reads "refresh the manifest + re-fan out: `apply --write`", never
a proposed downgrade; `apply --write` rewrites the stale `[servers.X]` table
in whichever manifest layer declares it (local overlay first).

Per key: a manifest value carrying a `${REF}` stays manifest-canonical — the
disk literal is just that ref's resolved form, and copying it back would leak
the secret into the manifest. Everything else follows the owner's disk,
including keys the owner app adds or removes. `targets`, `owner`, and other
adapters' `extra.*` are manifest bookkeeping and always kept. An `owner` id
that isn't a registered adapter is a validation error.

Trust interaction: the auto-refresh rewrites the manifest, which changes its
trust digest. Trust that was **valid** immediately before the rewrite is
re-pinned to the new digest (the change is machine-derived from a config the
owner harness already executes — nothing new is being authorized). Trust that
was already broken or absent is left untouched: pending human review stays
pending, and the refresh never mints trust.

### State tracking

`~/.agentstack/state.json` records what agentstack manages per target, so
`apply` prunes entries we own that left the manifest and `doctor`/`diff`
detect hand-edits — see [drift: adopt or apply?](#drift-adopt-or-apply) for
which fix to run.

### Scopes

Writes default to **global** (each CLI's `~/.claude.json`,
`~/.claude/skills`); pass `--scope project` to write a repo's project
locations (`.mcp.json`, `.claude/skills/`) so any agent opening the repo
inherits the setup.

## Where rendered files live (three modes)

You always commit the *intent* (`agentstack.toml` + `agentstack.lock`). The
rendered artifacts — `.mcp.json`, `.claude/skills/`, the compiled `CLAUDE.md`
/ `AGENTS.md` — are a per-project choice:

- **Static** (default) — artifacts sit on disk, kept out of git by a managed
  `.gitignore` block. Works however you launch your tools; pass
  `--no-gitignore` to commit the artifacts instead.
- **Clean-at-rest** — nothing generated exists between sessions. Profiles are
  injected by `agentstack run` / `session start` and reverted on exit;
  `agentstack lock` pins name refs without rendering, so `git status` stays
  silent.
- **Zero files** — nothing generated at all. `agentstack connect` registers
  the gateway once per harness and every **trusted** repo serves its own stack
  at session start — see [the zero-copy bridge](#the-zero-copy-bridge---auto-project--trust).

The managed `.gitignore` block is anchored to **outcomes, not declarations**:
an entry exists only for a file agentstack actually wrote or still manages
(tracked in state, or carrying the managed instruction region on disk). A run
whose writes were blocked — unresolved secrets, say — contributes nothing, so
a hand-maintained `.mcp.json` or `CLAUDE.md` is never hidden from
`git status`; and a managed config that still holds resolved secrets stays
ignored until agentstack itself removes it. `apply` and `use` derive the block
from the same records, so alternating them never churns a committed
`.gitignore`.

## Secrets and trust

### Secret resolution

The chain: process env → **varlock** → **OS keychain** → project `.env`.
Unresolved `${REF}`s are reported, never silently blanked.

A ref is a strict `${IDENTIFIER}`. Shell fallback syntax
(`${VAR:-fallback}` inside a command arg) and prompt-style placeholders
(`${input:key}`) pass through verbatim and are never counted as manifest
secrets — so `doctor` doesn't demand a "secret" the shell resolves at
runtime. Each distinct ref is resolved **once per run**; a transient
keychain read is retried, and a persistent failure is reported as
*keychain read failed* — an error distinct from *not found*, so a flaky
keychain daemon never blocks a write by claiming a stored secret is
missing.

### Unresolved secrets block writes

If a `${REF}` doesn't resolve on this machine, `apply`/`use`/dashboard writes
are refused for that target — never a `${TOKEN}` placeholder in live config.
Override with `--allow-unresolved`. Structural manifest validation errors
block `--write` too.

### Governance (`[policy]`)

`require`/`forbid` capabilities and an `allowed_sources` glob allowlist (e.g.
`git:github.com/acme/*`), enforced by `doctor --ci`. Cross-source trust gating
for executable-intent skills and MCPs.

### MCP firewall (`[policy.tools]`)

Per-server tool rules enforced at the runtime gateway:
`github = ["get_*", "list_*", "!list_secrets"]` — plain globs allow, `!`
denies; any allow pattern makes the list an allowlist. A denied tool is
**invisible** — filtered from `tools_search` and code-mode bindings — and
refused with the rule named if called anyway. `doctor` errors on rules naming
unknown servers; `explain <server>` shows the effective policy.

### Call audit log

Every tool call the gateway brokers (MCP proxy and code-mode alike) appends to
`~/.agentstack/audit/calls.jsonl`: timestamp, run id (when launched via
`agentstack run`), server, tool, **argument digest** (never values), outcome
(`ok`/`error`/`denied`), latency. Summarize with `agentstack audit --calls
[--since <days>] [--json]`; the dashboard's Runs panel shows each run's trust
footprint and an all-runs view. Best-effort and size-rotated; logging can
never fail a call.

### Content scanning and `audit`

Every `install` scans skill content for hidden Unicode (zero-width
characters, bidi overrides, tag characters) and prompt-injection heuristics.
Hidden-Unicode findings **block the install** (override with
`--allow-flagged`); injection heuristics warn. `agentstack audit` (`--json`)
re-scans everything materialized — skills and instruction files — and
`doctor --ci` fails on high-severity findings, so a poisoned skill can't slide
into CI unnoticed. Everyday `doctor` skips this scan (it reads every skill
body); opt in with `doctor --deep` — `--ci` always includes it, and the
dashboard's Doctor pane runs it too.

### `doctor --live`

Real MCP `initialize` handshake over HTTP; reports server name + tool count,
or classifies the error (auth / http / connect).

## The central library

One managed home — `~/.agentstack/lib/` — that projects reference **by name**
instead of copying files between repos.

### Layout and name resolution

Skill dirs (`lib/skills/`) and MCP server definitions (`lib/servers/*.toml`)
are indexed in `library.toml`. A profile's `skills = ["sql-review"]` /
`servers = ["kibana"]` resolve from there; an inline `[skills.*]` /
`[servers.*]` table always overrides the library. Provider folders are never
owned — only their skills and MCP entries are managed.

The runtime gateway resolves server name refs through the same
inline-first/central-library path as rendering, so a server declared only in
the library is proxied like an inline one. Where rendering hard-fails a run on
a broken ref, the gateway skips just that server (with a stderr report) and
keeps the rest of the surface up.

### Pinning and provenance

Name refs are pinned by digest in `agentstack.lock` — servers pin the
**definition** digest only; secret values stay `${REF}` and resolve at
render/gateway time, never in the library or the lock. `doctor`/`explain`
flag drift and show each item's origin and provenance. Profile resolution is
offline by default (dry-run `use`, `doctor`, `explain` never fetch);
`use --write` fetches git-backed skills when activation needs them.
`agentstack lock [--profile <name>]` pins every profile's name refs
**without** rendering configs or materializing skills — the lock-only path for
clean-at-rest repos.

### Adding capabilities

`lib add <name> --path <dir>` **copies** the source into
`lib/skills/<name>` — the library copy is canonical from then on (edits to
the source have no effect), provenance records the original path for
`lib list`/`explain`, and a temp-dir source gets a warning since that recorded
path will dangle. `lib add <name> --git <url> --subpath <dir>` (or
`--git <url>#<dir>`) installs a skill living in a repo subdirectory — the
marketplace/monorepo layout — recording truthful `git:<url>@<rev>#<dir>`
provenance. `lib add-server` stores a reusable server definition with its
`${REF}`s intact.

Every `lib add` runs the same hidden-unicode / prompt-injection content scan
as `install`/`audit` before the copy becomes canonical (high findings block
unless `--allow-flagged`), and warns when a skill exceeds ~10 MiB — vendored
dependencies make every full-library pass pay to read them.

### Syncing across machines (`lib sync`)

`lib sync` versions the library as a git repo (init/clone/pull/commit/push,
`--status` to preview); the content-store cache stays local. Its promise —
**secrets never travel** — is enforced by a gate that fails closed:

- Before anything is committed, every `lib/servers/*.toml` is scanned for
  literal (non-`${REF}`) secrets across **every field a credential could hide
  in** — headers, env, the `url` (userinfo passwords, secretish query
  params), and `args`.
- A server file that can't be read or parsed **blocks the sync** rather than
  slipping through unscanned — with any secret-looking line in it named.
- Before pushing, the **outgoing commits** are scanned too, so a secret
  committed once and later edited out of the file can't ride along in
  history. The message names the commit and file.
- `--allow-secrets` overrides all three, deliberately and loudly.

Pulled content passes the same supply-chain scan as `lib add` — warn-only,
since blocking a completed pull would strand the tree — and the scan is
incremental: a no-op pull scans nothing, and a real pull scans only the
skills it changed, so long-accepted content isn't re-flagged on every sync.

### Sweeping in what you already have

`consolidate` moves scattered skills from every CLI's folder into the library
and symlinks the originals back — preview first. `lib migrate` copies a
legacy `~/.agentstack/skills/` home in, preview-first and reversible. Manage
the rest with `lib list` / `remove` / `remove-server`.

## Capabilities

### Package manager

Skills declare a source (`path` or `git`); `install` fetches them into
`~/.agentstack/store/` and writes a SHA-256 `agentstack.lock`;
`install --locked` is reproducible (CI-safe); `update` re-resolves git skills;
`remove` drops a capability from manifest + lock. `install` is profile-aware:
skills a profile references by name (resolved from the central library, no
inline `[skills.*]` entry) keep their lock pins through the reconcile pass —
pin or refresh those with `agentstack lock`.

Repeat digests are served from a stat-fingerprint cache
(`~/.agentstack/digest-cache.json`: file count + total size + max mtime + a
hash of the sorted relative paths with each file's size and mtime). Any
mismatch falls back to the full read+hash, so `doctor`/`use` over a large
library cost stat calls, not a re-hash of every byte. Delete the file to
force full re-hashing.

### Selective skills via profiles

`use <profile>` materializes only that profile's skills (symlink, with copy
fallback), pruning the rest it owns and never clobbering hand-made skill
dirs. When a prune empties the managed skills dir (deactivation,
`session end`), the dir itself is removed too — rmdir semantics, so a dir
holding any user content always survives.

### Instruction files

Compile shared + harness-specific `[instructions.*]` fragments into each
CLI's `CLAUDE.md` / `AGENTS.md`, inside a managed `<!-- agentstack -->` region
that preserves all surrounding hand-written prose (`agentstack instructions`;
dry-run by default, `--write` applies). Part of the mainstream lifecycle:
`apply` (and therefore `setup`) compiles the region alongside
servers/settings/hooks behind the same `--write` gate — a manifest with no
`[instructions.*]` never touches a region another layer owns — and `doctor`
flags a stale managed region (warn ↳ `instructions --write`) or a missing
fragment source (error, gates `--ci`). Installing a pack's house rules prints
the exact compile command as the next step.

### The machine layer

**`init --global`** seeds `~/.agentstack/agentstack.toml` plus an
`instructions/` dir: a first-class home for *personal*, cross-project
instruction fragments — the operational knowledge you'd otherwise re-teach
each agent. Compile them with `agentstack instructions --manifest-dir ~
--write`. The zero-files bridge deliberately never discovers this layer as a
project — it cannot be `trust`ed or activated by `mcp --auto-project`.

**The user layer** merges the machine manifest's `[instructions]` (and only
those) in beneath each project load, order user → project → project-local; a
project fragment of the same name wins outright. Inherited fragments compile
at **global scope only** — personal rules never land in a repo's committed
`CLAUDE.md` — and servers/skills/settings deliberately do **not** inherit:
personal capabilities never auto-inject into a team project, and the trust
digest is unaffected. Provenance is visible everywhere: `instructions` labels
inherited fragments `(machine)`, `doctor` counts them, and
`explain <fragment>` names the layer.

**agentstack house rules** — a bundled fragment (`[instructions.agentstack]`)
that teaches every agent the manifest-first workflow: never edit rendered
configs, the three artifact modes (a clean-at-rest project's missing
`.mcp.json` is intentional), re-lock after editing profiles, and the drift
decision rule (keep a hand-added server → `adopt`; manifest is truth →
`apply --write`). `init --global` and `setup` offer to install it into the
machine manifest — opt-in, like pack instructions.

### Native settings

Manage each CLI's own settings file (Claude Code `~/.claude/settings.json`
permissions/feature flags, Codex `config.toml`) from one `[settings.<cli>]`
block. `apply` merges only the keys you declare into the real file (top-level
ownership), resolves `${REF}`s, preserves hand-set keys, and prunes keys that
leave the manifest. Editable from the dashboard.

### Lifecycle hooks

Declare `[hooks.*]` once (event + optional matcher + command) and `apply`
compiles them into each harness's native hooks config (Claude Code
`settings.json`, Codex `config.toml`), resolving secrets and pruning hooks
that leave the manifest. Add/list from the dashboard Hooks pane.

### Search across providers

`search` queries the embedded catalog **and the official MCP Registry**
(`registry.modelcontextprotocol.io`). `agentstack add from <id>` resolves a
registry/catalog server, lifts its secrets to `${REF}`s, and (on `apply`)
renders it to **all your CLIs at once**. agentstack is the cross-CLI *client*
over the registry + marketplaces, not another registry.

### Git-hosted versioned packs

Any repo with a `pack.toml` installs as a pack from any git host, pinned to a
version tag: `add from git:<host>/<repo>[@<tag>][#subdir]` (no tag → newest
version-shaped tag; a repo with no version tags is an error, never a floating
install). The ledger records `source`/`version`/`rev` (the resolved commit);
extracted skills are digest-pinned in the lock so `install --locked`
reproduces. `upgrade <pack>` lists remote tags, resolves the newest (never
downgrades), previews the member diff, and re-pins on `--write`.
`[policy] allowed_sources` is enforced **before** any fetch, and the clone's
content passes the install scan gate. `agentstack pack init` scaffolds a
publishable pack; the dashboard's Discover pane installs from a git URL with
the same gates. (Semver ranges and transitive pack dependencies are
deliberately not in v1.)

### `adopt` and `add`

`adopt` is the reverse of `apply`: pull a hand-added server from a target
config back into the manifest, lifting its inline secret, preserving
comments — the keep-side of every [drift decision](#drift-adopt-or-apply).
`add` is the flag-driven (scriptable / agent-operable) way to add a server or
skill to the manifest, optionally into a profile; comments preserved.

### `stats`

Local usage analytics: activation counts + per-capability footprint (which
target/scope slots it's live in) + **context cost**. `stats --live` measures
each server's `tools/list` token footprint through the gateway (HTTP + stdio)
and caches it (`~/.agentstack/footprint.json`); `stats`, `explain`, and the
dashboard's Servers matrix then show what each server taxes every session
offline, and `stats` flags dead weight — high-cost, never-activated servers —
with the exact `remove` command.

### Wire proxy (`proxy`)

Where `stats --live` gives you a **static** estimate — what a server's
`tools/list` *would* cost — the wire proxy gives you **runtime ground truth**:
what the `tools` block actually costs, in input tokens, on every real turn your
harness sends. It's the on-wire complement to `src/footprint.rs`'s static
counter, and it's the built-in version of the hand-rolled logging proxy from
[*How to kill the bloat in Claude Code's system
prompt*](https://www.aihero.dev/how-to-kill-the-bloat-in-claude-codes-system-prompt).

**Point a harness at it.** `agentstack proxy start` stands up a loopback proxy
(default `127.0.0.1:8787`; `--port`, `--upstream` to override) that relays every
request VERBATIM to the Anthropic API. Set the harness's base URL and use it
normally:

```bash
export ANTHROPIC_BASE_URL=http://127.0.0.1:8787
agentstack proxy start        # blocks, serving, in one shell
# …drive Claude Code (or any Anthropic-API harness) as usual…
agentstack proxy report       # --json for the raw aggregate
```

**What `proxy start` does.** For each `/v1/messages` request it walks the
`tools` array and buckets every tool into its capability — `mcp__<server>__<tool>`
→ `<server>`, everything else (`Read`, `Bash`, `Task`, …) → `builtin` — summing
each bucket's estimated per-turn token cost (same `estimate_tokens` heuristic as
the static footprint). Off the response it captures best-effort usage numbers
and the tool NAMES the model actually called, for both non-streamed JSON and
streamed (SSE) responses. The SSE path tees the stream through a pass-through
reader: bytes reach the client unchanged and undelayed while a side buffer
absorbs `tool_use` names and usage — so streamed turns now report real `calls`
(previously always 0 under streaming).

**Telemetry file.** Records append to `~/.agentstack/proxy/requests.jsonl`
(size-rotated, at most two ~5 MB generations, same contract as the call log). It
is **content-free by construction**: counts, capability/tool names, token
estimates, the model id, and best-effort usage numbers — never prompt or message
bodies, tool arguments, secrets, or header values.

**What `proxy report` shows.** It aggregates the log into a ranked,
per-capability table: `tools` (typical per-turn tool count), `avg tokens/turn`,
`calls` (how many times any of that capability's tools appeared in a
`tool_use`), and a loaded-vs-called `hint`. Headline tools/tokens are the max
seen in a single turn — a turn re-sends the whole block, so summing across turns
would inflate it. The hint is a modest ranking signal, not a verdict: a
capability called at least once is `keep`; the costliest never-called one is a
`drop / lazy` candidate (demote to a lazy server or drop it from the profile);
the cheap-and-unused rest is `watch`. Those names are the same servers and
profiles agentstack already manages, so the report closes the loop with the
static `footprint` / `stats` / `doctor` lenses using on-wire evidence.

**Phase-1 guardrails.** The proxy is **observe-only** (it never injects or
mutates the tools/system block, so the prompt-prefix cache stays warm), all
accounting is **best-effort and fail-open** (a parse hiccup never delays or
fails the proxied request — a forwarding error returns a 502 but keeps the
accept loop alive), and auth headers pass through untouched. Each request is
handled on its own thread so a long-lived SSE stream can't block concurrent
calls from parallel subagents or background token-count/compaction requests.

### `export` / `import`

An age-encrypted bundle (manifest + lock + optionally secrets) for moving a
setup to a new machine; passphrase-protected.

## Drift: adopt or apply?

`doctor` flags drift in both directions, and the fixes are opposites — pick
by which side holds the truth:

- **"edited on disk since last apply"** — the live config changed after our
  last write. Review with `agentstack diff`; if the hand-edit should stay,
  `agentstack adopt` pulls it into the manifest. If the manifest is right,
  `agentstack apply --write` re-renders over the edit.
- **"would REMOVE \<names\>"** — the manifest no longer selects entries we
  manage, so the next `apply --write` deletes them from the live config.
  `agentstack adopt` first if any of them should survive; apply only when
  the removal is intended.
- Entries recorded by a **different manifest** are never pruned implicitly
  (global scope is shared by every manifest on the machine): `apply` keeps
  them and says so, and `diff`/`doctor` keep surfacing them as kept — not as
  pending deletions — until you decide. Prune them with an explicit
  `apply --prune-foreign` (it still works after the guarded write recorded
  its own set), or `adopt` them into the current manifest.

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
# Bundled skills are copied into the central library and referenced by name
# (with plugin provenance recorded), so the recipe survives native plugin
# updates/uninstalls instead of path-pointing into a versioned plugin cache.
# Bundled servers are written with `targets = []` — recipe-owned: the native
# plugin already provides them on the adopted harness and the generated
# package carries them elsewhere, so `apply` never configures them a second
# time. Codex auth wiring (`bearer_token_env_var`, `env_http_headers`) is
# carried through as `${REF}` headers, so doctor demands the secret and the
# server authenticates wherever the definition travels.
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

The harness a recipe was adopted **from** is satisfied by its still-installed
native plugin: `plugins status` and `doctor` report *satisfied natively* at
the installed version + rev, and surface drift when the native plugin moves
ahead of the adopted recipe (re-adopt to catch up) — they never suggest
installing the agentstack copy alongside the original.

Generated Claude Code packages never reference `hooks/hooks.json` from
`plugin.json` — Claude Code auto-loads that path, and naming it again is a
duplicate-hooks load error that breaks the whole plugin. Hook-less recipes
ship no hooks file at all; the Codex manifest keeps its explicit reference.

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
repo needs only its `.agentstack/agentstack.toml` (+ lock — pin library refs
with `agentstack lock`, which never renders or materializes anything).

Auto-discovery is **trust-gated**, direnv-style. A manifest can declare stdio
servers (arbitrary local commands) and reference secrets, so a repo you just
cloned gets **control-plane tools only** — nothing spawned, nothing contacted,
no secrets resolved — until you review it and run:

```bash
agentstack trust .          # shows what the manifest runs/contacts, then pins its digest
agentstack trust --list     # every trusted project + whether its manifest still matches
agentstack trust --revoke   # withdraw
```

Trust is pinned to the content digest of the manifest layers (including
`agentstack.local.toml`) plus `agentstack.lock`: any edit — a `git pull`, a
re-lock — drops the project back to control-plane-only until re-trusted.
Explicit `--manifest-dir` skips the gate (naming a directory is the consent),
matching plain `agentstack mcp`. `trust .` previews the **effective runtime
surface** — inline servers and library name refs alike, each library ref
labeled pinned/unpinned/drifted — so the review covers exactly what the
gateway will serve.

Library-referenced server definitions live outside the digest, so the gateway
integrity-checks them at launch against the lock's pinned definition digests:
a definition that drifted from its pin is refused (with a
`re-run \`agentstack lock\`` message) and an unpinned ref is served with a
warning. Re-locking changes the lockfile, which re-gates trust — the pin, the
runtime check, and the consent digest close the loop.

(Upgrading across the digest-formula change — v0.6.x adds the lockfile to it —
flips previously trusted projects to "changed" once; re-run `agentstack trust`
after reviewing.)

The remaining scope limit is local code integrity: the digest does not cover
arbitrary files the manifest references. Trusting a repo whose server runs
`python3 ./server.py` authorizes *that command* — a later edit to `server.py`
does not re-gate the project (an edit to the manifest does). Review referenced
local scripts as part of `trust .`, the same way you'd review a `.envrc`
before `direnv allow`.

The gate is visible from inside the session, not just on stderr: when the
project is untrusted (or its manifest changed since it was trusted),
`tools_search` says so and names the exact `agentstack trust <dir>` command,
and `agentstack_doctor` includes a `Trust (auto mode):` line.

agentstack's own manual — the bundled `using-agentstack` skill — is always
loadable through the control plane: it appears in `agentstack_list_loadable`
even with no project manifest, in untrusted (control-plane-only) sessions, and
through session fences, served from the copy embedded in the binary (a
project's own `using-agentstack` skill overrides it). An agent that can reach
the gateway can always learn how to drive it.

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

`init` (`--global`), `add`, `install` (`--locked`, `--allow-flagged`), `update`,
`lock` (`--profile`), `remove`,
`upgrade`, `bootstrap` (`--write`), `apply` (`--scope`, `--write`,
`--prune-foreign`), `diff`,
`explain`, `use <profile>`, `session`, `instructions`, `adopt`, `consolidate`,
`lib add|add-server|list|remove|remove-server|migrate|sync`
(`lib add`: `--path`, `--git`/`--subpath`, `--allow-flagged`; `lib sync`:
`--init`, `--remote`, `--status`, `--allow-secrets`), `restore`,
`doctor` (`--ci`, `--live`, `--fix`, `--deep`), `audit` (`--json`, `--calls`,
`--since`), `optimize` (`--json`, `--write`, `--since`), `analyze` (`--json`),
`search`, `stats` (`--live`), `proxy start|report` (`start`: `--port`,
`--upstream`; `report`: `--json`),
`secret set|get|rm|list`, `export`/`import`, `adapters` (`list|show|validate`),
`pack init`, `plugins`,
`dashboard`, `mcp` (`--auto-project`), `connect`/`disconnect`,
`trust` (`--list`, `--revoke`), `codemode`, `hook`, `run`/`runs`/`kill`,
`self link|which`.

## Everything shipped so far

13 adapters · `init`/`add`/`apply`/`diff`/`use`/`instructions`/`adopt` ·
package manager (`install`/`update`/`remove` + lockfile) · central capability
library (`lib` skills + servers referenced by name, digest-pinned in the lock,
drift in `doctor`/`explain`, `consolidate` into `lib/skills`) · secrets (keychain +
varlock) · scopes (global/project) · `doctor` (`--live`/`--fix`/`--ci`/`--deep`) ·
content scanning on install + `audit` · official MCP Registry provider +
`search`/`add from` · `[policy]` trust gate · native per-CLI settings
(`[settings.*]` → settings.json) · managed plugin recipes (`[plugins.*]` →
native Claude Code/Codex packages + marketplaces) · atomic writes + backups ·
`export`/`import` · `hook` · agent-operable `mcp` server · local dashboard
(server/skill matrices, Discover, add-skill, settings editor) · live runs
(`run`/`runs`/`kill` + dashboard Runs panel) · GitHub Action trust gate ·
nightly adapter-conformance CI · zero-copy bridge (`connect` + `mcp
--auto-project` + digest-pinned `trust`) · `optimize` (evidence-backed
recommendations from usage/audit/cost signals, safe-class `--write`) ·
fail-closed `lib sync` secret gate (all server fields + outgoing history).
