# Feature reference

The complete, implemented-and-tested feature inventory. The
[README](../README.md) is the tour; this is the map.

**Contents**

- [Core engine](#core-engine)
  - [The manifest](#the-manifest)
  - [Data-driven adapters](#data-driven-adapters)
  - [Rendering and merging](#rendering-and-merging)
  - [Owned servers (`owner = "codex"`)](#owned-servers-owner--codex)
  - [State tracking](#state-tracking)
  - [Scopes](#scopes)
- [Where rendered files live (three modes)](#where-rendered-files-live-three-modes)
- [Secrets and trust](#secrets-and-trust)
  - [Secret resolution](#secret-resolution)
  - [Where lifted secrets go (`init`)](#where-lifted-secrets-go-init)
  - [Unresolved secrets block writes](#unresolved-secrets-block-writes)
  - [Governance (`[policy]`)](#governance-policy)
  - [MCP firewall (`[policy.tools]`)](#mcp-firewall-policytools)
  - [Egress rules (`[policy.egress]`)](#egress-rules-policyegress)
  - [Secret access (`[policy.secrets]`)](#secret-access-policysecrets)
  - [Filesystem scopes (`[policy.filesystem]`)](#filesystem-scopes-policyfilesystem)
  - [Call log](#call-log)
  - [Content scanning](#content-scanning)
  - [`doctor --live`](#doctor---live)
  - [One undo verb: `restore`](#one-undo-verb-restore)
  - [`doctor` shows what you use](#doctor-shows-what-you-use)
- [The central library](#the-central-library)
  - [Layout and name resolution](#layout-and-name-resolution)
  - [Pinning and provenance](#pinning-and-provenance)
  - [Adding capabilities](#adding-capabilities)
  - [Syncing across machines (`lib sync`)](#syncing-across-machines-lib-sync)
  - [The two mental models](#the-two-mental-models)
- [Capabilities](#capabilities)
  - [Package manager](#package-manager)
  - [Selective skills via profiles](#selective-skills-via-profiles)
  - [Instruction files](#instruction-files)
  - [The machine layer](#the-machine-layer)
  - [Native settings](#native-settings)
  - [Lifecycle hooks](#lifecycle-hooks)
  - [Native extensions](#native-extensions)
  - [Search across providers](#search-across-providers)
  - [Git-hosted versioned packs](#git-hosted-versioned-packs)
  - [`adopt` and `add`](#adopt-and-add)
  - [`report usage` (usage analytics)](#report-usage-usage-analytics)
  - [Wire proxy (`proxy`)](#wire-proxy-proxy)
  - [`export` / `import`](#export--import)
- [Drift: adopt or apply?](#drift-adopt-or-apply)
- [Ephemeral sessions (`agentstack session`)](#ephemeral-sessions-agentstack-session)
- [Live runs (`agentstack run`)](#live-runs-agentstack-run)
  - [Execution posture](#execution-posture)
  - [The Protected tier in detail (`run --locked`)](#the-protected-tier-in-detail-run---locked)
- [Agent-operable (`agentstack mcp`)](#agent-operable-agentstack-mcp)
  - [Transparent mode (`--transparent`)](#transparent-mode---transparent)
  - [The zero-files gateway (`--auto-project` + `trust`)](#the-zero-files-gateway---auto-project--trust)
  - [MCP profile leases](#mcp-profile-leases-one-connection-one-capability-fence)
  - [Compact proxied surface + code mode](#compact-proxied-surface--code-mode)
  - [Experimental `tools_execute`](#experimental-tools_execute)
- [Dashboard](#dashboard)
- [Optimize (`agentstack optimize`)](#optimize-agentstack-optimize)
- [Field notes and addenda](#field-notes-and-addenda)
  - [Launch timing and switching](#launch-timing-and-switching)
  - [Session and run recovery](#session-and-run-recovery)
  - [Lease survival across a mid-connection change](#lease-survival-across-a-mid-connection-change)
  - [Central library: server definitions and bundled catalog](#central-library-server-definitions-and-bundled-catalog)
  - [`tools_execute` cancellation](#tools_execute-cancellation)
- [All commands](#all-commands)
- [Everything shipped so far](#everything-shipped-so-far)

**A few words used throughout:**

- **CLI** — the agent tool you run (Claude Code, Codex, …). Some flags and output call it a *harness*.
- **adapter** — agentstack's per-CLI config compiler; `agentstack adapters list` shows their ids.
- **target** — an adapter id listed in `[targets]`, naming which CLIs a command acts on.

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

Nothing is dropped silently. A server whose transport a target's config can't
express is skipped with a spoken reason, and so is a server whose **name**
the CLI itself would refuse at startup — Codex validates names against
`^[a-zA-Z0-9_-]+$` (declared as `mcp.name_charset` in its descriptor), so a
name like `upstash/context7` renders for Claude Code but is skipped for
Codex with *"rename the server in the manifest"* rather than written into a
config that errors on every Codex launch. The conformance smoke
(`examples/sandbox/conformance-smoke.sh`) proves both sides against the real
CLIs.

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
those CLIs.

The gateway honors `cwd` too: stdio upstreams are spawned in the server's
`cwd` (relative paths anchor at the project root), defaulting to the project
root itself — never in whatever directory the client happened to launch
`agentstack mcp` from.

A server can also scope which targets it renders to at all, mirroring
instructions and hooks: `[servers.X] targets = ["claude-code"]` fans out to
that adapter only, the `["*"]` default means every target, and an explicit
`targets = []` opts out of the direct fan-out entirely (a server declared for
provenance and lock pinning but delivered by some other path). `apply`,
`diff`, and `doctor` drift all share the one filter, and a typo'd id in
`targets` is a validation error.

### Owned servers (`owner = "codex"`)

Some CLIs rewrite their own server entries — the Codex desktop app,
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

Writes default to the **manifest's home**: a repo manifest writes **project**
locations (`.mcp.json`, `.claude/skills/` — repo-local, behind the managed
`.gitignore` block), while the machine manifest (`~/.agentstack/`) writes
**global** locations (each CLI's `~/.claude.json`, `~/.claude/skills`).
`--scope` overrides either way — e.g. `apply --scope global` in a repo puts
its servers in every project's config on this machine. `doctor` follows the
scope your writes actually recorded, so a deliberate `--scope` choice is
honored, not second-guessed.

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
- **Zero files / MCP** — no persistent per-project provider artifacts. `agentstack
  gateway connect` registers the gateway once per CLI (one write to each
  CLI's global config) and every **trusted** repo serves its own stack
  live. `agentstack_lease_open(profile)` can fence one MCP connection to a
  profile without rendering native files; `agentstack_lease_status` shows its
  in-memory load trail, `agentstack_lease_freeze` promotes the observed set to
  a manifest profile (review it, then run `agentstack lock`), and close/process
  exit drops it. A machine-local
  `codemode/endpoint.json` coordinate may exist for the connection's duration — see
  [the zero-files gateway](#the-zero-files-gateway---auto-project--trust).

**Recommendation:** prefer the zero-file lease path for interactive work when
the CLI supports MCP; use static or clean-at-rest delivery when the CLI
must read native skill/instruction files. Add `--sandbox --lockdown` when the
agent process itself needs isolation—a lease is a capability fence, not a
sandbox. See [the primitives and decision table](ARCHITECTURE.md#operating-model--choose-the-boundary-you-need).

Interactive `init` presents these three as an arrow-key choice (each with its
help text) **before any write** — and the selection **forks** the rest of the
run. **static** takes the render path (preview → confirm → `apply --write` →
activate skills → doctor). **clean-at-rest** renders nothing: it pins the
lockfile (the `lock` flow), teaches the `session start`/`session end` rhythm,
and runs a drift-suppressed doctor. **zero-files** also renders nothing: it
offers to register the gateway (`gateway connect --all --write`) and then
points at `agentstack trust .` — which the wizard never runs for you, since
trust is human consent. Bare `agentstack` reports the project's current mode
on its `Mode` line — derived from what is actually on disk — so "which mode am
I in?" is a glance, not archaeology.

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

The enforcement core: how a secret resolves, where a policy narrows what a
server may do, and what every brokered call records. Read it if you run
untrusted repos, resolve credentials on this machine, or want a machine ceiling
no project can loosen.

### Secret resolution

The chain: process env → **varlock** → **OS keychain** → project `.env`.
Unresolved `${REF}`s are reported, never silently blanked.

The varlock link activates only when the project both opts in (a `.env.schema`
is present) and has the `varlock` binary on PATH — otherwise the chain silently
skips it. When active, agentstack shells out to
`varlock load --format json-full --compact` and delegates the whole
provider matrix to it: 1Password, AWS/Azure/GCP secret managers, Bitwarden,
and device-local encrypted stores all back the same `${REF}`s, no manifest
change. See [varlock.dev](https://varlock.dev).

A ref is a strict `${IDENTIFIER}`. Shell fallback syntax
(`${VAR:-fallback}` inside a command arg) and prompt-style placeholders
(`${input:key}`) pass through verbatim and are never counted as manifest
secrets — so `doctor` doesn't demand a "secret" the shell resolves at
runtime. Each distinct ref is resolved **once per run**; a transient
keychain read is retried, and a persistent failure is reported as
*keychain read failed* — an error distinct from *not found*, so a flaky
keychain daemon never blocks a write by claiming a stored secret is
missing.

### Where lifted secrets go (`init`)

When `init` finds inline tokens in an imported config it lifts each to a
`${REF}` and chooses where the value lands. An interactive run prompts with
three self-explaining options — a gitignored project `.env` (**the default**),
the OS keychain (service `agentstack`), or skip and write only the placeholder.
The non-interactive path takes `--secrets env|keychain|skip`; absent and
non-interactive it defaults to `keychain`, so CI and scripts never start
writing plaintext by surprise. `--no-keychain` is the deprecated alias for
`--secrets skip`, and a skip prints every unstored `${REF}` with the command to
store it — lifted values are never silently dropped. The `.env` writer places
values next to the manifest and adds a managed `.gitignore` entry when the
project is a git repo; `secret set --env-file` targets that same `.env` instead
of the keychain. The manifest itself only ever holds `${REF}` placeholders
(rule 5).

### Unresolved secrets block writes

If a `${REF}` doesn't resolve on this machine, `apply`/`use` writes are
refused for that target — never a `${TOKEN}` placeholder in live config.
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

**Machine layer with deny precedence.** The machine manifest
(`~/.agentstack/agentstack.toml`) may carry its own `[policy.tools]` — the
user's standing rules, checked **before** the project's on every brokered
call. A call must pass both layers, so a repo policy cannot loosen a machine
rule; a machine refusal names its layer in the error and the audit log.

Know what a rule binds to: policy is keyed on the **manifest-chosen server
name**, and a repo picks its own names — a machine rule for `github`
constrains a server *named* `github`, not the GitHub MCP server under any
name. For rules that must survive renaming, use the `"*"` wildcard key, which
constrains every server whatever a manifest calls it (named rules are best
thought of as guarding *your* naming conventions, plus profile/library servers
whose names you control):

```toml
# ~/.agentstack/agentstack.toml — applies to every project on this machine
[policy.tools]
"*" = ["!delete_*"]                   # rename-proof: no server may delete_*
github = ["get_*", "list_*"]          # servers NAMED github are read-only
```

The layer is loaded once per gateway launch (like everything else — "the
manifest is resolved once per launch"), so tightening it mid-session takes
effect on the next session. Each valid load stores a secret-free,
digest-labelled last-known-good policy snapshot. If a later edit is malformed,
the gateway enforces that snapshot and reports **DEGRADED**; if the first load
is malformed or the snapshot is unusable, protected activation is **BLOCKED**
instead of silently falling back to project-only policy. A genuinely absent
machine manifest is the separate, benign **UNCONFIGURED** state. `doctor`
distinguishes all three conditions.

### Egress rules (`[policy.egress]`)

Per-server outbound-host rules, keyed and evaluated exactly like
`[policy.tools]` (plain globs allow, `!` denies, the `"*"` key is rename-proof,
machine layer checked first and no repo can loosen it) — the subject is the
destination host instead of a tool name. A pattern may pin a port with a
`:port` suffix: `api.example.com:443` scopes to that port, a bare host means
any port. The write/spawn-time check matches the host and defers the port; the
sandbox egress proxy enforces the exact CONNECT port at runtime.

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

Bundle-global path-glob scopes (not per-server) in three lists. `write` is the
enforced one: in `run --sandbox` the workspace mounts **read-only** unless the
effective write scope covers its root (deny-by-default; a partial scope like
`src/**` does not grant it — the mount is all-or-nothing). `read` scopes are
informational. `deny` is a pure blocklist unioned across the machine and
bundle layers — a repo can add denies but never drop the machine's — matched
against the workspace-relative path, the absolute path, **and** the bare file
name, and enforced by the host-mode `agentstack guard` hook.

```toml
[policy.filesystem]
write = ["./**"]                      # sandbox: workspace mounts read-write
deny  = [".env*", "**/*.pem"]         # no tool call may touch these, ever
```

### Call log

Every tool call the gateway brokers (MCP proxy and code-mode alike) appends to
`~/.agentstack/audit/calls.jsonl` (created `0600`, dir `0700`): timestamp, run
id (when launched via `agentstack run`), server, tool, **keyed argument
digest** (never values — keyed with a per-machine secret so an exfiltrated log
can't confirm guessed arguments), outcome (`ok`/`error`/`denied`), latency,
and a detail that is either the policy rule (denials) or a **fixed error
class** (failures) — upstream error text is never written, so a malicious
server can't inject content into the log. Summarize with `agentstack report
calls [--since <days>] [--json]`; the dashboard's Runs panel shows each
run's trust footprint and an all-runs view.

Honest scope: this is best-effort local **diagnostics** (logging can never
fail a call; size-rotated at ~5 MB × 2). It is not durable or tamper-evident —
any process running as your user can edit it. Treat it as the input to
`report calls`/`optimize` and incident triage, not as forensic evidence.

### Content scanning

Every `install` scans skill content for hidden Unicode (zero-width
characters, bidi overrides, tag characters) and prompt-injection heuristics.
Hidden-Unicode findings **block the install** (override with
`--allow-flagged`); injection heuristics warn. `doctor --deep` is the on-demand
content re-scan — it re-scans everything materialized (skills and instruction
files), and `doctor --ci` fails on high-severity findings, so a poisoned skill
can't slide into CI unnoticed. Everyday `doctor` skips this scan (it reads every
skill body); opt in with `doctor --deep` — `--ci` always includes it, `--json`
emits the whole report machine-readably, and the dashboard's Doctor pane runs it
too. Interactive `init` offers the deep scan as an explicit yes/no at its
closing doctor step, but only when the project actually has skills — no empty
questions on a server-only manifest.

### `doctor --live`

Real MCP `initialize` handshake over HTTP; reports server name + tool count,
or classifies the error (auth / http / connect).

### One undo verb: `restore`

Every write agentstack makes (servers, settings, hooks, instructions — even
the owned-server manifest refresh) is captured in the history engine before it
lands. `agentstack restore` lists the recorded changes — the same applies the
dashboard's Activity tab shows. `restore <id> --write` (unique prefix) or
`restore --last --write` reverts one. `restore <adapter>` keeps the original
single-slot config restore as a fallback. Reverted files simply show up as
pending again.

### `doctor` shows what you use

Every check always runs, but the default report prints only the sections
relevant to this project — a feature you've never touched (the zero-files
gateway, native extensions, reproducibility pins…) stays out of the way until it
either gets used or produces a warning/error, which always shows. A closing
line counts what was hidden; `doctor --all` prints everything, and `--ci`
always shows the full report (a team gate prints exactly what it evaluated).
The dashboard's Doctor pane gets every section regardless, each tagged
`relevant`.

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
render/gateway time, never in the library or the lock. Native extensions pin
differently: a `[[extension]]` entry records the extension's `name`, its
`target` adapter, and a `checksum` computed with the **strict** integrity-root
digest over the whole source tree — so retargeting a byte-identical extension
is drift, and a one-byte source edit re-gates trust (see
[Native extensions](#native-extensions)). `doctor`/`explain`
flag drift and show each item's origin and provenance. Profile resolution is
offline by default (dry-run `use`, `doctor`, `explain` never fetch);
`use --write` fetches git-backed skills when activation needs them.
`agentstack lock [--profile <name>]` pins every profile's name refs
**without** rendering configs or materializing skills — the lock-only path for
clean-at-rest repos. The lockfile is part of a project's consent surface (its
bytes feed the trust digest), so when a currently-trusted project's pins
actually change, `lock` warns that its trust is now stale and must be re-granted
with `agentstack trust .` — new pins are new consent.

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
as `install`/`doctor --deep` before the copy becomes canonical (high findings block
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

### The two mental models

Three ways a skill or server reaches a profile, and the manifest syntax alone
picks which — get the distinction once and the empty-block trap below never
bites:

- **By-name library reference** — `skills = ["greet"]` / `servers = ["kibana"]`
  with **no** matching `[skills.greet]` / `[servers.kibana]` table. Resolved
  fresh from `~/.agentstack/lib` on every lock and pinned there by `checksum`
  (skills) or definition digest (servers); nothing is copied into the repo and
  the library copy stays canonical — you edit it in the library, once, for
  every project that names it. The cross-repo default.
- **Vendored pack copy** — installed with `add from git:<host>/<repo>`. The
  pack's members are copied into the project and digest-pinned, and a
  `[packs.<name>]` ledger records `source`/`version`/`rev` so `upgrade`
  re-resolves them. A self-contained snapshot that travels with the repo and
  versions as one unit — see
  [Git-hosted versioned packs](#git-hosted-versioned-packs).
- **Inline manifest** — a `[skills.greet]` / `[servers.*]` table authored in
  the manifest with its own `path`/`git`/`command`. Lives in the repo and
  **always overrides** a same-named library reference.

The trap: a `[skills.greet]` block with **no** source is read as an inline
skill *missing* its source — it errors, it does not fall back to a library
skill of the same name. Drop the block and list `greet` in `skills = […]` to
reference the library copy; keep the block only when you mean a distinct inline
skill. `explain` prints each capability's model on its `Model` line.

## Capabilities

The kinds of thing a profile can carry — skills, servers, instructions,
settings, hooks, extensions, packs — and the commands that add, search, and
account for them. It is a menu; jump to the capability you need.

### Package manager

Skills declare a source (`path` or `git`); `install` fetches them into
`~/.agentstack/store/` and writes a SHA-256 `agentstack.lock`;
`install --locked` is reproducible (CI-safe); `lock --update` re-resolves git skills;
`remove` drops a capability from manifest + lock. `install` is profile-aware:
skills a profile references by name (resolved from the central library, no
inline `[skills.*]` entry) keep their lock pins through the reconcile pass —
pin or refresh those with `agentstack lock`.

Skill and library content digests always hash current bytes; there is no digest
cache on the verification path. Older versions kept a stat-fingerprint cache and
may leave a harmless orphaned `~/.agentstack/digest-cache.json`; it is unused and
safe to delete.

### Selective skills via profiles

`use <profile>` materializes only that profile's skills (symlink, with copy
fallback), pruning the rest it owns and never clobbering hand-made skill
dirs. The profile is optional: one declared profile is chosen automatically,
and a manifest with **no** profiles activates its full inline set as the
implicit default — `agentstack use --write` just works. Several profiles need
a name. When a prune empties the managed skills dir (deactivation,
`session end`), the dir itself is removed too — rmdir semantics, so a dir
holding any user content always survives.

Interactive `init` finishes with the same activation: it picks the profile (an
explicit `--profile`, the only one declared, or an interactive offer of the
first-declared) and materializes its skills through the exact `use` code
path — so a completed init leaves nothing left to activate. Plain `apply`
still never touches skills; it reminds you which profile activates them.

### Instruction files

Compile shared + harness-specific `[instructions.*]` fragments into each
CLI's `CLAUDE.md` / `AGENTS.md`, inside a managed `<!-- agentstack -->` region
that preserves all surrounding hand-written prose (`agentstack instructions`;
dry-run by default, `--write` applies). Part of the mainstream lifecycle:
`apply` (and therefore `init`) compiles the region alongside
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
--write`. It also seeds the machine `[guard]` + `[policy.filesystem]` deny
defaults (the same list `guard install` writes) and offers to install the host
guard into detected CLIs. The zero-files gateway deliberately never discovers
this layer as a project — it cannot be `trust`ed or activated by
`mcp --auto-project`.

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
`apply --write`). `init --global` and the interactive `init` wizard offer to
install it into the machine manifest — opt-in, like pack instructions.

### Native settings

Manage each CLI's own settings file (Claude Code `~/.claude/settings.json`
permissions/feature flags, Codex `config.toml`) from one `[settings.<cli>]`
block. `apply` merges only the keys you declare into the real file (top-level
ownership), resolves `${REF}`s, preserves hand-set keys, and prunes keys that
leave the manifest. Viewable in the dashboard's Settings tab. Edit these keys
without hand-editing the manifest with `agentstack settings set <target> <key>
<value>` (and `settings unset <target> <key>` to drop one) — dry-run by
default, `--write` applies.

### Lifecycle hooks

Declare `[hooks.*]` once (event + optional matcher + command) and `apply`
compiles them into each harness's native hooks config (Claude Code
`settings.json`, Codex `config.toml`), resolving secrets and pruning hooks
that leave the manifest. Listed in the dashboard's Hooks tab.

### Native extensions

`[extensions.<name>]` manages a harness's native executable add-ons — pi's
TypeScript extensions, OpenCode's JS plugins — the way `[skills.*]` manages
skill dirs. It is the **highest-risk** capability agentstack delivers: the code
runs *inside the harness process at full user permission*, outside every policy
ceiling. agentstack pins and delivers the bytes; it never executes or governs
them at runtime. What it provides is provenance and content binding — which
bytes, from where, reviewed by whom, re-gated on any change.

```toml
[extensions.checkpoint]
description = "Git checkpoint on every agent turn"
path = "./extensions/checkpoint"   # or: git = "…", rev = "…", subpath = "…"
target = "pi"                      # exactly one adapter id
```

- **Source.** A local `path` (a `.ts`/`.js` file or a directory), anchored at
  the manifest dir exactly like skills and instructions; a `git` source (which
  requires a `subpath` pointing at the extension's directory in the repo, plus
  an optional `rev`); or a bare name resolved from the central library. Put
  one in the library with `agentstack lib add-extension <name> --target
  <adapter> --path <dir>` (or `--git <url> --subpath <dir>`, optionally
  `--rev`) — same content scan and strict digest as `lib add`. A declaration
  with none of these is a validation error, so an unpinnable extension can
  never exist half-declared.
- **`target` is singular.** Extension code is written against one CLI's API, so
  it names exactly one adapter id — there is no `targets` list and no `"*"`
  fan-out. An unknown target, or `"*"`, is a validation error.
- **Reserved names.** Any name beginning with `agentstack-guard` is rejected —
  those artifacts belong to the host guard and the renderer never authors,
  overwrites, or prunes them.
- **Strict pinning.** Each extension gets a `[[extension]]` lock entry
  (`name` / `target` / `checksum`) pinned with the strict integrity-root
  digest — a symlink anywhere in the source tree is a hard error and `.git` is
  included, unlike the lenient skill digest. Executable content is never
  first-pinned at render time: an unpinned extension blocks. Run
  `agentstack lock` to pin or accept a change.

`apply` renders each extension by **copying** (never symlinking) the lock-pinned
source into its target harness's extension directory, so the harness loads the
exact bytes that were pinned — a post-render source edit changes nothing on the
harness surface until a re-render that must pass trust + lock again. Rendered
artifacts are tracked in a per-directory ownership ledger, so a re-render prunes
exactly what agentstack placed and never touches hand-installed files or the
guard's reserved artifacts. An untrusted or drifted project renders **zero**
extension bytes.

Two adapters render extensions today: **pi** (`~/.pi/agent/extensions`, or
`.pi/extensions` at project scope) and **OpenCode** (`~/.config/opencode/plugins`
— global only; a project-scope render falls back to the user dir). Targeting any
other adapter validates but **warns and does not render** — the harness exposes
no extension directory agentstack can deliver into.

Under `agentstack run --locked`, a dedicated `rendered-verify` gate re-checks
that each delivered *copy* still matches its lock pin before launch (a copy
tampered after render, with its source left untouched, would otherwise reach the
harness unreviewed); a harness with nothing rendered has nothing to verify and
is reported absent, never refused.

### Search across providers

`search` queries **your central library first** (skill names and their
SKILL.md frontmatter descriptions, plus library server names — labelled
`[library]`), then the embedded catalog **and the official MCP Registry**
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
content passes the install scan gate. `agentstack lib pack-init` scaffolds a
publishable pack; the dashboard's Discover tab browses candidates and shows the
`add from git:…` command to install one with the same gates. (Semver ranges and
transitive pack dependencies are
deliberately not in v1.)

### `adopt` and `add`

`adopt` is the reverse of `apply`: pull a hand-added server from a target
config back into the manifest, lifting its inline secret, preserving
comments — the keep-side of every [drift decision](#drift-adopt-or-apply).
`add` is the flag-driven (scriptable / agent-operable) way to add a server or
skill to the manifest, optionally into a profile; comments preserved.

### `report usage` (usage analytics)

Local usage analytics: activation counts + per-capability footprint (which
target/scope slots it's live in) + **context cost**. `report usage --live` measures
each server's `tools/list` token footprint through the gateway (HTTP + stdio)
and caches it (`~/.agentstack/footprint.json`); `report usage`, `explain`, and the
dashboard's Servers matrix then show what each server taxes every session
offline, and `report usage` flags dead weight — high-cost, never-activated servers —
with the exact `remove` command.

### Wire proxy (`proxy`)

Where `report usage --live` gives you a **static** estimate — what a server's
`tools/list` *would* cost — the wire proxy gives you **runtime ground truth**:
what the `tools` block actually costs, in input tokens, on every real turn your
harness sends. It's the on-wire complement to `src/footprint.rs`'s static
counter, and it's the built-in version of the hand-rolled logging proxy from
[*How to kill the bloat in Claude Code's system
prompt*](https://www.aihero.dev/how-to-kill-the-bloat-in-claude-codes-system-prompt).

**Point a harness at it.** `agentstack proxy` stands up a loopback proxy
(default `127.0.0.1:8787`; `--port`, `--upstream` to override) that relays every
request VERBATIM to the Anthropic API. Set the harness's base URL and use it
normally:

```bash
export ANTHROPIC_BASE_URL=http://127.0.0.1:8787
agentstack proxy             # blocks, serving, in one shell
# …drive Claude Code (or any Anthropic-API harness) as usual…
agentstack report wire       # --json for the raw aggregate
```

**What `proxy` does.** For each `/v1/messages` request it walks the
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

**What `report wire` shows.** It aggregates the log into a ranked,
per-capability table: `tools` (typical per-turn tool count), `avg tokens/turn`,
`calls` (how many times any of that capability's tools appeared in a
`tool_use`), and a loaded-vs-called `hint`. Headline tools/tokens are the max
seen in a single turn — a turn re-sends the whole block, so summing across turns
would inflate it. The hint is a modest ranking signal, not a verdict: a
capability called at least once is `keep`; the costliest never-called one is a
`drop / lazy` candidate (demote to a lazy server or drop it from the profile);
the cheap-and-unused rest is `watch`. Those names are the same servers and
profiles agentstack already manages, so the report closes the loop with the
static `footprint` / `report usage` / `doctor` lenses using on-wire evidence.

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
  the removal is intended. Both scopes are checked: entries a
  `--scope project` apply recorded (e.g. in `.mcp.json`) get their own line,
  labeled `(project)` and hinting `apply --scope project --write`.
- Entries recorded by a **different manifest** are never pruned implicitly
  (global scope is shared by every manifest on the machine): `apply` keeps
  them and says so, and `diff`/`doctor` keep surfacing them as kept — not as
  pending deletions — until you decide. Prune them with an explicit
  `apply --prune-foreign` (it still works after the guarded write recorded
  its own set), or `adopt` them into the current manifest.

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
hooks, records the write, and reverts it on `end` (or `end --all` to clean up
everywhere). `freeze` captures the session's resolved set — the profile's
servers plus the skills actually loaded — into a new profile
(default `<profile>-frozen`) so CI can replay it deterministically; review the
manifest edit, then `agentstack lock`. The same start/end lifecycle backs the
MCP `agentstack_session_*` tools; the dashboard shows an active session
read-only.

## Live runs (`agentstack run`)

Launch an agent CLI as a **tracked run** and control it without leaving
agentstack. A run is a real OS process agentstack owns: it's spawned in its own
process group (so a kill takes down the whole tree), recorded in
`~/.agentstack/runs.json`, and visible to any other agentstack process — so the
dashboard can see runs it didn't start.

```bash
# Launch a harness, attached to your terminal, with a profile applied for the
# life of the run (its servers + skills are reverted automatically on exit).
agentstack run claude-code --profile design
agentstack run codex --profile backend --scope project
agentstack run claude-code --keep        # leave the profile applied after exit

# See runs (also in the dashboard's Runs panel) and stop them here.
agentstack report runs         # table; add --json for scripting
agentstack kill <id>           # SIGTERM, then SIGKILL if it won't go
agentstack kill <id> --force   # SIGKILL immediately
```

Launching is a terminal act (the CLIs are interactive TUIs); the dashboard's
**Runs** panel is for observing tracked runs (it shows the `kill` command). The registry is
self-healing: a run whose wrapper died is pruned on the next `report runs`. A
profile-bound run uses the session engine, so one is allowed per directory at a
time. Unix only for now. Showing the full per-run trust footprint in the
dashboard is part of the portable-runtime roadmap.

### Execution posture

Every run is labelled with its **enforcement posture** — how strongly the
effective policy is actually enforced at runtime, not merely declared. The label
appears on the run banner, in `agentstack run --sandbox --plan`, and in
`agentstack report run <id>`:

| Posture                                 | Mode                          | What it means |
|-----------------------------------------|-------------------------------|---------------|
| `HOST / ADVISORY`                       | `agentstack run` (host)       | No container. The gateway still brokers MCP tool calls, but nothing confines the process's own egress or filesystem — policy is advisory. The banner says so, once. |
| `HOST / PROTECTED`                       | `run --locked`                | No container either — but content trust, strict lock verification (including D3 executable pins), and policy admission are enforced *before* launch, the run's MCP surface is frozen into a machine-authenticated grant the bridge serves verbatim (no mid-run re-derivation, mutating control-plane tools refused), and every decision is recorded. Not kernel isolation: the harness still runs as you, on the host, and the evidence is a cooperative local audit trail. See [The Protected tier in detail](#the-protected-tier-in-detail-run---locked). |
| `SANDBOX / PROXIED · DIRECT ROUTE OPEN` | `run --sandbox`               | Container with a host-side egress proxy; proxied HTTPS egress is checked against compiled policy, but the container's bridge network still has a direct route a proxy-ignoring process could use — only `--lockdown` removes it. |
| `LOCKDOWN / ENFORCED · NO DIRECT ROUTE` | `run --lockdown`              | Container on an internal-only network whose sole peer is the egress sidecar — enforced *and* topologically confined (no host route, no direct internet). |

`ENFORCED` is reserved for lockdown, where the confinement is topological. The
honest claim even there is *unapproved egress is blocked*, not that
exfiltration is impossible. Host mode makes no runtime claim at all — it
only labels itself advisory so the two are never confused. A sandbox run records
its posture beside the flight-recorder log, and a `--locked` run carries it in
its `attempt_started` event, so `agentstack report` can label either after the
fact (`report --json` carries the `posture` slug).

`agentstack doctor` also prints a one-word **machine-policy posture** — `open`
(no machine policy, or empty/unreadable and failing open), `restrictive` (a
rename-proof `"*"` rule or a `[policy.filesystem]` scope binds every server), or
`mixed` (only dodgeable named-server rules). "restrictive" means a `"*"` rule
binds every server, not that the policy is tight — the line never overstates.

Ready-to-use machine policies for the common postures live in
[`examples/policies/`](../examples/policies/) (`compatible`, `developer`,
`locked-down`, `ci`), each a parseable `~/.agentstack/agentstack.toml` with
comments explaining every choice.

### The Protected tier in detail (`run --locked`)

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
   artifact **verbatim** — same ruleset, same servers the gates admitted — and
   fails closed (serving nothing, loudly) on a failed MAC, schema/version
   skew, a consent digest that no longer matches (any post-freeze manifest
   edit), lost trust, or a machine ceiling that changed since freeze. It
   never falls back to re-deriving authority from disk.
4. **Frozen control plane.** Under `--grant`, control-plane tools that would
   swap the surface or mutate state mid-run — lease open/close/freeze,
   `session_start` (which resolves secrets into native configs),
   `session_end`/`freeze`, `add_skill`/`add_server`/`add_from`,
   `create_profile` — are refused for the run's duration. Read-only
   discovery and trust-gated skill loading still answer.
5. **`--profile <name>` is a fence**, not a session: gates, grant, artifact,
   and bridge all see only that profile's server subset; no native session
   state is applied or reverted.
6. **Hygiene.** The original project MCP config is parked in the run's
   private dir (never left in the repo) and restored byte-identical; a
   sentinel makes overlapping locked runs refuse instead of stacking; a
   crash leaves the more restrictive state.

`run --locked --plan` walks the whole gate sequence read-only and prints
every decision the live path would — each gate's `✓` on the happy path, every
blocker at once on a refusal — plus the grant digest a live run would freeze,
and mutates nothing. Honest limits: this is pre-launch gating and a frozen surface on the
HOST tier, not isolation (the harness still runs as you — `--lockdown` is the
kernel fence), and the sealing key is readable by the same user, so the
artifact MAC defeats cross-machine replay and tampering, not a same-user
process (which already runs unconfined here). The asserted walkthrough is
[`examples/projects/locked-run/`](../examples/projects/locked-run/); the full
contract is [`docs/design/locked-run-contract.md`](design/locked-run-contract.md).

## Agent-operable (`agentstack mcp`)

agentstack can run as an MCP server over stdio, so the agent itself can discover
and propose capabilities. The control-plane surface it advertises, grouped:

- **Discover & inspect** (read-only): `agentstack_search`, `agentstack_list`,
  `agentstack_doctor`, `agentstack_explain`, `agentstack_diff`.
- **Propose manifest edits**: `agentstack_add_from`, `agentstack_add_server`,
  `agentstack_add_skill`, `agentstack_create_profile`. Writes go to the
  **manifest only** (commit-safe `${REF}`s, nothing executed) — the agent
  proposes, a human reviews and runs `apply` (the §9g/D20 trust gate).
- **Progressive skill loading**: `agentstack_list_loadable`, `agentstack_load`.
- **MCP profile leases**: `agentstack_lease_open`, `agentstack_lease_status`,
  `agentstack_lease_close`, `agentstack_lease_freeze` — see
  [MCP profile leases](#mcp-profile-leases-one-connection-one-capability-fence).
- **Native sessions**: `agentstack_session_start`, `agentstack_session_end`,
  `agentstack_session_list`, `agentstack_session_freeze` (these render/revert
  native config; `start` takes a `profile`, not a plugin).
- **Proxied tool surface**: `tools_search`, `tools_bindings`, and — on
  sandbox-enabled builds — the experimental
  [`tools_execute`](#experimental-tools_execute).

Register it once per CLI:

```bash
agentstack gateway connect claude-code codex   # dry-run: shows the config diff
agentstack gateway connect --all --write       # every installed harness
```

`gateway connect` writes one small entry — `agentstack mcp --auto-project` — into the
CLI's **global** MCP config (undo with `gateway disconnect`, verify with `doctor`).
You can still register it by hand like any stdio MCP server if you prefer:

```json
{ "mcpServers": { "agentstack": { "type": "stdio", "command": "agentstack", "args": ["mcp", "--auto-project"] } } }
```

### Transparent mode (`--transparent`)

Two ways to expose the proxied surface:

- **Compact (default)**: `tools/list` advertises agentstack's control-plane
  tools only; upstream tools collapse behind `tools_search` (and code mode),
  so the agent's tool context stays bounded no matter how many tools the
  upstreams expose. Requires the agent to use `tools_search` → call by
  namespaced name.
- **Transparent** (`agentstack mcp --transparent`, or register it with
  `connect --transparent`): `tools/list` additionally advertises every
  policy-filtered upstream tool as `<server>__<tool>` — a drop-in MCP proxy
  any standard client can consume with zero agentstack knowledge. The
  firewall, trust gate, and audit log apply identically; the first listing
  pays upstream discovery (bounded per-server timeouts, partial results).

In auto-project mode the gateway builds lazily, so transparent mode declares
the `listChanged` capability and sends `notifications/tools/list_changed`
once the (trust-gated) gateway comes up — clients re-fetch `tools/list` and
see the upstream tools without ever calling a control-plane tool first.

### The zero-files gateway (`--auto-project` + `trust`)

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
warning. A **missing** lockfile is the zero-lock workflow (everything
unpinned, warned); a lockfile that exists but can't be read — parse error,
future schema — fails **closed**: its pins are unknowable, so
library-referenced servers are refused until it's fixed, and `agentstack
trust` errors rather than reviewing an unverifiable surface. Re-locking
changes the lockfile, which re-gates trust — the pin, the runtime check, and
the consent digest close the loop.

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

The `initialize` handshake carries an ambient skill index: the server's
`instructions` field lists every loadable skill (name + one-line description),
so an agent sees the menu before its first tool call and can go straight to
`agentstack_load`. The index is exactly what `agentstack_list_loadable` would
return at connect time — the trust gate (untrusted projects list names only,
descriptions are inert bundle content) and any active session fence apply
identically. In `--auto-project` mode the project is only established after
the client answers `roots/list`, which is after `initialize`, so there the
instructions point at `agentstack_list_loadable` instead of probing the cwd.

Honest limits: MCP servers, secrets, the tool firewall, the call audit log, and
skills-over-MCP (`agentstack_list_loadable`/`agentstack_load`) create no
per-project native artifacts. Native skill folders and instruction files (`CLAUDE.md`/`AGENTS.md`)
are read from disk by the CLIs themselves and still need render mode
(`apply`/`use`) — `gateway connect` prints this per CLI.

### MCP profile leases: one connection, one capability fence

An MCP profile lease is process-local state owned by one `agentstack mcp`
process. It is the zero-file counterpart of a native `session start`, but the
cleanup contract is different: a lease never renders harness config, creates a
native skill folder, or writes `sessions.json`, so close/process exit has
nothing to restore.

The normal agent-side sequence is:

```text
agentstack_lease_open({ "profile": "backend" })
agentstack_list_loadable({})
agentstack_load({ "name": "sql-review", "reason": "review this migration" })
agentstack_lease_status({})
agentstack_lease_close({})
```

These are MCP tool calls, not CLI shell commands. While the lease is active:

- the live gateway exposes only servers from the selected profile;
- `agentstack_list_loadable` and `agentstack_load` expose only that profile's
  skills (plus the embedded `using-agentstack` manual);
  `agentstack_list_loadable` takes an optional `query` (case-insensitive
  substring over name + description) that filters **within** the fence;
- the first load of each skill is recorded with its reason, while repeated
  loads return the body without duplicating the trail;
- trust, lock/digest verification, machine policy, project policy, and call
  auditing continue to apply.

`agentstack_lease_freeze({ "name": "backend-observed" })` converts the leased
profile's server list plus the skills actually loaded into a new manifest
profile. It is a manifest-only proposal: review the edit, then run
`agentstack lock` to refresh the lockfile. It never applies or renders the new
profile.

The MCP control plane refuses to place a lease over an active native session,
or start a native session over its active lease. The lease is deliberately
invisible to separate processes, however: another terminal cannot inspect it.
Use `agentstack_lease_status` from the same MCP connection. Opening a different
valid profile replaces the current lease and starts a fresh in-memory load
trail.

See [`examples/mcp-profile-lease`](../examples/mcp-profile-lease/) for a
runnable stdio lifecycle with assertions that no native artifacts are created.

### Compact proxied surface + code mode

`agentstack mcp` also **proxies** the project's MCP servers — HTTP and stdio.
Stdio children spawn lazily in their own process group, get `${REF}`s resolved
into their env per session, and are tree-killed when the session ends. Instead of
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
**harness's** own sandbox. The client is fetched through the same MCP surface
(`tools_bindings` returns it inline) — there is nothing to install on disk.

### Experimental `tools_execute`

Sandbox-enabled release builds can also host the program themselves. The MCP
tool is advertised only when the machine manifest—not a repository—contains:

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

The default export becomes the JSON result. Imports are offline: no npm/package
installation or arbitrary module fetch exists. The runtime is the pinned
official Node 22 slim image; guest code runs as uid/gid `65532`, with a
read-only root, 16 MiB `noexec` tmpfs, all Linux capabilities dropped,
`no-new-privileges`, 128 MiB memory, one CPU, and 32 PIDs. Its only network peer
is the egress sidecar. The normal proxy port requires a credential the guest
does not receive; the separate execution relay requires a per-run token and
enforces the exact grant and call count before dispatching to the same
`Arc<Gateway>` used by ordinary MCP calls.

The policy ruleset is mounted only into the egress sidecar, outside the guest's
`/app`. The default export is written to a separate, pre-created result-file
bind capped at 1 MiB, so result size does not consume the stdout/stderr budget.
The host relay listens on host interfaces because Docker's host bridge cannot
reach a host loopback listener; it exists only for the execution lifetime and
accepts no request without the per-run token.

There is no host fallback. Missing trust, the sandbox build feature, Docker,
the pinned image, the sidecar, relay authentication, recording, or teardown
returns a stable non-sensitive error. Source, input, result, tokens, and secret
values are not recorded; their digests, limits, timings, outcomes, and
attributed tool calls are. A focused implementation review and regression-
hardening pass completed on 2026-07-13. This surface remains experimental while
image provenance/SBOM publication and longer-running soak evidence remain
outstanding.

## Dashboard

The one place to *see* everything the sections above manage, across every
harness, without running a write. Open it when you want to look, not change —
every action still happens through the CLI.

An embedded localhost server + a self-contained UI (shadcn aesthetic,
hand-written CSS — no Node, no framework, still one `cargo build`):

```sh
agentstack dashboard            # token-gated, localhost-only view
agentstack dashboard --no-open  # print the URL, don't open a browser
```

It opens a **read-only** cross-harness view with secrets, skills, settings,
profiles, runs, and usage panels. It shows state, previews diffs, and **runs
doctor** (full check-up rendered in the Health tab), but it never writes: bound
to 127.0.0.1 and token-gated, the server exposes read (GET) routes only, so a
POST to any path 404s — the read-only property is a property of the router, not
the UI, and a route-matrix test pins it. Secret values never reach the browser.
Every change happens through the CLI; where a control would live, the dashboard
shows the command to copy (e.g. `agentstack apply --write`, `agentstack secret
set <REF>`, `agentstack use <profile> --write`).

On a machine with no `agentstack.toml`, it opens a welcome screen instead: the
agent CLIs it detected, the MCP servers already in their configs, where those
tools disagree today, and the `agentstack init` command to reverse-engineer a
manifest (lifting inline secrets to `${REF}`s) from what's on disk.

**The tabs:**

- **Overview** — stat tiles, next-actions, stack summary, the zero-files gateway
  (connected CLIs + this repo's trust state), profiles, and usage. Each
  next-action links to the relevant tab or opens a read-only diff.
- **Runs** — live `agentstack run` processes, with uptime, profile, reachable
  capabilities, and per-run **Calls** (the audited tool-call footprint, digests
  only). Stop one with the shown `agentstack kill <id>`.
- **Discover** — search the embedded catalog and the official MCP Registry; each
  result shows its trust signals and the `agentstack add from <id>` command.
- **Servers** / **Skills** — the cross-harness matrix: where each capability is
  enabled, per CLI and scope (global/project switch at the top). Click a server
  for its config and the trust lens (**Explain trust ⓘ**); the **context**
  column shows each server's per-session token cost (click to sort). Skills also
  lists dirs discovered on disk but not in the manifest, each with the
  `agentstack adopt <name>` command.
- **Settings** — each tool's current settings, read from its real config file,
  and which keys agentstack manages. Edit `[settings.<tool>]`, then `agentstack
  apply --write`.
- **Hooks / Instructions / Extensions** — read-only inventories of lifecycle
  hooks, CLAUDE.md/AGENTS.md fragments, and content-pinned native add-ons.
- **Secrets** — every `${REF}` the manifest mentions, whether it resolves on
  this machine and from which layer (env / varlock / keychain / .env); missing
  ones show `agentstack secret set <REF>`. Values are never shown.
- **Activity** — every apply, with the files it touched; roll one back with the
  shown `agentstack restore`.
- **Health** — the standing summary plus **Run doctor**: the same checks as
  `agentstack doctor`, rendered as the familiar ✓/⚠/✗ report.
- **Proxy** — the wire lens, the same ranked report as `agentstack report wire`:
  per-turn tools and token weight, per-capability. Observe-only.
- **Insights** — **Optimize**, **Analyze**, and **Stats** as three read-only
  reports, each recommendation carrying its evidence and the exact command/TOML
  to act on it.

Anywhere drift exists — the pending bar, an Overview next-action, the Health
tab — **Review** opens a real diff of every native config that would change and
shows the `agentstack apply --write` to reconcile it; the write happens in your
terminal.

## Optimize (`agentstack optimize`)

Turns the signals agentstack already collects — activation counts, the gateway
call audit log, per-server context costs (`report usage --live`), the trust ledger —
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

## Field notes and addenda

Operational details that are easy to trip over, and a few reference facts that
were previously only on the docs site.

### Launch timing and switching

AgentStack assumes harness-native configuration is established **when the CLI
launches**. `use` and `session start` write a profile's native MCP config and
skills to disk, but a CLI that is already running may not observe the change
until it is relaunched. To switch profiles deterministically for a running
CLI — `use profile-B --write` rewrites the files, then relaunch it — rather
than assuming a live reload. Live, in-process switching is the MCP lease path's
job, not the native-file path's.

### Session and run recovery

`session end`/`end --all` and `run`'s auto-revert restore the pre-session state.
Two edge cases:

- **Force-killed parent.** If the parent `agentstack run` process is killed
  (or the machine stops) before it can revert, cleanup cannot execute; recover
  with `session end` or `session end --all`.
- **Skill-restore exactness.** Server files are snapshotted for exact restore.
  For skills, the current implementation records the names a session *newly
  added*; replacing an already-managed skill with the same name is an edge case
  that is not yet snapshotted, so restore of that specific case is not promised
  to be byte-exact.

### Lease survival across a mid-connection change

If manifest or lock bytes change while an `--auto-project` connection is open,
AgentStack empties the **live** gateway — no further server spawns, secret
resolution, or bundle content. The **in-memory lease object itself** can still
be inspected, and `lease_freeze` can still propose a manifest profile from it,
precisely because a lease serves no bundle content, resolves no secret, spawns
no server, and touches no file. Any renewed activity still requires fresh review,
locking where needed, and trust.

### Central library: server definitions and bundled catalog

- `lib add-server <name> --file <definition.toml>` stores a standalone server
  definition; `lib add-server <name> --from-manifest` lifts an existing inline
  `[servers.*]` entry into the library. Both keep `${REF}`s intact and **warn on
  literal secret-looking values at add time** (surfaced, not scrubbed or
  blocked) — an earlier checkpoint than `lib sync`'s fail-closed push-time gate.
- The bundled catalog (`crates/cli/catalog/skills/`) ships ready-made skills
  including `run-codex`, `sync-library`, `analyze-usage`, `route-by-cost`, and
  `using-agentstack`, among others; `search` finds them across providers.
- Every central-library flow is exercised by
  `examples/sandbox/demo-central-library.sh`, a sandboxed demo that never
  touches your real provider folders.

### `tools_execute` cancellation

Cancelling an execution kills the **entire process tree**: the executor's
container and its children are torn down (bounded by the 32-PID limit), so a
cancelled or timed-out run leaves no orphaned guest processes.

## All commands

The full command surface in one place, generated from the CLI's own command
tree by `agentstack self docs --write` (CI fails if this list goes stale, and
regenerating trues it up). Each line is a top-level command with its one-line
summary, visible subcommands, and long-form flags; hidden commands are marked.
Reach for it when you need the exact verb, flag, or subcommand.

<!-- agentstack:generated commands -->
- **`setup`** _(hidden)_ — Hidden alias of interactive `init` — same guided wizard, older name — flags `--target/--profile/--scope`
- **`init`** — Set up everything in one command: detect, import, choose, apply, verify — flags `--global/--force/--dry-run/--secrets/--no-keychain/--yes`
- **`status`** — Where this project stands, on one screen: detected CLIs, manifest, trust, secrets, and the one next step
- **`add`** — Add a server or skill to the manifest — subcommands `from/server/skill`
- **`set`** _(hidden)_ — Create or update a manifest entry in place (idempotent `add`) — subcommands `server`
- **`search`** — Search the capability catalog (and mark what's already added)
- **`apply`** — Render the manifest into each target's native config — flags `--target/--profile/--dry-run/--write/--scope/--allow-unresolved/--prune-foreign/--no-gitignore`
- **`instructions`** _(hidden)_ — Compile [instructions.*] into each CLI's CLAUDE.md / AGENTS.md — flags `--target/--scope/--write`
- **`doctor`** — Verify everything is wired up: adapters, secrets, drift, quirks, skills — flags `--ci/--live/--fix/--deep/--all/--json`
- **`dashboard`** _(hidden)_ — Open the local web dashboard — a read-only view of your stack — flags `--port/--no-open`
- **`remove`** _(hidden)_ — Remove a server or skill from the manifest (and lockfile) — flags `--write`
- **`install`** _(hidden)_ — Fetch skill sources into the store and write the lockfile — flags `--locked/--allow-flagged`
- **`lock`** _(hidden)_ — Resolve each profile's skill + server refs and pin `agentstack.lock` — flags `--profile/--update/--upgrade/--all/--with-instructions/--yes/--write`
- **`lib`** _(hidden)_ — Manage the central capability library — subcommands `add/add-server/add-extension/add-hook/list/remove/remove-server/remove-extension/remove-hook/sync/pack-init`
- **`adopt`** _(hidden)_ — Keep a hand-edit: pull drifted native config back into the manifest — flags `--target/--scope/--write/--no-keychain`
- **`use`** — Activate a profile: render its servers + materialize its skills — flags `--target/--scope/--write/--allow-unresolved/--prune-foreign/--no-gitignore`
- **`session`** _(hidden)_ — Manage ephemeral sessions: load a profile for now, then revert it — subcommands `start/end/list/freeze`
- **`run`** — Launch an agent CLI as a tracked run — flags `--locked/--profile/--scope/--keep/--sandbox/--lockdown/--plan`
- **`kill`** _(hidden)_ — Kill a tracked run by id (and revert its profile if it owned one) — flags `--force`
- **`report`** _(hidden)_ — Every "what happened" view in one place — subcommands `run/runs/usage/calls/wire`
- **`sign`** _(hidden)_ — Sign this project's agentstack.lock with a fresh ed25519 key (writes a detached agentstack.lock.sig, prints the public key to publish) — flags `--print-key-only`
- **`verify`** _(hidden)_ — Verify agentstack.lock against a published ed25519 public key and its detached signature — flags `--pubkey/--signature`
- **`guard`** _(hidden)_ — Machine-level destructive-command guard — subcommands `test/install/uninstall/status`
- **`gateway`** _(hidden)_ — The zero-files gateway: register it once per CLI (`connect`) and every trusted repo brings its own servers through `agentstack mcp --auto-project` with no per-project files — subcommands `connect/disconnect`
- **`trust`** — Trust a project's manifest for the zero-files gateway (direnv-style) — flags `--list/--revoke/--yes`
- **`mcp`** _(hidden)_ — Run agentstack as an MCP server over stdio (for an agent to call) — flags `--auto-project/--transparent`
- **`diff`** _(hidden)_ — Show drift between the manifest and the on-disk configs — flags `--target/--profile/--scope`
- **`explain`** _(hidden)_ — Explain a server or skill before you rely on it
- **`optimize`** _(hidden)_ — Turn agentstack's collected signals into concrete recommendations — flags `--json/--write/--since`
- **`proxy`** _(hidden)_ — Start the wire relay: a localhost proxy in front of the Anthropic API — flags `--port/--upstream`
- **`restore`** _(hidden)_ — Undo a recorded write: revert an apply/use/session history entry (servers, settings, hooks, instructions), or restore one adapter's config from its single-slot backup — flags `--last/--scope/--write`
- **`secret`** _(hidden)_ — Manage secrets in the OS keychain — subcommands `set/get/rm/list`
- **`settings`** _(hidden)_ — Edit a target's native `[settings.<target>]` entries — subcommands `set/unset`
- **`export`** _(hidden)_ — Export the manifest (+ lock, + optionally secrets) as an encrypted bundle — flags `--output/--secrets/--passphrase`
- **`import`** _(hidden)_ — Import an encrypted bundle on a new machine — flags `--force/--no-keychain/--passphrase`
- **`adapters`** _(hidden)_ — Inspect the available CLI adapters — subcommands `list/show/validate`
- **`self`** _(hidden)_ — Manage this binary's own install: `self link` puts a stable `agentstack` on PATH (a symlink, no installer needed); `self which` shows which binary a bare `agentstack` runs and flags stale links — subcommands `link/which`
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
`export`/`import` · portable lifecycle hooks · agent-operable `mcp` server · local read-only dashboard
(server/skill matrices, Discover, Doctor, Runs; GET-only, copies the CLI command for every change) · live runs
(`run`/`report runs`/`kill` + dashboard Runs panel) · GitHub Action trust gate ·
nightly adapter-conformance CI · zero-files gateway (`gateway connect` + `mcp
--auto-project` + digest-pinned `trust`) · `optimize` (evidence-backed
recommendations from usage/audit/cost signals, safe-class `--write`) ·
fail-closed `lib sync` secret gate (all server fields + outgoing history) ·
machine-level destructive-command `guard` · Docker `run --sandbox` and
no-direct-route `--lockdown` with compiled egress/filesystem policy · per-run
`report` (lifecycle, limits, egress, tool calls, secret refs) · detached
`sign`/`verify` · experimental frozen-plan `tools_execute`.
