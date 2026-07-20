<img alt="agentstack" src="docs/logo.svg" width="380">

> **Cloning a repo shouldn't hand your agent to a stranger.**
> AgentStack puts everything your AI coding tools (Claude Code, Codex,
> Cursor, …) are allowed to run into one reviewed file. A repo you clone
> can't auto-activate any of it until you approve that repo — and what
> runs through AgentStack's gateway is firewalled and logged. Each run is
> labelled with [how strongly that is actually enforced](docs/ENFORCEMENT.md).

**[Website](https://tarekkharsa.github.io/agentstack/)** ·
[Docs](https://tarekkharsa.github.io/agentstack/docs.html) ·
[Examples](https://tarekkharsa.github.io/agentstack/examples.html) ·
[Releases](https://github.com/Tarekkharsa/agentstack/releases)

[![CI](https://img.shields.io/github/actions/workflow/status/Tarekkharsa/agentstack/ci.yml?branch=main&style=flat&label=CI)](https://github.com/Tarekkharsa/agentstack/actions/workflows/ci.yml) [![Conformance](https://img.shields.io/github/actions/workflow/status/Tarekkharsa/agentstack/conformance.yml?branch=main&style=flat&label=conformance)](https://github.com/Tarekkharsa/agentstack/actions/workflows/conformance.yml) [![Release](https://img.shields.io/github/v/release/Tarekkharsa/agentstack?style=flat&label=release)](https://github.com/Tarekkharsa/agentstack/releases) [![License](https://img.shields.io/badge/license-MIT_OR_Apache--2.0-blue?style=flat)](https://github.com/Tarekkharsa/agentstack/blob/main/LICENSE-MIT)

## Why

Every skill, MCP (Model Context Protocol — the plugin standard agent CLIs use for tools) server, and
agent config you adopt is **unreviewed code plus instructions**, wired into a process that holds your
credentials, shell, and network. Adopting one is `npm install` with an agent attached — no lockfile,
no review gate, no record of what it did. AgentStack closes four gaps:

- **Anything a repo declares can run.** A clone stays *inert* until you trust its exact bytes; any edit re-gates it.
- **Nothing narrows or records what agents do.** Your machine policy — which no repo can loosen — fences tools, secrets, and egress, and every brokered call lands in an audit log.
- **Every CLI spells the same setup differently.** One reviewed [manifest](docs/concepts.md) renders them all; secrets stay references.
- **An agent can wreck your working tree by accident.** `agentstack guard` blocks `rm -rf`, `git reset --hard`, and `.env` reads before they run.

Using a single agent with one hand-managed server? You may not need this yet. The moment capabilities
come from repos you didn't write, you do.

![The trust gate: clone → inert → review → trust → firewalled → audited — and the library sync gate blocking a literal secret](docs/trust-gate.svg)

## Try it in 60 seconds

```sh
curl -fsSL https://raw.githubusercontent.com/Tarekkharsa/agentstack/main/install.sh | sh
agentstack init      # your CLI configs → one reviewed manifest, previewed and applied
agentstack doctor    # verify it landed — every warning names its exact fix
```

`init` is a guided wizard. Scripting or CI? `agentstack init --secrets skip` writes only the manifest
— no prompts, no token values — then `agentstack apply --write`. Inline tokens are lifted into
`${REF}` placeholders, resolved per machine and never stored in the manifest.

What you'll see — the core loop, one server you already have rendered into every CLI in its own
syntax (a re-run changes nothing: `agentstack apply` → `✓ up to date`):

```text
$ agentstack init --yes
🔍  6 CLI binaries on PATH: Claude Code · Codex CLI · … · Pi
📥  Imported 1 MCP server(s) from existing configs
✅  Wrote .agentstack/agentstack.toml

$ agentstack apply --write            # render the manifest into every CLI
Claude Code (.mcp.json)              ✓ wrote 1 server(s)
Codex CLI (.codex/config.toml)       ✓ wrote 1 server(s)
Gemini CLI (.gemini/settings.json)   ✓ wrote 1 server(s)
OpenCode (opencode.json)             ✓ wrote 1 server(s)
Applied to 4 target(s).
```

Condensed from a real run. Reproduce it fenced (never touches your real configs):
[`examples/sandbox/demo-firstrun.sh`](examples/sandbox/demo-firstrun.sh).

## Install

The one-line installer above verifies the release tarball against the `checksums.txt` published with
each release. Or build from a checkout:

```sh
cargo build --release                  # add --features sandbox for `run --sandbox` (step 6)
./target/release/agentstack self link  # symlink onto your PATH
```

Release binaries ship with sandbox support compiled in; a bare `cargo build` does not — pass
`--features sandbox` to get `run --sandbox` / `--lockdown`.

## Climb as far as you need

AgentStack is adopted in steps, not all at once. Each step pays off on its own, in minutes, and
nothing later is required to keep the earlier wins.

| Step | You run | You get |
| --- | --- | --- |
| [1 — Unify](#step-1--one-manifest-every-cli-5-minutes) | `agentstack init` → `apply` | one reviewed manifest for every CLI; real tokens out of your config files |
| [2 — Verify](#step-2--two-habits-that-keep-it-healthy) | `agentstack` · `agentstack doctor` | drift caught early; every warning names its exact fix |
| [3 — Guard](#step-3--block-the-accidents-one-command) | `agentstack guard install` | `rm -rf`, `git reset --hard`, and `.env` reads blocked before they land |
| [4 — Trust](#step-4--keep-strangers-repos-inert-until-review) | `gateway connect` · `trust .` | cloned repos stay inert until you review them; brokered calls firewalled and audited |
| [5 — Scale](#step-5--scale-it-up-profiles-library-teams) | profiles · `lib` · extensions | one governed stack across projects, machines, and teammates |
| [6 — Confine](#step-6--maximum-assurance-sandbox--lockdown-docker) | `run --sandbox --lockdown` | kernel-enforced confinement — the agent's only route out is the audited proxy |

Steps 1–4 are the everyday loop; 5–6 are shipped and hardening. What each mode enforces, and where it
stops, is spelled out per dimension in the [enforcement matrix](docs/ENFORCEMENT.md); the same ground
as a two-track walkthrough is the
[getting-started tracks](https://tarekkharsa.github.io/agentstack/start.html). New to the vocabulary?
[concepts.md](docs/concepts.md) defines every term.

## Step 1 — One manifest, every CLI (5 minutes)

`agentstack init` is a guided wizard: it detects your CLIs, imports their config, lifts inline tokens
into `${REF}` placeholders, and asks you to pick a **delivery mode** — where rendered files live.
Press Enter for **static** (the default); [which mode do I need?](docs/choose.md) explains the choice.
Then `agentstack apply --write` renders the manifest into every CLI in `[targets]`. Servers and skills
are two of six capability kinds — instructions, settings, hooks, and extensions are the rest, all in
the [reference](docs/reference.md). Full walkthrough:
[Track A](https://tarekkharsa.github.io/agentstack/start.html).

## Step 2 — Two habits that keep it healthy

- `agentstack` with no arguments names the one next step for the directory you're in.
- `agentstack doctor` verifies everything is wired and names the exact fix for anything that isn't.

Everything else you'll reach for day to day:

| Command | What it does |
| --- | --- |
| `agentstack apply` | Preview each CLI's config changes; confirm (or `--write`) to render |
| `agentstack diff` | What would change, read-only |
| `agentstack secret set NAME` | Store a secret in the OS keychain |
| `agentstack use --write` | Activate skills + servers (a profile, or everything when none declared) |
| `agentstack run <cli> --profile <p>` | Launch a CLI as a tracked run, with a profile for its lifetime |
| `agentstack report` | Every "what happened" view: live runs, a run's flight recorder, calls |

When `doctor` flags **drift**, the rule is directional: the hand-edit is right → `adopt` pulls it into
the manifest; the manifest is right → `apply --write` re-renders
([add a server](docs/howto/add-a-server.md) · [undo anything](docs/howto/undo.md)). Complete list: the
[reference](docs/reference.md).

## Step 3 — Block the accidents (one command)

`agentstack guard install` wires a **cooperative** pre-tool-use hook into 9 agent CLIs, blocking the
commands an agent runs by mistake — `rm -rf` outside the workspace, `git reset --hard`, `.env` reads —
before they touch your machine; every denial is logged. It catches **accidents, not a determined
attacker** (kernel-enforced confinement is [step 6](#step-6--maximum-assurance-sandbox--lockdown-docker)).
What it does and doesn't stop: the [enforcement matrix](docs/ENFORCEMENT.md). Runnable:
[`examples/guard-demo/`](examples/guard-demo/).

![agentstack guard blocking rm -rf, git reset --hard, and cat .env](docs/guard.svg)

## Step 4 — Keep strangers' repos inert until review

Register the gateway once — `agentstack gateway connect --all --write` — and every repo you open brings
its own MCP servers, no files copied in. An unreviewed repo is **inert**: no servers spawned or
contacted, no secrets resolved. `agentstack trust .` shows what it declares before you authorize its
exact bytes; any edit re-gates it. Then every brokered call is firewalled and audited under two policy
layers — the repo's `[policy]` and your machine policy, which no repo can loosen. Detail, and exactly
what trust does and doesn't cover: [trust a cloned repo](docs/howto/trust-a-repo.md).

Two extensions of the same consent machinery, both in the [reference](docs/reference.md):

- **`run <cli> --locked`** gates a whole launch with no Docker — pre-launch trust, strict lock verification, a frozen surface. Honest scope: not kernel isolation.
- **`[extensions.*]`** delivers native CLI add-ons under the strictest pinning agentstack has — provenance, not runtime enforcement: [matrix](docs/ENFORCEMENT.md#native-extensions).

## Step 5 — Scale it up: profiles, library, teams

Install a capability once into your machine-wide **central library** (`~/.agentstack/lib`), then
reference it by name from any project's profile — no copying files between repos:

```bash
agentstack search codex                              # your library + catalog + MCP registry
agentstack add skill anthropics/skills --skill pdf --write   # any skills repo: scanned, pinned, activated
agentstack lib add ./skills/sql-review --write               # yours, reusable across repos by name
agentstack lib sync                                  # version the library as a git repo
```

Every add is content-scanned (hidden-unicode / prompt-injection) before it lands — previews stage
transiently and touch nothing until `--write` — and `lib sync`'s fail-closed gate keeps secrets from
ever traveling. Curious first? `agentstack try owner/repo --skill pdf | claude` runs a skill once,
installing nothing. Full flow: [add a skill](docs/howto/add-a-skill.md). **Share with a team or CI:** commit `.agentstack/`,
then [set up a team](docs/howto/team-setup.md) or [wire it into CI](docs/howto/ci.md). Where rendered
files live — **static**, **clean-at-rest**, or **zero-files** — is a per-project choice
([which mode?](docs/choose.md) · [lifecycle](docs/reference.md#where-rendered-files-live-three-modes)).
Vendor packs, a personal layer, `agentstack optimize`, and the dashboard: the [reference](docs/reference.md).

## Step 6 — Maximum assurance: sandbox & lockdown (Docker)

Everything so far decides what an agent *may* do; this step decides what it *can*. `agentstack run
<cli> --sandbox --lockdown` launches the agent in a Docker container with **no host route and no
internet** — its only path out is the egress-proxy sidecar, which enforces your machine
`[policy.egress]` and records every decision to the run's flight recorder. **Unapproved egress is
blocked.** Still, AgentStack restricts destinations and records decisions; it cannot guarantee
sensitive content never leaves through a host you *allowed* — the
[enforcement matrix](docs/ENFORCEMENT.md) is the per-mode truth table. The escalation ladder
(`--locked` → `--sandbox` → `--lockdown`, and the posture each prints):
[lock down a run](docs/howto/lock-down-a-run.md).

## Develop

```bash
cargo test              # unit + golden + integration
cargo clippy --all-targets
cargo fmt --check
```

Install your build with `agentstack self link`. Ground rules and the security invariants:
[CONTRIBUTING.md](CONTRIBUTING.md). Release history: [CHANGELOG.md](CHANGELOG.md).

## License

Dual-licensed under either [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at your option.
