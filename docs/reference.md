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
[reference field notes](archive/design/reference-field-notes.md).

**Contents**

**[Part I — The everyday loop](#part-i--the-everyday-loop)**

- [The everyday loop, new in v0.18.0](#the-everyday-loop-new-in-v0180)
  - [Drop a file, say yes (`agentstack yes`)](#drop-a-file-say-yes-agentstack-yes)
  - [Undo: `undo` and `restore`](#undo-undo-and-restore)
  - [Sharing is signing: `share` and `receive`](#sharing-is-signing-share-and-receive)
  - [A setup that already exists (`agentstack up`)](#a-setup-that-already-exists-agentstack-up)
- [Secrets and trust](#secrets-and-trust)
  - [Secret resolution](#secret-resolution)
  - [Where lifted secrets go (`init`)](#where-lifted-secrets-go-init)
  - [Unresolved secrets block writes](#unresolved-secrets-block-writes)
  - [Does it actually run? `doctor --live` and `doctor --probe`](#does-it-actually-run-doctor---live-and-doctor---probe)
  - [The whole way out: `uninstall`](#the-whole-way-out-uninstall)
  - [`doctor` shows what you use](#doctor-shows-what-you-use)
- [Drift: adopt or apply?](#drift-adopt-or-apply)
  - [`adopt` and `add`](#adopt-and-add)
  - [Search across providers](#search-across-providers)
  - [Selective skills via toolsets](#selective-skills-via-toolsets)
- [Live runs (`agentstack run`)](#live-runs-agentstack-run)

**[Part II — The power surface](#part-ii--the-power-surface)**

- [Core engine](#core-engine)
  - [The manifest](#the-manifest)
  - [Data-driven adapters](#data-driven-adapters)
  - [Rendering and merging](#rendering-and-merging)
  - [State tracking](#state-tracking)
  - [Scopes](#scopes)
- [Delivery — routing, and where rendered files live](#delivery--routing-and-where-rendered-files-live)
  - [Owned servers (`owner = "codex"`)](#owned-servers-owner--codex)
- [Agent-operable (`agentstack mcp`)](#agent-operable-agentstack-mcp)
  - [Transparent mode (`--transparent`)](#transparent-mode---transparent)
  - [The zero-files gateway (`--auto-project` + `trust`)](#the-zero-files-gateway---auto-project--trust)
  - [MCP toolset leases](#mcp-toolset-leases-one-connection-one-capability-fence)
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
  - [The Protected tier in detail (the default `run`)](#the-protected-tier-in-detail-the-default-run)
- [The library: linked source folders](#the-library-linked-source-folders)
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

This part is everything `agentstack --help` shows by default: the everyday commands — `init`, `status`, and `add` through `apply` and `use`, plus `yes`, `undo`, `up`, `share`, and `receive`, which are **v0.18.0 and later** — and the machinery directly behind them. Read only what is here and you can reach a working setup: one manifest rendered into every CLI's native config, credentials kept out of that config, hand-edits caught, and a toolset activated. The power surface in Part II is entirely opt-in — nothing in the everyday loop requires it.

## The everyday loop, new in v0.18.0

Four moments the older releases spell out as several commands each: taking in
a file you wrote, taking a write back, handing a setup to someone else, and
standing one up on a new machine.

> These five verbs — `yes`, `undo`, `share`, `receive`, and `up` — are v0.18.0
> and later; the current stable install serves v0.17.1, which spells the same
> moments out the long way. Where there is a longer equivalent, the section
> below names it. `agentstack --version` says which you have, and
> `agentstack self update --write` upgrades once v0.18.0 is final.

### Drop a file, say yes (`agentstack yes`)

Writing a skill of your own needs no manifest edit: drop the folder under
`.agentstack/skills/` and run `agentstack yes` (v0.18.0+). The files are
noticed at the next command touchpoint, pinned, and reviewed on one card; one
yes records them in the manifest and lock and renders them to every CLI, with
the undo named in the preview before anything is written. The one-step path
applies only to content you demonstrably wrote here (untracked in git, or
newer than the last review) — anything that arrived with a clone takes the
full staged review that `trust` owns, and declining leaves the staged bytes
untouched and inert. Walkthrough: [add a skill](howto/add-a-skill.md); exact
boundaries: [ENFORCEMENT](ENFORCEMENT.md#intake-detection-dropped-files).

### Undo: `undo` and `restore`

Two faces of one record. Every recorded **write** — servers, settings, hooks,
instructions, even the owned-server manifest refresh — can be taken back.
`agentstack undo` (v0.18.0+) lists your recent changes newest-first and
reverts to the point you pick; the revert is itself recorded, so going one
step too far is recoverable. `restore` works the same record as the
script-friendly primitive, one write at a time by id, and is present in every
release — on v0.17.1 it is the whole undo story. Full walkthrough and the
table of the five actions undone by their own verb:
[undo anything](howto/undo.md).

```text
agentstack undo                    # timeline: pick a point, revert to it
agentstack restore                 # list the recorded changes (ids)
agentstack restore <id> --write    # revert one (unique id prefix)
agentstack restore --last --write  # revert the most recent
agentstack restore <adapter>       # single-slot config restore (fallback)
```

Reverted files show up as pending again; both verbs read the same recorded
writes and either can roll each one back.

### Sharing is signing: `share` and `receive`

`agentstack share <name>` (v0.18.0+) bundles this setup — manifest, lock, and
pinned content — and signs it as part of sharing (signing is not a flag: an
opt-in signature is one nobody opts into). `agentstack receive <path>`
(v0.18.0+) is the other side: the bundle is staged inert and carded first,
exactly like every other intake path — a signature from a publisher you
recognize makes the card shorter, never optional. `agentstack publisher`
manages your publishing key and the publishers you recognize; `sign`/`verify`
remain the scriptable primitives on the lockfile itself, and on releases
without `share` they are how a lockfile gets signed at all.

### A setup that already exists (`agentstack up`)

The moment is sitting down at a machine that has the checkout but nothing
configured. `agentstack up` (v0.18.0+) is that whole moment in one command: it
finds the agent CLIs this machine actually has, verifies the environment
against `agentstack.lock`, renders each CLI's config, and then names what is
left — which on a new machine is this machine's secrets, since values never
travel with a manifest.

The division of labour with `init` is the easy way to remember it: **`init`
creates a setup that does not exist yet** (it reads what your CLIs already
hold and writes the manifest), while **`up` materializes one that already
does**. You run `init` once, ever, per project; you run `up` once per machine.

```text
agentstack up                     # detect, verify, render, then say what's missing
agentstack up --targets claude    # only render these CLIs
agentstack up --toolset review    # materialize this toolset rather than the active one
agentstack up --no-gitignore      # skip the managed .gitignore block
```

`--manifest-dir <DIR>` points it at a project or manifest directory other than
the current one. On v0.17.1 the same journey is `agentstack apply --write`
followed by `agentstack doctor`, reading the missing-secret fix off the doctor
report yourself. Walkthrough: [share one setup with your
team](howto/team-setup.md).

## Secrets and trust

The enforcement core: how a secret resolves, where a policy narrows what a
server may do, and what every brokered call records. Read it if you run
untrusted repos, resolve credentials on this machine, or want a machine ceiling
no project can loosen.

### Secret resolution

The chain — process env → **varlock** → **OS keychain** → project `.env` — and
the `${REF}` rules live in [concepts.md — secrets](concepts.md#secrets);
unresolved refs are reported, never blanked. **varlock is the recommended
vault** — the one link in the chain that keeps values out of the project
entirely; the OS keychain and a gitignored `.env` are the local fallbacks.

Operational specifics: the varlock
link activates only when the project opts in (a `.env.schema` next to the
manifest, in `.agentstack/` — the directory the chain probes) and the
`varlock` binary is runnable, otherwise the chain silently skips it. That
silence is why `agentstack doctor` reports varlock's health in its **Secrets**
section: an opted-in project whose binary is missing would otherwise degrade
without a word. When active,
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
next to the manifest, and `secret set --env-file` targets that same `.env`. The
manifest itself only ever holds `${REF}` placeholders (rule 5).

`init` also **offers** a `.env.schema` when it lifts references — the opt-in for
varlock, written next to the manifest in `.agentstack/`, the same directory the
resolution chain probes. The offer is declined silently when nothing is
interactive (so a scripted `init` writes exactly what it wrote before), and an
existing schema is never overwritten. The file declares **names with empty
values and nothing else**, so it is safe to commit: values stay in the vault,
and a declared name with no value still fails closed at use time — which is the
whole point of `${REF}`.

Because that file holds real values rather than placeholders, it gets two
protections the rendered configs don't need. It is written **mode `0600`** —
owner-only, never the ambient umask — and a write tightens a file an older
version left more permissive; `doctor` warns (with the exact `chmod`) about any
`.env` still readable by other local accounts. And in a git repo it is ignored by
an **anchored** rule naming just that path (`/.agentstack/.env`), written outside
the managed `.gitignore` block so a re-render can never drop it. The rule is
deliberately not a bare `.env`: that would match at every depth and silence the
project's own env files, which AgentStack did not write and does not own.

### Unresolved secrets block writes

If a `${REF}` doesn't resolve on this machine, `apply`/`use` writes are
refused for that target — never a `${TOKEN}` placeholder in live config.
Override with `--allow-unresolved`. Structural manifest validation errors
block `--write` too.

### Does it actually run? `doctor --live` and `doctor --probe`

A plain `doctor` proves your config parses, your secrets resolve, and nothing
has drifted. It does not prove a server *starts* — that is a different
question, and these two flags answer it for the two transports. Both are
opt-in, and `--probe` is the only doctor flag with side effects.

```text
agentstack doctor --live     # HTTP servers: reach them over the network
agentstack doctor --probe    # stdio servers: actually start them
```

`--live` performs a real MCP `initialize` handshake over HTTP and reports the
server name + tool count, or classifies the error (auth / http / connect).

`--probe` does the same for stdio servers the only way there is: it spawns the
command your manifest declares — same args, same `env`, same `cwd` a rendered
config would give a harness — speaks `initialize`, counts the tools, and stops
it again.

```text
MCP server startup (--probe)
  ✓ notes          started in 62ms · demo-notes · 3 tools
  ✗ missing        did not start: No such file or directory (os error 2)
  ✗ stuck          no response 10s after starting — killed — waiting for the database…
  ⚠ needs-token    not probed — DEMO_API_TOKEN does not resolve ↳ agentstack secret set DEMO_API_TOKEN
```

Because it starts real processes, it is bounded on every side:

- **Trusted projects only.** A project that is not trusted at its current bytes
  gets a refusal, not a probe — starting a repo's servers is exactly the thing
  the trust gate exists to hold back. Same rule as `session start`.
- **Ten seconds per server, hard.** Spawn, handshake, and tool count share one
  deadline. On expiry the child is killed with its whole process group — so a
  launcher's real server process goes too — and reaped. Ctrl-C stops the loop
  before the next server rather than orphaning the one in flight.
- **No half-resolved environments.** A server whose `${REF}` doesn't resolve on
  this machine is reported as not-probeable and never started, so you get
  "set this secret" instead of an auth error that blames the server.
- **Child output is untrusted.** stdout and stderr are length-bounded and
  stripped of escape sequences before anything is printed.

One caveat worth knowing: the probe inherits *your* environment. Run from a
terminal, that includes your shell's `PATH` — so a bare `npx` server can pass
`--probe` here and still fail inside a GUI-launched app, which is the situation
the bare-launcher advisory warns about. Pin the launcher and the two agree.

`doctor --json` carries the same results under a top-level `probe` object
(`ran`, `skipped_reason`, and per-server `status` of `ok` / `failed` /
`not_probeable`); gate on the `doctor-probe-v1` feature name.

### The whole way out: `uninstall`

`restore` reverses one write. `uninstall` reverses all of them — every managed
region agentstack rendered (servers, settings, hooks, instruction blocks) in
every CLI's own config, then agentstack's own state directory.

```text
agentstack uninstall                      # show what would be removed (default)
agentstack uninstall --verbose            # ...with the full diff of each file
agentstack uninstall --write              # do it
agentstack uninstall --write --keep-home  # keep ~/.agentstack (and the undo ledger)
agentstack uninstall --scope project      # this project only (or `global`)
```

It removes what agentstack manages, not what you wrote: **your
`agentstack.toml` is never touched**, so re-running `apply --write` brings the
whole setup back. Foreign entries you or another tool added to those same files
are left alone, as are entries a *different* project's manifest manages at
global scope. A config file left holding nothing but an empty container is
deleted rather than left as a husk, and its directory with it if that is now
empty too.

Removal goes through the same planners `apply` uses — given an empty manifest —
so every file edit is captured in the history ledger first. **An uninstall is
itself undoable** with `agentstack restore --last --write`, as one entry. That
is why `~/.agentstack` goes last, and why `--keep-home` exists: the ledger lives
there, so keeping it keeps the undo. The binary itself is not removed — take it
off the way you installed it.

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
provides the complete machine-readable view for external tools and automation.

## Drift: adopt or apply?

```text
agentstack diff            # review the drift
agentstack adopt           # keep the on-disk version (pull it into the manifest)
agentstack apply --write   # keep the manifest (re-render over the change)
```

`doctor` flags drift in both directions, and the fixes are opposites — pick
by which side holds the truth:

- **"no longer matches what agentstack last wrote"** — the live config changed
  after our last write. `doctor` states the fact without guessing the cause: a
  hand-edit is the common one, but a session that ended onto a stale baseline
  reaches the same state. Review with `agentstack diff`, which now labels each
  entry `managed`, `foreign (kept)`, or `hand-edited`; if the on-disk version
  should stay, `agentstack adopt` pulls it into the manifest. If the manifest is
  right, `agentstack apply --write` re-renders over it.
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
optionally into a toolset.

```text
agentstack adopt --write                  # import hand-added server drift into the manifest
agentstack adopt --target claude-code --write   # only that CLI's config
agentstack add ...                        # flag-driven add of a server or skill
```

### Search across providers

`search` queries **your linked library sources first** (skill and library-server names,
labelled `[library]`), then the embedded catalog **and the official MCP
Registry**; `add from <id>` resolves a registry/catalog server, lifts its secrets
to `${REF}`s, and renders it to **all your CLIs at once**.

```text
agentstack search <query>
agentstack add from <id>
```

agentstack is the cross-CLI *client* over the registry + marketplaces, not
another registry.

### Selective skills via toolsets

`use <toolset>` materializes only that toolset's skills, pruning the rest it
owns and never clobbering hand-made skill dirs.

```text
agentstack use <toolset> --write   # materialize only that toolset's skills
```

The toolset is optional: one declared toolset is chosen automatically, and a
manifest with **no** toolsets activates its full inline set — `agentstack use
--write` just works; several toolsets need a name. Materialization is
symlink-with-copy-fallback; when a prune empties the managed skills dir
(deactivation, `session end`) the dir is removed too, but rmdir semantics spare
any dir holding user content. Interactive `init` activates through this exact
`use` code path; plain `apply` never touches skills, it only names which toolset
activates them.

## Live runs (`agentstack run`)

Launch an agent CLI as a **tracked run** and control it without leaving
agentstack. A run is a real OS process agentstack owns: spawned in its own
process group (so a kill takes down the whole tree), recorded in
`~/.agentstack/runs.json`, and visible to any other AgentStack process or
integrated supervisor.

```bash
# Launch a harness, attached to your terminal, with a toolset applied for the
# life of the run (its servers + skills are reverted automatically on exit).
agentstack run claude-code --toolset design
agentstack run codex --toolset backend --scope project
agentstack run claude-code --keep        # leave the toolset applied after exit

# See runs and stop them here.
agentstack report runs         # table; add --json for scripting
agentstack kill <id>           # SIGTERM, then SIGKILL if it won't go
agentstack kill <id> --force   # SIGKILL immediately
```

**A plain `run` is the Protected tier.** Before the harness starts, agentstack
checks content trust, verifies the lock strictly, admits every declared
capability against the machine ceiling, and freezes the run's tool surface — and
refuses the launch, naming the fix, if any of that fails. The banner reads
`HOST / PROTECTED`. That is pre-launch gating, not kernel isolation: the agent
still runs as you, on the host. `--unprotected` opts out (no gate at all, banner
`HOST / ADVISORY`), `--locked` asks for the default by name, and
`--sandbox`/`--lockdown` add containment. The whole gate sequence is
[the Protected tier in detail](#the-protected-tier-in-detail-the-default-run).

Launching is a terminal act (the CLIs are interactive TUIs). The registry is
self-healing: a run
whose wrapper died is pruned on the next `report runs`. A toolset-bound run uses
the session engine, so one is allowed per directory at a time. Every tracked run
records a minimal lifecycle and prints `agentstack report run <id>` when it exits;
gateway-brokered tool calls join that report without recording argument values.
Unix only for now.

## Part II — The power surface

These are the commands `agentstack --help` keeps hidden as progressive disclosure, plus the advanced delivery and enforcement modes the everyday loop only points at. Hidden is not unsupported — every command here is fully maintained and carries its own `--help`, exactly as the [All commands](#all-commands) preamble spells out. Reach in when you need a machine-wide policy ceiling, the zero-files gateway, ephemeral sessions, the protected run's full gate sequence, the linked library sources, or the observability tooling.

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
and per-OS config paths (`{config}/…`) resolve per platform. macOS and Linux are the
supported platforms; the published Windows binary is not exercised by CI, so treat
Windows paths as untested rather than supported.
`agentstack adapters list` shows their ids. Which of the thirteen is checked
against the real CLI nightly, which is snapshot-only, and what each one actually
manages: [adapters.md](adapters.md).

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
which fix to run. `diff --json` emits the selected scope, toolset, per-CLI
change/diff records (each with `existed_before`, so an absent config reads as
a first render rather than an edit), kept foreign entries, owner refreshes,
and warnings for CI or agent consumers.

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
## Delivery — routing, and where rendered files live

You always commit the *intent* (`agentstack.toml` + `agentstack.lock`). Where
the rendered artifacts — `.mcp.json`, `.claude/skills/`, the compiled
`CLAUDE.md` / `AGENTS.md` — come from is **routed**, not chosen (flip,
2026-08-03): the delivery planner sends each capability down a lane from its
kind and the CLI it is going to. What the lanes *are*:
[concepts.md — delivery](concepts.md#delivery-modes); the shape of the decision:
[how capabilities reach your CLIs](choose.md).

| Capability kind | Lane |
|---|---|
| Skills · MCP servers, on a CLI with MCP | dynamic — served live, digest-verified per load |
| House rules · settings | rendered — settings only a file carries; house rules because no live channel a CLI is *known* to consume varies by model |
| Hooks · extensions | rendered, full consent ceremony always |
| Any kind, on a CLI without MCP | rendered |

- `agentstack delivery` prints the routing per CLI; `--json` is the same reading
  for a UI (`delivery-routing-v1`), with `default`, per-harness
  `mcp_capable` / `render_locally` / `override`, and a `routes` array carrying
  each kind's `lane`, `why`, and `full_ceremony`.
- **Render locally** is the one override: `[delivery] render_locally = true`, or
  `[delivery.harness.<id>] render_locally = true` for a single CLI. Set it with
  `agentstack delivery render-locally [--harness <id>] [--off] --write`. It
  writes files even where the live channel would have worked — offline work,
  deterministic native files, filesystem inspection, a rule against a persistent
  background process, debugging without another runtime dependency, or
  compatibility testing against a CLI's own behaviour. Clearing it removes the
  key: automatic is the *absence* of an override, not a second stored value.
- A gateway-served project keeps **0 project artifacts for the capabilities
  served live** — never "0 files": the manifest, the lockfile, and any managed
  house-rules region remain.

The three older per-project modes still exist behind `agentstack set-mode` and
the wizard's "more control" path, and are no longer how delivery is decided:

- **static** — artifacts on disk, kept out of git by a managed
  `.gitignore` block; pass `--no-gitignore` to commit them instead.
- **clean-at-rest** — `agentstack lock` pins name refs *without rendering*, so
  `git status` stays silent; a toolset arrives via
  [`session start`](#ephemeral-sessions-agentstack-session) /
  [`run`](#live-runs-agentstack-run) and reverts on exit.
- **zero-files** — `agentstack gateway connect` registers the gateway once per
  CLI (one write to each CLI's global config) and every **trusted** repo serves
  its own stack live; `agentstack_lease_open(profile)` fences one MCP connection
  to a toolset without rendering native files. A machine-local
  `codemode/endpoint.json` coordinate may exist for the connection's duration —
  see [the zero-files gateway](#the-zero-files-gateway---auto-project--trust).

**Recommendation:** none needed — the planner already routes to the live lease
path where the CLI supports MCP and to files where it does not. Reach for
`render locally` only when you actively need files. Add `--sandbox --lockdown` when the agent process
itself needs isolation — a lease is a capability fence, not a sandbox. See
[the primitives and decision table](ARCHITECTURE.md#operating-model--choose-the-boundary-you-need).

Interactive `init` asks **one** question before any write — automatic, or "more
control" — and the answer **forks** the run. **Automatic** (the default, and
what a non-interactive `init` takes on a project that has never rendered) states
the routing per CLI, offers to register the bridge
(`gateway connect --all --write`), points at `agentstack trust .` (which the
wizard never runs for you — trust is human consent), and renders nothing itself:
the rendered lane's command is the explicit `apply --write`. Behind **more
control** sit **render locally** (record the override, then the render path:
preview → confirm → `apply --write` → activate skills → doctor) and the three
older modes, unchanged — **static** takes the render path; **clean-at-rest**
renders nothing and pins the lockfile, teaching the `session start`/`session
end` rhythm; **zero-files** renders nothing and offers the bridge. A project
that already has rendered files keeps its render path in a scripted run: the
files are a fact, and un-rendering stays the explicit `set-mode` act. Bare
`agentstack` reports the project's derived mode on its `Mode` line.

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

| Tools | What it does | More |
|---|---|---|
| `agentstack_search` | search catalog + your library for capabilities to install | [search](#search-across-providers) |
| `agentstack_list` | list the manifest's capabilities | — |
| `agentstack_doctor` | run the doctor checks (adds a `Trust (auto mode):` line) | [doctor](#doctor-shows-what-you-use) |
| `agentstack_explain` | explain a server/skill before relying on it | — |
| `agentstack_diff` | show manifest ↔ on-disk drift | [drift](#drift-adopt-or-apply) |
| `agentstack_add_from` | *propose:* add a catalog/registry server | [search](#search-across-providers) |
| `agentstack_add_server` | *propose:* add a server | [`adopt`/`add`](#adopt-and-add) |
| `agentstack_add_skill` | *propose:* add a skill | [`add skill`](#add-skill-source--install-from-any-skills-repo) |
| `agentstack_create_profile` | *propose:* create a toolset | [toolsets](#selective-skills-via-toolsets) |
| `agentstack_list_loadable`, `agentstack_load` | the two-step skill loader (below) | this section |
| `agentstack_lease_open` / `_status` / `_close` / `_freeze` | MCP toolset lease lifecycle (below) | [leases](#mcp-toolset-leases-one-connection-one-capability-fence) |
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
[field notes](archive/design/reference-field-notes.md#zero-files-gateway-always-on-manual).

Honest limits: MCP servers, secrets, the tool firewall, the call audit log, and
skills-over-MCP create no per-project native artifacts. Native skill folders and
instruction files (`CLAUDE.md`/`AGENTS.md`) are read from disk by the CLIs
themselves and still need render mode (`apply`/`use`) — `gateway connect` prints
this per CLI.

### MCP toolset leases: one connection, one capability fence

An MCP toolset lease is process-local state owned by one `agentstack mcp`
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
selected toolset; `agentstack_list_loadable`/`agentstack_load` expose only that
toolset's skills (plus the embedded `using-agentstack` manual), with an optional
case-insensitive `query` that filters **within** the fence; the first load of
each skill is recorded with its reason; and trust, lock/digest verification,
machine and project policy, and call auditing all continue to apply.
`agentstack_lease_freeze({ "name": "backend-observed" })` converts the leased
server list plus the skills actually loaded into a new manifest toolset — a
manifest-only proposal; review the edit, then `agentstack lock`.

The control plane refuses to place a lease over an active native session, or a
native session over an active lease. A lease is deliberately invisible to
separate processes — read `agentstack_lease_status` from the same connection;
opening a different valid toolset replaces the current lease. See
[`examples/mcp-profile-lease`](../examples/mcp-profile-lease/) for a runnable
lifecycle, and
[field notes](archive/design/reference-field-notes.md#lease-survival-across-a-mid-connection-change)
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
experimental (see [field notes](archive/design/reference-field-notes.md#tools_execute-review-status));
cancellation kills the
[whole process tree](archive/design/reference-field-notes.md#tools_execute-cancellation).

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
default JSON shape is unchanged without it. Add `--include-loads` to interleave
on-demand skill loads into that same `--json` events feed, each row tagged with
a `kind` of `"call"` or `"skill_load"`; off by default, so without it the feed
is unchanged. A load is never a call — it never enters the call counts.
Best-effort local
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
`--json` emits the whole report machine-readably for external tools and automation.
Interactive `init` offers the deep scan as an explicit yes/no at its
closing doctor step, but only when the project actually has skills.

## Ephemeral sessions (`agentstack session`)

A session loads a toolset **for now** and reverts it on exit — the clean-at-rest
mode's native primitive, so nothing generated persists between sessions.

```bash
agentstack session start backend          # render backend's toolset (project scope)
agentstack session start backend --scope global
agentstack session list                   # active sessions on this machine
agentstack session end                    # revert this directory's session
agentstack session end --all              # revert every active session
agentstack session freeze --name backend-ci   # pin the resolved set into a new toolset
```

`start` renders the toolset's servers, skills, instructions, settings, and
hooks, records the write, and reverts it on `end` (or `end --all`). `freeze`
captures the session's resolved set — the toolset's servers plus the skills
actually loaded — into a new toolset (default `<toolset>-frozen`) so CI can
replay it deterministically; review the manifest edit, then `agentstack lock`.
The same start/end lifecycle backs the MCP `agentstack_session_*` tools and
external toolset pickers.

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

Which label a run gets is decided by the flags you type, and the default moved:
a bare `agentstack run <cli>` is the Protected tier and prints
`HOST / PROTECTED`. `--unprotected` is the explicit opt-out to the ungated host
run and prints `HOST / ADVISORY`. `--sandbox` and `--lockdown` are unchanged and
print their own labels — they are checked before the protected default, so
`run --sandbox` means exactly what it has always meant.

The label appears on the run banner, in `agentstack run --sandbox --plan`, and in
`agentstack report run <id>` (`report --json` carries the `posture` slug); a
sandbox run records it beside the flight-recorder log, and a protected run
carries it in its `attempt_started` event. `agentstack doctor` also prints a
one-word **machine-policy summary** — `open`, `restrictive`, or `mixed` —
describing the machine policy's shape (`restrictive` means a `"*"` rule or a
`[policy.filesystem]` scope binds every server, not that the policy is tight).
Ready-to-use machine policies for common setups live in
[`examples/policies/`](../examples/policies/) (`compatible`, `developer`,
`locked-down`, `ci`).

<a id="the-protected-tier-in-detail-run---locked"></a>
### The Protected tier in detail (the default `run`)

```text
agentstack run <cli>            # the default — this IS the Protected tier
agentstack run <cli> --plan     # walk the gate sequence read-only
agentstack run <cli> --locked   # the same run, named explicitly
```

A protected run is a fail-closed **pre-launch gate sequence plus a frozen
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
   digest, the fencing toolset — is frozen into an `AuthorityGrant` whose
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
5. **`--toolset <name>` is a fence**, not a session: gates, grant, artifact,
   and bridge all see only that toolset's server subset; no native session
   state is applied or reverted.
6. **Hygiene.** The original project MCP config is parked in the run's
   private dir (never left in the repo) and restored byte-identical; a
   sentinel makes overlapping protected runs refuse instead of stacking; a
   crash leaves the more restrictive state.

**Spellings and opt-outs.** `--locked` still parses and still means exactly this
run — it is kept for the scripts, docs, and panels that already type it, and it
keeps its own combination rule: `--locked --sandbox` and `--locked --lockdown`
refuse as a named not-yet-wired combination, so reach for `--sandbox` or
`--lockdown` on their own. `--unprotected` is the way out: an ordinary host run
with **no pre-launch gate at all** — no trust check, no strict lock
verification, no policy admission, no frozen grant — labelled `HOST / ADVISORY`,
with a launch banner that names each check it skipped. `--locked --unprotected`
refuses rather than letting flag order decide which one you meant.

**Headless.** `agentstack run <cli> --prompt "<text>"` is the governed headless
form. It requires the protected run and refuses beside `--unprotected`,
`--sandbox`, or `--lockdown`; the prompt is committed verbatim into the frozen
grant's argv, so the recorded evidence binds what the agent was asked to do.

`run --plan` walks the whole sequence read-only, printing every decision
the live path would (plus the grant digest a live run would freeze) and mutating
nothing; `--unprotected --plan` refuses, because an ungated run has no gate
sequence to walk. What is and isn't claimed at this tier (pre-launch gating on
the HOST tier, not kernel isolation — the harness still runs as you, on the
host) is [ENFORCEMENT.md — the protected run's frozen
grant](ENFORCEMENT.md#the-protected-runs-frozen-grant-the-default-run); the asserted
walkthrough is [`examples/projects/locked-run/`](../examples/projects/locked-run/)
and the full contract is
the [protected-run enforcement contract](ENFORCEMENT.md#the-protected-runs-frozen-grant-the-default-run).

## The library: linked source folders

Managed folders that projects reference **by name** instead of copying files
between repos. Any folder on the device can be linked as a source, and several
at once — `~/.agentstack/lib/` is simply the first one on a fresh machine.
`agentstack lib link <path> --write` adds one, `lib unlink` removes one,
`lib sources` shows the order, and `lib reorder` changes it. The full contract
is [`design/linked-library-sources.md`](design/linked-library-sources.md).

**Precedence is `PATH` semantics:** the first source holding a capability of
the requested kind and name wins. A name held by more than one source is
reported — never silently shadowed — by `lib sources`, `lib list`, `status`,
and `doctor`, each naming the winner, the shadowed sources, and the
`<source>:<name>` reference that pins the other copy. A project that wants to
be explicit rather than order-dependent writes that qualified form
(`skills = ["team:sql-review"]`); it resolves only in the source it names, and
the capability's identity everywhere else — lock key, rendered directory,
gateway name — stays the bare name.

Reordering or relinking sources changes what the **next** `lock` selects and
changes nothing an already-locked project serves: serving reads the bytes the
lock pins, from the content store.

### Layout and name resolution

Each source holds the same taxonomy: skill dirs (`skills/`) and MCP server
definitions (`servers/*.toml`), indexed in that source's own `library.toml`.
A toolset's `skills = ["sql-review"]` / `servers = ["kibana"]` resolve through
the ordered sources; an inline `[skills.*]` / `[servers.*]` table always
overrides every source. Provider folders are never
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
flag drift and show each item's origin. Toolset resolution is offline by default
(dry-run `use`, `doctor`, `explain` never fetch); `use --write` fetches
git-backed skills when activation needs them. `agentstack lock [--profile <name>]`
pins every toolset's name refs **without** rendering — the lock-only path for
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

`lib add ./<dir>` **copies** the source into `<first linked source>/skills/<name>`
— the library copy is canonical from then on (source edits have no effect), provenance records
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

### Removing capabilities (and getting them back)

```text
agentstack lib remove <name> --write             # skill
agentstack lib remove-server <name> --write      # MCP server
agentstack lib trash                             # what's recoverable
agentstack lib trash --restore <id> --write      # put one back
agentstack lib trash --empty --write             # delete it for good
```

Every `lib remove*` (skills, servers, extensions, hooks) **moves** the entry to
`lib/.trash/<id>/` instead of deleting it: the body goes in as `body/` or
`body.toml`, and the dropped `library.toml` row is recorded beside it in
`entry.toml`. Each removal prints the `--restore` line that undoes it. A
git-backed entry has no local body — only the index row moves; the shared store
cache is never touched.

`lib trash --restore` puts the row back in `library.toml` and the body back at
its canonical path, refusing (unless `--replace`) when the name has been taken
again since. `--empty` is the only library operation that destroys content, and
it only ever deletes inside `lib/.trash`.

The trash is machine-local — `lib sync` gitignores it — so removing something on
one machine never pushes a resurrection copy to another.

Removing from the library does **not** edit any project: manifests, lockfiles,
and rendered configs are untouched. A project that references the name by bare
`skills = ["…"]` keeps working until its next `lock`/`use`, which is where the
now-unresolvable name surfaces. Removing a project's *own* entry is
`agentstack remove <name> --write`.

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

Three ways a skill or server reaches a toolset; the manifest syntax alone picks
which:

- **By-name library reference** — `skills = ["greet"]` / `servers = ["kibana"]`
  with **no** matching `[skills.greet]` / `[servers.kibana]` table (optionally
  qualified as `"team:greet"`). Resolved fresh through the linked library
  sources on every lock, pinned there by `checksum`
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

The kinds of thing a toolset can carry — skills, servers, instructions,
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

Toolset-aware: skills a toolset references by name (library-resolved, no inline
`[skills.*]`) keep their lock pins through the reconcile pass — pin or refresh
with `agentstack lock`. Content digests always hash current bytes; see
[field notes](archive/design/reference-field-notes.md#orphaned-digest-cache) for the
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
[field notes](archive/design/reference-field-notes.md#add-skill-discovery-and-staging)).

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
| static, unambiguous toolset (none declared, or exactly one) | manifest + lock + **materialize** into the default targets (project scope for a project manifest), per-target `✓`/`⚠`/`✗` reporting |
| static, several toolsets | manifest + lock; activate with `agentstack use <toolset> --write` (toolset fencing wins — which is live is unknowable) |
| clean-at-rest | manifest + lock; the next `agentstack session start <toolset>` picks it up (an active session won't) |
| zero-files | manifest + lock, the current lease untouched; **trust re-gates on the edit** — run `agentstack trust .`, or the gateway serves control-plane-only next connection |

Toolset membership: no declared toolsets → the implicit default; exactly one →
added automatically; several → `--toolset` (or an interactive pick). Naming a
nonexistent toolset is an error, never a silent create.

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

#### Variants: one fragment, per CLI and per model

A fragment can carry alternative bodies selected by `cli`, by `model`, or by
both. `targets` decides *whether* a fragment reaches a CLI; `variant` decides
*which bytes* it sends once it does.

```toml
[instructions.house]
path = "./instructions/house.md"

  [[instructions.house.variant]]
  cli = "claude-code"
  model = "opus"
  path = "./instructions/house.claude-opus.md"

  [[instructions.house.variant]]
  cli = "codex"
  path = "./instructions/house.codex.md"
```

**Most specific wins:** exact `(cli, model)` → `(cli)` → `(model)` → the base
`path`. Two variants with the identical selector resolve to the first declared.
A variant with neither selector is refused — it could never be chosen.

**The model comes from a declaration, never a guess:** the `model` of a toolset
a command explicitly names (`instructions --toolset backend`,
`apply --profile backend`), else `[settings.<cli>] model` — the value agentstack
itself writes into that CLI's config. With neither, the model is **unknown**,
the least specific matching body is used, and every surface says so. No harness
has native per-model instructions; the switch is agentstack's.

**Every variant body is pinned** in `agentstack.lock`, including one nothing
currently selects, so editing any of them re-gates review before delivery.

A fragment with no `path` resolves its bodies — base and variants — from the
[linked library sources](#the-library-linked-source-folders) by its own name,
first match wins, as `<source>/instructions/<name>/instruction.toml`.

**What carries them, per CLI.** `status` names, for each targeted CLI, the file
that actually carries house rules there, which variant it receives and why, and
whether that CLI's *live* channel (MCP's `initialize` `instructions` field) is
**confirmed** or merely **unconfirmed**. No live channel carries house rules
today, confirmed or not: none of them varies by model or sits behind a lease.
Seven of the thirteen adapters have no instruction channel at all, and `status`
says that plainly rather than omitting them. Design:
[instruction-variants.md](design/instruction-variants.md).

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
`config.toml`), resolving secrets and pruning hooks that leave the manifest. A
hook's command runs inside the harness's own lifecycle at full user permission —
agentstack governs the declaration and its delivery, never the hook at runtime;
see [ENFORCEMENT.md](ENFORCEMENT.md#hooks).

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
**warns and does not render**. Under a protected `run`, a `rendered-verify` gate
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
/ `watch`) — over the same servers and toolsets agentstack manages, closing the
loop with the static `footprint` / `report usage` / `doctor` lenses. Bucketing
and SSE internals: [field notes](archive/design/reference-field-notes.md#wire-proxy-internals).

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
                                 # no toolset, not rendered anywhere, ≥14d of
                                 # history) and trust grants for deleted dirs
```

The contract: **every recommendation carries its evidence** (numbers, window,
data source), **the exact command or TOML** to act on it, and **why it is safe
or why it needs review**. One stated limit: the audit log only sees
gateway-brokered calls — a server rendered into a native config is called
directly by the harness, so such servers are never auto-removed on "no calls"
evidence alone.

## Staying current (`agentstack self update`)

Upgrading the binary is part of the product, not a manual chore. `agentstack
self update` replaces the binary you are running with the newest published
release, and `agentstack doctor` tells you when there is one.

```bash
agentstack self update              # what a newer release would install; downloads nothing
agentstack self update --write      # download, verify the sha256, install it
```

Same shape as every other mutating command: **it previews by default and only
acts on `--write`.**

**The download is verified before it is used.** The release archive is checked
against the `checksums.txt` published with that release *before it is unpacked
or moved into place*, and the new binary is swapped in with an atomic rename. A
mismatch aborts, prints both digests, and leaves the binary you already have
byte-for-byte untouched — the same guarantee `install.sh` gives, for the same
reason. Be precise about what that proves: the archive and its checksums come
from the same TLS-authenticated origin, so this establishes the **integrity of
the transfer**, not the provenance of the release. Provenance is a separate,
stronger check the command points you at rather than claiming:

```bash
gh attestation verify agentstack-<target>.tar.gz --repo Tarekkharsa/agentstack
```

Three situations the command cannot fix, each detected **before anything is
downloaded** and answered with a command that works:

| Situation | What it tells you |
| --- | --- |
| Installed by Homebrew | `brew upgrade agentstack` — replacing the file directly desynchronizes the formula |
| Binary in a directory you cannot write | `sudo agentstack self update --write` |
| No published asset for your platform | the releases page, with your OS/arch named |

A source build (`target/release/agentstack`, the `self link` workflow) is
refused too and pointed at `git pull && cargo build --release`: downloading a
release over somebody's build output would be a surprise, not an upgrade.

### The version note in `doctor`

`agentstack doctor` carries one line when a newer release exists:

```text
Updates
  · AgentStack 0.17.1 is available (you are on 0.17.0) ↳ agentstack self update
```

It is a **note**, not a warning: it counts in `doctor --json`'s `advisories`,
never in `errors` or `warnings`, so it cannot move `state` off `ready` or become
the "start with" next action. A current binary prints no section at all.

The check is deliberately cheap: at most one short, bounded request per 24
hours, cached in `~/.agentstack/update-check.json`. It never blocks, and it is
silent when you are offline — a failed check backs off for the full day rather
than re-dialling on every command. Nothing is checked at all when the running
binary is a build-tree binary, which cannot take a downloaded release anyway.

Opt out of every release-channel request — this command and the note — with:

```bash
export AGENTSTACK_NO_UPDATE_CHECK=1
```

## Which build am I running? (`--version`)

The Docker sandbox is a compile-time option. Published release binaries are
built with it; a plain `cargo build --release` is not, and the two otherwise
carry the same version number and the same `--help`. `--version` says which one
you have:

```console
$ agentstack --version
agentstack 0.17.1 (sandbox: yes)     # a release binary — run --sandbox works
$ agentstack --version
agentstack 0.17.1 (sandbox: no)      # a plain source build — it does not
```

`agentstack doctor` repeats the fact in its **Adapters & CLIs** section, and
`agentstack run <cli> --sandbox` on a build without it refuses by naming the
real cause rather than blaming Docker:

```console
$ agentstack run claude-code --sandbox
error: this build has no sandbox support — nothing was launched

  it was compiled without the optional `sandbox` feature, so --sandbox and --lockdown have no container backend to start
  rebuild it with:  cargo build --features sandbox
  or install a published release binary — those ship with it
  either way, a sandbox run also needs a running Docker daemon
```

`agentstack run --sandbox --plan` still works on either build: a dry run
describes, it never launches.

## Shell completions (`agentstack completions`)

`agentstack completions <bash|zsh|fish>` prints a completion script on stdout.
It is generated by walking the CLI's own command tree, so it covers every
command — including the ones `--help` groups away — every nested subcommand, and
every long flag, and it cannot drift from the binary that produced it.

Values are deliberately **not** completed. Toolset names, harness ids, and paths
are left to the shell's own file completion: promising more would mean shelling
back out to `agentstack` on every keystroke, and a completion that guesses wrong
is worse than one that stops short.

Install it where your shell looks:

```bash
# bash — source it from ~/.bashrc
agentstack completions bash > ~/.local/share/bash-completion/completions/agentstack

# zsh — drop it anywhere on $fpath, before `compinit` runs
agentstack completions zsh > ~/.zfunc/_agentstack     # and: fpath=(~/.zfunc $fpath)

# fish — the completions directory is loaded automatically
agentstack completions fish > ~/.config/fish/completions/agentstack.fish
```

Regenerate after upgrading the binary; nothing checks for staleness, and a
script from an older version simply offers an older set of names.

## Integrations

Graphical surfaces consume the same read-only JSON reports and invoke a closed set of CLI-owned actions; see [Integrations](integrations.md). The AgentStack CLI remains the complete standalone and automation interface.

## Part III — Full command reference

The generated command tree and the one-glance census, unchanged. Everything above has a prose home; this is the exhaustive index behind it — the exact verb, flag, and subcommand surface, regenerated from the CLI itself.

## All commands

The full command surface, generated from the CLI's own command tree by
`agentstack self docs --write` (CI fails if this list goes stale). Bare
`agentstack --help` deliberately shows only the **everyday commands** —
`init`, `up`, `status`, `add`, `search`, `apply`, `doctor`, `share`, `receive`,
`toolset`, `use`, `yes`, `run`, `trust`, `restore`, `undo`, `adopt`. The rest
are hidden from `--help` as progressive disclosure but are **fully supported**,
each with its own `--help`; **hidden does not mean deprecated or unsupported**.
`agentstack --help --all` prints the entire tree, and each line below marks the
hidden ones: a hidden top-level command carries `_(hidden)_`, and a hidden
subcommand carries a trailing `*` (e.g. `guard`'s `check*`). Reach for it when
you need the exact verb, flag, or subcommand.

<!-- agentstack:generated commands -->
- **`init`** — Setup: find the CLIs you have and bring their setups together — flags `--global/--force/--dry-run/--plan/--secrets/--no-keychain/--project-servers/--yes/--consented-plan`
- **`up`** — Set this machine up from a setup that already exists: one command — flags `--targets/--toolset/--no-gitignore`
- **`status`** — Status: where this project stands, on one screen, and the one next step — flags `--json`
- **`add`** — Add a server or skill to this project's setup — subcommands `from/server/skill`
- **`set`** _(hidden)_ — Create or update a manifest entry in place (idempotent `add`) — subcommands `server`
- **`search`** — Search the capability catalog (and mark what's already added) — flags `--all/--json`
- **`apply`** — Write this setup into each CLI's own config — flags `--target/--toolset/--dry-run/--write/--scope/--allow-unresolved/--prune-foreign/--no-gitignore/--verbose`
- **`instructions`** _(hidden)_ — Compile [instructions.*] into each CLI's CLAUDE.md / AGENTS.md — flags `--target/--toolset/--scope/--write`
- **`doctor`** — Check the setup in depth: what is wired up, what is missing, what changed — flags `--ci/--live/--probe/--fix/--deep/--all/--json`
- **`remove`** _(hidden)_ — Remove a server or skill from the manifest (and lockfile) — flags `--write`
- **`install`** _(hidden)_ — Fetch skill sources into the store and write the lockfile — flags `--locked/--allow-flagged`
- **`share`** — Share this setup as a signed bundle others can review — flags `--out`
- **`receive`** — Review a shared bundle, then decide — flags `--yes`
- **`publisher`** _(hidden)_ — Your publishing key, and the publishers you recognize — subcommands `show/trust`
- **`lock`** _(hidden)_ — Resolve each toolset's skill + server refs and pin `agentstack.lock` — flags `--profile/--update/--upgrade/--all/--with-instructions/--yes/--write`
- **`try`** _(hidden)_ — Try a skill without installing anything: stage, scan, and emit a wrapper prompt on stdout for piping into any agent CLI — flags `--skill/--rev/--subpath/--allow-flagged`
- **`lib`** _(hidden)_ — Manage your linked capability library sources — subcommands `new/add/add-server/add-extension/add-hook/list/remove/remove-server/remove-extension/remove-hook/trash/sync/pack-init/link/unlink/sources/reorder`
- **`toolset`** — Work with toolsets: name one that bundles what you already have — subcommands `create/rename/delete/list`
- **`use`** — Toolset: switch to one — its servers and skills go live in your CLIs — flags `--target/--scope/--write/--allow-unresolved/--prune-foreign/--no-gitignore/--list/--json`
- **`yes`** — Review and activate the files you dropped into this project — one step — flags `--yes`
- **`session`** _(hidden)_ — Use a toolset temporarily: load it for now, then put every file back — subcommands `start/end/list/freeze`
- **`run`** — Launch an agent CLI as a tracked run — flags `--locked/--unprotected/--prompt/--toolset/--scope/--keep/--sandbox/--lockdown/--plan`
- **`kill`** _(hidden)_ — Kill a tracked run by id (and revert its toolset if it owned one) — flags `--force`
- **`image`** _(hidden)_ — Compose one toolset and its pinned capabilities into a container image — flags `--toolset/--harness/--tag/--from/--json/--write`
- **`shim`** _(hidden)_ — Exec-through launcher shim for external supervisors (e.g. t3code) — subcommands `make/exec*`
- **`workflow`** — Run a reviewed multi-agent task using toolsets you already approved — subcommands `run/report/list/runs/explain/declare`
- **`report`** _(hidden)_ — Every "what happened" view in one place — subcommands `run/runs/usage/calls/wire`
- **`sign`** _(hidden)_ — Sign this project's agentstack.lock with a fresh ed25519 key (writes a detached agentstack.lock.sig, prints the public key to publish) — flags `--print-key-only`
- **`verify`** _(hidden)_ — Verify agentstack.lock against a published ed25519 public key and its detached signature — flags `--pubkey/--signature`
- **`guard`** _(hidden)_ — Machine-level destructive-command guard — subcommands `check*/test/install/uninstall/status`
- **`gateway`** _(hidden)_ — The zero-files gateway: register it once per CLI (`connect`) and every trusted repo brings its own servers through `agentstack mcp --auto-project` with no per-project files — subcommands `connect/disconnect`
- **`lease`** _(hidden)_ — Runtime lease registry: which toolset leases are open on this machine — subcommands `status`
- **`delivery`** _(hidden)_ — How each capability reaches each of your tools — and the one override — subcommands `render-locally` — flags `--json`
- **`trust`** — Review and approve this project's declared capabilities — required before anything activates them — flags `--list/--revoke/--yes/--consented-digest/--preview`
- **`restore`** — Undo a recorded write: revert what apply/use/session changed — flags `--last/--list/--scope/--write/--json`
- **`undo`** — Take it back: pick a point from your recent changes and revert to it — flags `--to/--write/--json`
- **`adopt`** — Keep a hand-edit: pull a change you made in a CLI back into this setup — flags `--target/--scope/--write/--no-keychain/--to-library`
- **`mcp`** _(hidden)_ — Run agentstack as an MCP server over stdio (for an agent to call) — flags `--auto-project/--transparent`
- **`diff`** _(hidden)_ — Show drift between the manifest and the on-disk configs — flags `--target/--profile/--scope/--json`
- **`explain`** _(hidden)_ — Explain a server, skill, or instruction before you rely on it — flags `--json`
- **`optimize`** _(hidden)_ — Turn agentstack's collected signals into concrete recommendations — flags `--json/--write/--since`
- **`proxy`** _(hidden)_ — Start the wire relay: a localhost proxy in front of the Anthropic API — flags `--port/--upstream`
- **`secret`** _(hidden)_ — Manage secrets in the OS keychain — subcommands `set/get/rm/list`
- **`settings`** _(hidden)_ — Edit a target's native `[settings.<target>]` entries — subcommands `set/unset`
- **`export`** _(hidden)_ — Export the manifest (+ lock, + optionally secrets) as an encrypted bundle — flags `--output/--secrets/--passphrase`
- **`import`** _(hidden)_ — Import an encrypted bundle on a new machine — flags `--force/--no-keychain/--passphrase`
- **`adapters`** _(hidden)_ — Inspect the available CLI adapters — subcommands `list/show/validate`
- **`self`** _(hidden)_ — Manage this binary's own install: `self update` upgrades it to the newest published release (checksum-verified); `self link` puts a stable `agentstack` on PATH (a symlink, no installer needed); `self which` shows which binary a bare `agentstack` runs and flags stale links — subcommands `link/which/update/docs*`
- **`completions`** _(hidden)_ — Print a tab-completion script for bash, zsh, or fish
- **`add-skill-to-profile`** _(hidden)_ — Add a skill to a toolset and activate it (panel action; digest-bound) — flags `--profile/--name/--git/--rev/--subpath/--path/--preview/--yes/--consented/--allow-unresolved`
- **`add-server-to-profile`** _(hidden)_ — Add a server to a toolset and activate it (panel action; digest-bound) — flags `--profile/--name/--type/--url/--header/--command/--arg/--cwd/--env/--preview/--yes/--consented/--allow-unresolved`
- **`uninstall`** _(hidden)_ — Remove everything AgentStack manages, previewing first — flags `--scope/--write/--verbose/--keep-home`
- **`create-profile`** _(hidden)_ — Fixed-argv alias of `agentstack toolset create` (panel action) — flags `--name/--skill/--server/--preview/--yes/--consented/--allow-unresolved`
- **`set-gitignore`** _(hidden)_ — Record whether this project manages its `.gitignore` block (panel action; digest-bound) — flags `--enabled/--preview/--yes/--consented/--allow-unresolved`
- **`set-mode`** _(hidden)_ — Switch this project's delivery mode (panel action; digest-bound) — flags `--preview/--yes/--consented/--allow-unresolved`
- **`edit-profile`** _(hidden)_ — Change one toolset's membership as a batch (panel action; digest-bound) — flags `--profile/--add-skill/--remove-skill/--add-server/--remove-server/--preview/--yes/--consented/--allow-unresolved`
- **`rename-profile`** _(hidden)_ — Fixed-argv alias of `agentstack toolset rename` (panel action) — flags `--name/--to/--preview/--yes/--consented/--allow-unresolved`
- **`delete-profile`** _(hidden)_ — Fixed-argv alias of `agentstack toolset delete` (panel action) — flags `--name/--preview/--yes/--consented/--allow-unresolved`
- **`use-profile`** _(hidden)_ — Activate an existing toolset (panel action; digest-bound) — flags `--profile/--preview/--yes/--consented/--allow-unresolved`
- **`library-index`** _(hidden)_ — The library catalog (skills + servers), merged across linked sources, for the panel browser
- **`remove-from-library`** _(hidden)_ — Remove a skill or server from the library (panel action; digest-bound). Moves it to the library trash — recoverable with `agentstack lib trash --restore <id> --write` — flags `--kind/--name/--preview/--yes/--consented/--allow-unresolved`
- **`remove-capability`** _(hidden)_ — Remove a skill or server from this project's manifest (panel action; digest-bound), then re-lock and re-render — flags `--kind/--name/--preview/--yes/--consented/--allow-unresolved`
<!-- agentstack:end -->

## Everything shipped so far

A single-glance census of every capability that exists today — the fastest way
to confirm a feature is real before you go hunting for its section above.

13 adapters · `init`/`add`/`apply`/`diff`/`use`/`instructions`/`adopt` ·
package manager (`install`/`lock --update`/`remove` + lockfile) · central capability
library (`lib` skills + servers referenced by name, digest-pinned in the lock,
drift in `doctor`/`explain`) · secrets (keychain + varlock as the recommended
vault — `init` offers the `.env.schema`, `doctor` reports its
health) · scopes (global/project) · `doctor` (`--live`/`--fix`/`--ci`/`--deep`) ·
content scanning on install + `doctor --deep` · official MCP Registry provider +
`search`/`add from` · `[policy]` trust gate · native per-CLI settings
(`[settings.*]` → settings.json) · native extensions (`[extensions.*]` →
content-pinned harness add-ons, re-verified at the protected `run`) · atomic writes + backups ·
`export`/`import` · portable lifecycle hooks · agent-operable `mcp` server ·
graphical-integration contracts · live runs (`run` — protected by default,
`--unprotected` to opt out — plus `report runs`/`kill`) ·
GitHub Action trust gate ·
nightly adapter-conformance CI · zero-files gateway (`gateway connect` + `mcp
--auto-project` + digest-pinned `trust`) · `optimize` (evidence-backed
recommendations from usage/audit/cost signals, safe-class `--write`) ·
fail-closed `lib sync` secret gate (all server fields + outgoing history) ·
machine-level destructive-command `guard` · Docker `run --sandbox` and
no-direct-route `--lockdown` with compiled egress/filesystem policy · per-run
`report` (lifecycle, limits, egress, tool calls, secret refs) · detached
`sign`/`verify` · experimental frozen-plan `tools_execute`.
