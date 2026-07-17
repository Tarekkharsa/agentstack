# Feature reference

The complete, implemented-and-tested feature inventory. The
[README](../README.md) is the tour; this is the map.

**Contents:** [Core engine](#core-engine) ·
[Where rendered files live](#where-rendered-files-live-three-modes) ·
[Governance, trust, and secrets](#secrets-and-trust) ·
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
  connect` registers the gateway once per harness (one write to each
  harness's global config) and every **trusted** repo serves its own stack
  live. `agentstack_lease_open(profile)` can fence one MCP connection to a
  profile without rendering native files; `agentstack_lease_status` shows its
  in-memory load trail, `agentstack_lease_freeze` promotes the observed set to
  a manifest profile (review it, then run `agentstack lock`), and close/process
  exit drops it. A machine-local
  `codemode/endpoint.json` coordinate may exist for the connection's duration — see
  [the zero-file bridge](#the-zero-file-bridge---auto-project--trust).

**Recommendation:** prefer the zero-file lease path for interactive work when
the harness supports MCP; use static or clean-at-rest delivery when the harness
must read native skill/instruction files. Add `--sandbox --lockdown` when the
agent process itself needs isolation—a lease is a capability fence, not a
sandbox. See [the primitives and decision table](https://tarekkharsa.github.io/agentstack/primitives.html).

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

### One undo verb: `restore`

Every write agentstack makes (servers, settings, hooks, instructions — even
the owned-server manifest refresh) is captured in the history engine before it
lands. `agentstack restore` lists the recorded changes; `restore <id> --write`
(unique prefix) or `restore --last --write` reverts one — the same undo the
dashboard button drives. `restore <adapter>` keeps the original single-slot
config restore as a fallback. Reverted files simply show up as pending again.

### `doctor` shows what you use

Every check always runs, but the default report prints only the sections
relevant to this project — a feature you've never touched (the zero-files
bridge, plugin recipes, reproducibility pins…) stays out of the way until it
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

`lib consolidate` moves scattered skills from every CLI's folder into the
library and symlinks the originals back — preview first. Manage the rest with
`lib list` / `remove` / `remove-server`.

## Capabilities

### Package manager

Skills declare a source (`path` or `git`); `install` fetches them into
`~/.agentstack/store/` and writes a SHA-256 `agentstack.lock`;
`install --locked` is reproducible (CI-safe); `lock --update` re-resolves git skills;
`remove` drops a capability from manifest + lock. `install` is profile-aware:
skills a profile references by name (resolved from the central library, no
inline `[skills.*]` entry) keep their lock pins through the reconcile pass —
pin or refresh those with `agentstack lock`.

Skill and library content digests always hash current bytes; there is no digest
cache on the verification path. (Older versions kept a stat-fingerprint cache and
may leave a harmless orphaned `~/.agentstack/digest-cache.json`; it is unused and
safe to delete.)

### Selective skills via profiles

`use <profile>` materializes only that profile's skills (symlink, with copy
fallback), pruning the rest it owns and never clobbering hand-made skill
dirs. The profile is optional: one declared profile is chosen automatically,
and a manifest with **no** profiles activates its full inline set as the
implicit default — `agentstack use --write` just works. Several profiles need
a name. When a prune empties the managed skills dir (deactivation,
`session end`), the dir itself is removed too — rmdir semantics, so a dir
holding any user content always survives.

`setup` finishes with the same activation: it picks the profile (an explicit
`--profile`, the only one declared, or an interactive offer of the
first-declared) and materializes its skills through the exact `use` code
path — so a completed setup leaves nothing left to activate. Plain `apply`
still never touches skills; it reminds you which profile activates them.

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
  an optional `rev`); or a bare name resolved from the central library. A
  declaration with none of these is a validation error, so an unpinnable
  extension can never exist half-declared.
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
publishable pack; the dashboard's Discover pane installs from a git URL with
the same gates. (Semver ranges and transitive pack dependencies are
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
agentstack report runs         # table; add --json for scripting
agentstack kill <id>           # SIGTERM, then SIGKILL if it won't go
agentstack kill <id> --force   # SIGKILL immediately
```

Launching is a terminal act (the harnesses are interactive TUIs); the dashboard's
**Runs** panel is for observing and killing tracked runs. The registry is
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

`run --locked --plan` prints the same fully-aggregated picture — every
blocker at once, the grant digest a live run would freeze — and mutates
nothing. Honest limits: this is pre-launch gating and a frozen surface on the
HOST tier, not isolation (the harness still runs as you — `--lockdown` is the
kernel fence), and the sealing key is readable by the same user, so the
artifact MAC defeats cross-machine replay and tampering, not a same-user
process (which already runs unconfined here). The asserted walkthrough is
[`examples/projects/locked-run/`](../examples/projects/locked-run/); the full
contract is [`docs/design/locked-run-contract.md`](design/locked-run-contract.md).

## Agent-operable (`agentstack mcp`)

agentstack can run as an MCP server over stdio, so the agent itself can discover
and propose capabilities — tools: `agentstack_search`, `agentstack_list`,
`agentstack_doctor`, `agentstack_add_server`. Writes go to the **manifest only**
(commit-safe `${REF}`s, nothing executed): the agent proposes, a human reviews
and runs `apply` (the §9g/D20 trust gate). Register it once per harness:

```bash
agentstack gateway connect claude-code codex   # dry-run: shows the config diff
agentstack gateway connect --all --write       # every installed harness
```

`gateway connect` writes one small entry — `agentstack mcp --auto-project` — into the
harness's **global** MCP config (undo with `gateway disconnect`, verify with `doctor`).
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

### The zero-file bridge (`--auto-project` + `trust`)

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
are read from disk by the harnesses themselves and still need render mode
(`apply`/`use`) — `gateway connect` prints this per harness.

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

## All commands

`setup`, `init` (`--global`), `add`, `search`,
`install` (`--locked`, `--allow-flagged`),
`lock` (`--profile`; `--update [NAME]` re-resolves git skills, `--upgrade
[PACK]` + `--all`/`--with-instructions`/`--yes`/`--write` re-resolves vendor
packs), `remove`, `apply` (`--scope`, `--write`, `--prune-foreign`), `diff`,
`explain`, `use <profile>`, `session`, `instructions`, `adopt`,
`lib add|add-server|list|remove|remove-server|sync|consolidate|pack-init`
(`lib add`: `--path`, `--git`/`--subpath`, `--allow-flagged`; `lib sync`:
`--init`, `--remote`, `--status`, `--allow-secrets`), `restore` (`--last`; a
recorded-change id or an adapter id),
`doctor` (`--ci`, `--live`, `--fix`, `--deep`, `--all`), `audit` (`--json`), `optimize` (`--json`, `--write`, `--since`),
`report run <id>|runs|usage|calls` (`run`/`runs`/`calls`: `--json`; `usage`:
`--live`; `calls`: `--since`, `--transcripts`), `proxy start|report` (`start`: `--port`,
`--upstream`; `report`: `--json`),
`secret set|get|rm|list`, `export`/`import`, `adapters` (`list|show|validate`),
`plugins`, `settings`,
`dashboard`, `mcp` (`--auto-project`, `--transparent`),
`gateway connect|disconnect` (`connect`: `--all`, `--transparent`, `--write`),
`trust` (`--list`, `--revoke` — pins the manifest layers **and lockfile**;
re-locking re-gates),
`guard` (`install|uninstall|status|test|check` —
the machine-level destructive-command hook for every agent CLI; cooperative,
see ENFORCEMENT.md), `run` (`--sandbox`)/`kill`,
`sign`/`verify` (`--pubkey`, `--signature` — detached ed25519 lockfile
signatures), `self link|which`.

This inventory is checked by a test against the CLI's own command list
(`tests/docs_commands.rs`) — a new subcommand fails CI until it's documented
here.

## Everything shipped so far

13 adapters · `init`/`add`/`apply`/`diff`/`use`/`instructions`/`adopt` ·
package manager (`install`/`lock --update`/`remove` + lockfile) · central capability
library (`lib` skills + servers referenced by name, digest-pinned in the lock,
drift in `doctor`/`explain`, `lib consolidate` into `lib/skills`) · secrets (keychain +
varlock) · scopes (global/project) · `doctor` (`--live`/`--fix`/`--ci`/`--deep`) ·
content scanning on install + `audit` · official MCP Registry provider +
`search`/`add from` · `[policy]` trust gate · native per-CLI settings
(`[settings.*]` → settings.json) · managed plugin recipes (`[plugins.*]` →
native Claude Code/Codex packages + marketplaces) · atomic writes + backups ·
`export`/`import` · `hook` · agent-operable `mcp` server · local dashboard
(server/skill matrices, Discover, add-skill, settings editor) · live runs
(`run`/`report runs`/`kill` + dashboard Runs panel) · GitHub Action trust gate ·
nightly adapter-conformance CI · zero-file bridge (`gateway connect` + `mcp
--auto-project` + digest-pinned `trust`) · `optimize` (evidence-backed
recommendations from usage/audit/cost signals, safe-class `--write`) ·
fail-closed `lib sync` secret gate (all server fields + outgoing history) ·
machine-level destructive-command `guard` · Docker `run --sandbox` and
no-direct-route `--lockdown` with compiled egress/filesystem policy · per-run
`report` (lifecycle, limits, egress, tool calls, secret refs) · detached
`sign`/`verify` · experimental frozen-plan `tools_execute`.
