<img alt="agentstack" src="docs/logo.svg" width="380">

> **Build, govern, and run your agent stack from one local control plane.**
> Define servers, skills, instructions, settings, hooks, plugins, profiles,
> and secrets once; compile them across agent CLIs; then trust, constrain,
> run, audit, and optimize the result.

**[Website](https://tarekkharsa.github.io/agentstack/)** ·
[Docs](https://tarekkharsa.github.io/agentstack/docs.html) ·
[Examples](https://tarekkharsa.github.io/agentstack/examples.html) ·
[Releases](https://github.com/Tarekkharsa/agentstack/releases)

Define your stack once in `.agentstack/agentstack.toml`. AgentStack resolves
and pins capabilities, activates task-specific profiles, and either serves the
stack live through a trust-gated gateway or compiles it into the native config
of 13 agent CLIs — Claude Code, Claude Desktop, Codex, Cursor, Windsurf,
Gemini CLI, VS Code, GitHub Copilot CLI, OpenCode, Antigravity, Junie, Kiro,
and Pi. Secrets stay `${REFERENCES}` that resolve per machine, so the source of
truth is safe to commit and share.

Every capability you add is unreviewed code and instructions running with your
credentials and your shell. AgentStack makes **nothing run until it's trusted,
and nothing trusted run unobserved** — a clone stays inert until you pin its
exact bytes, a machine policy no repo can loosen fences what runs, and every
brokered call lands in an audit log.

## Why

Every skill, MCP server, and agent config you adopt is **unreviewed code plus
instructions**, wired into a process that holds your credentials, your shell,
and the network. Installing one is `npm install` with an agent attached —
except with no lockfile, no review gate, and no record of what it did. The gaps
that follow:

1. **Anything a repo declares can run.** Clone it, start an agent session,
   and its MCP servers want to spawn — commands you never read, with your
   keychain in reach. Here a clone is *inert* until you trust its exact
   bytes, and any edit re-gates it.
2. **Nothing narrows or records what agents do.** Consent today is
   all-or-nothing and unlogged; an injected prompt can turn a legitimate tool
   against you. Here your *machine policy* — which no repo can loosen —
   fences tools, secrets, and egress, and every brokered call lands in an
   audit log; `--lockdown` goes further and removes the agent's direct
   network route. (Honest scope per mode: the
   [enforcement matrix](docs/ENFORCEMENT.md).)
3. **Every CLI spells the same setup differently** — six config syntaxes,
   drifting copies, real tokens pasted into files that were never meant to be
   shared. Here one reviewed manifest renders them all, secrets stay
   references, and a lockfile makes it reproducible.
4. **An agent can wreck your working tree by accident.** No prompt injection
   needed — a wrong `rm -rf` or `git reset --hard`, with nothing between the
   command and your filesystem. Here `agentstack guard` blocks destructive
   commands and out-of-workspace writes before they run and records each
   denial — a **cooperative** net for accidents
   ([how it works](#step-3--block-the-accidents-one-command)); kernel-enforced
   confinement stays `run --sandbox` / `--lockdown`.

If you use a single agent with one hand-managed server, you may not need this
yet. The moment capabilities come from repos you didn't write, you do.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/Tarekkharsa/agentstack/main/install.sh | sh
```

The installer verifies the release tarball against the `checksums.txt`
published with each release before installing.

Or from a checkout:

```sh
cargo build --release
./target/release/agentstack self link   # symlink onto your PATH
```

One static binary for the core workflows. Docker is required only for
`run --sandbox` / `--lockdown` and experimental `tools_execute`.

## Climb as far as you need

AgentStack is adopted in steps, not all at once. Each step pays off on its
own, in minutes, and nothing later is required to keep the earlier wins —
stop wherever your setup stops hurting. Steps 1–5 run natively; Docker
appears only at step 6.

| Step | You run | You get |
| --- | --- | --- |
| [1 — Unify](#step-1--one-manifest-every-cli-5-minutes) | `agentstack init` → `apply` | one reviewed manifest for every CLI; real tokens out of your config files |
| [2 — Verify](#step-2--two-habits-that-keep-it-healthy) | `agentstack` · `agentstack doctor` | drift caught early; every warning names its exact fix |
| [3 — Guard](#step-3--block-the-accidents-one-command) | `agentstack guard install` | `rm -rf`, `git reset --hard`, and `.env` reads blocked before they land |
| [4 — Trust](#step-4--keep-strangers-repos-inert-until-review) | `gateway connect` · `trust .` | cloned repos stay inert until you review them; brokered calls firewalled and audited |
| [5 — Scale](#step-5--scale-it-up-profiles-library-plugins-teams) | profiles · `lib` · `plugins` | one governed stack across projects, machines, and teammates |
| [6 — Confine](#step-6--maximum-assurance-sandbox--lockdown-docker) | `run --sandbox --lockdown` | kernel-enforced confinement — the agent's only route out is the audited proxy |

The same ladder, with expected output at every step, is the
[getting-started walkthrough](https://tarekkharsa.github.io/agentstack/start.html)
on the site. Agents get the same map: the shipped
[`using-agentstack` skill](crates/cli/catalog/skills/using-agentstack/SKILL.md)
teaches them to detect which step a project is on and propose the next one.

## Step 1 — One manifest, every CLI (5 minutes)

`agentstack setup` is the guided path — it imports, previews, and applies
interactively. The same flow as individual commands:

```bash
agentstack init         # existing CLI configs → one manifest
                        # (nothing installed yet? init writes a starter
                        #  manifest with a commented example instead)
agentstack apply        # preview every CLI's changes, confirm to write
agentstack use --write             # activate skills (picks your profile,
                                   #  or everything when none declared;
                                   #  setup does this step for you)
```

> ▶ [Watch it live on the site](https://tarekkharsa.github.io/agentstack/#start) — same flow, replayed in your browser.

If `apply` or `doctor` reports a missing secret, store it once — it goes in
your OS keychain, never in the manifest:

```bash
agentstack secret set GH_PAT
```

What you just wrote looks like this — one file, reviewed like code:

```toml
version = 1

[servers.github]
type = "http"
url = "https://api.githubcopilot.com/mcp/"
headers = { Authorization = "Bearer ${GH_PAT}" } # resolved per machine — the manifest never holds the value

[servers.github.extra.codex]                 # native keys one CLI needs pass
startup_timeout_sec = 20                     # through verbatim, per adapter

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

Relative paths in the manifest (a skill's `path`, a server's `cwd`) anchor at
the **manifest's directory** — `.agentstack/` in the preferred layout — so
`path = "./skills/x"` lives at `.agentstack/skills/x`, not the repo root.
(`cwd` is the exception: it anchors at the project root, matching what a
harness gives a rendered config.)

One `agentstack apply` compiles this single manifest into the native config of
every CLI in `[targets]` — up to all 13 — each adapter's quirks handled for you
and secrets left as `${REF}`s:

![agentstack first run: init → apply](docs/firstrun.svg)

Three things worth knowing before you climb further:

- **Writes stay where the manifest lives.** In a repo, `apply`/`use` default
  to **project** scope — artifacts land repo-local (`.mcp.json`,
  `.claude/skills/`), kept out of git by a managed `.gitignore` block. Only
  the machine manifest (`~/.agentstack/`) defaults to your global configs.
  Pass `--scope` to override either way.
- **Skills activate through `use`, not `apply`.** `setup` runs this for you as
  its final step. Standalone `apply` renders servers, instructions, settings,
  and hooks only; `agentstack use --write` activates skills — your named
  profile, the single declared one, or (with no profiles declared) everything
  inline. Profiles are opt-in selectivity, not a prerequisite.
- Prefer **no rendered files at all**? Skip `apply` entirely and jump to
  [step 4](#step-4--keep-strangers-repos-inert-until-review) — one gateway
  registration serves every repo live.

Servers and skills are two of six capability kinds the manifest declares.
`[instructions.*]` fragments compile into a managed region of each harness's
`CLAUDE.md` / `AGENTS.md` (`agentstack instructions --write`; hand-written
prose around the region is preserved), `[settings.*]` renders native per-CLI
settings, `[hooks.*]` compiles declarative lifecycle hooks into each
harness's own hooks config, and `[extensions.*]` delivers native harness
add-ons under the strictest pinning agentstack has
([step 4](#step-4--keep-strangers-repos-inert-until-review) explains why).
The [feature reference](docs/reference.md) documents every kind.

## Step 2 — Two habits that keep it healthy

That's the whole everyday loop already. Two habits keep it that way:

- `agentstack` with no arguments tells you the one next step for the directory
  you're in.
- `agentstack doctor` verifies everything is wired up and names the exact fix
  for anything that isn't.

Everything else you'll reach for day to day:

| Command | What it does |
| --- | --- |
| `agentstack init` | Reverse-engineer a manifest from the configs you already have |
| `agentstack apply` | Preview each CLI's config changes; confirm (or `--write`) to render |
| `agentstack doctor` | Verify wiring; every warning comes with the exact fix command |
| `agentstack diff` | What would change, read-only |
| `agentstack secret set NAME` | Store a secret in the OS keychain |
| `agentstack use --write` | Activate skills + servers (a named profile, or everything when none declared) |
| `agentstack run <cli> --profile <p>` | Launch a harness as a tracked run, with a profile for its lifetime |
| `agentstack report` | Every "what happened" view: live runs, a run's flight recorder, call activity |
| `agentstack guard install` | Wire the destructive-command guard into your CLIs' hooks |
| `agentstack lock` | Pin profile refs in the lockfile without rendering anything |
| `agentstack restore --last --write` | Undo any write from its recorded history — servers, settings, hooks, instructions |
| `agentstack adopt --write` | Keep a hand-edit: pull drifted native config back into the manifest |
| `agentstack dashboard` | The same lifecycle in a local web UI |

When `doctor` flags drift, the rule is directional: the hand-edit on disk is
right → `adopt` pulls it into the manifest; the manifest is right →
`apply --write` re-renders over it. Never edit rendered files to "fix" drift.
And before acting on any capability, `agentstack explain <name>` shows its
provenance, secrets, and policy footprint. The
[feature reference](docs/reference.md) has the complete command list.

## Step 3 — Block the accidents (one command)

`agentstack guard install` wires a **cooperative** pre-tool-use hook into 9
agent CLIs (Claude Code, Codex, Gemini, Cursor, Windsurf, Copilot CLI,
Antigravity, OpenCode, and Pi; VS Code agent mode reads the Claude-format user
hooks). Once installed, it stops the commands an agent runs by mistake before
they touch your machine:

```text
agent → rm -rf ~/other-project   ✗ blocked   # destructive, outside the workspace
agent → git reset --hard         ✗ blocked   # discards uncommitted work
agent → cat .env                 ✗ blocked   # [policy.filesystem] deny-glob
every denial → ~/.agentstack/audit/calls.jsonl   (host-guard entry)
```

It also blocks file-tool writes outside the workspace (plus your `[guard]`
allow_roots and temp) and reads or writes to `[policy.filesystem]` deny-globs
like `.env`. `agentstack guard status` shows which CLIs are wired — and
`agentstack guard test rm -rf /` lets you watch a denial without an agent.

This is a **cooperative** boundary: it catches an agent's *accidents*, not a
determined attacker — a harness that ignores its own hook protocol bypasses it.
Kernel-enforced confinement is `run --sandbox` / `--lockdown` in
[step 6](#step-6--maximum-assurance-sandbox--lockdown-docker). Runnable
walkthrough: [`examples/guard-demo/`](examples/guard-demo/).

![agentstack guard blocking rm -rf, git reset --hard, and cat .env](docs/guard.svg)

## Step 4 — Keep strangers' repos inert until review

Register the AgentStack gateway, then clone a repo: its declared MCP servers
remain inactive until you inspect their runtime surface and trust the current
manifest/lock consent digest. Calls brokered after that are firewalled and
audited.

![The trust gate: clone → inert → review → trust → firewalled → audited — and the library sync gate blocking a literal secret](docs/trust-gate.svg)

> ▶ [Watch it live on the site](https://tarekkharsa.github.io/agentstack/#trust) — and run it yourself: [`docs/trust-gate-demo.sh`](docs/trust-gate-demo.sh).

Register the gateway once (`agentstack gateway connect --all --write`) and every repo
you open brings its own MCP servers with **no files copied in**. But a repo you haven't reviewed
is **inert** — none of its servers are spawned or contacted, no secrets resolved:

```bash
git clone <some-repo> && cd <some-repo>
agentstack mcp --auto-project    # an agent asks what it can use here → nothing (untrusted)

agentstack trust .               # you SEE what it declares before authorizing:
#   ▶ demo: runs `python3 ./server.py`
#   ✓ trusted at sha256:…        (editing the manifest re-gates it)
```

Trust pins the **manifest, local overlay, and lockfile**, not arbitrary code they point at —
which also means running `agentstack lock` re-gates the project (new pins =
new consent); expect to re-run `trust .` after locking. And:
you're authorizing the command `python3 ./server.py`, and a later edit to
`server.py` won't re-gate the project (an edit to the manifest or lock will).
Central-library servers are pinned by definition digest in `agentstack.lock`
and verified by the gateway before serving — a drifted definition is refused
until you re-lock (which re-gates trust). Review referenced scripts as part
of `trust .` — same discipline as reading a `.envrc` before `direnv allow`.

After that its servers are live through the gateway — and every brokered call is
**firewalled** and **audited**. Two policy layers apply: the repo's `[policy]`
and your own machine-level `[policy.tools]` in `~/.agentstack/agentstack.toml`,
which is checked first and which no repo can loosen:

```text
agent → demo.echo         ✓ ok        # brokered through the gateway, logged
agent → demo.secret_read  ✗ denied    # blocked by [policy.tools]
every call → ~/.agentstack/audit/calls.jsonl   (tool · outcome · latency)
```

Starter machine policies to copy from live in
[`examples/policies/`](examples/policies/).

No generated config files in the repo, and no untrusted repo-declared server is
auto-started by the gateway. This does not sandbox arbitrary repo code; use
`run --sandbox --lockdown` when the agent process itself needs confinement.
The whole thing is a runnable 60-second demo:
[`docs/trust-gate-demo.sh`](docs/trust-gate-demo.sh).

### Launch through the gate: `run <cli> --locked`

The same consent machinery can gate a whole launch — no Docker required.
`agentstack run <cli> --locked` refuses to start the harness unless every
gate passes, fail-closed and recorded: explicit trust at the current digest,
strict lock verification — including pinned local server executables, where a
**one-byte edit refuses the run** — and policy admission under your machine
ceiling. What passed is then **frozen**: the run's bridge serves exactly the
ruleset and server set the gates admitted, refuses mid-run mutations (no
lease swaps, no secret-resolving session starts), and never re-derives
authority from disk. `--plan` prints every gate decision and the grant digest
without launching anything.

Honest scope: this is pre-launch gating plus a frozen capability surface, not
kernel isolation — the harness still runs as you on the host. Confinement is
[step 6](#step-6--maximum-assurance-sandbox--lockdown-docker). The full gate
sequence: [feature reference → the Protected tier](docs/reference.md#the-protected-tier-in-detail-run---locked);
runnable, asserted example: [`examples/projects/locked-run/`](examples/projects/locked-run/).

### Native extensions: pinned bytes, honestly labelled

`[extensions.*]` manages a harness's native executable add-ons — pi's
TypeScript extensions, OpenCode's JS plugins — the way `[skills.*]` manages
skill dirs. It is the **highest-risk** kind agentstack delivers: the code runs
inside the harness process at full user permission, outside every policy
ceiling. So the governance is all pre-delivery, and labelled that way: the
source is content-pinned in `agentstack.lock`, an untrusted or drifted project
renders **zero** extension bytes, delivery copies (never symlinks) the
reviewed bytes, and `run --locked` re-verifies each delivered copy before
launch. What you get is provenance — which bytes, from where, re-gated on any
change — not runtime enforcement:
[enforcement matrix → native extensions](docs/ENFORCEMENT.md#native-extensions).

## Step 5 — Scale it up: profiles, library, plugins, teams

Nothing in this step is required — each piece exists for the moment one
project, one machine, or one person stops being enough.

### A shared library of skills & servers

Install a capability once into your machine-wide **central library**
(`~/.agentstack/lib`), then reference it **by name** from any project's profile —
no copying files between repos.

```bash
agentstack search codex                    # find shipped skills + registry servers
agentstack add from run-codex              # add a shipped skill to this manifest

# Add your own — from a local dir, or straight from a git repo:
agentstack lib add sql-review --path ./skills/sql-review --write
agentstack lib add improve --git https://github.com/acme/skills \
    --subpath skills/improve --write       # subdir layouts (marketplaces/monorepos)

agentstack lib list                        # what's installed, with provenance
```

Every add is content-scanned (hidden-unicode / prompt-injection) before it
lands — and `agentstack audit` re-scans a project's skills and instructions
any time. agentstack ships a starter catalog — `run-codex`, `sync-library`,
`analyze-usage`, `mine-skills` (distill reusable skills from your past agent
sessions), `adversarial-review` and `orchestrate-workflow` (governed
multi-agent generate-review-fix loops), `route-by-cost`, `using-agentstack`,
and more.

Keep the library consistent across machines by versioning it as a git repo.
Secrets never travel — a fail-closed gate scans every server field (headers,
env, url, args) **and the outgoing commits** before anything is pushed, and a
definition it can't parse blocks the sync rather than slipping through:

```bash
agentstack lib sync --init --remote git@github.com:you/agent-lib.git
agentstack lib sync                        # commit local changes, pull, push
```

### Take one CLI's plugins everywhere

A plugin you installed in one CLI shouldn't lock its capabilities to that CLI.
`plugins adopt` lifts an installed native plugin (Claude Code or Codex) into
the manifest: its skills are **copied into the central library** with
provenance recorded, and its MCP servers — auth wiring included, as `${REF}`s —
travel with the recipe:

```bash
agentstack plugins adopt cloudflare --harness codex --write     # skills → library, recipe → manifest
agentstack plugins sync --write                                 # generate native packages + marketplaces
agentstack plugins install cloudflare --target claude-code --write
```

The harness you adopted *from* stays satisfied by its native install —
`status`/`doctor` report it as up to date at the adopted version and flag
drift when the native plugin moves ahead, instead of ever double-installing.

### Share it with a team

Commit `.agentstack/` (manifest + lock). A teammate — or your CI — then runs:

```bash
git clone <repo>
agentstack secret set GH_PAT   # local only; never committed
agentstack apply --write
agentstack doctor              # verify the wiring
```

In CI, the trust gate is two commands — or the one-line GitHub Action:

```bash
agentstack install --locked   # fail if sources drifted from the pinned lock
agentstack doctor --ci        # fail on errors, drift, policy, unsafe content
```

```yaml
steps:
  - uses: actions/checkout@v4
  - uses: Tarekkharsa/agentstack@v0.11.0  # pin a release tag, not @main
```

A maintainer can also `agentstack sign` the lockfile (detached ed25519) so CI
or a recipient runs `verify` against the exact pinned bytes — the recipient
still makes their own local trust decision. Moving machines rather than
sharing with people? `agentstack export` / `import` carry the manifest, lock,
and optionally secrets as one age-encrypted, passphrase-protected bundle.

### Where rendered files live — pick a mode

You always commit the *intent* (`agentstack.toml` + `.lock`). The rendered
artifacts (`.mcp.json`, `.claude/skills/`, and the compiled `CLAUDE.md` /
`AGENTS.md`) are a per-project choice:

- **Static** (default) — artifacts sit on disk, kept out of git by a managed
  `.gitignore` block, so a repo tracks only your `.agentstack/` intent. The
  block only ever covers files agentstack actually wrote — a hand-maintained
  `.mcp.json` or `CLAUDE.md` is never hidden from `git status`. Works however
  you launch your tools. (Pass `--no-gitignore` to commit them instead.)
- **Clean-at-rest** — nothing generated exists between sessions; profiles are
  injected by `agentstack run` / `session start` and reverted on exit.
  `git status` stays silent.
- **Zero files** — `agentstack gateway connect` registers the gateway once per
  harness; every **trusted** repo then brings its own servers through
  `agentstack mcp --auto-project`, with a tool firewall and call audit log
  included. `agentstack_lease_open(profile)` selects a process-local profile
  fence for servers and progressively loaded skills without creating native
  files or `sessions.json`; close it or end the MCP process to drop it.
  Proxied tools collapse behind `tools_search` (search → inspect → call) so
  a dozen servers don't bloat every turn's context, and `tools_bindings`
  exposes the same surface as typed code-mode bindings.
  Untrusted repos are inert until you review and `agentstack trust .`

Inside the connected agent session, the zero-file lifecycle is:

```text
agentstack_lease_open({ "profile": "backend" })
agentstack_list_loadable({})
agentstack_load({ "name": "sql-review", "reason": "review this migration" })
agentstack_lease_status({})
agentstack_lease_close({})
```

These are MCP tool calls, not shell commands. To preserve the observed skill
set, call `agentstack_lease_freeze({ "name": "backend-observed" })`, review the
manifest edit, then run `agentstack lock`. A
[runnable stdio example](examples/mcp-profile-lease/) verifies the complete
lifecycle and the absence of native artifacts.

Details and trade-offs: [feature reference → three modes](docs/reference.md#where-rendered-files-live-three-modes).

### See what your tools cost on the wire

Every tool, server, and skill you load is re-billed as input tokens on *every*
turn — [one measurement](https://www.aihero.dev/how-to-kill-the-bloat-in-claude-codes-system-prompt)
clocked ~155 KB across 69 tools before a single word of your prompt. agentstack
gives you that visibility built in: point a harness at `agentstack proxy start`
and it relays every request verbatim (observe only — nothing injected, the
prompt cache stays warm) while ranking what each capability actually costs your
agent per turn.

```bash
export ANTHROPIC_BASE_URL=http://127.0.0.1:8787
agentstack proxy start        # in one shell
agentstack proxy report       # after some real usage
```

`report` ranks per-capability tokens/turn against how often each tool was
actually called — so a server that's expensive but never used is flagged
`drop / lazy`. Because those are the same servers and profiles agentstack
manages, that on-wire evidence closes the loop with the static `report usage` /
`doctor` lenses. Telemetry is privacy-preserving: counts, capability names, and
token estimates only — never prompt bodies or secrets.

### More power tools

- **Vendor packs** — `agentstack add from git:github.com/acme/pack@v1.2.0`
  installs a versioned MCP + skills + house-rules bundle, policy-gated and
  content-scanned before anything is written.
- **Personal layer** — `agentstack init --global` gives your machine-wide
  instructions a home; they merge beneath every project without ever landing
  in a repo's committed files.
- **App-managed servers** — `[servers.X] owner = "codex"` makes the owning
  app's config the source of truth: when the app rewrites its own entry (a
  self-update, say), `apply` refreshes the manifest and fans the fresh values
  out to every other CLI — instead of reverting the app.
- **Usage insight** — `agentstack report calls` reports what you actually call (from
  the runtime audit log) and flags library capabilities you installed but never
  use, so pruning is data-driven. `--transcripts` adds cross-harness reach from
  local Claude Code / Codex session logs — sessions, token totals, top tools;
  aggregates only, never prompt content. Read-only and local. When you're
  ready to act, `agentstack optimize` turns those signals into concrete
  recommendations — inert servers to remove, `[policy.tools]` allowlists to
  narrow — each carrying its evidence and the exact command or TOML.
- **[The no-terminal path](docs/dashboard.md)** — the dashboard's capability
  lifecycle, from discovery through undo.
- **[Examples](https://tarekkharsa.github.io/agentstack/examples.html)** — every
  capability by example, from a one-line manifest up to sandboxed governed
  execution — and **[primitives and recommendations](https://tarekkharsa.github.io/agentstack/primitives.html)**:
  when to use a static render, native session, MCP lease, trust, policy, or
  lockdown — and why none of those boundaries substitutes for another.

The closed loop in under a minute — install a versioned pack, spread it to
every CLI, firewall a tool, watch the refusal in the audit log, upgrade to the
vendor's next tag:

![agentstack closed loop: install a versioned pack, spread it everywhere, firewall a tool, watch the audited refusal, upgrade](docs/closed-loop.svg)

> ▶ Run it yourself: [`examples/sandbox/demo-closed-loop.sh`](examples/sandbox/demo-closed-loop.sh).

## Step 6 — Maximum assurance: sandbox & lockdown (Docker)

Everything so far decides what an agent *may* do. This step decides what it
*can* do. `agentstack run --sandbox --lockdown` launches the agent
in a container with **no host route and no internet**: its only path out is
the AgentStack egress-proxy sidecar, which enforces your machine
`[policy.egress]` and records every decision to the run's flight recorder
(`agentstack report`). Ignoring the proxy reaches nothing — the confinement
is topological. The proxy filters by host (case- and trailing-dot-normalized),
requires a TLS connection's SNI to match the host it dialed (no domain
fronting), and refuses any name that resolves to a loopback, private,
link-local, or cloud-metadata address (no SSRF into your own network). Build
with `--features sandbox` (Docker support is off by default). The container
runs your harness, so point `run --sandbox` at an image that carries it —
build one from [`docker/sandbox.Dockerfile`](docker/sandbox.Dockerfile) and
set `AGENTSTACK_SANDBOX_IMAGE`. The lockdown proxy sidecar needs no setup:
each release publishes it to GHCR and the binary pulls the tag pinned to its
own version (override with `AGENTSTACK_EGRESS_IMAGE`, e.g. a local
[`docker/egress-proxy.Dockerfile`](docker/egress-proxy.Dockerfile) build).
Runnable demo (needs Docker):
[`examples/sandbox/demo-lockdown.sh`](examples/sandbox/demo-lockdown.sh).

What each mode actually enforces — and where it stops — is spelled out per
dimension in the [enforcement matrix](docs/ENFORCEMENT.md). AgentStack
restricts destinations and records decisions; it cannot guarantee sensitive
content never leaves through a host you *allowed*.

![agentstack lockdown: a container with no host route — its only egress is the AgentStack proxy sidecar, which blocks a denied host and records it](docs/lockdown.svg)

> ▶ [Watch it live on the site](https://tarekkharsa.github.io/agentstack/#sandbox) — and run it yourself (needs Docker): [`examples/sandbox/demo-lockdown.sh`](examples/sandbox/demo-lockdown.sh).

### Experimental: governed TypeScript execution

Sandbox-enabled builds can expose `tools_execute`: one bounded TypeScript
program can call a small, exact set of MCP tools through the existing gateway.
The program runs as a non-root process in a read-only, resource-limited Docker
container with no workspace, ambient credentials, package installation, or
direct network route. Every allowed call is re-checked by the gateway and
attributed in `agentstack report`.

It is off by default and only the machine owner can enable it in
`~/.agentstack/agentstack.toml`:

```toml
[experimental]
tools_execute = true

# Optional machine-owned defaults; requests may only narrow them.
[experimental.tools_execute_limits]
timeout_ms = 30000
max_calls = 40
max_output_bytes = 131072
```

This is an experimental Docker-only surface, not a host-execution fallback.
See the [exact request contract](docs/reference.md#experimental-tools_execute)
and [enforcement matrix](docs/ENFORCEMENT.md#experimental-tools_execute).

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

Adding a CLI is one YAML descriptor — copy `crates/adapters/descriptors/codex.yaml`, check it with
`agentstack adapters validate my-adapter.yaml`, then drop it into
`~/.agentstack/adapters/` (no rebuild). `agentstack adapters list` marks which
adapters are yours; a broken drop-in is skipped with a warning, never fatal.

Ground rules, the security invariants, and what a good PR looks like:
[CONTRIBUTING.md](CONTRIBUTING.md). Release history: [CHANGELOG.md](CHANGELOG.md).

## License

Dual-licensed under either [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE)
at your option.
